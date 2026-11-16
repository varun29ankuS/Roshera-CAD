// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A four-sided torus patch -- a quarter of the tube between two arcs round
//! the ring and two arcs across the tube, as every STEP writer emits a
//! rolled edge -- must mesh with every facet facing along the surface. On a
//! thin tube along a long ring (CADGenBench part 102: ring 107 mm, tube
//! 1 mm, ~225 degrees round) the general triangulator left a facet facing
//! away from the surface, which fails the display-mesh check: its rim
//! chords are 4 mm long against a 1 mm tube, and an interior point placed
//! 0.02 mm inside one landed on the chord's far side.

use export_engine::ExportEngine;
use geometry_engine::math::Point3;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::tessellation::{
    edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
};

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('patch'),'2;1');
         FILE_NAME('patch.step','2026-01-01T00:00:00',(''),(''),'','','');
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));
ENDSEC;
DATA;
{body}
ENDSEC;
         END-ISO-10303-21;
"
    )
}

fn patch(ring: f64, tube: f64, u0: f64, u1: f64) -> String {
    let (c0, s0, c1, s1) = (u0.cos(), u0.sin(), u1.cos(), u1.sin());
    let (o, t) = (ring + tube, ring);
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=TOROIDAL_SURFACE('',#4,{ring:.1},{tube:.1});
#11=CARTESIAN_POINT('',({ax:.17},{ay:.17},0.));
#12=CARTESIAN_POINT('',({bx:.17},{by:.17},0.));
#13=CARTESIAN_POINT('',({cx:.17},{cy:.17},{tube:.1}));
#14=CARTESIAN_POINT('',({dx:.17},{dy:.17},{tube:.1}));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#31=CIRCLE('',#4,{o:.1});
#40=CARTESIAN_POINT('',({b0x:.17},{b0y:.17},0.));
#41=DIRECTION('',({s1:.17},{nc1:.17},0.));
#42=DIRECTION('',({c1:.17},{s1:.17},0.));
#43=AXIS2_PLACEMENT_3D('',#40,#41,#42);
#32=CIRCLE('',#43,{tube:.1});
#44=CARTESIAN_POINT('',(0.,0.,{tube:.1}));
#45=AXIS2_PLACEMENT_3D('',#44,#2,#3);
#33=CIRCLE('',#45,{t:.1});
#46=CARTESIAN_POINT('',({a0x:.17},{a0y:.17},0.));
#47=DIRECTION('',({s0:.17},{nc0:.17},0.));
#48=DIRECTION('',({c0:.17},{s0:.17},0.));
#49=AXIS2_PLACEMENT_3D('',#46,#47,#48);
#34=CIRCLE('',#49,{tube:.1});
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
        b0x = ring * c1,
        b0y = ring * s1,
        a0x = ring * c0,
        a0y = ring * s0,
        nc0 = -c0,
        nc1 = -c1,
    )
}

fn the_torus_face(model: &BRepModel) -> &geometry_engine::primitives::face::Face {
    model
        .faces
        .iter()
        .map(|(_, f)| f)
        .find(|f| {
            model
                .surfaces
                .get(f.surface_id)
                .is_some_and(|s| s.type_name() == "Torus")
        })
        .expect("a torus face")
}

/// Mesh the patch at the certificate's display parameters; the facets
/// facing away from the torus (the patch is `.T.`: outward is away from
/// the tube's core circle).
fn facets_against(ring: f64, tube: f64, u0: f64, u1: f64) -> (usize, usize) {
    let (model, _) = ExportEngine::new()
        .import_step_content(&envelope(&patch(ring, tube, u0, u1)), "patch")
        .expect("imports");
    let face = the_torus_face(&model);
    let params = TessellationParams::default();
    let cache = EdgeSampleCache::new(&params);
    let mut mesh = TriangleMesh::new();
    tessellate_face(face, &model, &params, &cache, &mut mesh);
    let outward = |p: Point3| {
        let rad = (p.x * p.x + p.y * p.y).sqrt();
        let core = Point3::new(p.x * ring / rad, p.y * ring / rad, 0.0);
        p - core
    };
    let bad = mesh
        .triangles
        .iter()
        .filter(|t| {
            let [a, b, c] = t.map(|i| mesh.vertices[i as usize].position);
            let m = Point3::new(
                (a.x + b.x + c.x) / 3.0,
                (a.y + b.y + c.y) / 3.0,
                (a.z + b.z + c.z) / 3.0,
            );
            (b - a).cross(&(c - a)).dot(&outward(m)) <= 0.0
        })
        .count();
    (bad, mesh.triangles.len())
}

#[test]
fn a_thin_long_torus_patch_has_no_facet_facing_away() {
    let mut bad = Vec::new();
    for (ring, tube) in [
        (107.0, 1.0),
        (103.0, 3.0),
        (67.8, 1.0),
        (130.0, 5.0),
        (10.0, 2.0),
    ] {
        for span_deg in [30.0_f64, 90.0, 180.0, 225.0, 300.0] {
            let (u0, u1) = (0.3_f64, 0.3 + span_deg.to_radians());
            let (n, of) = facets_against(ring, tube, u0, u1);
            assert!(
                of > 0,
                "ring {ring} tube {tube} span {span_deg}: the patch meshes"
            );
            if n > 0 {
                bad.push((ring, tube, span_deg, n, of));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "facets facing away (ring, tube, span deg, count, of): {bad:?}"
    );
}
