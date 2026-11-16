// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A fillet band cut at an angle -- one end a circular arc, the other an
//! elliptical arc where a slanted plane meets the cylinder -- has straight
//! rails of DIFFERENT lengths. The band stitcher paired its two curved ends by
//! whichever endpoints were nearest, which on such a band is the wrong pair:
//! it stitched the band diagonally, its rails stayed unmeshed and the part's
//! mesh opened (one CADGenBench part, rails 6.6 and 17.0 mm). The loop says
//! which ends each rail joins; the band must follow it.

use std::collections::{HashMap, HashSet};

use export_engine::ExportEngine;
use geometry_engine::math::Point3;
use geometry_engine::tessellation::{
    edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
};

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('band'),'2;1');\n\
         FILE_NAME('band.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// A quarter of the radius-2 cylinder about +Z (angle 0 to 90 degrees) from
/// z = 0 up to the slanted plane z = 10 + 3x: bottom arc A(2,0,0)-B(0,2,0),
/// rail B-C(0,2,10) (10 long), top elliptical arc C-D(2,0,16), rail D-A
/// (16 long).
fn slanted_band() -> String {
    let r10 = 10f64.sqrt();
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=CYLINDRICAL_SURFACE('',#4,2.);
#11=CARTESIAN_POINT('',(2.,0.,0.));
#12=CARTESIAN_POINT('',(0.,2.,0.));
#13=CARTESIAN_POINT('',(0.,2.,10.));
#14=CARTESIAN_POINT('',(2.,0.,16.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#31=CIRCLE('',#4,2.);
#40=CARTESIAN_POINT('',(0.,0.,10.));
#41=DIRECTION('',({nx:.17},0.,{nz:.17}));
#42=DIRECTION('',({xx:.17},0.,{xz:.17}));
#43=AXIS2_PLACEMENT_3D('',#40,#41,#42);
#32=ELLIPSE('',#43,{a:.17},2.);
#33=VECTOR('',#2,1.);
#34=LINE('',#12,#33);
#35=LINE('',#11,#33);
#51=EDGE_CURVE('',#21,#22,#31,.T.);
#52=EDGE_CURVE('',#22,#23,#34,.T.);
#53=EDGE_CURVE('',#24,#23,#32,.T.);
#54=EDGE_CURVE('',#21,#24,#35,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#54,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#5,.T.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));",
        nx = -3.0 / r10,
        nz = 1.0 / r10,
        xx = 1.0 / r10,
        xz = 3.0 / r10,
        a = 2.0 * r10
    )
}

/// The same quarter cylinder between two PARALLEL slanted planes, z = 3x and
/// z = 3x + 10: both ends are the same ellipse, so they carry the same number
/// of samples (as on CADGenBench part 108, 57 and 57) and the band stitcher
/// takes the face. Bottom arc A(2,0,6)-B(0,2,0), rail B-C(0,2,10), top arc
/// C-D(2,0,16), rail D-A. Both rails are 10 long, but A is only 4.9 from C:
/// pairing the ends by distance joins A to C, across the band.
fn parallel_slanted_band() -> String {
    let r10 = 10f64.sqrt();
    format!(
        "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,0.,1.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=CYLINDRICAL_SURFACE('',#4,2.);
#11=CARTESIAN_POINT('',(2.,0.,6.));
#12=CARTESIAN_POINT('',(0.,2.,0.));
#13=CARTESIAN_POINT('',(0.,2.,10.));
#14=CARTESIAN_POINT('',(2.,0.,16.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#41=DIRECTION('',({nx:.17},0.,{nz:.17}));
#42=DIRECTION('',({xx:.17},0.,{xz:.17}));
#44=AXIS2_PLACEMENT_3D('',#1,#41,#42);
#31=ELLIPSE('',#44,{a:.17},2.);
#40=CARTESIAN_POINT('',(0.,0.,10.));
#43=AXIS2_PLACEMENT_3D('',#40,#41,#42);
#32=ELLIPSE('',#43,{a:.17},2.);
#33=VECTOR('',#2,1.);
#34=LINE('',#12,#33);
#35=LINE('',#11,#33);
#51=EDGE_CURVE('',#21,#22,#31,.T.);
#52=EDGE_CURVE('',#22,#23,#34,.T.);
#53=EDGE_CURVE('',#24,#23,#32,.T.);
#54=EDGE_CURVE('',#21,#24,#35,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.F.);
#64=ORIENTED_EDGE('',*,*,#54,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#5,.T.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));",
        nx = -3.0 / r10,
        nz = 1.0 / r10,
        xx = 1.0 / r10,
        xz = 3.0 / r10,
        a = 2.0 * r10
    )
}

#[test]
fn a_slanted_fillet_band_meshes_along_its_own_rails() {
    band_meshes_along_its_rails(&slanted_band());
}

#[test]
fn a_band_between_parallel_slanted_cuts_meshes_along_its_own_rails() {
    band_meshes_along_its_rails(&parallel_slanted_band());
}

fn band_meshes_along_its_rails(body: &str) {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(body), "band")
        .expect("imports");
    let faces: Vec<_> = model.faces.iter().collect();
    assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
    let (_, face) = faces[0];
    let params = TessellationParams::default();
    let cache = EdgeSampleCache::new(&params);
    let mut mesh = TriangleMesh::new();
    tessellate_face(face, &model, &params, &cache, &mut mesh);
    assert!(!mesh.triangles.is_empty(), "the band meshes");

    let key = |p: Point3| {
        (
            (p.x * 1e6).round() as i64,
            (p.y * 1e6).round() as i64,
            (p.z * 1e6).round() as i64,
        )
    };
    let mut uses: HashMap<_, usize> = HashMap::new();
    for t in &mesh.triangles {
        for k in 0..3 {
            let (a, b) = (
                key(mesh.vertices[t[k] as usize].position),
                key(mesh.vertices[t[(k + 1) % 3] as usize].position),
            );
            *uses.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut boundary = HashSet::new();
    let lp = model.loops.get(face.outer_loop).expect("loop");
    for &e in &lp.edges {
        let s = cache.get_or_compute(e, &model);
        for w in s.windows(2) {
            let (a, b) = (key(w[0]), key(w[1]));
            boundary.insert((a.min(b), a.max(b)));
        }
    }
    let unused = boundary.iter().filter(|e| !uses.contains_key(*e)).count();
    assert_eq!(
        unused, 0,
        "{unused} boundary segment(s) in no triangle: the rails were skipped"
    );
    let holes = uses
        .iter()
        .filter(|(e, n)| **n == 1 && !boundary.contains(*e))
        .count();
    assert_eq!(
        holes, 0,
        "{holes} edge(s) inside the band used by one triangle only"
    );
}
