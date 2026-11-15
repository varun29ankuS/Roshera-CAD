// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An imported torus face that is only a PATCH of its torus -- a fillet
//! quarter bounded by four arcs, as every STEP writer emits a rolled edge --
//! must mesh as that patch, from its own edges. One CADGenBench part had 95
//! such faces fall through to the untrimmed torus grid, which meshes the
//! WHOLE torus and ignores the edges: the patch never met its neighbours and
//! the part's mesh was left with thousands of open edges.

use export_engine::ExportEngine;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::{
    edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
};
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

const RING: f64 = 10.0;
const TUBE: f64 = 2.0;

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('patch'),'2;1');\n\
         FILE_NAME('patch.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A torus about +Z (ring 10, tube 2) and the patch of it from ring angle
/// `u0` to `u1` (counter-clockwise) and from the outer equator up to the top
/// of the tube: corners A (outer, u0), B (outer, u1), C (top, u1), D (top,
/// u0), bounded by the equator arc A-B, the tube arc B-C, the top arc D-C and
/// the tube arc A-D.
fn patch(u0: f64, u1: f64) -> String {
    let (c0, s0, c1, s1) = (u0.cos(), u0.sin(), u1.cos(), u1.sin());
    let (o, t) = (RING + TUBE, RING);
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=TOROIDAL_SURFACE('',#4,{RING:.1},{TUBE:.1});
#11=CARTESIAN_POINT('',({ax:.17},{ay:.17},0.));
#12=CARTESIAN_POINT('',({bx:.17},{by:.17},0.));
#13=CARTESIAN_POINT('',({cx:.17},{cy:.17},{TUBE:.1}));
#14=CARTESIAN_POINT('',({dx:.17},{dy:.17},{TUBE:.1}));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#31=CIRCLE('',#4,{o:.1});
#40=CARTESIAN_POINT('',({b0x:.17},{b0y:.17},0.));
#41=DIRECTION('',({s1:.17},{nc1:.17},0.));
#42=DIRECTION('',({c1:.17},{s1:.17},0.));
#43=AXIS2_PLACEMENT_3D('',#40,#41,#42);
#32=CIRCLE('',#43,{TUBE:.1});
#44=CARTESIAN_POINT('',(0.,0.,{TUBE:.1}));
#45=AXIS2_PLACEMENT_3D('',#44,#2,#3);
#33=CIRCLE('',#45,{t:.1});
#46=CARTESIAN_POINT('',({a0x:.17},{a0y:.17},0.));
#47=DIRECTION('',({s0:.17},{nc0:.17},0.));
#48=DIRECTION('',({c0:.17},{s0:.17},0.));
#49=AXIS2_PLACEMENT_3D('',#46,#47,#48);
#34=CIRCLE('',#49,{TUBE:.1});
#51=EDGE_CURVE('',#21,#22,#31,.T.);
#52=EDGE_CURVE('',#22,#23,#32,.T.);
#53=EDGE_CURVE('',#24,#23,#33,.T.);
#54=EDGE_CURVE('',#21,#24,#34,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#54,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#5,.T.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));",
        ax = o * c0,
        ay = o * s0,
        bx = o * c1,
        by = o * s1,
        cx = t * c1,
        cy = t * s1,
        dx = t * c0,
        dy = t * s0,
        b0x = RING * c1,
        b0y = RING * s1,
        a0x = RING * c0,
        a0y = RING * s0,
        nc0 = -c0,
        nc1 = -c1,
    )
}

/// How far counter-clockwise ring angle `u` is past `u0`, in [0, 2pi).
fn past(u: f64, u0: f64) -> f64 {
    (u - u0).rem_euclid(TAU)
}

fn the_torus_face(model: &BRepModel) -> &geometry_engine::primitives::face::Face {
    let faces: Vec<_> = model
        .faces
        .iter()
        .filter(|(_, f)| {
            model
                .surfaces
                .get(f.surface_id)
                .is_some_and(|s| s.type_name() == "Torus")
        })
        .collect();
    assert_eq!(faces.len(), 1, "one torus face");
    faces[0].1
}

/// The patch from `u0` to `u1` imports, its edges lie on it, and it meshes
/// to triangles that all lie on it too.
fn assert_meshes_as_the_patch(u0: f64, u1: f64) {
    let span = u1 - u0;
    let (model, _report) = ExportEngine::new()
        .import_step_content(&envelope(&patch(u0, u1)), "patch")
        .expect("imports");
    let face = the_torus_face(&model);
    let on_patch = |p: geometry_engine::math::Point3, slack: f64| {
        let along = past(p.y.atan2(p.x) + slack, u0);
        along <= span + 2.0 * slack && p.z >= -slack
    };

    // The fixture's edges really are on the patch.
    for (_, e) in model.edges.iter() {
        let c = model.curves.get(e.curve_id).expect("curve");
        for k in 0..=8 {
            let t =
                e.param_range.start + (e.param_range.end - e.param_range.start) * k as f64 / 8.0;
            let p = c.evaluate(t).expect("evaluates").position;
            assert!(
                on_patch(p, 1e-9),
                "fixture edge point {p:?} is off the patch"
            );
        }
    }

    let params = TessellationParams::default();
    let cache = EdgeSampleCache::new(&params);
    let mut mesh = TriangleMesh::new();
    tessellate_face(face, &model, &params, &cache, &mut mesh);
    assert!(
        !mesh.triangles.is_empty(),
        "the patch u in [{u0:.3}, {u1:.3}] meshes"
    );
    for v in &mesh.vertices {
        assert!(
            on_patch(v.position, 1e-6),
            "patch u in [{u0:.3}, {u1:.3}]: vertex {:?} lies outside it -- the whole torus was meshed",
            v.position
        );
    }
}

#[test]
fn a_torus_fillet_patch_meshes_as_the_patch() {
    assert_meshes_as_the_patch(0.0, FRAC_PI_2);
}

/// A patch that runs across the torus's own seam (ring angle 0) must unwrap
/// there: one CADGenBench part's fillet quarters from 210 degrees round to 360
/// were refused with an outline that crossed itself.
#[test]
fn a_torus_patch_across_the_seam_meshes_as_the_patch() {
    assert_meshes_as_the_patch(-FRAC_PI_4, FRAC_PI_4);
    assert_meshes_as_the_patch(7.0 * PI / 6.0, TAU);
}
