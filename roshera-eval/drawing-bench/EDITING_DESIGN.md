# Drawing editing: dimensions, tolerances, GD&T (design)

Build-free design. **[V]** = verified by reading the cited code; **[I]** = inferred / needs a run or a decision. Paths are relative to `roshera-backend/` unless noted (`app/` = `roshera-app/src/`, `mcp/` = `roshera-mcp/src/`).

> **Status 2026-11-22 (partly checked):** pieces of slices 1–2 exist — `gdt/iso286.rs`, dimension
> suppress in `api-server/src/drawing_mgr.rs`, dimensions edited on the sheet (4e31204b),
> `drawing/dimension_edit.rs`. No smart-dimension add (slice 3) was found. The plan of record's
> P2 "minimal pinned human sheet edits" is the next use of this design.

Varun's requirement: "there is no way to edit the drawing"; "add subtract dimensions / tolerances, we need to go deep". Standing constraints: the kernel cannot lie (a dimension value is MEASURED, never typed), history is event-sourced, entities are bound by persistent id (PID), refusals are typed.

## 0. What exists today (so the new parts are scoped honestly)

| Area | Exists [V] | Missing [V] |
|---|---|---|
| Dimension generation | `Dimension2d` (geometry-engine/src/drawing/dimensioning.rs:34) built by `auto_dimensions` (:107) from `readable::extract_dimensions`; carries `id`, `kind`, `value`, `label`, `a/b` endpoints, `entities` (face ids), `pid`, `datum`, `tolerance: Option<ToleranceRef>`. Placement is **recomputed every render** (`layout::compute_layout` -> `place_dimensions_routed`, layout.rs:2213; `PlacedDimension` layout.rs:223) | No user-authored dimension; no delete/suppress; no per-dimension placement or text override persisted anywhere; no ordinate/baseline/chain; angular only from recognised features |
| Dimension edit | `POST /api/drawings/{id}/dimensions/{dim_id}` `{value}` (api-server/src/drawing_edit.rs `edit_dimension`): resolves the ONE recorded parameter that drives the dimension (`timeline_engine::dimension_drive::plan_dimension_edit`, `DriveResolution::{Driven, ReferenceOnly, Ambiguous, Undecided}`), solves by replay, commits via `param.mould`; imported parts get `ReferenceOnly` -> refused. `GET .../driver`. App: click a dimension -> edit box (`DrawingsWorkspace.tsx` `dimEdit`, ~l.191-213, 660-672). MCP `drawing_set_dimension` (mcp/tools/drawing.ts:109) | Nothing edits properties (text, tolerance, placement) of a dimension |
| Size tolerance | Model `gdt::model::DimensionalTolerance {nominal, bound}` with `ToleranceBound::{PlusMinus{plus,minus}, Limits{upper,lower}, Fit(FitClass{designation})}` (gdt/model.rs:222). Authored via `POST /api/agent/parts/{id}/size-tolerance`, recorded as event `gdt_annotation_create` (handlers/gdt.rs header; kernel mediators `gdt/edit.rs: gdt_create_annotation`), undoable/durable. Joined to the sheet by `attach_tolerances` (dimensioning.rs:2269) as `Dimension2d.tolerance: ToleranceRef` | **(a) it is never drawn:** `grep` for `tolerance`/`limits` in svg.rs / layout.rs / dxf.rs / pdf.rs finds only comments; the label stays "Ø20.00". It is only readable through `answer_query` (query.rs `TolerancedDiameter`). **(b)** the join is only for `kind == "diameter"` dims and hole-table rows (dimensioning.rs:2316-2330): a linear/length dimension can't carry one. **(c)** `Fit` is **unresolved**: no ISO 286 tables; `limit_range()` returns `None`, verify reports `NotYetVerified`, `ToleranceRef.limits = None`. **(d)** `PlusMinus` stores non-negative magnitudes, so +0.10/+0.05 (both positive) can only be `Limits`. **(e)** no basic/reference/general-override forms |
| General tolerance | `Drawing.tolerance_note` string + `types::GeneralTolerance::linear_for(nominal)` with the real ISO 2768-1 table f/m/c/v (types.rs:859-900) | Class is only whatever the note string says; no per-sheet selector route/UI, no per-dimension override |
| GD&T | Kernel sidecar `model.gdt: GdtSidecar` keyed by feature PID (gdt/sidecar.rs) + datum reference frame `model.drf`; `FeatureControlFrame`/`Datum`/`MaterialModifier` (gdt/model.rs:319,341); verification with fitted features (gdt/verify.rs, fit.rs); REST `/api/agent/parts/{id}/{datums,fcf,size-tolerance,gdt}` (main.rs:11761-11790) all **recorded events** (`gdt_datum_*`, `gdt_annotation_*`; tests in api-server/src/gdt_history_tests.rs); sheet draws them: `attach_gdt_annotations` (dimensioning.rs:1993) makes `PlacedDatumSymbol` / `PlacedFcfBlock`; certificate facts `Fcf`, `DatumSymbol` | Not authorable from the sheet (app `AnnotateMenu` lists datum/FCF/dimension/note/finish as disabled stubs with reasons: drawing-overlays.tsx:318-395); placement of frames is automatic ("view that best shows the feature"); no frame attached to a dimension, no leader to a picked face, no composite frames |
| History of sheets | Sheets rebuilt from events: `timeline-engine/src/drawing_history.rs::rebuild_drawings` replays `drawing.create_from_part / rename / title_block.update / add_view / remove_view / view.update / delete` on top of the sheet re-derived at the creation position; `drawing.set_projection` handled via `projection_switches`. Dotted kinds are "document events" excluded from geometry replay (`is_geometry_event`, :96) | no `drawing.dimension.*` |
| Sheet certificate | `certify_drawing` (sheet_certificate.rs): per-element `SheetFact` (kinds Dimension, Hole, BoltCircle, HoleStack, HolePattern, Fcf, DatumSymbol, Section, Note, RenderOnly, Omitted), live re-measure from provenance (`dim.entities/pid/dir3`, :985), FCF ink compared to block (`ink.tolerance == block.tolerance_text`, :559), live-side walk emits `Omitted` for features the sheet lacks; refuses export of an incomplete sheet. `verify_drawing` (verify.rs): layout collisions, `RedundantDimension`, `DimensionLabelCollision`, `DimensionOnGeometry`... | dimension-label ink (tolerance text) is not compared; no state for "omitted on purpose" |
| Picking | `DrawingQuery::EntityAt {view, xy_mm}` (query.rs:62) maps a sheet point to an entity; `PolylineSource {edge_id, face_ids}` per polyline (types.rs:475); `ProjectedCircle.face_ids`; kernel face-pair measure `queries/measure.rs::measure` (Distance between parallel planes / axis-axis / axis-plane, Angle between plane normals, Diameter of a cylinder; refuses `Unsupported` otherwise) and `POST /api/agent/measure` | no edge/vertex measure, no radius of an arc edge, no point/edge pairs |
| IDs | `PersistentId` for faces and edges: `BRepModel::face_by_pid / edge_by_pid / solid_by_pid` (primitives/topology_builder.rs:981-1010). The STEP importer assigns solid + face PIDs "keyed by this file's CONTENT and the STEP entity each came from" (export-engine/src/formats/step/mod.rs:188-197) | no vertex PIDs [V: no `vertex_by_pid`]; whether imported EDGES get PIDs is [I] (confirm in `assembly_placement::finalize`) |

## 1. Core model: a dimension is a *spec*; its value is *measured*

Introduce a persisted, value-free record (new, `geometry-engine/src/drawing/dimension_spec.rs`):

```rust
pub struct DimensionSpec {
    pub id: Uuid,                    // stable id of this annotation (events + data-dim-id)
    pub view: ProjectedViewId,       // the view it is drawn in
    pub kind: DimKind,               // Linear{axis: H|V} | Aligned | Angular | Radius | Diameter
                                     // | ArcLength | Chamfer | HoleCallout
    pub refs: Vec<EntityRef>,        // 1..=3 picks, see below
    pub placement: Placement,        // see 2.3
    pub text: TextOverride,          // prefix/suffix/replacement flags, see 2.4
    pub tolerance: ToleranceSpec,    // see 3
    pub role_override: Option<Role>, // Some(Reference) forces "(235)"; never forces Driving
    pub set: Option<SetRef>,         // member of a Baseline/Chain/Ordinate set
}
pub enum EntityRef {
    Face(PidHex),                    // plane, cylinder, cone, torus, sphere face
    Edge(PidHex),                    // straight edge, circle/arc edge
    EdgeEnd(PidHex, End),            // a vertex, addressed through its edge + end (no vertex PID exists)
    Axis(PidHex),                    // axis of a cylindrical/conical face
}
```

Rules:
1. **There is no `value` field and the add route rejects one** (`ValueNotAccepted`). Value = `measure_dimension(model, spec, view_basis) -> Result<Measured, DimRefusal>`, which resolves each PID live (`face_by_pid` / `edge_by_pid`), measures analytically, and returns `{value, a, b (view-space endpoints), axis3, dir3, entities}` so the existing `Dimension2d` fields (and therefore the existing certificate live-check path, sheet_certificate.rs:985) are populated identically to an auto dimension. Authored and auto dimensions are one type on the sheet.
2. **A lost entity is a typed refusal, never re-targeted.** A PID that no longer resolves yields `DimensionDangling{ id, ref, reason }` on the sheet (shown in the sheet certificate as a `stale/dangling` fact and as a red flagged callout; export refuses), mirroring how GD&T reports dangling datums (gdt_history_tests: "a datum face consumed by a later boolean is reported DANGLING"). No nearest-face fallback.
3. **Kernel measure extensions** (`queries/measure.rs`, add; do not change existing cases): edge-edge parallel distance, point-point (EdgeEnd), point-edge, point-plane, circle-edge radius/diameter, arc length, edge-edge angle, plane-edge angle. Each returns a typed `Unsupported{reason}` outside its analytic case (same policy as today). Projected (non-true-length) linear measures are only allowed for `Linear{axis}` (it is *defined* as the projection on the view axis); `Aligned` refuses `NotTrueLength` when the picked span is not parallel to the view plane, with the angle in the refusal.
4. **View validity:** the spec's entities must be representable in its view (`DimensionOnGeometry`/edge-on collapse is detected by `projected_span() ~ 0` as `visible_dimensions` does today, dimensioning.rs:180). Otherwise refusal `EdgeOnInView` (hint: "pick the view that shows it true").

## 2. Dimension editing

### 2.1 Add (smart dimension)
Pick sequences (what the user clicks in the view; the server resolves sheet point -> entity with the existing `EntityAt` machinery extended to return a PID and sub-type):

| Tool result | Picks | Measured by |
|---|---|---|
| Linear H / V | edge, or two of {edge, edge-end, face-edge-on, circle centre} | projection of resolved points on the view's x / y axis (`projection::view_matrix_for_projection`) |
| Aligned | edge (true length) or two points | 3D distance, `NotTrueLength` guard |
| Diameter | cylinder/cone face or circle edge | `measure_one` Diameter (existing) / curve eval |
| Radius | arc edge or cylinder/torus/fillet face | same |
| Angular | two straight edges or two planar faces seen edge-on | plane-plane angle (existing) / edge-edge |
| Hole callout | any face of a recognised hole stack | `hole_features::detect_hole_features` stack -> existing callout text ("4x O6.60 THRU, cbore ...") |
| Ordinate set | datum origin pick + N feature picks | each = offset along view axis from the origin pick |
| Baseline / Chain set | first pick + N more | members share first ext line (baseline, stacked by `STACK`=8 mm, layout.rs:189) or abut (chain) |

A set is persisted as one `DimensionSet` that *expands* at build time into member specs (so adding a point to a baseline is one event, deleting a member is one event, and members keep stable ids `set_id:index`).

Preview: `POST /api/drawings/{id}/dimensions/preview` (new) runs the same measure + layout and returns `{value, label, placed geometry, refusals}` **without recording** (drives the cursor ghost; refusals are shown before the user commits).

### 2.2 Delete
- Authored dimension: event `drawing.dimension.delete {id}`.
- Auto dimension: an auto dimension is regenerated every build, so deleting needs a record: `drawing.dimension.suppress {pid|id}` stored on the sheet as `suppressed: BTreeSet<DimKey>` (key = the auto dimension's durable `pid`, falling back to `(kind, entities, dir3)` when `pid` is `None`). Regeneration skips suppressed keys. **Certificate semantics:** the live-side walk of `certify_drawing` already emits `Omitted` when a feature loses its dimension. A suppressed dimension must show as `Omitted` **with `waived_by: {author, event_id}`**: visible in the certificate and the readback, not counted as "stale" and not silently clean. An export policy decides (default: allowed with the waiver listed; a strict mode refuses). A user can un-suppress (event) and undo works.

### 2.3 Move / re-place
`Placement { offset_mm: Option<f64> /* dim line distance from the feature, signed */, text_shift: Option<[f64;2]> /* along, perpendicular */, text_side: Auto|Inside|Outside, flip_arrows: bool, leader: Option<LeaderSpec>, ext_gap: Option<f64> }`, stored on the spec (authored) or in `placement_overrides: BTreeMap<DimKey, Placement>` on the sheet (auto). `place_dimensions_routed` (layout.rs:2213) consumes an override per dimension instead of computing the band slot; the rest of the stack still routes around the pinned ones. Taste is not refused: an overlapping move is **kept** and reported by `verify_drawing` (`DimensionLabelCollision`, `DimensionOnGeometry`), because a collision is a legibility fault, not a lie; the UI shows the fault inline.

### 2.4 Text
`TextOverride { prefix: String, suffix: String, show_value: bool, decimals: Option<u8>, symbols: Auto|Off }`. Palette symbols: O (diameter), R, square, S-radius, depth (down-arrow), counterbore, countersink, degree, plus-minus, "TYP", "THRU", "n x" count. **Anything that states a number is not free text:** `prefix`/`suffix` are validated against a grammar (symbols, count `\d+X`, words from a closed list + free words without digits). A number typed into the text is refused `TextContainsValue` ("the value is measured"; a replacement string is only allowed as an explicit `NOT_TO_SCALE` underline, which draws the standard underline and is flagged on the sheet). `4X`/`TYP` are validated against the model: `4X` is accepted only when 4 coincident-equal features are found (hole pattern recognition, hole_features.rs), else refusal `CountMismatch{found}`.

### 2.5 Driving vs reference
`role` is **derived, not stored**: `Driven` when `plan_dimension_edit`'s resolution (timeline-engine/src/dimension_drive.rs `DriveResolution::Driven`) finds exactly one parameter; else `Reference`. Reference dimensions draw in parentheses `(235)` (ISO/ASME convention) and `role_override: Reference` lets the user force that even on a driven one. Authored dimensions are Reference unless their measured quantity maps to the same driver (same resolver, same refusal classes: ReferenceOnly/Ambiguous/Undecided). Editing the value of a Driven one stays the existing route (`edit_dimension`); imported parts never get driving dimensions (ReferenceOnly), consistent with "sheet-only annotations".

## 3. Tolerances

### 3.1 The forms and their storage

```rust
pub enum ToleranceSpec {
    None,                                   // falls to general tolerance
    Symmetric { pm: f64 },                  // 25 +/-0.15
    Bilateral { upper: f64, lower: f64 },   // signed deltas, any sign: +0.05/-0.10, +0.10/+0.05, 0/-0.2
    Limits { lower: f64, upper: f64 },      // absolute limits: 25.15 / 24.85 (also min/max-only: "MAX 12")
    Fit { class: FitDesignation, show: FitShow },  // H7, g6, H7/g6; show = Designation | Limits | Both
    Basic,                                  // boxed theoretical-exact (ASME Y14.5 / ISO 1101 TED)
    Reference,                              // (235)
    General { class: Option<Iso2768> },     // explicit per-dimension override of the sheet class
}
```
Units: entered in the document unit, stored in mm (kernel unit; `units.rs`), as the GD&T store already does.

Two tiers, one resolution order **dimension-level spec > feature-level sidecar tolerance > sheet general class**:
- Feature-level = the existing sidecar `Annotation::Dimensional` keyed by feature PID (kept; this is how a bore's Ø H7 is verified by `gdt::verify` and survives booleans via `hand_on`).
- Dimension-level = `DimensionSpec.tolerance` / `tolerance_overrides` (new) for dimensions between two features (linear, ordinate), where the sidecar (keyed by one PID) cannot express a pair. Single-feature Ø/R dims write through to the sidecar via the existing size-tolerance mediator so the verifier keeps one source of truth. [Decision to confirm: extending `Annotation::Dimensional` with an optional second feature PID is the alternative; it touches the sidecar model and `hand_on` rules, so I prefer the two-tier design.]
- Fix `ToleranceBound::PlusMinus` sign limitation by mapping signed `Bilateral` to `Limits` internally (limits already absolute); no change to existing stored data.

### 3.2 ISO 286 fits, computed from the nominal (new `gdt/iso286.rs`)
Table-driven, not a formula guess: (1) size bands (1-3, 3-6, 6-10, 10-18, 18-30, 30-50, 50-80, 80-120, 120-180, 180-250, 250-315, 315-400, 400-500) with the nominal's band chosen by `over < D <= up to` (ISO 286-1 table 1); (2) standard tolerance grades IT01..IT18 values in micrometres per band (ISO 286-1 table 1); (3) fundamental deviations: shafts a..zc (es / ei) and holes A..ZC (EI / ES) per ISO 286-1 tables 2-3 with the H/h = 0 rule and the delta correction for holes J-ZC; JS/js = +/-IT/2. Output: `ResolvedFit { lower, upper, band, it_grade, fundamental_dev }`. Out of range (nominal > 500 mm first release, unknown letter/grade pair) -> typed refusal `FitOutOfScope`, never an interpolated number. For `H7/g6` also compute the **fit** (clearance / transition / interference) with min/max clearance, so the readback answers "will it assemble".
Validation vectors (hand-known; verify against the printed standard before merge [I]): O25 H7 = +0.021/0; O25 g6 = -0.007/-0.020; O10 H7 = +0.015/0; O10 h6 = 0/-0.009; O20 k6 = +0.015/+0.002; O50 p6 = +0.042/+0.026; O25 H7/g6 = clearance 0.007..0.041.
Wire-up: `ToleranceBound::Fit(FitClass)` gains `resolve(nominal) -> Result<(lo, hi), FitRefusal>`; `limit_range()` returns it; `attach_tolerances` (dimensioning.rs:2269) fills `ToleranceRef.limits`; `gdt::verify` evaluates fits instead of `NotYetVerified` (the existing honesty rule stays for out-of-scope).

### 3.3 General tolerance (ISO 2768)
Per-sheet class `f|m|c|v` + angular class + (ISO 2768-2 geometric K/H/L optional) stored on the sheet (`Drawing.general_tolerance_class`, serde default) instead of only inside the note string; `tolerance_note` is derived from it ("GENERAL TOLERANCES: ISO 2768-mK"). `GeneralTolerance::linear_for` (types.rs:859) already holds the table. Readback uses it for any dimension whose resolved spec is `None`. Per-dimension `General{class}` override renders nothing on the dimension but is recorded and appears in the readback. Style switch ISO / ASME (decimal comma vs point is a text option; default point): `Drawing.style: DimStyle {standard, decimals_linear, decimals_angular, arrow, text_height_mm}`.

### 3.4 Rendering (every renderer, one layout path)
Today a dimension is one string (`PlacedDimension.label`, layout.rs:234; `svg.rs` emits one `<text class="dim-text" data-dim-id>` at l.1016). Change `label` into structured runs, keep `label` as the flattened ink for compatibility with `text_bbox` collision boxes and the certificate:

`DimText { prefix, value, tol: TolRender, suffix, boxed: bool }` where `TolRender = None | Inline("+/-0.15") | Stacked{upper, lower} | LimitsStacked{max, min} | FitDesignation("H7") | FitWithLimits("H7 (+0.021/0)")`.

| Renderer | How |
|---|---|
| SVG (svg.rs) | one `<text>` with `<tspan>`s; stacked tolerance = two `<tspan>`s at 0.7x font size with `dy` offsets, left-aligned digits (decimals aligned); boxed dimension = `<rect>` around the text; reference = parentheses |
| DXF (dxf.rs) | `MTEXT` with stacking codes `\S+0.05^-0.10;` (AutoCAD stack syntax), `%%c`/`%%p` symbols; basic dimension = text with a box via MTEXT border flag; DXF has real layers, put tolerance on the dimension's layer |
| PDF (pdf.rs, 139 lines, hand-written) | extend the text emitter with two font sizes and baseline offsets for stacks; box via `re S` [I: confirm the writer has per-run size control; if not, this is the main renderer work] |
Format rules (ISO 129-1 / ASME Y14.5): symmetric and bilateral use equal decimals; a zero deviation prints "0" without sign; limits stacked max over min; ISO fit prints designation (and limits when `show` says so); ASME drops the leading zero on inch dimensions (style switch).
Layout: label width/height for collision (`layout::text_bbox`, :102) must be computed from the structured runs so stacked tolerances do not collide (today's `STACK`=8 mm may need `STACK_TOL` for dims carrying a stacked tolerance).
**Certificate:** the ink compare used for FCF (`ink.tolerance == block.tolerance_text`, sheet_certificate.rs:559) is extended to dimension labels: the rendered tolerance text must equal the text generated from the resolved spec; a mutated/stale string fails the certificate. `SheetFact` for a dimension gains `tolerance: ToleranceRef` (field exists) + the resolved limits + "source: dimension | feature | general".

## 4. GD&T on the sheet

### 4.1 Where it lives and how an imported STEP part gets it
The store is per **model**, keyed by PID (`GdtSidecar` in gdt/sidecar.rs; DRF in `model.drf`), not per CAD-built history. The STEP importer already gives every imported solid/face a content+entity-keyed PID (step/mod.rs:188), so datums, FCFs and size tolerances authored on an imported part use the same routes and mediators (`/api/agent/parts/{id}/datums|fcf|size-tolerance`; `gdt_create_annotation`, `gdt_designate_datum`) and are recorded as timeline events (`gdt_*`) like for a built part [V for built parts via gdt_history_tests; imported-part path is [I], needs one RED test, see 7]. They follow the part across booleans (`hand_on_gdt`).
- **Re-import caveat:** the PID is keyed by file content, so a re-exported/modified STEP gives new PIDs; GD&T would dangle (typed `Dangling`, as designed). Mapping across re-imports (by geometric signature) is a later, explicit feature, not a silent rebind.
- **Future AP242 PMI import** (`export-engine/src/formats/step/`, another workstream): read `GEOMETRIC_TOLERANCE` subtypes + `DATUM`/`DATUM_REFERENCE` + `SHAPE_ASPECT` (-> face set -> PID), `DIMENSIONAL_SIZE` + `PLUS_MINUS_TOLERANCE` / `LIMITS_AND_FITS`, and optionally `DRAUGHTING_CALLOUT` presentation for default placement; convert to the same mediators with `provenance: imported:step:pmi` and a status "as imported, unverified until `gdt::evaluate` runs". Test set: NIST CTC/FTC (`reference-nist/`), geometry-only STEP + PMI PDF as the oracle until the importer reads PMI.

### 4.2 Authoring and placement on the sheet (new)
- **Datum feature symbol**: pick a face/edge-on/dim extension line in a view -> `designate_datum` (existing mediator) + `DatumPlacement {view, anchor sheet pt, on: face|dimension_ext_line}` stored as an event (`drawing.gdt.place`). Letter auto-proposed (next free A..), ISO 5459 triangle vs ASME box styles via `DimStyle`.
- **Feature control frame**: pick a feature (face/edge/axis) or a dimension -> choose characteristic, zone (diametral), value, modifier (Ⓜ Ⓛ; existing `MaterialModifier`), datum refs (+ modifiers) -> `gdt_create_annotation` + placement. Attach modes: leader to the picked entity; stacked under a dimension text (position FCF under "4X O12 THRU"); on an extension line. `PlacedFcfBlock` gains `attach: Leader{to}|UnderDimension{dim_id}|OnExtLine`. New: composite frames, "all around", projected tolerance zone, "(basic)" boxed dims auto-drawn for the position FCF's `basic [x,y]` (`FeatureControlFrame.basic` exists, gdt/model.rs) so the TED dimensions that locate the feature are generated as `Basic` specs.
- Certificate/conformance: frames keep their `Fcf` facts; each frame shows the kernel's measured conformance (`gdt::evaluate`) in the readback only (not printed on the sheet).

## 5. History (timeline events)

New document-event kinds, all dotted so they're excluded from geometry replay (`is_geometry_event`, drawing_history.rs:96), each recorded with `RecordedOperation::new(kind).with_input_drawing(id)` exactly like `drawing.view.update` (drawing_mgr.rs:1147) and replayed by a new arm in `rebuild_drawings` (the match at drawing_history.rs:~143-375):

`drawing.dimension.add {spec}` · `drawing.dimension.update {id|key, patch}` (text, tolerance, placement, role_override) · `drawing.dimension.delete {id}` · `drawing.dimension.suppress|unsuppress {key}` · `drawing.dimension.set.add|member_add|member_remove` · `drawing.general_tolerance.set {class, angular, standard}` · `drawing.style.set {...}` · `drawing.gdt.place {...}`. GD&T content itself keeps its existing `gdt_*` events (no duplication: the sheet event only stores placement).
Properties: events carry the full spec (replay is deterministic, no measurement stored), authored `author` for the waiver/audit trail, and undo/redo come from the existing timeline undo (same mechanism that today undoes `drawing.view.update`). Replay of an event whose PID dangles at that history position yields a `DimensionDangling` issue in `DrawingRebuild.issues`, the sheet still rebuilds (current policy for unreplayable edits: `issues` list, drawing_history.rs `DrawingRebuild`). Sheet cache key `sheet_key` (drawing_history.rs:~75) must include the hash of dimension events so a cached sheet is reused only for the same edits. This is the "GD&T is design history" decision applied to dimensions/tolerances.

## 6. API, MCP and UI

### REST (new unless marked exists)
| Route | Purpose |
|---|---|
| `POST /api/drawings/{id}/dimensions` | add authored dimension / set (body: `view`, `kind`, `refs[]`, optional placement/text/tolerance; **no value**) -> `{dimension (measured), issues}` |
| `POST /api/drawings/{id}/dimensions/preview` | dry run, nothing recorded |
| `PATCH /api/drawings/{id}/dimensions/{dim_id}` | text, tolerance, placement, role_override |
| `DELETE /api/drawings/{id}/dimensions/{dim_id}` | delete authored / suppress auto (response states `waiver` if it leaves a feature undimensioned) |
| `POST /api/drawings/{id}/dimensions/{dim_id}` exists | change the part (driving value); unchanged |
| `GET /api/drawings/{id}/dimensions/{dim_id}/driver` exists | |
| `PATCH /api/drawings/{id}/general-tolerance`, `PATCH /api/drawings/{id}/style` | sheet class / style |
| `POST /api/drawings/{id}/gdt/place` (+ existing `/api/agent/parts/{id}/{datums,fcf,size-tolerance}`) | sheet placement of datum/FCF; content via existing routes |
| `POST /api/drawings/{id}/pick` | resolve a sheet point (+ candidates) to `{kind, pid, sub}`; wraps `EntityAt` and returns PIDs |
| `GET /api/iso286?nominal=&class=` | resolved fit limits (agents and the UI panel preview) |
Errors use the typed `ApiError` catalog (error_catalog.rs): `DimensionDangling`, `EdgeOnInView`, `NotTrueLength`, `ValueNotAccepted`, `TextContainsValue`, `CountMismatch`, `FitOutOfScope`, `UnsupportedMeasure`, `NotDriven` (existing).

### MCP (mcp/tools/drawing.ts, gdt.ts; remember: rebuild dist + reconnect after tool changes, CLAUDE.md)
`drawing_add_dimension` (picks by `pid` or `selector` like `gdt_*`), `drawing_edit_dimension`, `drawing_delete_dimension`, `drawing_place_dimension`, `drawing_set_tolerance` (`symmetric|bilateral|limits|fit|basic|reference`), `drawing_set_general_tolerance`, `drawing_place_gdt`, `drawing_pick`. Existing `drawing_set_dimension`, `drawing_read_semantics`, `drawing_query`, `gdt_*` stay. Registry/gates (`registry.ts`, `gates.ts`, `metatools.ts` keyword table incl. "tolerance"/"fcf") get the new names; `find_tool` ranks shift when tools are added (known IDF effect in memory notes).

### UI (app/components/panels/DrawingsWorkspace.tsx, drawing-overlays.tsx, lib/drawings-api.ts)
Today: click a dimension -> edit value; Annotate menu = disabled stubs. New, SolidWorks/Inventor/NX conventions:
1. **Smart Dimension tool** (toolbar + key `D`): cursor hover highlights pickable entities (via `/pick`), status line shows what to pick next; one pick on a circle -> Ø, on an arc -> R, on a line -> aligned, two picks -> linear (cursor position chooses H/V/aligned like SolidWorks), two lines -> angle; ghost from `/preview`; click to place; Esc ends tool; Shift = lock orientation.
2. **Ordinate / Baseline / Chain** as sub-tools under the same flyout; origin pick then successive picks.
3. **Property panel** (right card, pattern of `TitleBlockCard`): Value (read-only measured, with Driving/Reference badge and the "change the part" field only when Driven), Tolerance type segmented control (None / Symmetric / Bilateral / Limit / Fit / Basic / Reference) with live-validated fields, ISO fit picker (hole/shaft letter + grade, shows resolved limits from `/iso286` and the fit clearance), precision, prefix/suffix with symbol palette, arrow style, text side, "Show in 3D" (selects the PID entities in the viewport).
4. **Drag handles** on the selected dimension: text (free / along line), dimension-line offset, arrow flip, leader elbow; drag uses the same drag/ghost pattern as views (`lib/drawing-drag.ts`) and commits one `PATCH` on release; snap rows to the stack grid with Alt to disable.
5. **Right-click menu** on a dimension: Edit text, Tolerance..., Flip arrows, Move to other view, Make reference / driving, Show in 3D, Delete (states if it leaves a feature undimensioned -> waiver note). Del key = same; Ctrl+Z / Ctrl+Shift+Z through the timeline.
6. **Annotate menu** stubs for dimension, datum flag, GD&T frame become real; note/centre mark/surface finish stay disabled with their reason until they have routes (not faked, same policy as the file comment).
7. **Sheet settings** card: general tolerance class (f/m/c/v + angular), standard (ISO/ASME), decimals, arrow/text size.
All edits show the certificate delta (green/amber/red chip) after each commit: the app is a thin client; every number comes from the response.

## 7. Test plan (RED-first, each with its mutation proof)

| # | Feature | Fails today because | Test | Mutation that must turn it red |
|---|---|---|---|---|
| T1 | ISO 286 | `Fit` resolves to `None`/`NotYetVerified` | vectors in 3.2 + band-edge cases (3, 6, 10, 18, 30, 50, 80, 120, 180, 250, 315, 400, 500) + out-of-range refusal + H7/g6 clearance | change one table row / band comparator `<` vs `<=` |
| T2 | tolerance rendering | grep: nothing draws `ToleranceRef` | a bore with sidecar H7 / +/-0.05 / limits: SVG text contains the tspans, DXF contains `\S...^...;`, PDF text includes it; layout reserves room (no `DimensionLabelCollision`) | render from `label` only |
| T3 | certificate ink | dimension label ink not compared | tamper the SVG tolerance digits -> `certify` flags mismatch | remove the compare |
| T4 | tolerance on a linear dim | join is `kind == "diameter"` only (dimensioning.rs:2316) | authored linear between two faces carries a bilateral tolerance; survives restart | restore the diameter-only join |
| T5 | add dimension | route absent (404/405) | add Linear between two parallel faces: value == `measure` exactly; body with `value` -> `ValueNotAccepted`; edge-on view -> `EdgeOnInView` | accept client value |
| T6 | measured, not typed | n/a | perturb the part by a mould edit: authored dims re-measure and the sheet follows; stale sheet certifies STALE | cache value in the spec |
| T7 | lost entity | n/a | a boolean consumes a picked face: `DimensionDangling`, sheet still builds, callout flagged, **no re-target** (assert pid-by-pid) | fall back to nearest face |
| T8 | delete/suppress | no route | suppress an auto Ø: absent after rebuild + restart; certificate lists `Omitted waived_by`; unsuppress/undo restores | skip suppression in `auto_dimensions` |
| T9 | placement | layout is recompute-only | move text/offset/flip: persisted, replayed, survive restart; overlap kept + `verify` issue present | ignore override in `place_dimensions_routed` |
| T10 | text grammar | n/a | `4X` on a 4-hole pattern OK, on 3 -> `CountMismatch`; digits in prefix -> `TextContainsValue` | skip the validation |
| T11 | driving vs reference | n/a | Roshera-built part: driven dim shows plain and edits the part; imported part: authored dims print `(235)` and `NotDriven` on value edit | store `role` |
| T12 | history | no events | add/update/delete then undo/redo each; `rebuild_drawings` from the event list equals the live sheet (JSON equal) | drop an arm in `rebuild_drawings` |
| T13 | GD&T imported | untested | import a STEP fixture (export-engine tests produce small ones: `produce_fixed_sample.rs`), designate datum + FCF + size tolerance via the routes, sheet carries symbols/frames, restart-durable, dangling after consuming boolean | key by `FaceId` |
| T14 | sheet-placed GD&T | stubs only | place FCF attached `UnderDimension`: frame sits under the dimension text, no `GdtSymbolCollision` | |
| T15 | general class | class only in note string | set `K`/`m`->`c`: note text, readback tolerance and the per-dimension answer change; per-dim override beats it | |
| T16 | UI | stubs | component tests for the tool state machine (pick sequence -> request body), property panel validation, undo button; (no dev server; vitest) | |
| T17 | disconnection gate | | each new capability gets a production-call-site test through the router (`router_integration_tests` pattern) | |
Corpus gate: after each slice re-run the phase-1 bench (49 CADGenBench sheets + NIST) and report delta in dimension recall, verify errors, certificate omissions.

## 8. Build order and smallest useful first slice

**Slice 1 (smallest, user-visible, no new event kind): "a tolerance I authored shows on the sheet".**
1. `gdt/iso286.rs` + `Fit` resolution (T1): pure code, RED vectors.
2. Structured `DimText`/`TolRender` through layout + SVG (+DXF/PDF) for tolerances already bound by `attach_tolerances` (diameter dims + hole table rows) (T2), certificate ink compare (T3).
3. Tolerance property panel on the existing click-a-dimension flow, writing through the existing `POST /api/agent/parts/{id}/size-tolerance` (already recorded, undoable); MCP `drawing_set_tolerance` wrapper.
This delivers the Volvo-relevant "O25 H7 / +/-0.05 on the drawing" story using existing history and storage, and builds the rendering + certificate foundation every later slice needs.
**Slice 2:** dimension-level tolerance + linear dims (T4), `drawing.dimension.update/suppress/delete` events and replay arm (T8, T9, T12), text prefix/suffix (T10), reference parentheses + driven/reference badge (T11), right-click menu and drag handles.
**Slice 3:** smart dimension add: Linear/Aligned/Diameter/Radius/Angular + `/preview` + `/pick` + kernel measure extensions (T5-T7), Smart Dimension tool.
**Slice 4:** ordinate / baseline / chain sets; hole-callout dimension; basic dimensions auto-drawn from position FCFs.
**Slice 5:** GD&T from the sheet (datum + FCF placement/attach, composite frames, T13-T14); imported-part GD&T test; AP242 PMI import (coordinate with the STEP stream) scored on NIST.
**Slice 6:** general tolerance class/style UI (T15), ASME style, DWG/DXF layer mapping.

Risks to settle before slice 2/3 [I]: (a) PID for imported edges and vertex addressing (`EdgeEnd`) if edges lack PIDs on import; (b) PDF writer font-run support; (c) how undo composes with the `drawing.*` events in the timeline UI (mirror `drawing.view.update`); (d) the two-tier tolerance resolution vs extending the sidecar model; (e) `STACK` spacing for stacked tolerances and its effect on `dimension_band_gaps` (layout sizing for dense sheets such as ids 110/120 in the bench).
