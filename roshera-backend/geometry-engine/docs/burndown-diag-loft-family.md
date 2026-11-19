# burndown-diag: the loft family

Diagnosis document for the three `KNOWN_REDS.md` entries added 2026-09-06.
Referenced by `geometry-engine/KNOWN_REDS.md`; the ratchet rule requires one.

Earlier burndown diagnoses (`burndown-diag-cf.md`, `burndown-diag-boolean.md`,
`burndown-diag-drawing-tess.md`) live under `.superpowers/sdd/`, which
`.gitignore:182` excludes -- so a `KNOWN_REDS.md` entry pointing there points
outside version control. This file lives inside the crate for that reason, and
`KNOWN_REDS.md`'s rule text now names this directory.

## Breaking commit

**None.** These are not regressions. The defects have been present and PRINTED
by their own harnesses for as long as those harnesses have existed; what changed
on 2026-09-06 is that the harnesses stopped discarding their verdict counts, so
the defects became visible to the runner. Recording "no breaking commit" is the
honest entry -- a bisect would find only the commit that started asserting.

Evidence that the harnesses were printing and passing: the pre-change runs
recorded 33 FAIL rows across 95 cases in `blend_weld_stress` under
`test result: ok. 9 passed; 0 failed`, and two `BUILT-BUT-CORRUPT` lines in
`op_stress_round3` under `test result: ok. 24 passed; 0 failed`.

## The entries

| binary::test | parked as |
|---|---|
| `blend_weld_stress::broaden_loft_varied_sections` | `#[ignore]` |
| `op_stress_round3::loft_four_sections` | `#[ignore]` |
| `op_stress_round3::loft_square_circle_square_dissimilar` | `#[ignore]` |

All three are `#[ignore]`d, i.e. PARKED: `red-gate.ps1` reports them as PARKED
and does not judge them; `ignored-reds.ps1` runs them with `--ignored` and fails
if one PASSES.

## Root cause (as far as measurement goes -- not fixed here)

A loft between **dissimilar** cross-sections produces a solid that is
structurally accepted and whose welded display mesh CLOSES, but whose facets are
not consistently wound. Measured 2026-09-06:

| test | case | measurement |
|---|---|---|
| `op_stress_round3::loft_four_sections` | loft 4 sections, circle/square alternating | `be=0 nme=0 oriented=false`, `scoped-valid`, `cert.is_sound=false`, `fails=[oriented,self_intersection_free,tessellation,mesh_quality]` |
| `op_stress_round3::loft_square_circle_square_dissimilar` | loft square-circle-square dissimilar | identical signature |
| `blend_weld_stress::broaden_loft_varied_sections` | `loft circle25 -> square40 -> circle15` | `NOT_SOUND[brep=true wt=true manif=true orient=true self_int_free=true tess_clean=true mesh_q_clean=false]` |
| `blend_weld_stress::broaden_loft_varied_sections` | `loft square20 -> circle8` | `NOT_SOUND[... self_int_free=false tess_clean=false mesh_q_clean=false]` |

The discriminator is **dissimilarity of the sections**, not the loft op as such:
`op_stress_round3::loft_three_circles` (10 -> 6 -> 8, all circles) is
`BUILT+SOUND`, and `broaden_loft_varied_sections`'s circle-to-circle case
passes. Every failing case interpolates between a circle and a square, i.e.
between profiles whose vertex counts and parameterisations do not correspond.
That is the signature of a section-correspondence / ruling-alignment problem in
the loft's profile matching, upstream of tessellation: `boundary_edges == 0`
means the topology is being built and welded, and `oriented == false` means
neighbouring rulings disagree about which way is out.

## The related refusal, recorded but NOT pinned as a defect

`broaden_loft_varied_sections`'s third FAIL is the **Cubic** loft
`circle12 -> 20 -> 6` returning

```
Err(InvalidBRep("Lofted solid failed validation (1999 errors):
 OrientationError { message: \"Inconsistent face orientations: the boundary walk of
 loop 5 on face 5 does not close (a loop sense is inconsistent with the vertex
 chain) on edge 103\", ... }"))
```

By the harness's own rule (`Verdict::class_tag` -> `[reject -- honest, not a
bug]`) that is a refusal, and it is pinned as one in the test's `refused` count.
It is almost certainly the same defect seen from the other side: the loft
produced a solid with inconsistent loop senses and validation caught it on the
way out instead of letting it through. Recorded here so a future fix knows to
expect this case to change class (from refusal to PASS) and to re-pin.

## Removal criteria

Remove an entry from `KNOWN_REDS.md` **and** its `#[ignore]` when its test
passes with the assertions in place. `ignored-reds.ps1` exits 1 the moment one
of them starts passing, which is the mechanism that stops a fixed red from
staying parked.

## Closed by

**2026-09-07 — all three entries removed from `KNOWN_REDS.md` and all three
`#[ignore]`s removed.** Fixed at root in `geometry-engine/src/operations/loft.rs`
by `register_correspondence`, a new step in `loft_profiles_body` between
`densify_correspondence` and the loft-type dispatch.

The root cause named above ("a section-correspondence / ruling-alignment problem
in the loft's profile matching") is confirmed and is specifically an **index
origin** problem, not a vertex-count one. `establish_correspondence` reads each
profile's ring off that profile's own outer loop and `densify_correspondence`
resamples it from that loop's first edge, so index `k` of one ring was ruled to
index `k` of the next with nothing relating the two index origins. A circle
profile starts at its curve seam (angle 0); a centred square listed from its
`(-h, -h)` corner starts at 225°. Every failing case interpolated between a
circle and a square, which is exactly the pair whose origins disagree — and
`loft_three_circles` passed because three circles built the same way all start
at angle 0 by accident, not by construction.

`register_correspondence` searches, for each consecutive ring pair, the full
dihedral group of adjacency-preserving relabelings (`n` cyclic shifts, each with
and without a reversal of sense) and keeps the one minimising total squared rail
length — the objective `create_minimal_twist_loft` already minimised over shifts
alone, promoted so every loft type is built on a registered correspondence.

Measured after the fix (same commands as above):

| test | case | measurement |
|---|---|---|
| `op_stress_round3::loft_four_sections` | loft 4 sections, circle/square alternating | `BUILT+SOUND` |
| `op_stress_round3::loft_square_circle_square_dissimilar` | loft square-circle-square dissimilar | `BUILT+SOUND` |
| `blend_weld_stress::broaden_loft_varied_sections` | `loft circle25 -> square40 -> circle15` | `PASS` |
| `blend_weld_stress::broaden_loft_varied_sections` | `loft square20 -> circle8` | `PASS` |

The related refusal recorded above — the Cubic `circle12 -> 20 -> 6` loft — is
**unchanged**: it still returns the same `InvalidBRep`/`OrientationError`, still
counts as the test's one pinned refusal, and needed no re-pin. Registration is a
no-op for it (three circles, one seam angle, best shift 0), which is the evidence
that its `OrientationError` is a DIFFERENT defect from this family and not the
same one seen from the other side, as the entry above had guessed.

The regression is pinned going forward by `geometry-engine/tests/loft_orientation.rs`,
which lofts two congruent squares whose edge lists start one corner apart (the
correct answer is a box of volume `side^2 * height` exactly; before the fix the
kernel built a sound, closed, ORIENTED body of 2/3 that volume — the wrong shape
reported honestly) and a triangle to a square (the smallest dissimilar pair).

## 2026-11-18: two loft reds found by the drawing rewrite's full gate

Both were measured RED on the code BEFORE the drawing rewrite (main at
94917545, geometry-engine untouched), so neither is caused by the drawing work.

### `fidelity_requested_vs_measured::loft_cap_faces_are_found_but_the_trimmed_integral_is_not_the_path`

**No breaking commit: an improvement tripped a tripwire.** The test is a
deliberate tripwire: it asserts `Face::area` still CANNOT measure a nurbs_loft
cap (a loop of one self-closing periodic-NURBS edge), so that the day it can,
someone switches `queries::fidelity::mesh_cross_section_area` back to the exact
trimmed integral. It now measures the r = 6 cap as 113.0968916743036 mm^2
(pi * 36 = 113.0973..., 4e-6 relative), so the tripwire fired as designed. The
most recent commit touching face area is `a9d13a70` ("every minted face
measures its own parametric domain"); not bisected.

**Fix (queued, kernel):** `mesh_cross_section_area` takes `&BRepModel`, while
`Face::area` needs `&mut LoopStore`; switch the reading to the exact integral
(signature to `&mut BRepModel`, or clone the face and its loops), keep the
tessellated reading only as the stated fallback, then delete this test case.

### `loft_orientation::loft_l_section_whose_cap_the_mesh_cannot_orient_is_refused_not_minted`

The test expects the L-section loft to be REFUSED (its cap could not be
oriented by the mesh); the kernel now MINTS a solid (`must not mint: 0`).
Suspected cause: `439ccbc2` ("a loft orients each band from its slab's signed
volume, or refuses") made this case orientable -- UNVERIFIED (not bisected).
Whether the minted solid is right is NOT yet known: it may be an improvement
(test stale) or a wrong solid slipping through (a real defect). Removal
requires measuring it: closed, oriented, volume equal to the analytic L-prism
volume; if so, the test is re-pinned to "builds the right solid"; if not, the
refusal is restored.
