# Registry schema parity test

Date: 2026-11-17. Stage 0 of the single-registry migration (`SINGLE_REGISTRY_DESIGN.md`, on its own branch). Test and allow list only. No production code changed.

| File | Role |
|---|---|
| `roshera-mcp/test/registry_schema_parity.mjs` | The test. Runs in `npm run test:gates`. |
| `roshera-mcp/test/registry_schema_parity.allow.json` | The ratchet list of today's differences. |

## What it compares

The two copies of every tool's input schema:

- **MCP:** the zod schema each tool registers, compiled into `test/.build` and converted by `registry.ts::toolJsonSchema`. This is the JSON Schema an MCP client receives.
- **Backend:** the `t(…)` rows of `roshera-backend/api-server/src/agent_registry.rs::raw_tools`. This is what `GET /api/agent/tool-registry` serves.

Tools that exist on only one side are printed and not failed. The name gate (`ontology_coverage.mjs` o5 and the registry hash) already covers them, and other worktrees are adding tools to one side at a time. The three funnel tools (`find_tool`, `describe_tool`, `invoke`) are MCP-only by design and are skipped.

For each tool on both sides, at the top level of the input object (the fields an agent fills in):

| Kind | Fails when | `side` in the entry |
|---|---|---|
| `param` | a property exists in one copy only | the copy that has it |
| `required` | a shared property is required in one copy only, or `required` names a property that copy does not have | the copy that requires it |
| `type` | both copies declare a type for a shared property and the type sets differ | `both`, plus `detail: "mcp=… backend=…"` |

These three are the contract an agent sends. A parameter missing from one copy is dropped by anything generated from that copy. A required-ness difference means one copy accepts a call the other refuses. A type difference does the same.

How the type set is computed: the union of `type` over the schema and its `anyOf` / `oneOf` members. `enum` and `const` values add their JSON type. Local `$ref`s (`#/properties/a`) are followed: zod emits one when two fields share an object, as `assembly_mate.b` does. `integer` and `number` are different types, because a backend `number` accepts `2.5` where zod `.int()` refuses it. A property with no type it can determine (`z.any()`, `{}`) is untyped and its type is not compared. The run prints the untyped fields. Today the only one is `assembly_mate.kind`, which is untyped on both sides.

Not compared: descriptions, defaults, numeric bounds, enum value lists and nested properties. Those differences are real (see "Not verified"), but a text diff is the wrong tool for them. Stage 1 derives one schema from the Rust `Args` struct and removes the second copy.

## How the Rust side is read

The test reuses the approach of `ontology_coverage.mjs` o5: it reads the checked-in source with no backend process and no cargo, and checks the parse against the file's own `assert_eq!(tools.len(), N)`. If the counts differ, the test fails.

The regex used there finds row names only, so the schema needs more. The test contains a small evaluator for the part of `json!` the table actually uses:

- objects, arrays, string, number, bool and null literals;
- `ntuple(n, desc)`, evaluated from its own `json!` body in the same file;
- `plane_schema(default_xy)`, evaluated from its `json!` body plus the `default: "xy"` insertion. The insertion is matched literally, so if the helper changes shape the test refuses instead of modelling it wrongly.

Anything else fails the test with a line number (`agent_registry.rs:219: call to 'axis_schema(…)' inside a schema: no evaluator for it`). The evaluator never guesses.

Two more checks guard against a pass that compares nothing: the test fails if no tool is on both sides or if no parameter type was compared. Each run prints its coverage: 357 shared parameters, 163 required in at least one copy, 356 type-compared.

## The ratchet

Every live difference must have an entry in the allow list, `{ tool, kind, field, side, reason }`, plus `detail` for `type`. Every entry must still match a live difference. The test fails both ways:

- `NEW DRIFT  <tool> | <kind> | <field> | <side>`: a difference with no entry. Fix the copy that is wrong. Add an entry only with a reason.
- `RATCHET    <tool> | <kind> | <field> | <side>`: an entry whose difference has closed. Delete the line.

So the list can only shrink. This is the same rule as `geometry-engine/KNOWN_REDS.md`. Entries are also checked for unknown keys, duplicates, a valid `side` for their kind, and a reason of at least 10 characters.

### Shrinking the list

1. Reconcile one copy. Usually that means adding the missing property to the `agent_registry.rs` row, transcribed from the zod schema. For the blackboard rows it means removing `part_id` / `scope` from the backend, since the MCP side dropped them on purpose (`blackboard.ts` header).
2. Run `npm run test:gates:build && node test/registry_schema_parity.mjs`. The closed entry now shows up as `RATCHET`.
3. Delete that entry and run the test again until it is green.

The backend has its own tests: `agent_registry_tests` and the registry hash that MCP compares against its own tools. A change to a backend row needs those run with cargo, which this test does not do.

## Today's differences (measured on this tree)

There are 23 entries across 17 of 115 tools. Every one is a `param` difference. No `required` or `type` difference exists today.

| Tool | Only in MCP | Only in the backend row |
|---|---|---|
| `shell`, `fillet_edges`, `chamfer_edges`, `boolean`, `boolean_many`, `drill_pattern`, `transform`, `export_part`, `make_drawing` | `acknowledge_unsound` | |
| `drawing_export_sheet` | `acknowledge_layout_issues`, `acknowledge_unsound` | |
| `psketch_extrude` | `direction`, `plane` | |
| `psketch_revolve` | `plane` | |
| `timeline_checkpoint` | `skip_verification` | |
| `blackboard_add_entry`, `blackboard_edit_entry`, `blackboard_list`, `blackboard_clear` | | `part_id`, `scope` |

This matches the audit's 17 tools (`MCP_AUDIT.md`, "Drift between the two copies") field for field. The audit counted tools and this test counts entries: 9 + 2 + 2 + 1 + 1 + 8 = 23. The audit compared names and required-ness only. This test also compares types on 356 shared parameters and finds none that differ.

## Proof (commands run from `roshera-mcp/`)

| Case | How | Result |
|---|---|---|
| GREEN on the real tree | `npm run test:gates` | exit 0; `registry schema parity: PASS`, 23 entries live |
| measured, empty list | `PARITY_ALLOW=../.scratch/empty_allow.json node test/registry_schema_parity.mjs` | exit 1; 23 `NEW DRIFT` lines, 17 tools |
| type mismatch (backend copy) | `create_box.height` changed to `integer` in a scratch copy, run with `PARITY_RUST_SOURCE` | exit 1; `NEW DRIFT create_box \| type \| height \| both \| mcp=number backend=integer` |
| required mismatch (backend copy) | `create_sphere` `required: []` | exit 1; `NEW DRIFT create_sphere \| required \| radius \| mcp` |
| new parameter (backend copy) | `boolean` gains `keep_tool` | exit 1; `NEW DRIFT boolean \| param \| keep_tool \| backend` |
| mismatch on the MCP side | compiled `test/.build/tools/create.js` edited: `radius` optional, `name` renamed `label`; rebuilt afterwards | exit 1; three `NEW DRIFT` lines naming `create_sphere` `label` / `name` / `radius` |
| listed difference closed | `boolean` row gains `acknowledge_unsound` | exit 1; `RATCHET boolean \| param \| acknowledge_unsound \| mcp` |
| live entry removed from the list | allow-list copy without `shell` | exit 1; `NEW DRIFT shell \| param \| acknowledge_unsound \| mcp` |
| unparseable row | `plane_schema(true)` changed to an unknown `axis_schema(true)` | exit 1; parse refused at `agent_registry.rs:219` |

The scratch copies were generated by `.scratch/make_red_copies.mjs`. That script is uncommitted and does not ship with this change. Each mutation is fully described in its row above, so any case can be reproduced without it. None of the real files were edited. The hand-edited compiled bundle was restored by the rebuild that `npm run test:gates` runs, and the restore was checked.

**Merge order.** The allow list is pinned to this tree. Other branches still have unmerged edits to `agent_registry.rs` and `src/tools/*.ts`. When those land, this test fails until whoever merges re-runs it and edits the list. The failure can take three forms: `NEW DRIFT`, `RATCHET`, or a parse refusal for a new `json!` construct. That failure is deliberate.

## Not verified

- **The parse is not checked against cargo's served JSON.** The test reads Rust source, not the JSON the server serves. The evaluator follows the same helpers, and the row count is cross-checked, but no backend test dumps the registry for comparison. The design proposes that dump as the next step, and it needs cargo.
- **Enum values, defaults, bounds and nested properties are not compared.** For example, `create_sphere.name` is described as "display name" in MCP and "display name for the part" in the backend, and descriptions are out of scope. Expect more differences once these are added. Add each one as a new kind with its own entries.
- **The MCP side is whatever `toolJsonSchema` emits.** If zod-to-JSON conversion fails, it returns `{type: "object"}`. The test then fails with "MCP schema has no properties", but only when the backend row has properties.
