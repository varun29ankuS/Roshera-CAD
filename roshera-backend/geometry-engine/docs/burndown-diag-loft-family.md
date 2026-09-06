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
