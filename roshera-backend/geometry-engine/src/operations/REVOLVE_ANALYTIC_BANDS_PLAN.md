# #19 — Revolve emits analytic bands (implementation plan)

**v1 SHIPPED 2026-06-16 (commit 5ce82b0).** Cylinder walls + annular Plane caps,
one analytic face per band; 48-seg tube = 4 faces not 192. `try_analytic_band_
revolution` + `build_analytic_bands` in revolve.rs, wired at the top of
`create_revolution`. Self-verifying (validate_solid_scoped + manifold_report
inside a nested `with_rollback`) → grid fallback on any failure (zero regression).
Gates green: revolve_analytic_faces 2/2, section_revolve tube x/y 2/2 (unblocked
#9), revolve_watertight 7/7, revolve_volume 14/14.

**The "#21 tessellator block" was a MISDIAGNOSIS.** The prior attempt was reverted
believing annular-Plane + Cylinder shared circle edges tessellate at mismatched
densities. The primitive watertightness battery (tests/primitive_tess_watertight.rs)
DISPROVED that — a planar face sharing a circle edge with a curved face is
watertight. The real prior bug was almost certainly each band getting its OWN
circle copy (different EdgeIds → the cache samples them independently → gaps). v1
fixes it by sharing ONE circle EdgeId per ring (mirrors create_cylinder_topology).

**v2 REMAINING:** cone bands (sloped edges → `create_frustum_topology`-style
Cone band) + disc/apex bands (a profile vertex ON the axis → a real disc cap, not
the v1 sliver-cylinder). Both currently fall back to the watertight grid path.

## Problem
`create_revolution` (revolve.rs:174) builds one face per **(profile-edge ×
angular-segment)** with a `SurfaceOfRevolution` patch → a 48-seg tube = 192
faces, all `surfaceofrevolution`. Same faceting class #24 fixed for extrude.
Breaks section (#9), inflates face counts, hurts dimensioning/booleans.

## Fix: a fast analytic-band path, with fallback
Add `try_analytic_band_revolution(model, base_face, base_face_id, options)
-> OperationResult<Option<SolidId>>` and call it at the TOP of
`create_revolution`; on `Some(sid)` do the base-face cleanup (revolve.rs
427–445) and return it; on `None` fall through to the existing per-segment code
(unchanged — preserves all hard-won behaviour).

### Eligibility (else return Ok(None) → fallback)
- `is_full` (angle ≈ 2π) only. (Partial revolution keeps the per-segment path:
  it needs flat end-caps + an open seam.)
- `base_face.inner_loops` empty.
- EVERY profile edge's curve downcasts to `Line` (curved edges → SurfaceOf-
  Revolution fallback).
- SCOPE v1: require every profile vertex radius `> 1e-4` (no apex/disk). The
  apex (cone-tip) and disk (r=0 plane) loops are special 3-edge / 1-edge cases —
  defer to v2; fall back for now. This still covers tubes, frustum tubes,
  stepped tubes, the housing, the de Laval engine, and the #9 section gate.

### Geometry per profile vertex (build a `Ring`)
`ao = Vector3(axis_origin)`, `axis` normalized. For vertex `v` at world `p`:
- `t = (p - ao)·axis` (axial param), `r = |(p-ao) - axis*((p-ao)·axis)|` (radius)
- `center = ao + axis*t`
- seam direction `ref_dir = Circle::new(axis_origin, axis, 1.0)?.x_axis()`
  (canonical, SAME for all rings — a full revolution is rotationally symmetric,
  so anchoring the seam at the canonical dir, not the input angle, gives the
  same solid AND makes seam meridians line up → watertight).
- `seam_pos = center + ref_dir*r`; `seam_v = vertices.add(seam_pos)`
- ring `Circle::new(Point3(center), axis, r)?` → closed edge
  `Edge::new(0, seam_v, seam_v, cid, Forward, ParameterRange::new(0,1))`.
Store `Ring { center, radius:r, seam_v, circle_edge }` keyed by the profile
`VertexId`. Closed profile ⇒ consecutive bands SHARE a ring ⇒ each ring circle
used exactly twice (top of band below, bottom of band above) ⇒ watertight.

### Per band (profile edge sp→ep, honour loop orientation)
`r0,t0 = ring[sp]`, `r1,t1 = ring[ep]`. Seam meridian = `Line(ring[sp].seam_v →
ring[ep].seam_v)`, one edge, used twice (fwd+bwd) in the band loop.
Loop (mirrors `create_cylinder_topology` lateral):
`bottom_circle(fwd) · seam(fwd) · top_circle(bwd) · seam(bwd)` where bottom =
ring[sp].circle, top = ring[ep].circle.

Surface by classification (`eps = 1e-7`):
- `|r0-r1|<eps && |t0-t1|>eps` → **Cylinder**: base = lower-t ring center,
  `Cylinder::new_finite(Point3(base), axis, r0, |t1-t0|)?`; set
  `.ref_dir = ref_dir`.
- `|t0-t1|<eps && |r0-r1|>eps` → **Plane** (annular): `Plane::from_point_normal(
  Point3(center0), axis)?` (orientation flag fixes which way).
- else → **Cone (frustum)**: slope `m=(r1-r0)/(t1-t0)`; `t_apex=t0 - r0/m`;
  `apex=Point3(ao + axis*t_apex)`; `cone_axis = axis * m.signum()`;
  `half_angle = atan(m.abs())`; `d0=(t0-t_apex)*m.signum()`,
  `d1=(t1-t_apex)*m.signum()` (both >0); `Cone::truncated(apex, cone_axis,
  half_angle, d0.min(d1), d0.max(d1))?`; set `.ref_dir = ref_dir`.

### Orientation (the hard part — same trap as #24's wall orientation)
`orient_face_for_outward(surface, target)` samples the surface normal at its
parametric MIDPOINT. For a full-circle Cylinder/Cone, u_mid is at angle π from
the seam (`ref_dir`), i.e. at world direction `-ref_dir` rotated into the plane.
So compute the outward TARGET at that same u=π location, NOT at the input
profile angle:
- radial-out at u=π = `-ref_dir` (the radial unit there).
- Band outward sign from the profile-loop winding (the n_p×d rule the existing
  code uses at revolve.rs 340–351, which fixed the ⅓-volume bug): compute
  `d = ep_seam - sp_seam`, `rhat_pi = -ref_dir`, `n_p = axis × rhat_pi`,
  `outward = (n_p × d).normalize()`. For a Plane band (`d` ⟂ axis) this yields
  ±axis correctly; for Cylinder/Cone it yields ±rhat_pi. Pass that as `target`.
  VERIFY against revolve_watertight volumes (divergence theorem) — if a wall is
  inverted, volume halves/negates (the historical ⅓-volume symptom).

### Shell/solid
`ShellType::Closed`; add all band faces; `Shell` → `Solid`. Then run the SAME
base-face cleanup as create_revolution (remove profile edges, inner loops,
outer loop, base face) before returning `Some(sid)`.

## Acceptance gates (must pass)
1. `cargo test -p geometry-engine --test revolve_watertight` — STILL GREEN (the
   full-rev line profiles now route through the analytic path: vertical tube,
   cone shell, frustum tube, stepped tube, de Laval, coarse/fine; the 180°
   partial stays on the fallback path).
2. Un-ignore `tests/section_revolve.rs` tube +X / +Y → must PASS (clean caps).
   (solid_cylinder uses r=0.001 → still falls back v1; leave it ignored or bump
   its radius once apex/disk land in v2.)
3. New face-count assertion: a 48-seg revolved tube = **4 faces** (2 Cylinder +
   2 annular Plane), not 192; surface kinds analytic. Add to the analytic-face
   harness or a new `tests/revolve_analytic_faces.rs`.
4. `validate_solid_scoped` valid + `manifold_report (0,0)` at several deflections
   (reuse the revolve_watertight helper).

## Watch-outs
- `triangulate_cap` can PANIC in the `cdt` crate on degenerate fragments (seen
  in #9) — unrelated to this path but log for #11.
- Keep `ref_dir` identical across all rings AND set it on every band surface, or
  the seam vertex won't sit at the surface's u=0 and the curved-CDT tessellator
  fails `PointOnFixedEdge` (the cdt-γ.3 lesson).
- Cone `height_limits` are axial distances from the APEX along `cone_axis`, both
  positive.
