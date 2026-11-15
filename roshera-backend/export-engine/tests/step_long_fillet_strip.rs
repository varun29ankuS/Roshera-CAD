// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A long, thin fillet strip -- a quarter turn of a radius-2 cylinder running
//! 220 mm, one rail split into three edges where other faces meet it, the
//! other rail one edge -- must mesh as ONE closed patch: every triangle edge
//! inside the face is shared by two of its triangles. One CADGenBench part's
//! fillet strips meshed with holes inside them (a diagonal used by one
//! triangle only), which left the part's mesh open.

use std::collections::HashMap;

use export_engine::ExportEngine;
use geometry_engine::math::Point3;
use geometry_engine::tessellation::{
    edge_cache::EdgeSampleCache, tessellate_face, TessellationParams, TriangleMesh,
};

fn envelope(body: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('strip'),'2;1');\n\
         FILE_NAME('strip.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\n\
         END-ISO-10303-21;\n"
    )
}

/// Cylinder r = 2 about +Y through the origin, reference direction +X, so
/// angle 0 is (2, y, 0) and a quarter turn is (0, y, -2). The strip runs
/// y = 16 .. 236. Rail at angle 0: A(y16) - B(y35.943) - C(y216.057) - D(y236),
/// three edges. Rail at the quarter turn: E(y236) - F(y16), one edge. Ends:
/// quarter arcs D-E at y = 236 and A-F at y = 16.
const STRIP: &str = "#1=CARTESIAN_POINT('',(0.,0.,0.));
#2=DIRECTION('',(0.,1.,0.));
#3=DIRECTION('',(1.,0.,0.));
#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5=CYLINDRICAL_SURFACE('',#4,2.);
#11=CARTESIAN_POINT('',(2.,16.,0.));
#12=CARTESIAN_POINT('',(2.,35.943,0.));
#13=CARTESIAN_POINT('',(2.,216.057,0.));
#14=CARTESIAN_POINT('',(2.,236.,0.));
#15=CARTESIAN_POINT('',(0.,236.,-2.));
#16=CARTESIAN_POINT('',(0.,16.,-2.));
#21=VERTEX_POINT('',#11);
#22=VERTEX_POINT('',#12);
#23=VERTEX_POINT('',#13);
#24=VERTEX_POINT('',#14);
#25=VERTEX_POINT('',#15);
#26=VERTEX_POINT('',#16);
#30=DIRECTION('',(0.,1.,0.));
#31=VECTOR('',#30,1.);
#32=LINE('',#11,#31);
#33=DIRECTION('',(0.,-1.,0.));
#34=VECTOR('',#33,1.);
#35=LINE('',#15,#34);
#40=CARTESIAN_POINT('',(0.,236.,0.));
#41=AXIS2_PLACEMENT_3D('',#40,#2,#3);
#42=CIRCLE('',#41,2.);
#43=CARTESIAN_POINT('',(0.,16.,0.));
#44=AXIS2_PLACEMENT_3D('',#43,#2,#3);
#45=CIRCLE('',#44,2.);
#51=EDGE_CURVE('',#21,#22,#32,.T.);
#52=EDGE_CURVE('',#22,#23,#32,.T.);
#53=EDGE_CURVE('',#23,#24,#32,.T.);
#54=EDGE_CURVE('',#24,#25,#42,.T.);
#55=EDGE_CURVE('',#25,#26,#35,.T.);
#56=EDGE_CURVE('',#21,#26,#45,.T.);
#61=ORIENTED_EDGE('',*,*,#51,.T.);
#62=ORIENTED_EDGE('',*,*,#52,.T.);
#63=ORIENTED_EDGE('',*,*,#53,.T.);
#64=ORIENTED_EDGE('',*,*,#54,.T.);
#65=ORIENTED_EDGE('',*,*,#55,.T.);
#66=ORIENTED_EDGE('',*,*,#56,.F.);
#70=EDGE_LOOP('',(#61,#62,#63,#64,#65,#66));
#71=FACE_OUTER_BOUND('',#70,.T.);
#80=ADVANCED_FACE('',(#71),#5,.F.);
#90=OPEN_SHELL('',(#80));
#91=SHELL_BASED_SURFACE_MODEL('',(#90));";

#[test]
fn a_long_fillet_strip_meshes_as_one_closed_patch() {
    let (model, report) = ExportEngine::new()
        .import_step_content(&envelope(STRIP), "strip")
        .expect("imports");
    let faces: Vec<_> = model.faces.iter().collect();
    assert_eq!(faces.len(), 1, "one face: {:#?}", report.warnings);
    let (_, face) = faces[0];

    for chord in [0.1, TessellationParams::default().chord_tolerance] {
        let params = TessellationParams {
            chord_tolerance: chord,
            ..TessellationParams::default()
        };
        let cache = EdgeSampleCache::new(&params);
        let mut mesh = TriangleMesh::new();
        tessellate_face(face, &model, &params, &cache, &mut mesh);
        assert!(
            !mesh.triangles.is_empty(),
            "chord {chord}: the strip meshes"
        );

        // Triangle edges by welded position, with how many triangles use each.
        let key = |p: Point3| {
            (
                (p.x * 1e6).round() as i64,
                (p.y * 1e6).round() as i64,
                (p.z * 1e6).round() as i64,
            )
        };
        let mut uses: HashMap<_, (usize, Point3, Point3)> = HashMap::new();
        for t in &mesh.triangles {
            for k in 0..3 {
                let a = mesh.vertices[t[k] as usize].position;
                let b = mesh.vertices[t[(k + 1) % 3] as usize].position;
                let (ka, kb) = (key(a), key(b));
                let e = uses.entry((ka.min(kb), ka.max(kb))).or_insert((0, a, b));
                e.0 += 1;
            }
        }
        // The face's boundary as the edge cache samples it -- the same
        // samples its neighbours mesh against.
        let mut boundary = std::collections::HashSet::new();
        let lp = model.loops.get(face.outer_loop).expect("outer loop");
        for &e in &lp.edges {
            let s = cache.get_or_compute(e, &model);
            for w in s.windows(2) {
                let (a, b) = (key(w[0]), key(w[1]));
                boundary.insert((a.min(b), a.max(b)));
            }
        }
        let unused: Vec<_> = boundary.iter().filter(|e| !uses.contains_key(*e)).collect();
        assert!(
            unused.is_empty(),
            "chord {chord}: {} boundary segment(s) are in no triangle -- the strip skipped vertices its neighbours have",
            unused.len()
        );
        let holes = uses
            .iter()
            .filter(|(e, (n, _, _))| *n == 1 && !boundary.contains(*e))
            .count();
        assert_eq!(
            holes, 0,
            "chord {chord}: triangle edges inside the strip used by one triangle only"
        );
    }
}
