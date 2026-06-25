//! STEP round-trip OpenCascade gate for the pcurve export fix.
//!
//! ## What this verifies
//!
//! A closed / periodic B-spline solid (a swept turbine/impeller BLADE — a
//! freeform NURBS loft whose seam runs oblique to the world axes) is exported
//! to STEP at SIX rotations about Z (0°, 60°, 120°, 180°, 240°, 300°) and each
//! file is run through the real OpenCascade core via
//! `tools/step_occt_validate.py` (OCP / cadquery). Every rotation must be
//! OCC-valid with NO face carrying a `BRepCheck_UnorientableShape` status.
//!
//! ## Why this exact blade, and why six rotations
//!
//! The blade section data below was SAMPLED from the real OpenCascade-failing
//! solid in `.step_verify/multi.step` (the 3-blade impeller export whose
//! middle blade FreeCAD/OCC rejected). It is therefore the proven geometry
//! class that exposes the bug.
//!
//! The missing-pcurve seam bug is *rotation-dependent*: without explicit
//! parameter-space curves OpenCascade must reproject each seam edge onto the
//! periodic surface, and the U=0 vs U=1 branch it picks depends on the
//! surface's world orientation — so the SAME geometry passes at some rotations
//! and fails (`UnorientableShape`, face dropped) at others. Verified directly
//! against OCC for this blade: WITHOUT pcurves the 120° and 300° rotations
//! fail (a 45°-periodic failure pattern); a single rotation can pass by luck,
//! so the gate sweeps six. WITH explicit pcurves (the fix in
//! `formats/step/pcurve.rs` + `formats/step/writer.rs`) OCC never reprojects,
//! so ALL six pass.
//!
//! This test is RED before the pcurve fix and GREEN after.
//!
//! ## Environment handling
//!
//! If `python` or OCP/OpenCascade is unavailable, the OCC step is SKIPPED with
//! a printed message (probed up front) rather than failing — CI without an OCC
//! build does not go red. The skip path still EXERCISES the export at all six
//! rotations (and asserts the SEAM_CURVE/PCURVE structure is emitted), so a
//! writer regression is still caught.

use std::path::{Path, PathBuf};
use std::process::Command;

use export_engine::ExportEngine;
use geometry_engine::math::{Matrix4, Point3};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::primitives::topology_builder::BRepModel;
use tempfile::TempDir;

/// Locate `tools/step_occt_validate.py` relative to this crate.
fn validator_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("step_occt_validate.py")
}

/// Probe whether a Python interpreter with OCP is importable. Returns the
/// python executable name on success.
fn probe_python_occ() -> Option<String> {
    for exe in ["python", "python3"] {
        let out = Command::new(exe)
            .arg("-c")
            .arg("import OCP.STEPControl, OCP.BRepCheck")
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                return Some(exe.to_string());
            }
        }
    }
    None
}

/// The five cross-section rings of the OCC-failing impeller blade, sampled
/// from `.step_verify/multi.step` solid #2's periodic B-spline lateral. Each
/// ring is an OPEN 8-point loop (the loft closes it into a periodic seam).
fn blade_sections() -> Vec<Vec<Point3>> {
    let p = Point3::new;
    vec![
        vec![
            p(9.26120, 8.55172, 3.52436),
            p(6.94069, 8.58842, 4.91610),
            p(2.89756, 7.65258, 9.28592),
            p(-0.49977, 6.29242, 14.07404),
            p(-1.26120, 5.30469, 16.47564),
            p(1.05931, 5.26799, 15.08390),
            p(5.10244, 6.20382, 10.71408),
            p(8.49977, 7.56399, 5.92596),
        ],
        vec![
            p(9.20218, 20.13736, 9.65051),
            p(7.17099, 19.34003, 10.62108),
            p(3.66323, 17.16126, 14.04204),
            p(0.73372, 14.87734, 17.90942),
            p(0.09851, 13.82617, 19.95777),
            p(2.12971, 14.62350, 18.98720),
            p(5.63747, 16.80227, 15.56624),
            p(8.56698, 19.08619, 11.69886),
        ],
        vec![
            p(3.94094, 31.06477, 15.52077),
            p(2.45411, 29.74197, 16.14060),
            p(-0.13697, 26.71699, 18.68224),
            p(-2.31448, 23.76180, 21.65684),
            p(-2.80287, 22.60753, 23.32192),
            p(-1.31604, 23.93033, 22.70210),
            p(1.27504, 26.95531, 20.16045),
            p(3.45255, 29.91049, 17.18585),
        ],
        vec![
            p(-5.55231, 39.54219, 21.05669),
            p(-6.43781, 38.01826, 21.40167),
            p(-7.97256, 34.62779, 23.16268),
            p(-9.25752, 31.35688, 25.30816),
            p(-9.53998, 30.12158, 26.58130),
            p(-8.65448, 31.64551, 26.23632),
            p(-7.11973, 35.03597, 24.47530),
            p(-5.83477, 38.30689, 22.32983),
        ],
        vec![
            p(-17.91783, 44.49997, 26.21834),
            p(-18.27553, 43.02521, 26.36599),
            p(-18.82049, 39.70659, 27.47083),
            p(-19.23348, 36.48811, 28.88564),
            p(-19.27258, 35.25511, 29.78166),
            p(-18.91488, 36.72987, 29.63401),
            p(-18.36992, 40.04849, 28.52917),
            p(-17.95693, 43.26697, 27.11436),
        ],
    ]
}

/// Project a ring exactly onto its own best-fit plane (Newell normal through
/// the centroid). The blade rings were *sampled*, so the two end caps carry
/// sub-micron planarity noise that `nurbs_loft`'s planar-cap check rejects;
/// flattening only the END caps restores exact planarity without changing the
/// freeform character of the lateral (the interior rings are left untouched).
fn flatten_onto_plane(ring: &mut [Point3]) {
    let n = ring.len() as f64;
    let c = ring.iter().fold(Point3::new(0.0, 0.0, 0.0), |a, p| {
        Point3::new(a.x + p.x, a.y + p.y, a.z + p.z)
    });
    let c = Point3::new(c.x / n, c.y / n, c.z / n);
    let (mut nx, mut ny, mut nz) = (0.0, 0.0, 0.0);
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        nx += (a.y - b.y) * (a.z + b.z);
        ny += (a.z - b.z) * (a.x + b.x);
        nz += (a.x - b.x) * (a.y + b.y);
    }
    let l = (nx * nx + ny * ny + nz * nz).sqrt();
    if l == 0.0 {
        return;
    }
    let (nx, ny, nz) = (nx / l, ny / l, nz / l);
    for p in ring.iter_mut() {
        let d = (p.x - c.x) * nx + (p.y - c.y) * ny + (p.z - c.z) * nz;
        *p = Point3::new(p.x - d * nx, p.y - d * ny, p.z - d * nz);
    }
}

/// Build the rotated, end-cap-flattened section stack for one rotation.
fn rotated_sections(angle: f64) -> Vec<Vec<Point3>> {
    let m = Matrix4::rotation_z(angle);
    let mut sections: Vec<Vec<Point3>> = blade_sections()
        .into_iter()
        .map(|ring| ring.iter().map(|p| m.transform_point(p)).collect())
        .collect();
    let last = sections.len() - 1;
    flatten_onto_plane(&mut sections[0]);
    flatten_onto_plane(&mut sections[last]);
    sections
}

#[tokio::test]
async fn closed_bspline_loft_passes_occ_at_six_rotations() {
    let angles_deg = [0.0_f64, 60.0, 120.0, 180.0, 240.0, 300.0];

    let temp = TempDir::new().expect("temp dir");
    let engine = ExportEngine::with_output_directory(temp.path().to_string_lossy().to_string());

    // Export every rotation. The export itself must always succeed (a writer
    // panic / error is a hard failure independent of OCC availability).
    let mut files: Vec<PathBuf> = Vec::with_capacity(angles_deg.len());
    for (idx, deg) in angles_deg.iter().enumerate() {
        let sections = rotated_sections(deg.to_radians());

        let mut model = BRepModel::new();
        nurbs_loft(
            &mut model,
            sections,
            NurbsLoftOptions {
                degree_u: 3,
                degree_v: 3,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("blade loft @ {deg}° must build: {e:?}"));

        let name = format!("occ_gate_rot_{idx}");
        let filename = engine
            .export_step(&model, &name)
            .await
            .unwrap_or_else(|e| panic!("export @ {deg}° must succeed: {e:?}"));
        let path = temp.path().join(&filename);

        // Structural assertion (holds even when OCC is unavailable): the
        // periodic lateral must carry explicit pcurves now — a SEAM_CURVE for
        // the seam edge, PCURVEs, each in a DEFINITIONAL_REPRESENTATION.
        let text = std::fs::read_to_string(&path).expect("read exported step");
        assert!(
            text.contains("SEAM_CURVE"),
            "rotation {deg}°: closed periodic loft must emit a SEAM_CURVE for its seam edge"
        );
        assert!(
            text.contains("PCURVE("),
            "rotation {deg}°: pcurves (PCURVE) must be present"
        );
        assert!(
            text.contains("DEFINITIONAL_REPRESENTATION"),
            "rotation {deg}°: each pcurve must be wrapped in a DEFINITIONAL_REPRESENTATION"
        );

        files.push(path);
    }

    // OCC validation — skip cleanly if OCP/python is unavailable.
    let Some(python) = probe_python_occ() else {
        eprintln!(
            "SKIP closed_bspline_loft_passes_occ_at_six_rotations: \
             python with OCP/OpenCascade not available; export at all six \
             rotations succeeded and emitted SEAM_CURVE/PCURVE/\
             DEFINITIONAL_REPRESENTATION, but OCC semantic validation was skipped."
        );
        return;
    };

    let script = validator_script();
    assert!(
        script.exists(),
        "validator script must exist at {}",
        script.display()
    );

    for (deg, path) in angles_deg.iter().zip(files.iter()) {
        let output = Command::new(&python)
            .arg(&script)
            .arg("--require-occ")
            .arg(path)
            .output()
            .expect("spawn step_occt_validate.py");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "rotation {deg}°: OCC validation FAILED.\n\
             This is the missing-pcurve seam bug (BRepCheck_UnorientableShape).\n\
             stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        eprintln!("rotation {deg}°: {}", stdout.trim());
    }
}
