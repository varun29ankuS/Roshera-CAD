//! Fillet operation implementation
//!
//! Creates rounded edges on a solid

use super::common::{brep_to_entity_state, entity_state_to_brep, validate_edges_same_solid};
use crate::{
    entity_mapping::get_entity_mapping,
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    types::BlendRadiusDto,
    EntityId, EntityType, Operation, OperationOutputs, TimelineError, TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::Tolerance,
    operations::{
        blend_graph::{BlendRadius, EdgeFilletProfile},
        fillet::FilletType,
    },
    primitives::{edge::EdgeId, topology_builder::GeometryId as GeometryEngineId},
};
use std::collections::HashMap;

/// Implementation of fillet operation
pub struct FilletOp;

#[async_trait]
impl OperationImpl for FilletOp {
    fn operation_type(&self) -> &'static str {
        "fillet"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Fillet {
            edges,
            radius,
            per_edge_overrides,
        } = operation
        {
            // Check we have edges
            if edges.is_empty() {
                return Err(TimelineError::ValidationError(
                    "Fillet requires at least one edge".to_string(),
                ));
            }

            // Validate radius — covers Constant / Linear / Variable.
            // For Variable the minimum is over all samples; for Linear
            // it's min(start, end); for Constant it's the value.
            let min_r = radius.min_radius();
            if min_r <= 0.0 {
                return Err(TimelineError::ValidationError(format!(
                    "Fillet radius must be positive everywhere on the edge, got min={}",
                    min_r
                )));
            }

            // Validate all edges exist
            for edge_id in edges {
                if !context.entity_exists(*edge_id) {
                    return Err(TimelineError::ValidationError(format!(
                        "Edge {:?} not found",
                        edge_id
                    )));
                }
            }

            // Validate edges belong to the same solid
            validate_edges_same_solid(edges, context)?;

            // F5-β.5.4 — per-edge overrides validation. Every key
            // must be one of `edges` (extra entries are a wire-
            // shape bug, not a silent ignore) and every override
            // profile must be positive on its full domain.
            if let Some(overrides) = per_edge_overrides {
                for (edge_id, override_radius) in overrides {
                    if !edges.contains(edge_id) {
                        return Err(TimelineError::ValidationError(format!(
                            "per_edge_overrides contains edge {:?} that is not in `edges`",
                            edge_id
                        )));
                    }
                    let min_override = override_radius.min_radius();
                    if min_override <= 0.0 {
                        return Err(TimelineError::ValidationError(format!(
                            "per_edge_overrides[{:?}] radius must be positive everywhere (min={})",
                            edge_id, min_override
                        )));
                    }
                }
            }

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Fillet operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Fillet {
            edges,
            radius,
            per_edge_overrides,
        } = operation
        {
            // Find the solid that contains these edges
            let solid_entity_id = validate_edges_same_solid(edges, context)?;

            // Get the solid's BRep
            let solid_entity = context.get_entity(solid_entity_id)?;
            let mut brep = entity_state_to_brep(&solid_entity)?;

            // Get edge IDs from entity mapping. We also keep the
            // parallel `entity → kernel` lookup so per-edge override
            // dispatch can convert `EntityId` keys to `EdgeId`s.
            let mapping = get_entity_mapping();
            let mut edge_ids = Vec::new();
            let mut entity_to_edge: HashMap<EntityId, EdgeId> = HashMap::new();
            for edge_entity_id in edges {
                if let Some(geom_id) = mapping.get_geometry_id(*edge_entity_id) {
                    if let GeometryEngineId::Edge(edge_id) = geom_id {
                        edge_ids.push(edge_id);
                        entity_to_edge.insert(*edge_entity_id, edge_id);
                    }
                }
            }

            // Apply fillet operation using geometry-engine
            use geometry_engine::operations::fillet::{
                fillet_edges, FilletOptions as GeomFilletOptions,
            };

            // F5-β.5.4 — dispatch the (radius, per_edge_overrides)
            // pair onto the kernel's `FilletType` shape.
            //
            // - No overrides           → legacy F3-ε.2 path
            //   (`blend_radius_dto_to_fillet_type(radius)`).
            // - All overrides Constant → `FilletType::PerEdgeConstant`
            //   (every edge in `edges` must have an entry in the
            //   merged map, either from `per_edge_overrides` or
            //   from the default `radius` when it is itself
            //   Constant; mixed-kind defaults+constants are not
            //   expressible in `PerEdgeConstant` and are rejected
            //   below as NotImplemented until F5-β.5.6/F5-β.5.8).
            // - Mixed-kind             → typed `NotImplemented`,
            //   gated to ship in F5-β.5.7 / F5-β.5.8.
            let fillet_type =
                build_fillet_type_from_overrides(radius, per_edge_overrides, &entity_to_edge)?;
            let conservative_radius = max_radius_across(radius, per_edge_overrides);

            // Create fillet options
            let fillet_options = GeomFilletOptions {
                common: geometry_engine::operations::CommonOptions {
                    tolerance: Tolerance::default(),
                    validate_before: true,
                    validate_result: true,
                    merge_entities: true,
                    track_history: false,
                },
                fillet_type,
                radius: conservative_radius,
                propagation: geometry_engine::operations::fillet::PropagationMode::Tangent,
                preserve_edges: true,
                quality: geometry_engine::operations::fillet::FilletQuality::Standard,
                partial_corner_vertices: Vec::new(),
                seam_continuity:
                    geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity::C0,
                graceful_corner_skip: false,
            };

            // Get the solid ID from the BRep (there should be one solid)
            let solid_id = if let Some((solid_id, _)) = brep.solids.iter().next() {
                solid_id
            } else {
                return Err(TimelineError::ExecutionError(
                    "No solid found in BRep".to_string(),
                ));
            };

            // Apply the fillet operation
            let result = fillet_edges(&mut brep, solid_id, edge_ids, fillet_options);

            // Check if operation succeeded
            if let Err(e) = result {
                return Err(TimelineError::ExecutionFailed(format!(
                    "Fillet operation failed: {:?}",
                    e
                )));
            }

            // Update the entity state
            let updated_entity = brep_to_entity_state(
                &brep,
                solid_entity_id,
                EntityType::Solid,
                Some(format!("Filleted_Solid_{}", solid_entity_id.0)),
            )?;

            // Add operation metadata
            let mut final_entity = updated_entity;
            if let Some(obj) = final_entity.properties.as_object_mut() {
                // Get existing fillet count or 0
                let fillet_count = obj
                    .get("fillet_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    + 1;

                obj.insert("last_operation".to_string(), serde_json::json!("fillet"));
                obj.insert("fillet_count".to_string(), serde_json::json!(fillet_count));
                // `last_fillet_radius` now carries the structured DTO
                // so replay-tracking tools can read back the full
                // profile shape, not just a scalar.
                obj.insert(
                    "last_fillet_radius".to_string(),
                    serde_json::to_value(radius).unwrap_or(serde_json::Value::Null),
                );
                obj.insert(
                    "last_fillet_edges".to_string(),
                    serde_json::json!(edges.len()),
                );
                // F5-β.5.4 — surface the override map alongside
                // the default radius so replay/diff tooling sees
                // the full per-edge spec, not just the default.
                if let Some(overrides) = per_edge_overrides {
                    obj.insert(
                        "last_fillet_per_edge_overrides".to_string(),
                        serde_json::to_value(overrides).unwrap_or(serde_json::Value::Null),
                    );
                }
            }

            // Update context
            context.add_temp_entity(final_entity)?;
            context.increment_geometry_ops();

            // Create output
            let outputs = OperationOutputs {
                created: vec![],
                modified: vec![solid_entity_id],
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Fillet operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Fillet { edges, .. } = operation {
            // Variable-radius profiles cost slightly more memory and
            // time per edge — the sample list adds a few hundred
            // bytes and the per-station rolling-ball sweep evaluates
            // the closure at every parameter step — but the dominant
            // cost is still per-edge surgery. The estimate stays
            // edges-proportional; the small per-sample overhead falls
            // out as below-noise inside the existing budget.
            ResourceEstimate {
                entities_created: 0, // Fillet modifies existing entity
                entities_modified: 1,
                memory_bytes: edges.len() as u64 * 10000,
                time_ms: edges.len() as u64 * 50,
            }
        } else {
            ResourceEstimate::default()
        }
    }
}

/// Translate a [`BlendRadiusDto`] into the kernel's
/// [`geometry_engine::operations::fillet::FilletType`] dispatch shape.
///
/// - `Constant(r)` → `FilletType::Constant(r)`
/// - `Linear { start, end }` → `FilletType::Variable(start, end)`
///   (the kernel's legacy "linear interp between endpoints" path)
/// - `Variable(samples)` → `FilletType::VariableStations(samples)`
///   (the F3-ε.2 per-station path)
///
/// Exposed publicly so consumers outside timeline-engine that
/// own a `BlendRadiusDto` (e.g. the api-server REST surface) can
/// reuse the canonical mapping rather than re-implementing it.
pub fn blend_radius_dto_to_fillet_type(
    dto: &BlendRadiusDto,
) -> geometry_engine::operations::fillet::FilletType {
    use geometry_engine::operations::fillet::FilletType;
    match dto {
        BlendRadiusDto::Constant(r) => FilletType::Constant(*r),
        BlendRadiusDto::Linear { start, end } => FilletType::Variable(*start, *end),
        BlendRadiusDto::Variable(samples) => FilletType::VariableStations(samples.clone()),
        // F5-β.5.7: a default-only `Chord` DTO maps to the top-
        // level chord path (`FilletType::Chord`). When chord
        // appears inside `per_edge_overrides` the dispatch
        // happens one level up in `build_fillet_type_from_
        // overrides`, which builds `FilletType::PerEdgeProfile`
        // with `EdgeFilletProfile::Chord` entries.
        BlendRadiusDto::Chord(c) => FilletType::Chord(*c),
    }
}

/// Build the kernel [`FilletType`] for a timeline `Fillet` event,
/// folding in any `per_edge_overrides`.
///
/// Dispatch:
///
/// - `per_edge_overrides == None` → legacy single-profile path
///   via [`blend_radius_dto_to_fillet_type`]. A `Chord` default
///   takes the top-level `FilletType::Chord` path through this
///   helper.
/// - All overrides + default are `Constant` → `PerEdgeConstant`.
///   Cheap single-`f64` shape; the all-Constant fast path. A
///   `Chord` entry never matches this predicate, so any chord
///   in the override map falls through to `PerEdgeProfile`.
/// - Any non-`Constant` profile present (override or default
///   serving as fallback), or any `Chord` profile → `PerEdge
///   Profile`. Every selected edge is filled with its explicit
///   override (when present) or the default profile (when not).
///   F5-β.5.6 lifted the prior `NotImplemented` gate here;
///   F5-β.5.7 extended the variant to carry chord entries
///   alongside radius schedules.
pub(crate) fn build_fillet_type_from_overrides(
    default_radius: &BlendRadiusDto,
    per_edge_overrides: &Option<HashMap<EntityId, BlendRadiusDto>>,
    entity_to_edge: &HashMap<EntityId, EdgeId>,
) -> TimelineResult<FilletType> {
    let overrides = match per_edge_overrides {
        None => return Ok(blend_radius_dto_to_fillet_type(default_radius)),
        Some(overrides) => overrides,
    };

    // Fast path: every *resolved* per-edge DTO is `Constant`.
    // The default's kind only matters for entities that don't
    // have an override — if every selected entity has its own
    // Constant override, the default never fires, so the merged
    // spec is still all-Constant and packs as the cheap
    // `PerEdgeConstant(HashMap<EdgeId, f64>)` shape.
    let default_is_constant = matches!(default_radius, BlendRadiusDto::Constant(_));
    let every_entity_resolves_to_constant = entity_to_edge.keys().all(|entity_id| match overrides
        .get(entity_id)
    {
        Some(BlendRadiusDto::Constant(_)) => true,
        Some(_) => false,
        None => default_is_constant,
    });

    if every_entity_resolves_to_constant {
        let mut per_edge: HashMap<EdgeId, f64> = HashMap::with_capacity(entity_to_edge.len());
        for (entity_id, edge_id) in entity_to_edge {
            let dto = overrides.get(entity_id).unwrap_or(default_radius);
            // `dto` is guaranteed Constant by the resolve check
            // above — the `else 0.0` fallback is defensive and
            // would be rejected by `validate_fillet_inputs`.
            let r = match dto {
                BlendRadiusDto::Constant(r) => *r,
                _ => 0.0,
            };
            per_edge.insert(*edge_id, r);
        }
        if per_edge.is_empty() {
            return Ok(blend_radius_dto_to_fillet_type(default_radius));
        }
        return Ok(FilletType::PerEdgeConstant(per_edge));
    }

    // Mixed-kind path: at least one profile is non-Constant
    // (or any profile is `Chord`, which never collapses into
    // `PerEdgeConstant` regardless of its value). Pack as
    // `PerEdgeProfile(HashMap<EdgeId, EdgeFilletProfile>)`.
    // Every selected edge gets its own profile entry; missing
    // entity ids fall back to the default profile.
    let mut per_edge: HashMap<EdgeId, EdgeFilletProfile> =
        HashMap::with_capacity(entity_to_edge.len());
    for (entity_id, edge_id) in entity_to_edge {
        let dto = overrides.get(entity_id).unwrap_or(default_radius);
        per_edge.insert(*edge_id, blend_radius_dto_to_edge_profile(dto));
    }
    if per_edge.is_empty() {
        // Defensive: empty entity→edge mapping degrades to the
        // single-profile path so the kernel emits its standard
        // empty-selection rejection at validation time.
        return Ok(blend_radius_dto_to_fillet_type(default_radius));
    }
    Ok(FilletType::PerEdgeProfile(per_edge))
}

/// Convert a wire-level [`BlendRadiusDto`] to the kernel
/// [`EdgeFilletProfile`] shape used inside `PerEdgeProfile`. The
/// three radius-schedule DTO variants wrap into
/// `EdgeFilletProfile::Radius(BlendRadius::*)`; the F5-β.5.7
/// `Chord` DTO maps to `EdgeFilletProfile::Chord` so the raw
/// chord length survives intact to surgery time, where
/// `create_chord_fillet` converts it to a per-edge radius with
/// the local dihedral.
fn blend_radius_dto_to_edge_profile(dto: &BlendRadiusDto) -> EdgeFilletProfile {
    match dto {
        BlendRadiusDto::Constant(r) => EdgeFilletProfile::Radius(BlendRadius::Constant(*r)),
        BlendRadiusDto::Linear { start, end } => EdgeFilletProfile::Radius(BlendRadius::Linear {
            start: *start,
            end: *end,
        }),
        BlendRadiusDto::Variable(samples) => {
            EdgeFilletProfile::Radius(BlendRadius::Variable(samples.clone()))
        }
        BlendRadiusDto::Chord(c) => EdgeFilletProfile::Chord(*c),
    }
}

/// Compute the conservative upper-bound radius across the
/// default profile and every per-edge override. Used to seed
/// `FilletOptions.radius` (the F6-α curvature budget bound).
pub(crate) fn max_radius_across(
    default_radius: &BlendRadiusDto,
    per_edge_overrides: &Option<HashMap<EntityId, BlendRadiusDto>>,
) -> f64 {
    let mut max_r = default_radius.max_radius();
    if let Some(overrides) = per_edge_overrides {
        for dto in overrides.values() {
            let r = dto.max_radius();
            if r > max_r {
                max_r = r;
            }
        }
    }
    max_r
}

#[cfg(test)]
mod per_edge_overrides_tests {
    //! F5-β.5.4 — unit coverage for the dispatch helpers
    //! [`build_fillet_type_from_overrides`] and [`max_radius_across`].
    //! End-to-end execute is exercised by the api-server router-
    //! integration tests; these pin the pure dispatch shape.
    use super::*;

    fn make_entity_to_edge(edges: &[EntityId], edge_ids: &[EdgeId]) -> HashMap<EntityId, EdgeId> {
        edges
            .iter()
            .zip(edge_ids.iter())
            .map(|(e, k)| (*e, *k))
            .collect()
    }

    #[test]
    fn none_overrides_falls_through_to_legacy_path() {
        let edges = vec![EntityId::new()];
        let edge_ids: Vec<EdgeId> = vec![1];
        let mapping = make_entity_to_edge(&edges, &edge_ids);
        let ty = build_fillet_type_from_overrides(&BlendRadiusDto::Constant(0.5), &None, &mapping)
            .expect("none-overrides must succeed");
        match ty {
            FilletType::Constant(r) => assert_eq!(r, 0.5),
            other => panic!("expected Constant(0.5), got {other:?}"),
        }
    }

    #[test]
    fn all_constant_overrides_yields_per_edge_constant() {
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let e2 = EntityId::new();
        let edge_ids: Vec<EdgeId> = vec![10, 11, 12];
        let mapping = make_entity_to_edge(&[e0, e1, e2], &edge_ids);
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(e0, BlendRadiusDto::Constant(1.0));
        overrides.insert(e1, BlendRadiusDto::Constant(1.5));
        overrides.insert(e2, BlendRadiusDto::Constant(2.0));
        let ty = build_fillet_type_from_overrides(
            &BlendRadiusDto::Constant(0.5),
            &Some(overrides),
            &mapping,
        )
        .expect("all-Constant overrides must succeed");
        match ty {
            FilletType::PerEdgeConstant(map) => {
                assert_eq!(map.get(&10).copied(), Some(1.0));
                assert_eq!(map.get(&11).copied(), Some(1.5));
                assert_eq!(map.get(&12).copied(), Some(2.0));
                assert_eq!(map.len(), 3);
            }
            other => panic!("expected PerEdgeConstant, got {other:?}"),
        }
    }

    #[test]
    fn partial_overrides_fall_back_to_constant_default_for_unspecified_edges() {
        // Only e0 has an override; e1 and e2 should pick up the
        // default Constant value of 0.5.
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let e2 = EntityId::new();
        let edge_ids: Vec<EdgeId> = vec![20, 21, 22];
        let mapping = make_entity_to_edge(&[e0, e1, e2], &edge_ids);
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(e0, BlendRadiusDto::Constant(3.0));
        let ty = build_fillet_type_from_overrides(
            &BlendRadiusDto::Constant(0.5),
            &Some(overrides),
            &mapping,
        )
        .expect("partial Constant overrides must succeed");
        match ty {
            FilletType::PerEdgeConstant(map) => {
                assert_eq!(map.get(&20).copied(), Some(3.0));
                assert_eq!(map.get(&21).copied(), Some(0.5));
                assert_eq!(map.get(&22).copied(), Some(0.5));
            }
            other => panic!("expected PerEdgeConstant, got {other:?}"),
        }
    }

    #[test]
    fn linear_override_now_builds_per_edge_profile() {
        // F5-β.5.6: the prior `NotImplemented` gate is lifted.
        // A single Linear override mixed with a Constant override
        // and a Constant default now builds a `PerEdgeProfile`
        // with each edge's profile preserved verbatim.
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let edge_ids: Vec<EdgeId> = vec![30, 31];
        let mapping = make_entity_to_edge(&[e0, e1], &edge_ids);
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(e0, BlendRadiusDto::Constant(1.0));
        overrides.insert(
            e1,
            BlendRadiusDto::Linear {
                start: 0.5,
                end: 1.5,
            },
        );
        let ty = build_fillet_type_from_overrides(
            &BlendRadiusDto::Constant(0.5),
            &Some(overrides),
            &mapping,
        )
        .expect("mixed-kind overrides must now build PerEdgeProfile");
        match ty {
            FilletType::PerEdgeProfile(map) => {
                assert_eq!(
                    map.get(&30),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(1.0)))
                );
                assert_eq!(
                    map.get(&31),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Linear {
                        start: 0.5,
                        end: 1.5,
                    }))
                );
                assert_eq!(map.len(), 2);
            }
            other => panic!("expected PerEdgeProfile, got {other:?}"),
        }
    }

    #[test]
    fn mixed_kind_overrides_with_constant_default_fills_every_edge() {
        // Three edges, default Constant, e0 Constant override, e1
        // Linear override, e2 Variable override. The all-Constant
        // fast path is bypassed because at least one override is
        // non-Constant.
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let e2 = EntityId::new();
        let edge_ids: Vec<EdgeId> = vec![50, 51, 52];
        let mapping = make_entity_to_edge(&[e0, e1, e2], &edge_ids);
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(e0, BlendRadiusDto::Constant(0.3));
        overrides.insert(
            e1,
            BlendRadiusDto::Linear {
                start: 0.2,
                end: 0.5,
            },
        );
        overrides.insert(
            e2,
            BlendRadiusDto::Variable(vec![(0.0, 0.25), (0.5, 0.4), (1.0, 0.25)]),
        );
        let ty = build_fillet_type_from_overrides(
            &BlendRadiusDto::Constant(0.5),
            &Some(overrides),
            &mapping,
        )
        .expect("mixed-kind overrides with Constant default must build");
        match ty {
            FilletType::PerEdgeProfile(map) => {
                assert_eq!(
                    map.get(&50),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(0.3)))
                );
                assert_eq!(
                    map.get(&51),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Linear {
                        start: 0.2,
                        end: 0.5,
                    }))
                );
                assert_eq!(
                    map.get(&52),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Variable(vec![
                        (0.0, 0.25),
                        (0.5, 0.4),
                        (1.0, 0.25),
                    ])))
                );
                assert_eq!(map.len(), 3);
            }
            other => panic!("expected PerEdgeProfile, got {other:?}"),
        }
    }

    #[test]
    fn variable_default_with_mixed_overrides_fills_every_edge() {
        // Variable default + partial Linear / Constant overrides.
        // The unspecified edge picks up the Variable default
        // (the default itself is non-Constant, so the fast path
        // is skipped even though every explicit override is
        // Constant).
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let e2 = EntityId::new();
        let edge_ids: Vec<EdgeId> = vec![60, 61, 62];
        let mapping = make_entity_to_edge(&[e0, e1, e2], &edge_ids);
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(
            e0,
            BlendRadiusDto::Linear {
                start: 0.2,
                end: 0.4,
            },
        );
        overrides.insert(e1, BlendRadiusDto::Constant(0.6));
        // e2 falls through to the Variable default.
        let default = BlendRadiusDto::Variable(vec![(0.0, 0.5), (1.0, 0.7)]);
        let ty = build_fillet_type_from_overrides(&default, &Some(overrides), &mapping)
            .expect("Variable default with mixed overrides must build");
        match ty {
            FilletType::PerEdgeProfile(map) => {
                assert_eq!(
                    map.get(&60),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Linear {
                        start: 0.2,
                        end: 0.4,
                    }))
                );
                assert_eq!(
                    map.get(&61),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(0.6)))
                );
                assert_eq!(
                    map.get(&62),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Variable(vec![
                        (0.0, 0.5),
                        (1.0, 0.7)
                    ])))
                );
                assert_eq!(map.len(), 3);
            }
            other => panic!("expected PerEdgeProfile, got {other:?}"),
        }
    }

    #[test]
    fn variable_default_with_constant_overrides_fills_every_edge() {
        // Variable default — but every edge has its own Constant
        // override. The default never fires, so the build succeeds.
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let edge_ids: Vec<EdgeId> = vec![40, 41];
        let mapping = make_entity_to_edge(&[e0, e1], &edge_ids);
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(e0, BlendRadiusDto::Constant(0.8));
        overrides.insert(e1, BlendRadiusDto::Constant(1.2));
        let ty = build_fillet_type_from_overrides(
            &BlendRadiusDto::Linear {
                start: 0.5,
                end: 1.5,
            },
            &Some(overrides),
            &mapping,
        )
        .expect("Variable default + full Constant overrides must succeed");
        assert!(matches!(ty, FilletType::PerEdgeConstant(_)));
    }

    #[test]
    fn max_radius_across_picks_largest_of_default_and_overrides() {
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(e0, BlendRadiusDto::Constant(1.0));
        overrides.insert(
            e1,
            BlendRadiusDto::Linear {
                start: 2.5,
                end: 4.0,
            },
        );
        let max = max_radius_across(&BlendRadiusDto::Constant(0.5), &Some(overrides));
        assert_eq!(max, 4.0);
    }

    #[test]
    fn max_radius_across_with_none_overrides_returns_default_max() {
        let max = max_radius_across(
            &BlendRadiusDto::Linear {
                start: 0.3,
                end: 0.9,
            },
            &None,
        );
        assert_eq!(max, 0.9);
    }

    // ------------------------------------------------------------------
    // F5-β.5.7 — chord-in-per-edge-overrides coverage.
    //
    // Each test pins one dispatch path for the new
    // `BlendRadiusDto::Chord(_)` DTO variant inside the override map:
    //   - Default-only Chord (no overrides) → top-level
    //     `FilletType::Chord`.
    //   - Chord override mixed with Constant default + Constant
    //     overrides → `PerEdgeProfile` (Chord never collapses into
    //     `PerEdgeConstant`).
    //   - All-Chord overrides → `PerEdgeProfile`.
    //   - Mixed Linear default + Chord / Constant overrides → all
    //     three EdgeFilletProfile shapes accounted for.
    // ------------------------------------------------------------------

    #[test]
    fn chord_default_with_no_overrides_yields_fillet_type_chord() {
        let result =
            build_fillet_type_from_overrides(&BlendRadiusDto::Chord(0.5), &None, &HashMap::new())
                .expect("Chord default with no overrides must succeed");
        match result {
            FilletType::Chord(c) => assert!((c - 0.5).abs() < 1e-12),
            other => panic!("expected FilletType::Chord, got {other:?}"),
        }
    }

    #[test]
    fn chord_override_with_constant_default_builds_per_edge_profile() {
        // Three edges, default Constant, one edge overridden with
        // Chord. The Chord entry blocks the all-Constant fast path,
        // so the dispatch lands on PerEdgeProfile with two
        // Radius(Constant) entries (e0 explicit override, e2
        // default fallback) and one Chord (e1).
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let e2 = EntityId::new();
        let edge_ids: Vec<EdgeId> = vec![70, 71, 72];
        let mapping = make_entity_to_edge(&[e0, e1, e2], &edge_ids);
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(e0, BlendRadiusDto::Constant(0.4));
        overrides.insert(e1, BlendRadiusDto::Chord(0.6));
        let ty = build_fillet_type_from_overrides(
            &BlendRadiusDto::Constant(0.5),
            &Some(overrides),
            &mapping,
        )
        .expect("Chord override with Constant default must build");
        match ty {
            FilletType::PerEdgeProfile(map) => {
                assert_eq!(map.len(), 3);
                assert_eq!(
                    map.get(&70),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(0.4)))
                );
                assert_eq!(map.get(&71), Some(&EdgeFilletProfile::Chord(0.6)));
                assert_eq!(
                    map.get(&72),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(0.5)))
                );
                let chord_count = map
                    .values()
                    .filter(|p| matches!(p, EdgeFilletProfile::Chord(_)))
                    .count();
                assert_eq!(chord_count, 1, "exactly one Chord entry expected");
            }
            other => panic!("expected PerEdgeProfile, got {other:?}"),
        }
    }

    #[test]
    fn all_chord_overrides_build_per_edge_profile() {
        // Three edges, default Constant, every edge overridden with
        // Chord. Chord never collapses to PerEdgeConstant — even
        // though every entry is "constant" in shape, the radius
        // derivation requires the local dihedral, so PerEdgeProfile
        // is the correct dispatch.
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let e2 = EntityId::new();
        let edge_ids: Vec<EdgeId> = vec![80, 81, 82];
        let mapping = make_entity_to_edge(&[e0, e1, e2], &edge_ids);
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(e0, BlendRadiusDto::Chord(0.3));
        overrides.insert(e1, BlendRadiusDto::Chord(0.4));
        overrides.insert(e2, BlendRadiusDto::Chord(0.5));
        let ty = build_fillet_type_from_overrides(
            &BlendRadiusDto::Constant(0.5),
            &Some(overrides),
            &mapping,
        )
        .expect("all-Chord overrides must build");
        match ty {
            FilletType::PerEdgeProfile(map) => {
                assert_eq!(map.len(), 3);
                assert_eq!(map.get(&80), Some(&EdgeFilletProfile::Chord(0.3)));
                assert_eq!(map.get(&81), Some(&EdgeFilletProfile::Chord(0.4)));
                assert_eq!(map.get(&82), Some(&EdgeFilletProfile::Chord(0.5)));
            }
            other => panic!("expected PerEdgeProfile, got {other:?}"),
        }
    }

    #[test]
    fn mixed_radius_and_chord_overrides_build_per_edge_profile() {
        // Three edges, default Linear, overrides: e0 Chord, e1
        // Constant, e2 nothing (picks up Linear default). Asserts
        // PerEdgeProfile with one Chord, one Radius(Constant), one
        // Radius(Linear).
        let e0 = EntityId::new();
        let e1 = EntityId::new();
        let e2 = EntityId::new();
        let edge_ids: Vec<EdgeId> = vec![90, 91, 92];
        let mapping = make_entity_to_edge(&[e0, e1, e2], &edge_ids);
        let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
        overrides.insert(e0, BlendRadiusDto::Chord(0.7));
        overrides.insert(e1, BlendRadiusDto::Constant(0.45));
        let default = BlendRadiusDto::Linear {
            start: 0.2,
            end: 0.6,
        };
        let ty = build_fillet_type_from_overrides(&default, &Some(overrides), &mapping)
            .expect("mixed radius and chord overrides must build");
        match ty {
            FilletType::PerEdgeProfile(map) => {
                assert_eq!(map.len(), 3);
                assert_eq!(map.get(&90), Some(&EdgeFilletProfile::Chord(0.7)));
                assert_eq!(
                    map.get(&91),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(0.45)))
                );
                assert_eq!(
                    map.get(&92),
                    Some(&EdgeFilletProfile::Radius(BlendRadius::Linear {
                        start: 0.2,
                        end: 0.6,
                    }))
                );
            }
            other => panic!("expected PerEdgeProfile, got {other:?}"),
        }
    }
}
