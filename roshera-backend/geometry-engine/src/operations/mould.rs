//! The mould verb (#16, thin vertical slice) — edit a dimension, re-evaluate,
//! references survive.
//!
//! This is the agent's core edit loop, built on the event-sourcing-as-source-of-
//! truth model: a [`Recipe`] is a replayable sequence of parameterised steps (an
//! event log). `evaluate` replays it into a fresh `BRepModel`, setting each
//! step's stable event key so the persistent-id lineage (#11) derives
//! deterministically. `set_dimension` is the MOULD verb: it edits one numeric
//! parameter on one step. Because each step carries a fixed event key and the
//! kernel derives persistent-ids from operation lineage (not from the resulting
//! geometry), re-evaluating the moulded recipe yields a DIFFERENT shape whose
//! topology keeps the SAME persistent-ids — so an agent that grabbed "the top
//! cap of the boss" before the edit can still resolve it after.
//!
//! Here the recipe IS the timeline; in the productised path (#11 slice 40-G) the
//! real `timeline-engine` event log replaces it and the same `set_event_key`
//! seam is driven from replay. This slice proves the loop end-to-end on the
//! lineage that primitives (40-B) and extrude (40-C) already carry.

use crate::math::{Point3, Vector3};
use crate::operations::extrude::{extrude_profile, ExtrudeOptions};
use crate::operations::{OperationError, OperationResult};
use crate::primitives::curve::{Line, ParameterRange};
use crate::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// One step of a [`Recipe`] — a parameterised, replayable operation. Each
/// carries a stable `key` (its timeline event id) that seeds the persistent-id
/// lineage, so re-evaluation is deterministic and edit-stable.
#[derive(Debug, Clone)]
pub enum Step {
    /// A box primitive centred at the origin.
    Box {
        key: String,
        width: f64,
        height: f64,
        depth: f64,
    },
    /// Extrude a square (side `side`) in the z=0 plane along +Z by `dist`.
    ExtrudeSquare { key: String, side: f64, dist: f64 },
}

impl Step {
    /// Named numeric parameters the mould verb can edit on this step.
    pub fn dimensions(&self) -> &'static [&'static str] {
        match self {
            Step::Box { .. } => &["width", "height", "depth"],
            Step::ExtrudeSquare { .. } => &["side", "dist"],
        }
    }

    fn set(&mut self, param: &str, value: f64) -> bool {
        match self {
            Step::Box {
                width,
                height,
                depth,
                ..
            } => match param {
                "width" => *width = value,
                "height" => *height = value,
                "depth" => *depth = value,
                _ => return false,
            },
            Step::ExtrudeSquare { side, dist, .. } => match param {
                "side" => *side = value,
                "dist" => *dist = value,
                _ => return false,
            },
        }
        true
    }

    fn key(&self) -> &str {
        match self {
            Step::Box { key, .. } | Step::ExtrudeSquare { key, .. } => key,
        }
    }
}

/// A replayable design — the event log that is the single source of truth.
#[derive(Debug, Clone, Default)]
pub struct Recipe {
    pub steps: Vec<Step>,
}

impl Recipe {
    pub fn new(steps: Vec<Step>) -> Self {
        Recipe { steps }
    }

    /// Replay the recipe into a fresh model, returning the model + the last
    /// solid produced. Each step's event key drives the persistent-id lineage,
    /// so the same recipe always rebuilds the same persistent-ids.
    pub fn evaluate(&self) -> OperationResult<(BRepModel, SolidId)> {
        let mut model = BRepModel::new();
        let mut last: Option<SolidId> = None;
        for step in &self.steps {
            model.set_event_key(Some(step.key().to_string()));
            let solid = apply_step(&mut model, step)?;
            model.set_event_key(None);
            last = Some(solid);
        }
        let solid = last.ok_or_else(|| OperationError::InvalidGeometry("empty recipe".into()))?;
        Ok((model, solid))
    }

    /// The MOULD verb: edit one numeric `param` on step `index` to `value`.
    /// Returns false if the step or parameter name is unknown. The caller
    /// re-evaluates to realise the edit; persistent-ids of unchanged-role
    /// topology survive.
    pub fn set_dimension(&mut self, index: usize, param: &str, value: f64) -> bool {
        match self.steps.get_mut(index) {
            Some(step) => step.set(param, value),
            None => false,
        }
    }
}

fn apply_step(model: &mut BRepModel, step: &Step) -> OperationResult<SolidId> {
    match step {
        Step::Box {
            width,
            height,
            depth,
            ..
        } => {
            let g = TopologyBuilder::new(model)
                .create_box_3d(*width, *height, *depth)
                .map_err(|e| OperationError::InvalidGeometry(format!("box: {e:?}")))?;
            solid_of(g)
        }
        Step::ExtrudeSquare { side, dist, .. } => {
            let edges = square_profile(model, *side);
            let opts = ExtrudeOptions {
                direction: Vector3::Z,
                distance: *dist,
                cap_ends: true,
                ..Default::default()
            };
            extrude_profile(model, edges, opts)
        }
    }
}

fn solid_of(g: GeometryId) -> OperationResult<SolidId> {
    match g {
        GeometryId::Solid(s) => Ok(s),
        o => Err(OperationError::InvalidGeometry(format!(
            "expected solid, got {o:?}"
        ))),
    }
}

/// Build a `side`-wide square profile in the z=0 plane as 4 line edges.
fn square_profile(model: &mut BRepModel, side: f64) -> Vec<EdgeId> {
    let h = side / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let verts: Vec<_> = pts
        .iter()
        .map(|(x, y)| model.vertices.add(*x, *y, 0.0))
        .collect();
    let mut edges = Vec::new();
    for i in 0..4 {
        let j = (i + 1) % 4;
        let line = Line::new(
            Point3::new(pts[i].0, pts[i].1, 0.0),
            Point3::new(pts[j].0, pts[j].1, 0.0),
        );
        let cid = model.curves.add(Box::new(line));
        edges.push(model.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::surface::Plane;

    /// The top cap of a +Z extrusion: the planar face whose outward normal is
    /// +Z. Returns `(face_id, height)`.
    fn top_cap(model: &BRepModel, solid: SolidId) -> (u32, f64) {
        let s = model.solids.get(solid).expect("solid");
        let shell = model.shells.get(s.outer_shell).expect("shell");
        let mut best: Option<(u32, f64)> = None;
        for &fid in &shell.faces {
            let face = model.faces.get(fid).expect("face");
            let surf = model.surfaces.get(face.surface_id).expect("surface");
            if let Some(p) = surf.as_any().downcast_ref::<Plane>() {
                let n = p.normal.normalize().unwrap_or(Vector3::Z);
                // Account for face orientation: the cap's OUTWARD normal is +Z.
                let sign = match face.orientation {
                    crate::primitives::face::FaceOrientation::Forward => 1.0,
                    crate::primitives::face::FaceOrientation::Backward => -1.0,
                };
                if (n * sign).dot(&Vector3::Z) > 0.99 {
                    let z = p.origin.z;
                    if best.map(|b| z > b.1).unwrap_or(true) {
                        best = Some((fid, z));
                    }
                }
            }
        }
        best.expect("a +Z planar cap")
    }

    #[test]
    fn mould_dimension_preserves_pid_reference() {
        // An agent designs: extrude a 10mm square by 10mm. It grabs a durable
        // reference to "the top cap".
        let mut recipe = Recipe::new(vec![Step::ExtrudeSquare {
            key: "boss".into(),
            side: 10.0,
            dist: 10.0,
        }]);
        let (m1, s1) = recipe.evaluate().expect("eval 1");
        let (cap1, h1) = top_cap(&m1, s1);
        assert!((h1 - 10.0).abs() < 1e-9, "cap starts at z=10");
        let cap_pid = m1.face_pid(cap1).expect("cap has a persistent id");

        // The agent MOULDS the height: dist 10 → 30, then re-evaluates.
        assert!(recipe.set_dimension(0, "dist", 30.0), "set dist");
        let (m2, _s2) = recipe.evaluate().expect("eval 2");

        // The durable reference STILL resolves — to the moved cap.
        let cap2 = m2
            .face_by_pid(cap_pid)
            .expect("the cap PID survives the dimension edit");
        let (cap2_geo, h2) = top_cap(&m2, _s2);
        assert_eq!(cap2, cap2_geo, "the surviving PID names the new top cap");
        assert!(
            (h2 - 30.0).abs() < 1e-9,
            "the edit took effect: cap now at z=30"
        );
        // The transient FaceId may differ; the PID is what carried the identity.
    }

    #[test]
    fn mould_unknown_param_is_rejected() {
        let mut recipe = Recipe::new(vec![Step::Box {
            key: "b".into(),
            width: 1.0,
            height: 1.0,
            depth: 1.0,
        }]);
        assert!(!recipe.set_dimension(0, "radius", 2.0), "unknown param");
        assert!(!recipe.set_dimension(9, "width", 2.0), "unknown step");
        assert!(recipe.set_dimension(0, "width", 2.0), "known param");
    }

    #[test]
    fn re_evaluation_is_deterministic() {
        let recipe = Recipe::new(vec![
            Step::Box {
                key: "base".into(),
                width: 20.0,
                height: 20.0,
                depth: 5.0,
            },
            Step::ExtrudeSquare {
                key: "boss".into(),
                side: 8.0,
                dist: 12.0,
            },
        ]);
        let (m1, s1) = recipe.evaluate().expect("a");
        let (m2, s2) = recipe.evaluate().expect("b");
        // Same recipe → same solid PID for the boss.
        assert_eq!(m1.solid_pid(s1), m2.solid_pid(s2));
    }
}
