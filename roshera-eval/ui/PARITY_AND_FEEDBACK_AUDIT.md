# Parity and feedback audit

2026-11-17, at commit `cc0e6fb2`. Two questions:

1. **Parity.** For everything an agent can build through MCP/REST, what can a person do with it afterwards in the app?
2. **Feedback.** What can an agent know about what the person is pointing at, and how fast?

**How this was made.** Everything here is read from source; nothing was exercised in a running app. The part and assembly tables were compiled by two read-only sub-agents and I re-read the code behind the findings marked ✔. Rows marked *(inferred)* follow from the code but nobody ran them. Paths are relative to the repo root.

## The ten gaps that matter most

| # | Gap | Where |
|---|---|---|
| 1 | **An agent cannot create a mate through MCP.** `makeConnector` returns `conn.id`; the server answers `connector_id`. The mate request goes out with no `a`/`b` and is refused, and each attempt leaves two orphan connectors in the history. ✔ | `roshera-mcp/src/tools/assembly.ts:354` vs `roshera-backend/api-server/src/assembly_mates.rs:75-81` |
| 2 | **Agent assembly edits do not appear in the open app.** No assembly handler broadcasts; the panel reloads only after the person's own action. Undo rebuilds assemblies on the server and the panel stays stale. | `roshera-app/src/stores/assembly-doc-store.ts:29-30`, `AssemblyWorkspace.tsx:95-123` |
| 3 | **Almost nothing in an assembly is editable after creation.** Instances cannot be moved, renamed or recoloured; mates cannot be edited (kind, offset, angle, limits), only deleted. The agent has 17 mate kinds; the person can create 6, with no limits. | `AssemblyWorkspace.tsx:43-50,266-330` |
| 4 | **There is no history view in the Assembly workspace.** `<Timeline/>` is mounted only in the Part branch. | `roshera-app/src/App.tsx:175-186` vs `:311` |
| 5 | **A feature's edit dialog is numbers only.** Fillet/chamfer edges cannot be re-picked, boolean operands and type cannot be changed, a pattern's count is not recorded anywhere, axes and directions are excluded, a transform is a matrix. | `timeline-engine/src/dimension_drive.rs:42-52,184-266`, `FeatureEditPanel.tsx:137-283` |
| 6 | **An agent's revolved sketch feature has no "Edit sketch".** `psketch_revolve` records `sketch_revolve`; `kind_of` maps only commands that *start with* `revolve`, so the chapter's kind is the raw string and the button (extrude/cut/boss/revolve) never shows. The server would accept the reopen. ✔ | `timeline-engine/src/chapters.rs:509`, `FeatureEditPanel.tsx:52,175` |
| 7 | **No reorder, no feature rename, no un-delete, no "reject" of a branch.** No routes for the first three; `DELETE /api/branches/{id}` exists with no caller in the app. | router `api-server/src/main.rs:11564-11587,11885-12040` |
| 8 | **GD&T and labels made by an agent are read-only for the person.** Datums, FCFs, size tolerances: no app caller of the routes. Labels are not even recorded in the timeline. | `roshera-app/src` has no `/datums`, `/fcf`, `/size-tolerance` caller; `geometry-engine/.../topology_builder.rs:1328-1360` |
| 9 | **Undo is one recorded step, not one feature.** A hole is two steps; `drill_pattern` with N holes is 2N or more; `cad_program` has no rollback. | `timeline-engine/src/timeline.rs:1702-1785`, `roshera-mcp/src/cad_program.ts:255` |
| 10 | **The agent could only learn the last click.** No hover, no selection, no tool, no camera, no persistent id, one global slot, no way to tell a stale click from a fresh one, and nothing told the person an agent had read it. (Closed by the slice in this change — see "Feedback channel".) | `api-server/src/handlers/agent.rs:2947-2996`, `roshera-app/src/components/viewport/CADMesh.tsx:25-42` |

There is no second-class "agent geometry" by design: the edit panel, delete and undo never look at who authored a step. The gaps above are gaps for everyone; they bite hardest on agent-built parts because agents use the operations (patterns, revolves, mates with limits, GD&T) the app has the least UI for.

## 1. Parts

### What every row shares

- **A feature's row.** The tree is `ChapterList` (in `ModelTree`, `App.tsx:227`). A chapter is the kernel's grouping of recorded steps (`timeline-engine/src/chapters.rs`): a cutter plus the subtract that consumes it is one Hole; three or more equally spaced holes are one Pattern.
- **Select.** Clicking a row selects it (`history-view-store.ts`, `selectedChapter`) and never moves time. The feature is lit on the part with a callout.
- **Edit.** `FeatureEditPanel` (`App.tsx:231`, and as the callout in `FeatureHighlight.tsx:81`) lists the feature's recorded numbers from `GET /api/timeline/parameters/{branch}` and applies one with `POST /api/timeline/mould`. The kernel trials the change and refuses in its own words.
- **Delete.** Trash or the Delete key sends `POST /api/timeline/suppress` with the chapter's steps; trialled, refused if a later feature depends on it.
- **Edit sketch.** For kinds extrude/cut/boss/revolve: `POST /api/timeline/sketch/reopen`, then `POST /api/timeline/resketch`.
- **Undo/redo.** `POST /api/timeline/undo|redo`, one recorded step at a time, whoever made it. A person's Ctrl+Z undoes an agent's last step and the reverse.
- **Author.** Agent steps get a green dot and a "Made by … · intent" byline (`ChapterList.tsx:110-114`). Nothing is gated on it. Steps made in the app are recorded as `system`, not `user` (`timeline-engine/src/recorder_bridge.rs:1504`): the tree cannot say "you made this".

What a number is, to the edit panel: any numeric key in the recorded step, except five id keys and the `axis_|dir_|direction_|normal_` prefixes. Two consequences:

- **Ids and counts are offered as sizes**: `face_id` on a face extrude, `datum_id`, `source_solid_id`, `segments`, `sections`, `degree_v`, with "mm" after most of them (`roshera-app/src/lib/feature-edit.ts:68-72`).
- **Anything that is a string, a boolean or an array can never be edited**: edge lists, operand choice, a direction, a matrix, a profile.

### Matrix

"Edit" lists what the panel offers; **not** lists what it cannot change. "Create in app" says whether a person can make the same thing through the UI.

| Agent operation (MCP tool) | Recorded as → tree row | Edit | **Not** editable | Create in app |
|---|---|---|---|---|
| `create_box` / `create_cylinder` / `create_cone` / `create_sphere` | `create_*_3d` → box, cylinder, cone, sphere | sizes, position | axis | Create menu, fixed 10 mm defaults (`ToolBar.tsx:1405-1409`) |
| `revolve` (profile) | `revolve_typed` / `revolve_meridian` → revolve | angle, bore radius, wall | axis, the profile | **no** (only from a sketch) |
| `nurbs_loft` | `nurbs_loft` → raw row "Nurbs loft", no icon | counts shown as sizes | the sections | **no** |
| `create_sketch` … `sketch_extrude` (click-draft) | `sketch_extrude`, no saved outline → extrude | distance | direction; **outline: "Edit sketch" shows and the server refuses `SketchNotSaved`** | SketchPanel |
| `psketch_*` then `psketch_extrude` | `sketch_extrude` with the outline saved → extrude | distance; outline through Edit sketch | direction, plane | Sketch bench ▸ Extrude / Cut |
| `psketch_*` then `psketch_revolve` | `sketch_revolve` → **raw row, no icon** | angle | axis; **outline: no Edit sketch button** (gap 6) | Sketch bench ▸ Revolve |
| `psketch_op` (trim, offset, mirror, pattern) | `csketch_*`, folded into the feature | numbers are listed *(inferred: moulding them does nothing — replay builds from the saved regions)* | — | Sketch bench tools |
| `boolean` / `boolean_many` | `boolean_*` → hole / cut / boss / subtract / union / intersect | the cutter's own sizes when it was absorbed | **operands, operation** | Boolean menu on the selection |
| `drill_pattern` | N cylinders + N subtracts → one Pattern when N ≥ 3 and regular | radius, depth (all holes together) | **count, pitch circle, start angle, each position, axis** — none recorded as such | **no Hole or Pattern-of-holes tool** |
| `fillet_edges` | `fillet_edges` → fillet | radius | **the edges**; variable radii | Fillet ▸ four variants (`ModifyDialog`) |
| `chamfer_edges` | `chamfer_edges` → chamfer | distances | the edges | Chamfer |
| `shell` | `offset_solid` → shell | thickness | the opened faces | Shell |
| `transform` | `transform_solid` → folded into the create, else "Move" | **nothing** (a matrix) | everything | Move/Rotate gizmo |
| `import_step` | `import_step` → raw row "Import step" | nothing geometric | — | Import / drag-and-drop |
| `delete_part`, `clear_parts` | `delete_solid`, `clear_geometry` → raw rows | — | — | tree Delete; no Clear |
| `part_rename` | `set_name` on the part's chapter | — | a feature's own name (no route) | tree Rename |
| `set_part_color` | `set_color` → raw row | nothing | — | Properties colour is local only, never sent (`PropertiesPanel.tsx:233-247`) |
| `label_create` / `rename` / `delete` | **not recorded** | — | — | **no** (labels are display-only) |
| `gdt_datum`, `gdt_fcf` | `gdt_datum_designate`, `gdt_annotation_create` → raw rows | **nothing** (GD&T is excluded from the panel) | everything; `PATCH`/`DELETE` routes exist, unused | **no** |
| `timeline_mould` | `param.mould`, folded as "Edit" | — | — | the edit panel |
| `timeline_checkpoint`, `undo`, `redo`, `branch`, `switch`, `merge` | not features | — | — | buttons on the timeline bar |
| `cad_program` | the sum of its steps | per step | no single undo, no rollback | — |

The person can make things the agent has no tool for: torus, mirror, linear and circular patterns of bodies, face extrude, datum planes, size tolerances (REST only). Draft, sweep, loft from profiles, hole and thread have neither an MCP tool nor app UI.

### Smaller findings

- `FeatureTree.tsx`, `AddComponentDialog.tsx`, `AddMateDialog.tsx`, `AddReferenceDialog.tsx`, `ui/separator.tsx` and `lib/parts-api.ts` are imported by nothing; `node scripts/dead-modules.mjs` fails on them today. ✔ (run)
- The kernel's answer to a face pick replaces the whole sub-element selection with one element (`ws-bridge.ts`, `SubElementResult`: clear, then add), so a shift-click multi-selection of faces collapses to the last one. ✔
- Renaming an already named feature's part *(inferred)* changes the part row but not the feature title (`chapters.rs:493-498` keeps the first name).
- With `ROSHERA_REQUIRE_INTENT=1` the app's own edit and delete calls would be refused: they send no intent header (`feature-edit.ts:175-209`).
- The timeline's right-click "truncate" is destructive (`Timeline.tsx:1774-1832`).

## 2. Assemblies

Three things are called "assembly" in the code:

- **A.** `/api/assembly/...` — instances and kinematic mates. Used by MCP and by the live `AssemblyWorkspace`. Recorded in the timeline and replayed.
- **B.** `/api/assemblies/...` — the older mate-centric manager. No MCP tool; its app UI (the three dialogs above, `AssemblyTransformGizmo`, `MateSuggestionPopover`) is unreachable because nothing produces the `asm-comp:` ids it keys on. Its events are recorded but skipped on replay.
- **C.** `AssemblyBrowser` — the product tree of an *imported STEP file*, shown in the Part workspace. It never shows A's documents.

Matrix for A:

| Agent operation | Recorded | In the app | After creation the person can | **Cannot** | Create in app |
|---|---|---|---|---|---|
| `assembly_create` | `assembly.create` | switcher entry | open it | rename, delete (client exists, no caller) | "+" (auto-named) |
| `assembly_add_instance` (pose, name, colour) | `assembly.add_instance` | Parts row + mesh | delete (row ✕); click selects (outline only) | **move, rename, recolour**; colour is not even shown | "+" at identity pose, part name |
| `assembly_transform_instance` | `assembly.transform_instance` | pose, after a reload | — | **no gizmo, no pose fields** | **no** |
| `assembly_mate` create — 7 joints with limits, 4 overlays with values, 3 couplings | `assembly.mate_add` | Mates row: glyph, "A · B", kind | delete (re-solves) | **edit kind, value, limits**; the value and limits are not shown | 6 joints, **no limits, no overlays, no couplings, no PinSlot** |
| `assembly_mate` edit / remove | `assembly.mate_edit` / `mate_remove` | row updates after a reload | delete | edit | delete only |
| connectors (made inside `assembly_mate`) | `assembly.connector_add` | **not shown** | — | delete (they leak when a mate is removed) | implicit |
| ground (a per-call argument) | not stored | ⚓ on the *first* row, whatever the agent solved with | — | choose the ground | **no** |
| `assembly_solve` | `assembly.solve` (poses) | parts move after a reload | re-solve | see why it did not converge: the answer is discarded (`AssemblyWorkspace.tsx:112-123`) | Solve button |
| `assembly_dof` | read-only | header verdict + per-instance freedom | read | — | automatic |
| `assembly_certify` | read-only | **not shown** | — | — | **no** |
| `assembly_drag` | `assembly.solve` | poses after a reload | — | **no joint slider** | **no** |
| `assembly_interference` | read-only | one line with a count | read | see which pairs | Clearance button |
| `assembly_view` | read-only | Agent Eye, Part workspace only | — | — | — |

Not on A at all: BOM, exploded view, instance patterns, sub-assemblies, suppress or flip a mate.

Ids are shared and stable: assembly, instance, connector and mate are UUIDs on MCP, REST and in the app; an instance in the viewport is `asmi:<uuid>`.

## 3. The feedback channel

### What existed before this change

| | |
|---|---|
| Route | `POST` / `GET /api/agent/pointer`; MCP tool `get_pointer` |
| Sent by | `CADMesh.tsx` `reportPointer`, **on click only** (object mode: the part; face mode: the face). The route's comment says "click or hover-dwell"; nothing ever sent a hover. |
| Payload | `{kind:"click", object_id, face_id, position, at_ms}` |
| Agent gets | the click, joined with the face's report (surface type, area, host part) |
| Identity | raw kernel `face_id`; **no persistent id** |
| Scope | **one slot for the whole server** — not per document, user or session |
| Freshness | client clock only; the agent cannot tell a click from an hour ago from one just made |
| Persistence | in memory; lost on restart |
| Latency | the click reaches the server at once; the agent sees it only when it next calls the tool (pull) |
| Person's view | no sign that anything is shared, or that an agent read it; no way to pause |

Next to it, three other pieces exist:

- **Agent → person**: `guide_attention` (`POST /api/agent/show`) drives the app — workspace, camera, feature, faces, history step — under a "Claude is showing you" bar with Pause and Take over.
- **Agent → person, a closed question**: `ask_choice` renders a choice card on the Blackboard.
- **`GET /api/agent/hover/{id}`**: resolves a face id into a hover report. No caller in the app.

### What an agent could not know

| Signal | Before | Now (this change) |
|---|---|---|
| Hovered part / face | no | yes, face by kernel id + persistent id |
| Hovered edge / vertex | no | the part and the point; no id until picked (the mesh has no edge map) |
| Selection (parts, faces, edges, vertices) | last click only | yes, the whole selection |
| Selected feature in the history | no | yes (anchor step) |
| History step being viewed | no | yes |
| Active tool, pick filter, workspace | no | yes |
| Camera | no | yes (position, direction, up, projection, named view) |
| Fresh or stale | no | `live` / `stale` / `paused` / `none`, with age |
| Per document | no | yes |
| The person knows what is shared, and when it is read | no | status-bar chip; "Claude is looking" |
| The person can stop it | no | click the chip; the server then keeps only the pause |

### Still missing for a continuous loop

1. **Push.** The agent still has to ask. Nothing wakes an agent when the person selects something or rejects a step. (Design §a: MCP resource subscription.)
2. **With the message.** The selection and hover are not attached to the person's message when they send it. (Design §a.)
3. **Pick mode both ways.** The agent cannot ask "pick the face" and get the pick back as an answer; it cannot propose a pick for the person to correct. (Design §b.)
4. **Accept / reject / edit at step k.** No plan preview on a scratch branch, no per-step verdict. (Design §c.)
5. **Edges and vertices by persistent id on hover.** Needs a per-triangle edge map from the tessellator, or a server-side pick on dwell.
6. **Assemblies.** An instance id (`asmi:<uuid>`) is reported but the server resolves only parts; mates cannot be selected at all.
7. **Sketch entities and drawing dimensions** are not in the context.
8. **Per person.** The context is per document, not per user: two people in one document overwrite each other.
