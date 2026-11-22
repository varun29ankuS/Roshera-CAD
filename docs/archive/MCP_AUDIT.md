# MCP surface audit

Date: 2026-11-17. Scope: every tool `roshera-mcp` registers (115 kernel tools + `find_tool` / `describe_tool` / `invoke`), read against the `api-server` route each one calls.

> **HISTORICAL (2026-11-22).** A dated snapshot; its "Status" column is out of date. Fixed since:
> 1.1 and 1.3 (a2dcbbff); 1.2 (persistent ids as 32 hex digits), 1.4, 1.5, 1.6, 1.7, 1.8 and typed
> error bodies on the history/undo/redo/scrub/lens/GD&T routes (d7f8dba2). Still open as of
> d7f8dba2: 1.10 registry drift (ratchet at 23 differences), non-atomic composites (`drill_pattern`,
> `boolean_many`, `sketch_points`, `timeline_branch`, `drawing_set_title`), and the remaining
> bare-status handlers. Open items are carried in `PARKING_LOT.md`.

Out of scope, by instruction: `perception.ts`, `registry.ts`, `gates.ts`, `test/scale_s2.mjs`, backend `agent_registry.rs` / `main.rs` were read but not edited (another worktree has unmerged changes to them).

## How this was checked

| Question | Method | Strength |
|---|---|---|
| Does the tool's request/response match the route? | Read every handler in `src/tools/*.ts`; for each response field a handler reads and each body field it sends, read the Rust request/response struct or the handler's `json!` body. | Source against source. Nothing was run against a live server. |
| Do the two copies of the surface agree? | `.scratch/drift.mjs`: parses `agent_registry.rs` as text and compares it with the compiled MCP table (property names, `required`, bench, stability). | Measured. Names and required-ness only, not types or descriptions. |
| Which handlers does a test ever call? | Same script: a tool counts as covered when a file in `roshera-mcp/test/` dispatches its handler. | Measured. A test that calls a handler may still not check its wire shape. |
| Can an agent find the tool? | `.scratch/find_probe.mjs`: 33 plain-language intents I wrote without looking at the ranking code or the existing recall suites, run through `rankTools`. | Measured, small sample, written by one person. |
| Errors | Read `core.ts` `api()` / `fail()`, every raw `fetch`, and the return type of every handler behind a tool. | Source. |

The three scripts are in `.scratch/` at the worktree root and are read-only.

## The numbers

| Fact | Value |
|---|---|
| Tools in the MCP table | 118 (115 + 3 funnel tools) |
| Rows in the backend registry | 115 |
| REST routes in `api-server` | 285 |
| Tools whose schema differs between the two copies today | 17 of 115 |
| Tools whose handler no test ever calls | 74 of 115 before this task, 72 after |
| `api-server` handlers that return a bare status code, no typed error body (whole server; the ones a tool reaches are marked `bare` below) | 63 |
| Tools with an untyped parameter (`z.any()` / `z.record(z.unknown())`) | 8 |
| Tools that declare an output schema, or MCP annotations (read-only / destructive / idempotent) | 0 |
| MCP resources, subscriptions, prompts, progress notifications | 0 |
| Token cost of the whole surface (the registry's own estimate) | 26,930 |
| `find_tool` on 33 unseen intents | right tool first: 16; in the top five: 25 |

## Ranked gaps

Ordered by what they cost an agent. "Fixed" means fixed in this task with a failing-first test.

### 1. Wire mismatches: the tool and the route disagree (the `conn.id` class)

| # | Tool | Mismatch | Effect | State |
|---|---|---|---|---|
| 1.1 | `assembly_mate` | Reads `conn.id`; the connector route answers `connector_id` (`assembly_mates.rs::ConnectorResponse`). | No agent could create a mate. Each attempt left two connectors no mate referenced, recorded in history. | **Fixed.** |
| 1.2 | `assembly_mate` (`a.pid` / `b.pid`) | Schema says `pid: string`; the route's `FaceSelector.pid` is `Option<u128>`, a bare JSON integer. The server itself hands PIDs to agents as a decimal string (`select_face` → `persistent_id`) and as 32 hex digits (Kernel Lens). | The "durable, unnamed" rung of the anchor ladder cannot be used from MCP: a string is refused (422), and JavaScript cannot emit a 128-bit integer. Three encodings of one id across the server's own surface. | Not fixed. Needs `assembly_mates.rs` to accept the string forms, and an `api-server` test run. |
| 1.3 | `create_sphere` | Declares `name`; the handler never read it and `POST /api/geometry` takes no name. | The part was named "Sphere 7" whatever the agent asked for, with no word that the argument was dropped. | **Fixed** (applied through the part-name route; a refused name is reported on the result). |
| 1.4 | `timeline_undo`, `timeline_redo` | The routes answer HTTP 200 with `success: false` when there is nothing to undo, the session is unknown, or the replay was refused. The tools wrap any 200 in `ok()`. | A failed undo is success-shaped: `isError` is unset, `cad_program` does not stop, the refusal cache never sees it. | Not fixed. |
| 1.5 | `timeline_merge` | A conflicted merge is HTTP 200 with `success: false` and `conflicts`. Wrapped in `ok()`. | Same as 1.4. The agent must read the body to learn nothing merged. | Not fixed. |
| 1.6 | `transform` | The route returns `solid_id` and a `perception` block (the certificate). The tool returns `{ object_uuid, moved: true, note: "render_part to confirm" }`. | A solid-mutating verb whose result carries no verdict, although the server sent one. | Not fixed. |
| 1.7 | `psketch_solve` | Projects the report to `status / violations / dof{…} / certificate / points` and drops `entities_skipped` and `dof.unverified_components`. | The agent is not told which entities the solver did not handle, which is the fact that qualifies "converged". | Not fixed. |
| 1.8 | `gdt_fcf` | Drops `annotation_id` and `modifier` from the answer. | Harmless today only because no tool edits or deletes an FCF. | Not fixed. |
| 1.9 | `verify_part` | Sends `?full=1`; the route has no such parameter (full is the default; `?fast=1` is the opt-out). | None. Dead parameter. | In a file this task may not edit. |
| 1.10 | 17 tools, registry copy | The backend registry row lacks parameters the tool has, or has parameters the tool lacks. See "Drift between the two copies". | Anything generated from the backend registry today would lose `acknowledge_unsound` on nine tools and `plane` on both sketch-to-solid tools. | Not fixed (`agent_registry.rs` is owned elsewhere). |

Everything else read clean: assembly (apart from 1.1 and 1.2), create, modify, psketch field names, io, inspect, labels, queries, lens, drawing, GD&T and timeline handlers read fields the routes do return, and send bodies the request structs accept.

### 2. Errors: typed on the server, flattened on the way to the agent

- **63 `api-server` handlers return a bare `StatusCode`** (25 in `handlers/timeline.rs`, 14 in `main.rs`, 8 in `handlers/agent.rs`, 5 datums, 4 sessions, others). The tools that reach one: `timeline_undo` / `redo` / `history` / `scrub` / `mould` / `clear_timeline`, `rebuild_certificate`, `bind_parameter_name`, `lens_explain`, `lens_why_not_sound`, `gdt_report`, `get_face`, `get_revolve_profile`, `get_pointer`. For these the agent sees `ERROR: POST /api/timeline/undo → 500: ` with an empty body: no `error_code`, no hint, no `retryable`. The error catalogue exists and `fail()` reads it well; these handlers never reach it.
- **`set_part_color` cannot fail**: the route stores and records a colour for any integer, including a part that does not exist.
- **Six tools use a raw `fetch` with no timeout**: `select_face`, `select_edge`, `label_create`, `label_resolve`, `gdt_datum`, `gdt_fcf`. `api()` aborts at 120 s and reports a 504; these hang for as long as the server does.
- **`gdt_datum` and `gdt_fcf` rebuild the error as prose** (`REFUSED: <message>`), so the catalogue's `error_code` / `details` never reach `structuredContent`.
- **`refusalOrFail` (timeline tools)** turns any 404 / 409 / 422 into `{ refused: <body> }` and loses the hint tier that `fail()` computes.
- **Success-shaped failures**: 1.4 and 1.5 above. `import_step` also returns a non-error result for an import that is not sound (it says so in the body, `ok: false`); that one is arguably right, but it is a third convention.
- **Error text leaks the transport**: `ERROR: POST /api/assembly/<id>/mate → 422: Failed to deserialize the JSON body…`. An agent learns a REST path it cannot call, and nothing about which argument was wrong. Server-side request structs are not validated against the tool schema, so a shape error surfaces as a serde message.
- **Timeout advice invites a duplicate**: the 504 text says the backend "may still be computing". No tool carries an idempotency key, so a retry after a slow boolean can build the geometry twice.

### 3. Evidence in the result ("the kernel cannot lie")

Good: every `create_*`, `shell`, `fillet_edges`, `chamfer_edges`, `boolean`, `psketch_extrude`, `psketch_revolve`, `import_step` result carries the one-line certified verdict through `okp()`; `boolean_many` and `drill_pattern` certify each step and stop on the first unsound one; assembly solve / certify / dof / interference pass the server's typed facts through whole.

Gaps:
- `transform` (1.6): no verdict at all.
- `delete_part`, `clear_parts`, `timeline_undo` / `redo` / `mould` / `merge`: the model changed, the result does not say what is now sound. `timeline_merge` fetches a rebuild certificate; the others do not.
- `drawing_set_dimension`: reduces the mould's certificate to one boolean (`sound`).
- `psketch_solve` (1.7).
- `export_part`, `drawing_export_sheet`: the file leaves with no certificate beside it (the verified-export idea in memory would close this).
- No result names the timeline event it recorded. An agent cannot tie "what I just did" to a step it could later mould, suppress or cite, without a second call to `timeline_history`.
- Results are JSON inside a text block. Only errors use `structuredContent`. Nothing declares an output schema, so no client can validate or type a result.

### 4. Side effects and idempotency

- **Composite tools are not atomic.** `drill_pattern` creates all N cutter cylinders first, then subtracts them one by one; a failure or an unsound step at hole k leaves N−k cutter solids in the scene. `boolean_many` stops part-way with the base already changed. `sketch_points` issues one mutation per point and stops at the first failure with no count of what landed. `timeline_branch` (create, then activate) and `drawing_set_title` (rename, then title block) can each half-apply. `assembly_mate` had the same shape (connector, connector, mate); it now takes back what it made.
- **Destructive tools take no confirmation**: `clear_parts`, `clear_timeline`, `blackboard_clear`, `delete_part`. `clear_timeline` and `blackboard_clear` are outside the intent gate.
- **No idempotency key anywhere.** Every create is repeatable by accident (see the 504 note).
- **Two id spaces, split by tool.** `fillet_edges`, `chamfer_edges`, `make_drawing`, every inspect and label tool take the integer `part_id`; `boolean`, `shell`, `transform`, `export_part`, `plane_from_face`, assembly tools take the `object_uuid`. Booleans mint a new integer id and keep the base's uuid. An agent must hold both for every part and pick the right one per tool; two tools pay an extra scene-snapshot fetch to convert.

### 5. Schemas and descriptions, read as an LLM would

- **Eight untyped parameters** carry their grammar in prose: `assembly_verify.mates`, `assembly_verify.mechanisms`, `assembly_mate.kind`, `psketch_add_entity.params`, `psketch_constrain.constraint_type`, `psketch_op.params`, `kb_lookup`, `cad_program` op args. The server has exact Rust types for all of the first six (`DocMateKind`, `Constraint`, each `Add*Request`). The model gets no validation before the round trip and learns the shape from a serde error.
- **JSON inside a string**: `label_create.selector`, `gdt_datum.selector`, `gdt_fcf.target_selector` take a JSON document as a string.
- **Angle units are mixed**, each documented, none consistent: degrees in `revolve.angle_deg`, `transform.rotation.angle_deg`, `drill_pattern.start_angle_deg`, `assembly_add_instance.rotation_deg`; radians in `psketch_revolve.angle`, every sketch constraint, mate limits, `assembly_drag.value`. `revolve` and `psketch_revolve` make the same shape with different angle units.
- **No examples field.** A few descriptions inline one; most do not. `find_tool` and `describe_tool` have nothing to show but the schema.
- **Descriptions carry policy and history** ("NEVER loop this tool", gate rationale, slice numbers). `psketch_add_entity` is 1,132 characters, `kb_lookup` 1,369, `cad_program` 1,014. This is steering text paid for on every listing.
- **No server `instructions`**, so the units convention, the two id spaces and the checkpoint rule are restated per tool or not at all.
- `assembly_create`, `assembly_add_instance`, `assembly_list_instances`, `assembly_transform_instance`, `assembly_view` type `assembly_id` as a plain string; the later assembly tools type it as a uuid.

### 6. Discoverability (`find_tool`)

33 intents, written as an engineer would say them. Right tool first 16 times, in the top five 25 times, absent 8 times.

Misses, all domain vocabulary the descriptions do not contain:

| Intent | Wanted | Got first |
|---|---|---|
| bolt the cover to the housing | `assembly_mate` | `part_coverage` |
| constrain the shaft concentric with the bore | `assembly_mate` | `psketch_constrain` |
| rotate the crank 30 degrees | `assembly_drag` | `assembly_dof` |
| round the corners of the sketch | `psketch_op` | `create_sketch` |
| change the hole diameter to 12 | `timeline_mould` | `drill_pattern` |
| try an alternative design without losing this one | `timeline_branch` | `gdt_datum` |
| cut the part and look inside | `section_view` | `part_rename` |
| show the user the front view | `guide_attention` | `section_view` |

Two structural problems beyond vocabulary:
- **A capability that does not exist still returns confident matches.** "mirror the part" returns `clear_parts` first; "thread the hole" returns `drill_pattern`. There is no "nothing does this" answer.
- **The ranking is a hand-tuned lexical scorer in the MCP process.** Any new tool shifts every score (already noted in project memory). The existing recall suites were tuned against; these 33 were not.

### 7. Capabilities the server has and agents cannot reach

285 routes, 115 tools. Routes with no tool, that an agent would want:

- Solid operations: `mirror`, `pattern/linear`, `pattern/circular`, `face/extrude`, click-draft `extrude_cut` and `revolve`.
- Sketch: `ellipse`, sketch `fillet` / `chamfer` (corner), `trim-at`, `delete` entities, delete constraint, change a constraint's value, `drag`, `snap`, `infer-constraints`, bulk `build`, and reading a sketch back (`GET /api/csketch/{id}`).
- Assembly: remove an instance, delete an assembly, delete a connector.
- GD&T: edit / delete a datum or FCF, size tolerances, per-face tolerance attach and verify.
- Timeline: suppress a feature, re-sketch, reopen a sketch, the evidence pack, editable parameters, the feature tree and chapters (what the user sees in the app's tree).
- Drawing: add / move / remove a view.
- Datum planes (`/api/datums`), transactions (`/api/tx/*`), oriented bounding box, best view.

The agent and the engineer do not have the same verbs. That is the cost of writing each tool by hand.

### 8. Test coverage

- **74 of 115 handlers were never called by a test (72 now).** Uncovered whole: the assembly bench (12 tools, which is how 1.1 shipped), drawing edit (5), sketch constrain / solve / certify / dof / op / revolve, timeline branch / switch / merge / conflicts / redo / scrub / recipe, every query and most inspect tools, `transform`, `revolve`, `import_step`, `export_part`.
- **Where tests exist they stub the server by hand**, so a stub can agree with the tool and disagree with the server. Only two suites pin a stub to the Rust source (`structured_errors` s5, and the two added here).
- **Four suites do not run in a clean worktree**: `find_tool_recall.mjs`, `scale_s3s4.mjs` and `dist_fresh.mjs` need a built `dist/`; `ask_choice.test.mjs` needs the `yaml` module, which is not in `roshera-mcp`'s dependencies.
- **The drift gate compares names, not schemas.** `consumeRegistry` checks the hash against the backend's own tools (encoder parity) and the two name sets. Seventeen schema differences pass it.

### 9. MCP features not used

Resources and subscriptions (parts, the document, the user's context), prompts, progress notifications and cancellation for long operations, output schemas, tool annotations, elicitation (`ask_choice` builds a text fence instead), server instructions. One timeout (120 s) covers a fillet and a STEP import alike.

## Drift between the two copies (measured)

`node .scratch/drift.mjs`. Same 115 names on both sides. Parameters that exist on one side only:

| Tool | In the MCP tool, missing from the backend registry | In the backend registry, missing from the MCP tool |
|---|---|---|
| `shell`, `fillet_edges`, `chamfer_edges`, `boolean`, `boolean_many`, `drill_pattern`, `transform`, `export_part`, `make_drawing` | `acknowledge_unsound` | |
| `drawing_export_sheet` | `acknowledge_layout_issues`, `acknowledge_unsound` | |
| `psketch_extrude` | `direction`, `plane` | |
| `psketch_revolve` | `plane` | |
| `timeline_checkpoint` | `skip_verification` | |
| `blackboard_add_entry`, `blackboard_edit_entry`, `blackboard_list`, `blackboard_clear` | | `part_id`, `scope` |

Types, defaults, enum values and descriptions were not compared; expect more there.

## Tool by tool

Columns: **Wire** = request and response fields against the route (ok / finding number). **Errors** = `cat` when the route returns catalogue errors that `fail()` surfaces typed, `bare` when the route returns a status code alone (read from the handler's return type), `raw` when the tool bypasses `api()`. **Evidence** = what the result carries about soundness. **Effects** = R read-only, W writes, W* composite and not atomic, D destructive. **Tests** = suites that dispatch the handler.

### Core

| Tool | Route | Wire | Errors | Evidence | Effects | Tests |
|---|---|---|---|---|---|---|
| `create_box` | `POST /api/geometry/box` | ok | cat | certified verdict | W | 6 |
| `create_cylinder` | `POST /api/geometry/cylinder` | ok | cat | certified verdict | W | 3 |
| `create_cone` | `POST /api/geometry/cone` | ok | cat | certified verdict | W | 0 |
| `create_sphere` | `POST /api/geometry` (+ part-name) | 1.3 fixed | cat on create; the generic route returns an untyped error tuple | certified verdict | W | 1 (new) |
| `revolve` | `POST /api/geometry/revolve` | ok; 10 params, three exclusive modes described in prose | cat | certified verdict | W | 0 |
| `nurbs_loft` | `POST /api/geometry/nurbs_loft` | ok | cat | certified verdict (soundness, not shape fidelity) | W | 0 |
| `boolean` | `POST /api/geometry/boolean` | ok | cat | certified verdict, `consumed` | W | 1 |
| `boolean_many` | same, looped | ok | cat | per-step verdict, halts | W* | 2 |
| `drill_pattern` | cylinder ×N, boolean ×N | ok | cat | per-step verdict, halts | W*, leaves cutters behind on failure | 2 |
| `shell` | `POST /api/geometry/shell` | ok | cat | certified verdict | W | 3 |
| `fillet_edges` | `POST /api/geometry/fillet` | ok; takes `part_id`, converts to uuid by scene snapshot | cat, computed `r_max` hint | certified verdict | W | 1 |
| `chamfer_edges` | `POST /api/geometry/chamfer` | ok | cat | certified verdict | W | 0 |
| `transform` | `POST /api/geometry/transform` | 1.6 | cat | none | W | 0 |
| `part_rename` | `POST /api/parts/uuid/{uuid}/name` | ok | cat | n/a | W | 0 |
| `delete_part` | `DELETE /api/agent/parts/{id}` | ok | cat | none | D | 4 |
| `clear_parts` | `DELETE /api/agent/parts` | ok | cat | none | D, no confirmation | 2 |
| `list_parts` | `GET /api/agent/parts` | ok | cannot fail | n/a | R | 5 |
| `get_part` | `GET /api/agent/parts/{id}` | ok | cat | n/a | R | 1 |
| `mass_properties` | `GET …/mass` | ok | cat | exact-vs-mesh stated by the route | R | 1 |
| `render_part` | `GET …/render` | ok | cat | open / non-manifold counts | R | 0 |
| `scene_view` | `GET /api/agent/scene/orbit` | ok | own response, not read | open / non-manifold counts | R | 0 |
| `section_view` | `GET …/section` | ok | cat | area, extents | R | 0 |
| `verify_part` | `GET …/perception`, `…/render` | 1.9 | cat | the full certificate | R | 3 |
| `get_pointer` | `GET /api/agent/pointer` | ok | 404 handled | n/a | R | 0 |
| `guide_attention` | `POST /api/agent/show` | ok (tagged enum, built by hand) | cat | `delivered_to` count | W (the user's screen) | 0 |
| `select_face` | `POST …/select-face` | ok | raw, no timeout; 409 candidates preserved | n/a | R | 1 |
| `select_edge` | `POST …/select-edge` | ok | raw, no timeout | n/a | R | 1 |
| `set_part_color` | `POST …/color` | ok | cannot fail (an unknown part id is accepted) | n/a | W | 0 |
| `document_units` | `GET` / `PATCH /api/document/units` | ok | refusal reason surfaced | n/a | W | 0 |
| `document_rename` | `PATCH /api/documents/{id}` | ok | cat | n/a | W | 0 |
| `import_step` | `POST /api/geometry/import_step` | ok | cat; unsound import is a non-error result | per-solid verdict, coverage report | W | 0 |
| `export_part` | `POST /api/export`, download | ok | typed export refusals | none travels with the file | W (file) | 0 |
| `lens_explain` | `GET /api/lens/{part}[/face/{id}]` | ok | bare | the journal itself | R | 0 (helper tested) |
| `lens_why_not_sound` | same, fanned out | ok | bare | blocking evidence, grouped | R | 0 (helper tested) |
| `workbench` | none (MCP-side) | n/a | n/a | n/a | session state | 1 |
| `cad_program` | none (dispatches tools) | n/a | stops on first refusal | per-op certificate summary | W* by design | 3 |

### Sketch

| Tool | Route | Wire | Errors | Evidence | Effects | Tests |
|---|---|---|---|---|---|---|
| `create_sketch` | `POST /api/sketch` | ok | cat | n/a | W | 0 |
| `sketch_add_shape` | `POST /api/sketch/{id}/shape` | ok | cat | n/a | W | 0 |
| `sketch_points` | `POST …/point` ×N | ok | cat | n/a | W*, one mutation per point | 1 |
| `sketch_extrude` | `POST /api/sketch/{id}/extrude` | ok; `direction`, `consume` not exposed | cat | certified verdict | W | 0 |
| `plane_from_face` | `POST /api/sketch/plane-from-face` | ok | typed refusal | n/a | R | 0 |
| `psketch_begin` | `POST /api/csketch` | ok | cat | n/a | W | 1 |
| `psketch_add_entity` | `POST /api/csketch/{id}/{kind}` | ok; `params` untyped; `ellipse` route not exposed | cat | n/a | W | 2 |
| `psketch_constrain` | `POST …/constraint` | ok; `constraint_type` untyped | cat | n/a (read `psketch_certify`) | W | 0 |
| `psketch_solve` | `POST …/solve`, `GET` sketch | 1.7 | cat | certificate digest | W (moves entities) | 0 |
| `psketch_certify` | `POST …/certify` | ok, passed whole | cat | the full sketch certificate | R | 0 |
| `psketch_dof` | `POST …/certify` | ok, projection | cat | constrainedness, witnesses | R | 0 |
| `psketch_op` | nine routes by `op` | ok; `params` untyped; corner fillet / chamfer, `trim-at`, delete not exposed | cat | certificate digest | W | 0 |
| `psketch_plane_from_face` | `POST /api/sketch/plane-from-face` | ok (duplicate of `plane_from_face` with a different parameter name: `object_uuid` vs `object_id`) | typed refusal | n/a | R | 0 |
| `psketch_extrude` | `POST …/extrude` | ok | cat | certified verdict | W | 1 |
| `psketch_revolve` | `POST …/revolve` | ok; angle in radians where `revolve` takes degrees | cat | certified verdict | W | 0 |

### Assembly

| Tool | Route | Wire | Errors | Evidence | Effects | Tests |
|---|---|---|---|---|---|---|
| `assembly_verify` | `POST /api/assembly/verify` | ok; `mates`, `mechanisms` untyped; a second mate vocabulary (Concentric / Coincident / Fixed) beside `assembly_mate`'s | cat | five-part verdict | R | 0 |
| `assembly_create` | `POST /api/assembly` | ok | cat | n/a | W | 0 |
| `assembly_add_instance` | `POST …/instance` | ok | cat | instance summary with soundness | W | 0 |
| `assembly_list_instances` | `GET /api/assembly/{id}` | ok; also lists mates (the only place mate ids can be read back) | cat | `all_sound` | R | 0 |
| `assembly_transform_instance` | `PATCH …/instance/{iid}` | ok | cat | summary | W | 0 |
| `assembly_view` | `GET …/view` | ok | cat | open / non-manifold counts | R | 0 |
| `assembly_mate` | connector ×2, mate; `PATCH` / `DELETE` mate | 1.1 fixed; 1.2 open; `kind` untyped | cat | mate, connector provenance (added) | W, now takes back its connectors on failure | 1 (new) |
| `assembly_solve` | `POST …/solve` | ok | cat | per-mate facts, witnesses | W (writes poses) | 0 |
| `assembly_certify` | `POST …/certify` | ok | cat | the assembly certificate | R | 0 |
| `assembly_dof` | `GET …/dof` | ok | cat | structural vs numeric DOF | R | 0 |
| `assembly_drag` | `POST …/drag` | ok | cat | scope proof, rank transitions | W | 0 |
| `assembly_interference` | `GET …/interference` | ok | cat | swept facts, certified clearance | R | 0 |

### Drawing and GD&T

| Tool | Route | Wire | Errors | Evidence | Effects | Tests |
|---|---|---|---|---|---|---|
| `make_drawing` | `POST /api/parts/{id}/drawing` | ok; `scale`, `plain` not exposed | cat, unsound-base gate | layout quality report | W | 1 |
| `drawing_export_sheet` | `GET /api/drawings/{id}/{svg,pdf,dxf}` | ok | typed gate refusals | none travels with the file | W (file) | 1 |
| `drawing_set_dimension` | `POST …/dimensions/{dim}` | ok; `mould.is_sound` read on trust, not traced to the struct | cat | one boolean | W (changes the model) | 0 |
| `drawing_set_projection` | `PATCH …/projection` | ok | cat | quality | W | 0 |
| `drawing_set_title` | `PATCH …/rename`, `…/title-block` | ok | cat | n/a | W* (two writes) | 0 |
| `drawing_read_semantics` | `GET …/semantic` | ok | cat | sheet certificate line, stale facts | R | 0 |
| `drawing_query` | `POST …/query` | ok (eleven optional fields forwarded as given) | cat | answer with provenance | R | 0 |
| `dimension_part` | `GET …/dimensions` | ok | cat | n/a | R | 0 |
| `gdt_datum` | `GET` / `POST …/datums` | ok | raw, no timeout; the route's typed refusal is flattened to prose | persistence stated ("session") | W | 1 |
| `gdt_fcf` | `POST …/fcf` | 1.8 | raw, no timeout; the route's typed refusal is flattened to prose | conformance verdict line | W | 1 |
| `gdt_report` | `GET …/gdt` | ok | bare | verdict per annotation | R | 0 |

### Analysis

| Tool | Route | Wire | Errors | Evidence | Effects | Tests |
|---|---|---|---|---|---|---|
| `ground_truth` | `GET …/truth` | ok | cat | kernel facts | R | 0 |
| `occupancy_view` | `GET …/occupancy` | ok | cat | fill fraction | R | 0 |
| `measure_faces` | `POST /api/agent/measure` | ok | 422 / 404 refusal preserved | value with entities | R | 0 |
| `part_coverage` | `GET …/coverage` | ok | cat | coverage | R | 0 |
| `verify_claim` | `POST /api/agent/verify-claim` | ok | cannot fail (errors ride in the verdict) | measured vs claimed | R | 1 |
| `get_revolve_profile` | `GET …/profile` | ok | bare | n/a | R | 0 |
| `get_face` | `GET /api/agent/faces/{id}` | ok | bare | n/a | R | 0 |
| `part_distance` | `GET …/distance/{a}/{b}` | ok | cat | n/a | R | 0 |
| `part_features` | `GET …/features` | ok | cat | n/a | R | 0 |
| `dfm_check` | `POST …/dfm` | ok | typed `{error, reason, remedy}`, not the catalogue shape | findings | R | 0 |
| `point_query`, `ray_query`, `region_query` | `POST …/point-query` etc. | ok | cat | exact classification | R | 0 |
| `kb_lookup` | none (bundled data) | n/a | typed refusal | citations | R | 3 |

### Timeline

| Tool | Route | Wire | Errors | Evidence | Effects | Tests |
|---|---|---|---|---|---|---|
| `timeline_checkpoint` | `POST /api/timeline/checkpoint`, notebook line | ok | cat | n/a | W* (two writes; the second is reported if it fails) | 15 |
| `timeline_checkpoints` | `GET /api/timeline/checkpoints` | ok; a non-array answer becomes an empty list, silently | cannot fail | n/a | R | 1 |
| `timeline_history` | `GET /api/timeline/history/{branch}` | ok | bare | durability disclosure | R | 1 |
| `timeline_undo` | `POST /api/timeline/undo` | 1.4 | bare on 5xx; 200 on refusal | none | W | 1 |
| `timeline_redo` | `POST /api/timeline/redo` | 1.4 | same | none | W | 0 |
| `timeline_mould` | `POST /api/timeline/mould` | ok; `linked` not exposed | typed refusal | rebuild certificate in the answer | W | 3 |
| `bind_parameter_name` | `POST /api/timeline/parameter-name` | ok | typed refusal | n/a | W | 0 |
| `rebuild_certificate` | `GET /api/timeline/rebuild-certificate/{branch}` | ok; branch not URL-encoded | bare | the certificate | R | 0 |
| `timeline_scrub` | `GET /api/timeline/scrub/{branch}/{seq}` | ok; branch not URL-encoded; drops `events_skipped` | bare | none (a state is shown without its verdict) | R | 0 |
| `recipe_get` | `GET /api/timeline/recipe/{ref}` | ok | typed refusal | certificate summary | R | 0 |
| `timeline_branch` | `POST /api/branches`, `…/active` | ok | typed refusal | n/a | W* | 0 |
| `timeline_branches` | `GET /api/branches` | ok; a non-array answer becomes an empty list, silently | cat | n/a | R | 0 |
| `timeline_switch` | `POST /api/branches/active` | ok | typed refusal | n/a | W | 0 |
| `timeline_merge` | `POST /api/branches/{id}/merge` | 1.5 | typed refusal on 4xx; conflicts are a 200 | rebuild certificate when merged | W | 0 |
| `timeline_conflicts` | `GET /api/branches/{id}/conflicts` | ok | typed refusal | witnesses | R | 0 |
| `clear_timeline` | `POST /api/timeline/clear` | ok | bare | `model_reconciled` | D, no confirmation, outside the intent gate | 1 |

### Labels and notebook

| Tool | Route | Wire | Errors | Evidence | Effects | Tests |
|---|---|---|---|---|---|---|
| `label_create` | `POST …/labels` | ok; `selector` is JSON in a string | raw, no timeout; refusal body preserved | n/a | W | 1 |
| `label_list`, `propose_labels` | `GET …/labels`, `…/propose-labels` | ok | own JSON body, not the catalogue shape | n/a | R | 1 / 0 |
| `label_resolve` | `GET …/labels/{name}/resolve` | ok | raw, no timeout | Holds / Stale | R | 1 |
| `label_delete`, `label_rename` | `DELETE` / `PATCH …/labels/{name}` | ok | own JSON body, not the catalogue shape | n/a | W | 0 |
| `blackboard_add_entry`, `_edit_entry`, `_list`, `_clear` | `/api/blackboard…` | ok; scope deliberately not exposed (the backend registry row says otherwise) | cat | n/a | W; `_clear` is D with no confirmation | 2 / 1 / 1 / 1 |
| `ask_choice` | none (builds a fence) | n/a | typed refusal | n/a | none | 1 |

### Funnel

| Tool | Finding |
|---|---|
| `find_tool` | Section 6. Lexical, MCP-side, no "no such capability" answer. |
| `describe_tool` | Returns the schema only: no example, no route, no gate list, no result shape. |
| `invoke` | Validates with the tool's own schema; correct. The model still pays a `describe_tool` round trip for every long-tail call. |

## What was changed in this task

| File | Change |
|---|---|
| `roshera-mcp/src/tools/assembly.ts` | `assembly_mate` reads `connector_id`, refuses a connector answer with no id, deletes the connectors of a mate that was not created, and returns each connector's anchor provenance. |
| `roshera-mcp/src/tools/create.ts` | `create_sphere` applies `name` through the part-name route and reports a name the server did not take. |
| `roshera-mcp/test/assembly_mate_wire.test.mjs`, `create_sphere_name.test.mjs` | New. Each drives the real dispatch stack against a stub pinned to the Rust source. Added to `npm run test:gates`. |
| `roshera-backend/timeline-engine/src/chapters.rs`, `tests/chapters.rs` | A `sketch_revolve` step is a `revolve` chapter (see the report for this task). |

## Not verified

- Nothing here was run against a live `api-server`. Every "ok" in the Wire column is a source reading.
- Types, defaults and enum values were not compared between the two registry copies; only names and required-ness.
- `drawing_set_dimension`'s `mould.is_sound`, the drawing semantic fields and `guide_attention`'s body were checked by name, not by building the structs.
- Read tools were not checked field by field where the tool passes the server's answer through whole.
- The 63 bare-status handlers were counted by return type; which of them a given tool can actually reach was checked only for the tools named above.
