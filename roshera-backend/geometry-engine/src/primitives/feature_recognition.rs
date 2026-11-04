//! Automatic feature recognition from B-Rep topology.
//!
//! Detects common manufacturing features (holes, fillets, chamfers, pockets, bosses)
//! by analyzing surface types, face adjacency, and topology patterns.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::primitives::surface::{Cylinder, Sphere, SurfaceType, Torus};
use crate::primitives::topology_builder::BRepModel;

/// A recognized geometric feature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecognizedFeature {
    ThroughHole {
        diameter: f64,
        axis: [f64; 3],
        face_id: u32,
    },
    BlindHole {
        diameter: f64,
        depth: f64,
        face_id: u32,
    },
    /// A proven bore (concave cylindrical wall) whose THRU-vs-blind depth
    /// could not be derived exactly — e.g. a wall bounded by NURBS edges.
    /// Reported as a hole with no depth claim rather than guessed.
    Hole {
        diameter: f64,
        axis: [f64; 3],
        face_id: u32,
    },
    Fillet {
        radius: f64,
        face_ids: Vec<u32>,
    },
    Chamfer {
        distance: f64,
        face_ids: Vec<u32>,
    },
    CylindricalBoss {
        diameter: f64,
        height: f64,
        face_id: u32,
    },
    SphericalFeature {
        radius: f64,
        face_id: u32,
    },
}

impl RecognizedFeature {
    /// Every topological face this feature occupies (for cross-checking that a
    /// recognized feature's faces are live and visible). Used by the dual-eye
    /// reconcile.
    pub fn face_ids(&self) -> Vec<u32> {
        match self {
            RecognizedFeature::ThroughHole { face_id, .. }
            | RecognizedFeature::BlindHole { face_id, .. }
            | RecognizedFeature::Hole { face_id, .. }
            | RecognizedFeature::CylindricalBoss { face_id, .. }
            | RecognizedFeature::SphericalFeature { face_id, .. } => vec![*face_id],
            RecognizedFeature::Fillet { face_ids, .. }
            | RecognizedFeature::Chamfer { face_ids, .. } => face_ids.clone(),
        }
    }

    /// Human-readable description of the feature.
    pub fn to_description(&self) -> String {
        match self {
            RecognizedFeature::ThroughHole { diameter, axis, .. } => {
                let axis_name = if axis[2].abs() > 0.9 {
                    "Z"
                } else if axis[1].abs() > 0.9 {
                    "Y"
                } else if axis[0].abs() > 0.9 {
                    "X"
                } else {
                    "oblique"
                };
                format!("through-hole ⌀{:.1}mm along {} axis", diameter, axis_name)
            }
            RecognizedFeature::BlindHole {
                diameter, depth, ..
            } => format!("blind hole ⌀{:.1}mm, {:.1}mm deep", diameter, depth),
            RecognizedFeature::Hole { diameter, .. } => {
                format!("hole ⌀{:.1}mm (depth not provable)", diameter)
            }
            RecognizedFeature::Fillet {
                radius, face_ids, ..
            } => format!("fillet R{:.1}mm ({} faces)", radius, face_ids.len()),
            RecognizedFeature::Chamfer {
                distance, face_ids, ..
            } => format!("chamfer {:.1}mm ({} faces)", distance, face_ids.len()),
            RecognizedFeature::CylindricalBoss {
                diameter, height, ..
            } => format!("cylindrical boss ⌀{:.1}mm, {:.1}mm tall", diameter, height),
            RecognizedFeature::SphericalFeature { radius, .. } => {
                format!("spherical feature R{:.1}mm", radius)
            }
        }
    }

    /// Feature type as a string tag.
    pub fn feature_type(&self) -> &'static str {
        match self {
            RecognizedFeature::ThroughHole { .. } => "through_hole",
            RecognizedFeature::BlindHole { .. } => "blind_hole",
            RecognizedFeature::Hole { .. } => "hole",
            RecognizedFeature::Fillet { .. } => "fillet",
            RecognizedFeature::Chamfer { .. } => "chamfer",
            RecognizedFeature::CylindricalBoss { .. } => "boss",
            RecognizedFeature::SphericalFeature { .. } => "spherical",
        }
    }
}

/// Recognize features in a solid by analyzing its B-Rep topology.
///
/// Detection strategy:
/// - **Holes**: concave cylindrical walls by the material-side rule
///   ([`crate::readable::bore_face_ids`]); THRU vs blind (and blind depth) from
///   [`crate::dfm::analyzers::bore_metrics`]; a bore whose depth cannot be
///   proven is [`RecognizedFeature::Hole`].
/// - **Bosses**: convex cylindrical walls (outward normal away from the axis).
/// - **Fillets**: Toroidal faces (constant-radius blend between adjacent faces).
/// - **Spherical features**: Spherical face segments.
///
/// Chamfers are not detected (no variant is ever emitted by this function).
pub fn recognize_features(solid_id: u32, model: &BRepModel) -> Vec<RecognizedFeature> {
    let mut features = Vec::new();

    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return features,
    };

    let mut cylindrical_faces: Vec<(u32, f64, [f64; 3])> = Vec::new(); // (face_id, radius, axis)
    let mut toroidal_faces: Vec<(u32, f64)> = Vec::new(); // (face_id, minor_radius)
    let mut spherical_faces: Vec<(u32, f64)> = Vec::new(); // (face_id, radius)

    for &shell_id in &solid.all_shells() {
        let shell = match model.shells.get(shell_id) {
            Some(s) => s,
            None => continue,
        };

        for &face_id in &shell.faces {
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };

            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };

            match surface.surface_type() {
                SurfaceType::Cylinder => {
                    if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
                        cylindrical_faces.push((
                            face_id,
                            cyl.radius,
                            [cyl.axis.x, cyl.axis.y, cyl.axis.z],
                        ));
                    }
                }
                SurfaceType::Torus => {
                    if let Some(tor) = surface.as_any().downcast_ref::<Torus>() {
                        toroidal_faces.push((face_id, tor.minor_radius));
                    }
                }
                SurfaceType::Sphere => {
                    if let Some(sph) = surface.as_any().downcast_ref::<Sphere>() {
                        spherical_faces.push((face_id, sph.radius));
                    }
                }
                _ => {}
            }
        }
    }

    // --- Holes vs bosses ---
    // Hole-vs-boss is the kernel's material-side rule (`bore_face_ids`: a
    // cylinder whose outward normal points toward its axis is a cavity wall),
    // and THRU-vs-blind is `dfm::bore_metrics`' proven comparison of the wall's
    // trimmed extent against the solid's own extent on the same axis. A bore
    // whose depth cannot be proven exactly is reported as `Hole` — never
    // guessed into THRU or blind.
    let bore_faces = crate::readable::bore_face_ids(model, solid_id);
    let bore_records = crate::dfm::analyzers::bore_metrics(model, solid_id)
        .map(|outcome| outcome.bores)
        .unwrap_or_default();
    for &(face_id, radius, axis) in &cylindrical_faces {
        let diameter = radius * 2.0;
        if bore_faces.contains(&face_id) {
            match bore_records.iter().find(|r| r.face == face_id) {
                Some(rec) if rec.is_through => features.push(RecognizedFeature::ThroughHole {
                    diameter,
                    axis,
                    face_id,
                }),
                Some(rec) => features.push(RecognizedFeature::BlindHole {
                    diameter,
                    depth: rec.depth.value,
                    face_id,
                }),
                None => features.push(RecognizedFeature::Hole {
                    diameter,
                    axis,
                    face_id,
                }),
            }
        } else {
            let height = estimate_face_extent(face_id, &axis, model);
            if height > 1e-6 {
                features.push(RecognizedFeature::CylindricalBoss {
                    diameter,
                    height,
                    face_id,
                });
            }
        }
    }

    // --- Detect fillets (toroidal faces) ---
    // Group toroidal faces by similar minor radius
    if !toroidal_faces.is_empty() {
        let mut grouped: HashMap<i64, Vec<u32>> = HashMap::new();
        for &(face_id, minor_r) in &toroidal_faces {
            // Group by radius rounded to 0.01mm
            let key = (minor_r * 100.0).round() as i64;
            grouped.entry(key).or_default().push(face_id);
        }
        for (key, face_ids) in grouped {
            let radius = key as f64 / 100.0;
            features.push(RecognizedFeature::Fillet { radius, face_ids });
        }
    }

    // --- Detect spherical features ---
    for &(face_id, radius) in &spherical_faces {
        features.push(RecognizedFeature::SphericalFeature { radius, face_id });
    }

    features
}

/// Estimate the extent (height/depth) of a face along a given axis
/// by projecting its vertices onto the axis direction.
fn estimate_face_extent(face_id: u32, axis: &[f64; 3], model: &BRepModel) -> f64 {
    let face = match model.faces.get(face_id) {
        Some(f) => f,
        None => return 0.0,
    };

    let mut min_proj = f64::INFINITY;
    let mut max_proj = f64::NEG_INFINITY;

    for loop_id in face.all_loops() {
        let loop_data = match model.loops.get(loop_id) {
            Some(l) => l,
            None => continue,
        };

        for &edge_id in &loop_data.edges {
            let edge = match model.edges.get(edge_id) {
                Some(e) => e,
                None => continue,
            };

            for &v_id in &[edge.start_vertex, edge.end_vertex] {
                if let Some(vertex) = model.vertices.get(v_id) {
                    let proj = vertex.position[0] * axis[0]
                        + vertex.position[1] * axis[1]
                        + vertex.position[2] * axis[2];
                    if proj < min_proj {
                        min_proj = proj;
                    }
                    if proj > max_proj {
                        max_proj = proj;
                    }
                }
            }
        }
    }

    if min_proj == f64::INFINITY {
        0.0
    } else {
        (max_proj - min_proj).abs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::primitive_traits::Primitive;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> u32 {
        let mut builder = TopologyBuilder::new(model);
        match builder.create_box_3d(w, h, d).unwrap() {
            GeometryId::Solid(id) => id,
            other => panic!("Expected Solid, got {:?}", other),
        }
    }

    fn make_cylinder(model: &mut BRepModel, r: f64, h: f64) -> u32 {
        use crate::primitives::cylinder_primitive::{CylinderParameters, CylinderPrimitive};
        let params = CylinderParameters::new(r, h).unwrap();
        CylinderPrimitive::create(params, model).unwrap()
    }

    #[test]
    fn test_box_no_features() {
        let mut model = BRepModel::new();
        let solid_id = make_box(&mut model, 50.0, 30.0, 20.0);

        let features = recognize_features(solid_id, &model);

        // A plain box should have no special features (all faces are planar)
        let holes: Vec<_> = features
            .iter()
            .filter(|f| matches!(f, RecognizedFeature::ThroughHole { .. }))
            .collect();
        assert!(holes.is_empty(), "Plain box should have no through-holes");
    }

    #[test]
    fn test_cylinder_recognized() {
        let mut model = BRepModel::new();
        let solid_id = make_cylinder(&mut model, 10.0, 30.0);

        let features = recognize_features(solid_id, &model);

        // A standalone cylinder's cylindrical face touches 2 planar caps,
        // which matches the through-hole heuristic (cylindrical + 2 planar adjacents).
        // This is expected — the feature recognition sees the pattern.
        // In context (after boolean), it would correctly identify as a hole.
        let cyl_features: Vec<_> = features
            .iter()
            .filter(|f| {
                matches!(
                    f,
                    RecognizedFeature::ThroughHole { .. }
                        | RecognizedFeature::BlindHole { .. }
                        | RecognizedFeature::CylindricalBoss { .. }
                )
            })
            .collect();

        assert!(
            !cyl_features.is_empty(),
            "Cylinder should be recognized as a cylindrical feature"
        );
    }

    #[test]
    fn test_feature_descriptions() {
        let hole = RecognizedFeature::ThroughHole {
            diameter: 10.0,
            axis: [0.0, 0.0, 1.0],
            face_id: 0,
        };
        let desc = hole.to_description();
        assert!(desc.contains("through-hole"));
        assert!(desc.contains("10.0"));
        assert!(desc.contains("Z"));

        let fillet = RecognizedFeature::Fillet {
            radius: 2.5,
            face_ids: vec![1, 2, 3],
        };
        let desc = fillet.to_description();
        assert!(desc.contains("fillet"));
        assert!(desc.contains("2.5"));
        assert!(desc.contains("3 faces"));
    }
}

/// Hole-vs-boss and THRU-vs-blind must follow the kernel's material-side rule
/// (`readable::bore_face_ids`) and its proven bore metrics (`dfm::bore_metrics`),
/// not face-adjacency counts: `Face::adjacent_faces` is never populated outside
/// tests, so the old heuristic labelled every cylinder a boss.
#[cfg(test)]
mod material_side_tests {
    use super::*;
    use crate::dfm::analyzers::bore::fixtures::{plate_with_blind_bore, plate_with_through_bore};

    #[test]
    fn through_bore_in_plate_is_a_through_hole_not_a_boss() {
        let (model, solid_id, bore_face) = plate_with_through_bore(20.0, 20.0, 10.0, 3.0);
        let features = recognize_features(solid_id, &model);

        let on_bore: Vec<_> = features
            .iter()
            .filter(|f| f.face_ids().contains(&bore_face))
            .collect();
        assert_eq!(
            on_bore.len(),
            1,
            "one feature on the bore wall: {features:?}"
        );
        match on_bore[0] {
            RecognizedFeature::ThroughHole { diameter, .. } => {
                assert!((diameter - 6.0).abs() < 1e-9, "diameter {diameter}")
            }
            other => panic!("bore wall must be a through-hole, got {other:?}"),
        }
    }

    #[test]
    fn blind_bore_in_plate_is_a_blind_hole_with_its_depth() {
        let (model, solid_id, bore_face) = plate_with_blind_bore(20.0, 20.0, 10.0, 3.0, 4.0);
        let features = recognize_features(solid_id, &model);

        let on_bore: Vec<_> = features
            .iter()
            .filter(|f| f.face_ids().contains(&bore_face))
            .collect();
        assert_eq!(
            on_bore.len(),
            1,
            "one feature on the bore wall: {features:?}"
        );
        match on_bore[0] {
            RecognizedFeature::BlindHole {
                diameter, depth, ..
            } => {
                assert!((diameter - 6.0).abs() < 1e-9, "diameter {diameter}");
                assert!((depth - 4.0).abs() < 1e-9, "depth {depth}");
            }
            other => panic!("bore wall must be a blind hole, got {other:?}"),
        }
    }

    #[test]
    fn solid_cylinder_outer_wall_is_a_boss_not_a_hole() {
        use crate::primitives::cylinder_primitive::{CylinderParameters, CylinderPrimitive};
        use crate::primitives::primitive_traits::Primitive;
        let mut model = BRepModel::new();
        let params = CylinderParameters::new(10.0, 30.0).unwrap();
        let solid_id = CylinderPrimitive::create(params, &mut model).unwrap();

        let features = recognize_features(solid_id, &model);
        assert!(
            features.iter().any(|f| matches!(
                f,
                RecognizedFeature::CylindricalBoss { diameter, .. } if (diameter - 20.0).abs() < 1e-9
            )),
            "outer wall must be a Ø20 boss: {features:?}"
        );
        assert!(
            !features.iter().any(|f| matches!(
                f,
                RecognizedFeature::ThroughHole { .. } | RecognizedFeature::BlindHole { .. }
            )),
            "a solid cylinder has no holes: {features:?}"
        );
    }
}

#[cfg(test)]
mod face_ids_tests {
    use super::*;

    #[test]
    fn through_hole_reports_single_face() {
        let f = RecognizedFeature::ThroughHole {
            diameter: 2.0,
            axis: [0.0, 0.0, 1.0],
            face_id: 7,
        };
        assert_eq!(f.face_ids(), vec![7]);
    }

    #[test]
    fn fillet_reports_all_faces() {
        let f = RecognizedFeature::Fillet {
            radius: 1.0,
            face_ids: vec![3, 4, 5],
        };
        assert_eq!(f.face_ids(), vec![3, 4, 5]);
    }
}
