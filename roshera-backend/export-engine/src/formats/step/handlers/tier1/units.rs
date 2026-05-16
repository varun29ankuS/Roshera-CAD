//! Unit-phase handlers: STEP unit declarations → kernel canonical units.
//!
//! The kernel canonical units are **millimetres** for length, **radians**
//! for plane angle, **steradians** for solid angle. STEP files can
//! declare any combination of SI base units (`SI_UNIT`), SI prefixes
//! (`.MILLI.`, `.KILO.`, …), and conversion-based units
//! (`CONVERSION_BASED_UNIT('INCH', LENGTH_MEASURE_WITH_UNIT(0.0254,
//! metre_unit))`). The handlers here normalise every declaration into
//! a single scalar: how many millimetres (or radians, or steradians)
//! one source-file value represents.
//!
//! ## Entity shapes
//!
//! STEP unit declarations almost always appear as **complex
//! sub-super entities**. Common shapes (AP203 / AP214 / AP242):
//!
//! ```text
//! #11 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );
//! #12 = ( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );
//! #13 = ( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() );
//! #21 = ( CONVERSION_BASED_UNIT('INCH',#22) LENGTH_UNIT() NAMED_UNIT(#23) );
//! #22 =  LENGTH_MEASURE_WITH_UNIT( LENGTH_MEASURE(25.4), #11 );
//! ```
//!
//! ruststep stores complex entities as `EntityKind::Complex(Vec<Record>)`.
//! The dispatcher picks the *primary* (first) record's name and hands
//! us only that record. Our handler re-looks up the full entity via
//! the registry and walks every constituent record to assemble the
//! full declaration.
//!
//! ## Phase routing
//!
//! Everything in this module declares `Phase::Unit`, so the dispatcher
//! runs all unit declarations before any geometry resolution. Within
//! the phase, dispatch order is HashMap-arbitrary; cross-references
//! (`CONVERSION_BASED_UNIT` → SI metre, `UNCERTAINTY_MEASURE_WITH_UNIT`
//! → unit instance, `GLOBAL_UNIT_ASSIGNED_CONTEXT` → unit list) are
//! resolved lazily via [`super::resolver::ensure_resolved`].

use ruststep::ast::{Parameter, Record};

use crate::formats::step::{
    context::ImportContext,
    diagnostics::{Severity, Warning},
    dispatch::{EntityDispatch, EntityHandler, HandlerOutcome, Phase},
    registry::{EntityKind, EntityRegistry},
};

use super::params;
use super::resolver::ensure_resolved;

/// Multiplier for one SI prefix. Returns `None` for unknown tokens.
fn si_prefix_multiplier(token: &str) -> Option<f64> {
    match token {
        "EXA" => Some(1.0e18),
        "PETA" => Some(1.0e15),
        "TERA" => Some(1.0e12),
        "GIGA" => Some(1.0e9),
        "MEGA" => Some(1.0e6),
        "KILO" => Some(1.0e3),
        "HECTO" => Some(1.0e2),
        "DECA" => Some(1.0e1),
        "DECI" => Some(1.0e-1),
        "CENTI" => Some(1.0e-2),
        "MILLI" => Some(1.0e-3),
        "MICRO" => Some(1.0e-6),
        "NANO" => Some(1.0e-9),
        "PICO" => Some(1.0e-12),
        "FEMTO" => Some(1.0e-15),
        "ATTO" => Some(1.0e-18),
        _ => None,
    }
}

/// The kind of physical quantity an SI base name carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SiKind {
    Length,
    PlaneAngle,
    SolidAngle,
    Other,
}

fn si_base_kind(name: &str) -> SiKind {
    match name {
        "METRE" => SiKind::Length,
        "RADIAN" => SiKind::PlaneAngle,
        "STERADIAN" => SiKind::SolidAngle,
        _ => SiKind::Other,
    }
}

/// Parsed `(prefix_multiplier, base_kind)` for one `SI_UNIT` record.
#[derive(Debug, Clone, Copy)]
struct SiParts {
    /// Multiplier from the prefix token (1.0 when absent / `$`).
    prefix_mult: f64,
    /// Kind derived from the base name (`METRE` / `RADIAN` / …).
    kind: SiKind,
}

/// Extract `(prefix?, name)` from a `SI_UNIT(prefix?, name)` record.
/// `prefix?` is `.NAME.` or `$` (absent). Returns `None` if the record
/// shape is unrecognised — caller emits a Warning.
fn parse_si_unit(record: &Record) -> Option<SiParts> {
    let items = match &record.parameter {
        Parameter::List(items) => items,
        _ => return None,
    };
    if items.len() != 2 {
        return None;
    }
    let prefix_mult = match &items[0] {
        Parameter::NotProvided | Parameter::Omitted => 1.0,
        Parameter::Enumeration(p) => si_prefix_multiplier(p.as_str())?,
        _ => return None,
    };
    let kind = match &items[1] {
        Parameter::Enumeration(n) => si_base_kind(n.as_str()),
        _ => return None,
    };
    Some(SiParts { prefix_mult, kind })
}

/// What the complex unit entity tells us. Filled in as we walk the
/// constituent records of a unit-style complex entity.
#[derive(Debug, Default)]
struct UnitScan {
    has_length: bool,
    has_plane_angle: bool,
    has_solid_angle: bool,
    si: Option<SiParts>,
    /// `CONVERSION_BASED_UNIT('NAME', #measure)` → `(name, measure_ref)`.
    conversion: Option<(String, u64)>,
}

/// Walk every record of `records` (or the single Simple record) and
/// populate the [`UnitScan`].
fn scan_unit(
    records: &[&Record],
    instance: u64,
    ctx: &mut ImportContext<'_>,
) -> UnitScan {
    let mut scan = UnitScan::default();
    for rec in records {
        match rec.name.as_str() {
            "LENGTH_UNIT" => scan.has_length = true,
            "PLANE_ANGLE_UNIT" => scan.has_plane_angle = true,
            "SOLID_ANGLE_UNIT" => scan.has_solid_angle = true,
            "NAMED_UNIT" => { /* DIMENSIONAL_EXPONENTS ignored for tier-1 */ }
            "SI_UNIT" => {
                if let Some(parts) = parse_si_unit(rec) {
                    scan.si = Some(parts);
                } else {
                    ctx.report.push_warning(Warning {
                        severity: Severity::Warn,
                        entity: "SI_UNIT".to_string(),
                        instance: Some(instance),
                        message: "malformed SI_UNIT(prefix, name) payload".to_string(),
                    });
                }
            }
            "CONVERSION_BASED_UNIT" => match parse_conversion_based(rec) {
                Some(c) => scan.conversion = Some(c),
                None => ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: "CONVERSION_BASED_UNIT".to_string(),
                    instance: Some(instance),
                    message: "malformed CONVERSION_BASED_UNIT(name, measure) payload".to_string(),
                }),
            },
            _ => { /* unknown constituent — silently tolerated */ }
        }
    }
    scan
}

/// Extract `(name, measure_ref)` from a `CONVERSION_BASED_UNIT(name, measure)`
/// record's payload.
fn parse_conversion_based(record: &Record) -> Option<(String, u64)> {
    let items = match &record.parameter {
        Parameter::List(items) => items,
        _ => return None,
    };
    if items.len() != 2 {
        return None;
    }
    let name = match &items[0] {
        Parameter::String(s) => s.clone(),
        _ => return None,
    };
    let measure_ref = match &items[1] {
        Parameter::Ref(ruststep::ast::Name::Entity(id)) => *id,
        _ => return None,
    };
    Some((name, measure_ref))
}

/// Resolve a `*_MEASURE_WITH_UNIT(MEASURE(value), unit_ref)` simple
/// entity to its `(value, referenced_unit_instance)` pair. The measure
/// value is a `Parameter::Typed { keyword = "..._MEASURE", parameter =
/// Real(v) }`.
fn parse_measure_with_unit(record: &Record) -> Option<(f64, u64)> {
    let items = match &record.parameter {
        Parameter::List(items) => items,
        _ => return None,
    };
    if items.len() < 2 {
        return None;
    }
    let value = match &items[0] {
        Parameter::Real(v) => *v,
        Parameter::Integer(v) => *v as f64,
        Parameter::Typed { parameter, .. } => match parameter.as_ref() {
            Parameter::Real(v) => *v,
            Parameter::Integer(v) => *v as f64,
            _ => return None,
        },
        _ => return None,
    };
    let unit_ref = match &items[1] {
        Parameter::Ref(ruststep::ast::Name::Entity(id)) => *id,
        _ => return None,
    };
    Some((value, unit_ref))
}

/// Gather every constituent record for `instance`, whether it lives
/// in a `Simple` or a `Complex` entry. Returns `None` when the
/// instance is missing.
fn collect_records<'a>(
    registry: &'a EntityRegistry,
    instance: u64,
) -> Option<Vec<&'a Record>> {
    let entity = registry.get(instance)?;
    Some(match &entity.kind {
        EntityKind::Simple(r) => vec![r],
        EntityKind::Complex(records) => records.iter().collect(),
    })
}

/// Handler for SI / conversion-based unit declarations. Registered
/// under every name that can appear as the *primary* (first) record
/// of a unit complex, so files emitted in any constituent order land
/// in the same code path.
///
/// On success, writes the source `#N` → mm-per-source-unit (or
/// rad-per-source-unit, sr-per-source-unit) scale into the relevant
/// `ctx.caches.*_units` map.
pub struct UnitDeclarationHandler;

/// Static binding consumed by [`register`].
pub static UNIT_DECLARATION_HANDLER: UnitDeclarationHandler = UnitDeclarationHandler;

impl EntityHandler for UnitDeclarationHandler {
    fn names(&self) -> &'static [&'static str] {
        // Every name that can lead a unit-style complex entity, plus
        // SI_UNIT as a defensive bare-simple form.
        &[
            "LENGTH_UNIT",
            "PLANE_ANGLE_UNIT",
            "SOLID_ANGLE_UNIT",
            "CONVERSION_BASED_UNIT",
            "NAMED_UNIT",
            "SI_UNIT",
        ]
    }

    fn phase(&self) -> Phase {
        Phase::Unit
    }

    fn handle(
        &self,
        instance: u64,
        _record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let records = match collect_records(registry, instance) {
            Some(r) => r,
            None => {
                return HandlerOutcome::Failed {
                    message: format!("#{instance} vanished from registry mid-dispatch"),
                };
            }
        };
        let records_ref: Vec<&Record> = records.iter().copied().collect();
        let scan = scan_unit(&records_ref, instance, ctx);

        // CONVERSION_BASED takes precedence: it carries the explicit
        // scale via its MEASURE_WITH_UNIT reference.
        if let Some((_, measure_ref)) = scan.conversion.as_ref().map(|(n, m)| (n.clone(), *m)) {
            return resolve_conversion(instance, measure_ref, &scan, registry, dispatch, ctx);
        }

        // Pure-SI form. Need an SI_UNIT plus exactly one of
        // length / plane-angle / solid-angle markers.
        let si = match scan.si {
            Some(s) => s,
            None => {
                ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: records_ref.first().map(|r| r.name.clone()).unwrap_or_default(),
                    instance: Some(instance),
                    message: "unit entity has no SI_UNIT constituent".to_string(),
                });
                return HandlerOutcome::Skipped {
                    reason: "no SI_UNIT constituent",
                };
            }
        };

        if scan.has_length && si.kind == SiKind::Length {
            // 1 metre = 1000 mm; prefix scales further.
            let mm_per_source = si.prefix_mult * 1000.0;
            ctx.caches.length_units.insert(instance, mm_per_source);
            HandlerOutcome::Resolved
        } else if scan.has_plane_angle && si.kind == SiKind::PlaneAngle {
            // SI base for plane angle is radian.
            let rad_per_source = si.prefix_mult * 1.0;
            ctx.caches.angle_units.insert(instance, rad_per_source);
            HandlerOutcome::Resolved
        } else if scan.has_solid_angle && si.kind == SiKind::SolidAngle {
            let sr_per_source = si.prefix_mult * 1.0;
            ctx.caches.solid_angle_units.insert(instance, sr_per_source);
            HandlerOutcome::Resolved
        } else if !scan.has_length && !scan.has_plane_angle && !scan.has_solid_angle {
            // Bare SI_UNIT without a quantity-kind marker — we infer
            // from the base name. METRE → length, RADIAN → angle.
            match si.kind {
                SiKind::Length => {
                    ctx.caches
                        .length_units
                        .insert(instance, si.prefix_mult * 1000.0);
                    HandlerOutcome::Resolved
                }
                SiKind::PlaneAngle => {
                    ctx.caches.angle_units.insert(instance, si.prefix_mult);
                    HandlerOutcome::Resolved
                }
                SiKind::SolidAngle => {
                    ctx.caches
                        .solid_angle_units
                        .insert(instance, si.prefix_mult);
                    HandlerOutcome::Resolved
                }
                SiKind::Other => HandlerOutcome::Skipped {
                    reason: "non-mechanical SI base unit",
                },
            }
        } else {
            HandlerOutcome::Skipped {
                reason: "unit kind / base-name mismatch",
            }
        }
    }
}

/// Resolve a `CONVERSION_BASED_UNIT` instance. The `measure_ref`
/// points at a `*_MEASURE_WITH_UNIT` that in turn references a base
/// unit (typically SI metre or SI radian). The base unit may or may
/// not have been resolved yet — we use the lazy resolver to force it.
fn resolve_conversion(
    instance: u64,
    measure_ref: u64,
    scan: &UnitScan,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> HandlerOutcome {
    // Resolve the *_MEASURE_WITH_UNIT directly — these aren't
    // dispatcher-registered handlers, so we read the record inline.
    let measure_entity = match registry.get(measure_ref) {
        Some(e) => e,
        None => {
            ctx.report.push_warning(Warning {
                severity: Severity::Error,
                entity: "CONVERSION_BASED_UNIT".to_string(),
                instance: Some(instance),
                message: format!("references missing measure #{measure_ref}"),
            });
            return HandlerOutcome::Failed {
                message: "measure ref missing".to_string(),
            };
        }
    };
    let measure_record = match &measure_entity.kind {
        EntityKind::Simple(r) => r,
        EntityKind::Complex(records) => match records.first() {
            Some(r) => r,
            None => {
                return HandlerOutcome::Failed {
                    message: "empty complex measure".to_string(),
                };
            }
        },
    };
    let (value, base_unit_ref) = match parse_measure_with_unit(measure_record) {
        Some(v) => v,
        None => {
            ctx.report.push_warning(Warning {
                severity: Severity::Warn,
                entity: measure_record.name.clone(),
                instance: Some(measure_ref),
                message: "malformed MEASURE_WITH_UNIT payload".to_string(),
            });
            return HandlerOutcome::Failed {
                message: "malformed measure".to_string(),
            };
        }
    };

    // Force the base unit to resolve (it may live in this same Unit
    // phase but iterate after us).
    let needs_resolve = !(ctx.caches.length_units.contains_key(&base_unit_ref)
        || ctx.caches.angle_units.contains_key(&base_unit_ref)
        || ctx.caches.solid_angle_units.contains_key(&base_unit_ref));
    if needs_resolve {
        let _ = ensure_resolved(
            base_unit_ref,
            &[
                "LENGTH_UNIT",
                "PLANE_ANGLE_UNIT",
                "SOLID_ANGLE_UNIT",
                "CONVERSION_BASED_UNIT",
                "NAMED_UNIT",
                "SI_UNIT",
            ],
            registry,
            dispatch,
            ctx,
        );
    }

    // Quantity kind from the scan's marker. CONVERSION_BASED_UNIT
    // always carries a quantity marker (LENGTH_UNIT / PLANE_ANGLE_UNIT
    // / SOLID_ANGLE_UNIT) by spec.
    if scan.has_length {
        let base_scale = match ctx.caches.length_units.get(&base_unit_ref) {
            Some(s) => *s,
            None => {
                ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: "CONVERSION_BASED_UNIT".to_string(),
                    instance: Some(instance),
                    message: format!(
                        "length base unit #{base_unit_ref} did not resolve",
                    ),
                });
                return HandlerOutcome::Failed {
                    message: "base length unit unresolved".to_string(),
                };
            }
        };
        ctx.caches
            .length_units
            .insert(instance, value * base_scale);
        HandlerOutcome::Resolved
    } else if scan.has_plane_angle {
        let base_scale = match ctx.caches.angle_units.get(&base_unit_ref) {
            Some(s) => *s,
            None => {
                ctx.report.push_warning(Warning {
                    severity: Severity::Warn,
                    entity: "CONVERSION_BASED_UNIT".to_string(),
                    instance: Some(instance),
                    message: format!(
                        "plane-angle base unit #{base_unit_ref} did not resolve",
                    ),
                });
                return HandlerOutcome::Failed {
                    message: "base angle unit unresolved".to_string(),
                };
            }
        };
        ctx.caches
            .angle_units
            .insert(instance, value * base_scale);
        HandlerOutcome::Resolved
    } else if scan.has_solid_angle {
        let base_scale = match ctx.caches.solid_angle_units.get(&base_unit_ref) {
            Some(s) => *s,
            None => {
                return HandlerOutcome::Failed {
                    message: "base solid-angle unit unresolved".to_string(),
                };
            }
        };
        ctx.caches
            .solid_angle_units
            .insert(instance, value * base_scale);
        HandlerOutcome::Resolved
    } else {
        HandlerOutcome::Skipped {
            reason: "CONVERSION_BASED_UNIT missing quantity marker",
        }
    }
}

/// Handler for `GEOMETRIC_REPRESENTATION_CONTEXT` /
/// `GLOBAL_UNIT_ASSIGNED_CONTEXT` / `GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT`
/// — the entity that *applies* the unit declarations to the file as a
/// whole. Walks the constituent records, finds:
///
/// - `GLOBAL_UNIT_ASSIGNED_CONTEXT((#u1, #u2, #u3))` → resolve each
///   unit ref and write `ctx.unit` (length × angle).
/// - `GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#uncertainty_ref))` →
///   resolve and write `ctx.default_tolerance`.
pub struct UnitContextHandler;
pub static UNIT_CONTEXT_HANDLER: UnitContextHandler = UnitContextHandler;

impl EntityHandler for UnitContextHandler {
    fn names(&self) -> &'static [&'static str] {
        &[
            "GEOMETRIC_REPRESENTATION_CONTEXT",
            "GLOBAL_UNIT_ASSIGNED_CONTEXT",
            "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT",
            "REPRESENTATION_CONTEXT",
        ]
    }

    fn phase(&self) -> Phase {
        Phase::Unit
    }

    fn handle(
        &self,
        instance: u64,
        _record: &Record,
        registry: &EntityRegistry,
        dispatch: &EntityDispatch,
        ctx: &mut ImportContext<'_>,
    ) -> HandlerOutcome {
        let records = match collect_records(registry, instance) {
            Some(r) => r,
            None => {
                return HandlerOutcome::Failed {
                    message: format!("#{instance} vanished from registry mid-dispatch"),
                };
            }
        };

        let mut applied_units = false;
        let mut applied_uncertainty = false;

        for rec in &records {
            match rec.name.as_str() {
                "GLOBAL_UNIT_ASSIGNED_CONTEXT" => {
                    if apply_unit_assignment(rec, instance, registry, dispatch, ctx) {
                        applied_units = true;
                    }
                }
                "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT" => {
                    if apply_uncertainty_assignment(rec, instance, registry, dispatch, ctx) {
                        applied_uncertainty = true;
                    }
                }
                _ => { /* representation/geometric context dimension etc. — ignored */ }
            }
        }

        if applied_units || applied_uncertainty {
            HandlerOutcome::Resolved
        } else {
            HandlerOutcome::Skipped {
                reason: "context entity carries no unit / uncertainty assignment",
            }
        }
    }
}

/// Walk the `GLOBAL_UNIT_ASSIGNED_CONTEXT((#u1,#u2,#u3))` parameter,
/// resolve each `#u` to a scale, and update `ctx.unit`. Returns `true`
/// if any scale was applied.
fn apply_unit_assignment(
    record: &Record,
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> bool {
    let items = match &record.parameter {
        Parameter::List(items) => items,
        _ => return false,
    };
    if items.len() != 1 {
        return false;
    }
    let list = match &items[0] {
        Parameter::List(l) => l,
        _ => return false,
    };

    let mut applied_length = false;
    let mut applied_angle = false;
    let mut new_unit = ctx.unit;

    for (i, item) in list.iter().enumerate() {
        let unit_ref = match params::as_entity_ref(
            item,
            "GLOBAL_UNIT_ASSIGNED_CONTEXT",
            instance,
            &format!("units[{i}]"),
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                continue;
            }
        };

        // Ensure the unit has been resolved.
        let already = ctx.caches.length_units.contains_key(&unit_ref)
            || ctx.caches.angle_units.contains_key(&unit_ref)
            || ctx.caches.solid_angle_units.contains_key(&unit_ref);
        if !already {
            let _ = ensure_resolved(
                unit_ref,
                &[
                    "LENGTH_UNIT",
                    "PLANE_ANGLE_UNIT",
                    "SOLID_ANGLE_UNIT",
                    "CONVERSION_BASED_UNIT",
                    "NAMED_UNIT",
                    "SI_UNIT",
                ],
                registry,
                dispatch,
                ctx,
            );
        }
        if let Some(scale) = ctx.caches.length_units.get(&unit_ref) {
            new_unit.length = *scale;
            applied_length = true;
        } else if let Some(scale) = ctx.caches.angle_units.get(&unit_ref) {
            new_unit.angle_radians_per_source = *scale;
            applied_angle = true;
        }
        // Solid angle has no slot on `UnitScale` — tier-1 doesn't
        // consume it; leave the value in `ctx.caches.solid_angle_units`
        // for any future handler that needs it.
    }

    if applied_length || applied_angle {
        ctx.unit = new_unit;
        true
    } else {
        false
    }
}

/// Walk the `GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#u))` parameter,
/// resolve `#u` via the `UNCERTAINTY_MEASURE_WITH_UNIT` handler, then
/// copy the scaled measure into `ctx.default_tolerance`.
fn apply_uncertainty_assignment(
    record: &Record,
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> bool {
    let items = match &record.parameter {
        Parameter::List(items) => items,
        _ => return false,
    };
    if items.len() != 1 {
        return false;
    }
    let list = match &items[0] {
        Parameter::List(l) => l,
        _ => return false,
    };
    let mut applied = false;
    for (i, item) in list.iter().enumerate() {
        let unc_ref = match params::as_entity_ref(
            item,
            "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT",
            instance,
            &format!("uncertainties[{i}]"),
        ) {
            Ok(r) => r,
            Err(e) => {
                ctx.report.push_warning(e.into_warning());
                continue;
            }
        };
        if let Some(tol) = resolve_uncertainty(unc_ref, registry, dispatch, ctx) {
            ctx.default_tolerance = tol;
            applied = true;
        }
    }
    applied
}

/// Resolve an `UNCERTAINTY_MEASURE_WITH_UNIT` entity to a scaled
/// tolerance value in mm. The entity is a simple record:
/// `UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(value), unit_ref,
/// name, description)`.
fn resolve_uncertainty(
    instance: u64,
    registry: &EntityRegistry,
    dispatch: &EntityDispatch,
    ctx: &mut ImportContext<'_>,
) -> Option<f64> {
    let entity = registry.get(instance)?;
    let record = match &entity.kind {
        EntityKind::Simple(r) => r,
        EntityKind::Complex(records) => records.first()?,
    };
    if !record.name.eq_ignore_ascii_case("UNCERTAINTY_MEASURE_WITH_UNIT") {
        ctx.report.push_warning(Warning {
            severity: Severity::Warn,
            entity: record.name.clone(),
            instance: Some(instance),
            message: "expected UNCERTAINTY_MEASURE_WITH_UNIT".to_string(),
        });
        return None;
    }
    let (value, unit_ref) = parse_measure_with_unit(record)?;

    // Make sure the referenced length unit is resolved.
    if !ctx.caches.length_units.contains_key(&unit_ref) {
        let _ = ensure_resolved(
            unit_ref,
            &[
                "LENGTH_UNIT",
                "CONVERSION_BASED_UNIT",
                "NAMED_UNIT",
                "SI_UNIT",
            ],
            registry,
            dispatch,
            ctx,
        );
    }
    let scale = *ctx.caches.length_units.get(&unit_ref)?;
    Some(value * scale)
}

/// Register every unit-phase handler.
pub fn register(dispatch: &mut EntityDispatch) {
    dispatch.register(&UNIT_DECLARATION_HANDLER);
    dispatch.register(&UNIT_CONTEXT_HANDLER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::step::{
        context::ImportContext,
        diagnostics::ImportReport,
        dispatch::EntityDispatch,
        parser::parse_step,
        registry::EntityRegistry,
    };
    use geometry_engine::primitives::topology_builder::BRepModel;

    fn wrap(body: &str) -> String {
        format!(
            "ISO-10303-21;\n\
             HEADER;\n\
             FILE_DESCRIPTION(('t'),'2;1');\n\
             FILE_NAME('t.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
             FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\n\
             ENDSEC;\n\
             DATA;\n\
             {body}\n\
             ENDSEC;\n\
             END-ISO-10303-21;\n"
        )
    }

    #[test]
    fn si_milli_metre_yields_scale_1mm() {
        let src = wrap(
            "#11=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));",
        );
        let ex = parse_step(&src, "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let resolved = dispatch.run_all(&reg, &mut ctx);
        assert!(resolved >= 1, "report = {:?}", report.warnings);
        assert_eq!(ctx.caches.length_units.get(&11), Some(&1.0));
    }

    #[test]
    fn si_unprefixed_metre_yields_scale_1000() {
        let src = wrap("#11=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.));");
        let ex = parse_step(&src, "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let _ = dispatch.run_all(&reg, &mut ctx);
        assert_eq!(ctx.caches.length_units.get(&11), Some(&1000.0));
    }

    #[test]
    fn si_radian_yields_angle_one() {
        let src = wrap(
            "#12=(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.));",
        );
        let ex = parse_step(&src, "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let _ = dispatch.run_all(&reg, &mut ctx);
        assert_eq!(ctx.caches.angle_units.get(&12), Some(&1.0));
    }

    #[test]
    fn conversion_based_inch_in_mm_yields_25_4() {
        let src = wrap(
            "#11=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\
             #13=(NAMED_UNIT(*) LENGTH_UNIT() CONVERSION_BASED_UNIT('INCH',#14));\
             #14=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#11);",
        );
        let ex = parse_step(&src, "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let _ = dispatch.run_all(&reg, &mut ctx);
        let inch = ctx.caches.length_units.get(&13).copied().unwrap_or(0.0);
        assert!(
            (inch - 25.4).abs() < 1e-9,
            "inch scale = {inch}, report = {:?}",
            report.warnings
        );
    }

    #[test]
    fn conversion_based_degree_in_radian_yields_pi_over_180() {
        let src = wrap(
            "#12=(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.));\
             #15=(NAMED_UNIT(*) PLANE_ANGLE_UNIT() CONVERSION_BASED_UNIT('DEGREE',#16));\
             #16=PLANE_ANGLE_MEASURE_WITH_UNIT(PLANE_ANGLE_MEASURE(0.017453292519943295),#12);",
        );
        let ex = parse_step(&src, "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let _ = dispatch.run_all(&reg, &mut ctx);
        let deg = ctx.caches.angle_units.get(&15).copied().unwrap_or(0.0);
        assert!(
            (deg - std::f64::consts::PI / 180.0).abs() < 1e-12,
            "deg scale = {deg}, report = {:?}",
            report.warnings
        );
    }

    #[test]
    fn global_unit_assigned_context_applies_inch_length_scale() {
        let src = wrap(
            "#11=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\
             #12=(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.));\
             #13=(NAMED_UNIT(*) LENGTH_UNIT() CONVERSION_BASED_UNIT('INCH',#14));\
             #14=LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(25.4),#11);\
             #20=(GEOMETRIC_REPRESENTATION_CONTEXT(3) \
                  GLOBAL_UNIT_ASSIGNED_CONTEXT((#13,#12)) \
                  REPRESENTATION_CONTEXT('','3D'));",
        );
        let ex = parse_step(&src, "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let _ = dispatch.run_all(&reg, &mut ctx);
        assert!(
            (ctx.unit.length - 25.4).abs() < 1e-9,
            "length = {}, report = {:?}",
            ctx.unit.length,
            report.warnings
        );
        assert!((ctx.unit.angle_radians_per_source - 1.0).abs() < 1e-12);
    }

    #[test]
    fn global_uncertainty_assigned_context_sets_tolerance() {
        let src = wrap(
            "#11=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.));\
             #12=(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.));\
             #30=UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(0.005),#11,\
                 'distance_accuracy_value','maximum tolerance');\
             #20=(GEOMETRIC_REPRESENTATION_CONTEXT(3) \
                  GLOBAL_UNIT_ASSIGNED_CONTEXT((#11,#12)) \
                  GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#30)) \
                  REPRESENTATION_CONTEXT('','3D'));",
        );
        let ex = parse_step(&src, "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let _ = dispatch.run_all(&reg, &mut ctx);
        assert!(
            (ctx.default_tolerance - 0.005).abs() < 1e-12,
            "tol = {}, report = {:?}",
            ctx.default_tolerance,
            report.warnings
        );
    }

    #[test]
    fn malformed_si_unit_emits_warning_not_panic() {
        // SI_UNIT with wrong arity — the parser still produces a
        // List, but our parse_si_unit returns None.
        let src = wrap(
            "#11=(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.METRE.));",
        );
        let ex = parse_step(&src, "test").unwrap();
        let reg = EntityRegistry::build(&ex);
        let mut dispatch = EntityDispatch::new();
        register(&mut dispatch);
        let mut model = BRepModel::new();
        let mut report = ImportReport::new();
        let mut ctx = ImportContext::new(&mut model, &mut report);
        let _ = dispatch.run_all(&reg, &mut ctx);
        assert!(!ctx.caches.length_units.contains_key(&11));
        assert!(report
            .warnings
            .iter()
            .any(|w| w.message.contains("malformed SI_UNIT")));
    }

    #[test]
    fn si_prefix_table_round_trip() {
        assert_eq!(si_prefix_multiplier("MILLI"), Some(1e-3));
        assert_eq!(si_prefix_multiplier("KILO"), Some(1e3));
        assert_eq!(si_prefix_multiplier("MICRO"), Some(1e-6));
        assert_eq!(si_prefix_multiplier("BOGUS"), None);
    }
}
