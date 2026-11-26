// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The face-orientation check decides which side of an edge a face lies on by
//! projecting the face's outline along the normal at that edge and counting
//! crossings. That picture is the face only while the projected outline is a
//! simple polygon.
//!
//! A thread flank is a helicoid: it winds round its axis several times, every
//! turn directly above the one before, and all its normals within a few
//! degrees of each other -- so the "no part of the face is seen edge-on"
//! gate passes it. Seen along the normal the turns land on each other, the
//! outline passes through itself, and the crossing count at the end of the
//! flank only says whether the number of turns stacked there is odd or even.
//! The check reported a thread's flank and the flat face ending it as walking
//! their shared edge the same way although their loops oppose (CADGenBench
//! part 109).

use std::f64::consts::TAU;

use geometry_engine::math::nurbs::{interpolate_nurbs_curve, ParameterizationType};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::curve::{Line, NurbsCurve, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::shell::{Shell, ShellId, ShellType};
use geometry_engine::primitives::surface::{Plane, RuledSurface};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::check_face_orientations;

const INNER: f64 = 5.0;
const OUTER: f64 = 8.0;
const PITCH: f64 = 2.0;
/// The outer helix rides this much above the inner one: a flank, not a flat
/// spiral ramp.
const RISE: f64 = 1.5;

fn helix(radius: f64, z0: f64, turns: f64) -> (NurbsCurve, ParameterRange) {
    let n = (turns * 48.0).ceil() as usize;
    let pts: Vec<Point3> = (0..=n)
        .map(|k| {
            let a = TAU * turns * k as f64 / n as f64;
            Point3::new(radius * a.cos(), radius * a.sin(), z0 + PITCH * a / TAU)
        })
        .collect();
    let fit = interpolate_nurbs_curve(&pts, 3, ParameterizationType::ChordLength).expect("fit");
    let knots = fit.knots.values().to_vec();
    let range = ParameterRange::new(knots[0], knots[knots.len() - 1]);
    let curve = NurbsCurve::new(
        fit.degree,
        fit.control_points.clone(),
        fit.weights.clone(),
        knots,
    )
    .expect("curve");
    (curve, range)
}

/// A helical flank of `turns` turns between radius 5 and radius 8, and a flat
/// face hanging below its first ruling (in the plane y = 0), the two sharing
/// that ruling and walking it in opposite directions. Returns the shell and
/// the shared edge.
fn flank_and_end_face(turns: f64) -> (BRepModel, ShellId, u32) {
    let mut model = BRepModel::new();
    let top_z = PITCH * turns;
    let a = model.vertices.add(INNER, 0.0, 0.0);
    let b = model.vertices.add(OUTER, 0.0, RISE);
    let end = TAU * turns;
    let c = model
        .vertices
        .add(INNER * end.cos(), INNER * end.sin(), top_z);
    let d = model
        .vertices
        .add(OUTER * end.cos(), OUTER * end.sin(), top_z + RISE);
    let a_low = model.vertices.add(INNER, 0.0, -1.0);
    let b_low = model.vertices.add(OUTER, 0.0, -1.0);
    let point = |v: u32| {
        let p = model.vertices.get(v).expect("vertex").position;
        Point3::new(p[0], p[1], p[2])
    };
    let (pa, pb, pc, pd, pal, pbl) = (
        point(a),
        point(b),
        point(c),
        point(d),
        point(a_low),
        point(b_low),
    );

    let (inner, inner_range) = helix(INNER, 0.0, turns);
    let (outer, outer_range) = helix(OUTER, RISE, turns);
    let flank = model.surfaces.add(Box::new(RuledSurface::new(
        Box::new(inner.clone()),
        Box::new(outer.clone()),
    )));
    let inner_curve = model.curves.add(Box::new(inner));
    let outer_curve = model.curves.add(Box::new(outer));
    let mut line = |from: Point3, to: Point3, s: u32, e: u32, model: &mut BRepModel| {
        let curve = model.curves.add(Box::new(Line::new(from, to)));
        model.edges.add(Edge::new(
            0,
            s,
            e,
            curve,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ))
    };
    let e_inner = model.edges.add(Edge::new(
        0,
        a,
        c,
        inner_curve,
        EdgeOrientation::Forward,
        inner_range,
    ));
    let e_outer = model.edges.add(Edge::new(
        0,
        b,
        d,
        outer_curve,
        EdgeOrientation::Forward,
        outer_range,
    ));
    let e_top = line(pc, pd, c, d, &mut model);
    let e_bottom = line(pa, pb, a, b, &mut model);
    let e_right = line(pb, pbl, b, b_low, &mut model);
    let e_low = line(pbl, pal, b_low, a_low, &mut model);
    let e_left = line(pal, pa, a_low, a, &mut model);

    // The ruled surface's normal is (along the helix) x (outward along the
    // ruling): downward. Walked with the face on its left under that
    // normal: up the inner helix, out along the top ruling, back down the
    // outer helix, in along the bottom ruling.
    let mut flank_loop = Loop::new(0, LoopType::Outer);
    flank_loop.add_edge(e_inner, true);
    flank_loop.add_edge(e_top, true);
    flank_loop.add_edge(e_outer, false);
    flank_loop.add_edge(e_bottom, false);
    let flank_loop = model.loops.add(flank_loop);
    let flank_face = model
        .faces
        .add(Face::new(0, flank, flank_loop, FaceOrientation::Forward));

    // The end face walks the shared ruling the other way (a -> b), then
    // down, back and up: anticlockwise seen from +Y.
    let plane = model.surfaces.add(Box::new(
        Plane::new(Point3::new(0.0, 0.0, 0.0), Vector3::Y, Vector3::Z).expect("plane"),
    ));
    let mut end_loop = Loop::new(0, LoopType::Outer);
    end_loop.add_edge(e_bottom, true);
    end_loop.add_edge(e_right, true);
    end_loop.add_edge(e_low, true);
    end_loop.add_edge(e_left, true);
    let end_loop = model.loops.add(end_loop);
    let end_face = model
        .faces
        .add(Face::new(0, plane, end_loop, FaceOrientation::Forward));

    let mut shell = Shell::new(0, ShellType::Open);
    shell.add_face(flank_face);
    shell.add_face(end_face);
    let shell = model.shells.add(shell);
    (model, shell, e_bottom)
}

fn same_way_errors(turns: f64) -> Vec<String> {
    let (model, shell, _) = flank_and_end_face(turns);
    check_face_orientations(&model, shell)
        .iter()
        .map(|e| format!("{e:?}"))
        .collect()
}

/// The calibration case: under one turn the flank's outline does not pass
/// over itself, the check decides the shared ruling, and decides it right.
#[test]
fn a_flank_of_under_one_turn_and_its_end_face_are_consistent() {
    let errors = same_way_errors(0.8);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// And it still catches the real defect there: the end face turned over.
#[test]
fn a_flank_of_under_one_turn_with_its_end_face_turned_over_is_caught() {
    let (mut model, shell, shared) = flank_and_end_face(0.8);
    let end_face = model.shells.get(shell).expect("shell").faces[1];
    {
        let face = model.faces.get_mut(end_face).expect("face");
        face.orientation = face.orientation.flipped();
    }
    let errors: Vec<String> = check_face_orientations(&model, shell)
        .iter()
        .map(|e| format!("{e:?}"))
        .collect();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("have same orientation") && e.contains(&format!("edge {shared}"))),
        "{errors:#?}"
    );
}

/// Wound more than once, the stack of turns over the shared ruling is odd
/// for some turn counts and even for others; none of them is an error.
#[test]
fn a_flank_wound_several_times_and_its_end_face_are_consistent() {
    for turns in [1.5, 2.5, 3.5, 4.5, 2.0, 3.0, 7.25] {
        let errors = same_way_errors(turns);
        assert!(errors.is_empty(), "{turns} turns: {errors:#?}");
    }
}
