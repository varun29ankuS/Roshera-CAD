# One tool registry, owned by the backend

Date: 2026-11-17. Status: design, nothing built. Companion to `MCP_AUDIT.md` (the evidence for every claim about today's state).

## Decision

The backend owns each tool: its name, schema, description, examples, gates, effects and the code that runs it. The in-app agent calls that code in-process. MCP is a generic adapter that lists what the backend serves and forwards calls. No tool is written twice, so the two cannot disagree.

## Why the current shape cannot be patched into this

Today a tool exists in up to four places:

| Copy | Where | Kept in step by |
|---|---|---|
| The schema and description agents see | zod in `roshera-mcp/src/tools/*.ts` | hand |
| The same schema, transcribed | `api-server/src/agent_registry.rs` (115 rows, "transcribed verbatim from the live zod") | hand; the drift gate compares **names only** |
| The translation from tool arguments to a REST body, and from the REST answer to a result | each TS handler | nothing (72 of 115 handlers are never called by a test) |
| The gates | `roshera-mcp/src/gates.ts` and, separately, backend route layers (`require_declared_intent`, `refuse_unsound_base`, the export gates) | a set-equality test over source text |

Measured today: 17 of 115 tools differ between the first two copies, and the third copy is where `conn.id` lived.

The important fact for the design: **most TS handlers are not pass-throughs.** They convert `part_id` to a uuid, turn a plane name into axes, compose several routes (`drill_pattern`, `assembly_mate`, `timeline_checkpoint`), pick fields out of the answer, attach the certified verdict, and decide whether to send pixels. A registry that only describes REST routes would leave all of that in TypeScript. So the unit the backend must own is **the tool as a function**, not the route.

## The shape

```
                    ┌──────────────────────── api-server ────────────────────────┐
 in-app agent ────► │ tools::dispatch(ctx, name, args)                           │
 (in-process)       │   1. look up ToolDef          ┐                            │
                    │   2. permission (RBAC)        │ one implementation,        │
 REST ────────────► │   3. deserialize + validate   │ one order, every caller    │
 POST /api/agent/   │   4. gates (declared on def)  │                            │
   tools/{name}/call│   5. run handler ──► kernel   │                            │
                    │   6. envelope: data + evidence┘                            │
                    │ GET /api/agent/tools   (the registry: every ToolDef, JSON) │
                    └─────────────────────────────────────────────────────────────┘
                              ▲                               ▲
              roshera-mcp (generic adapter)          roshera-app (UI)
              lists registry, forwards calls         keeps its REST routes; those
              no per-tool code                       routes call the same handlers
```

### ToolDef (Rust, in `api-server/src/tools/`)

One value per tool. Everything an agent or a gate needs is a field, so nothing has to be restated elsewhere.

| Field | Meaning | Replaces |
|---|---|---|
| `name`, `title` | stable id, display name | zod registration name |
| `summary` | one line; what listings and search show | the first sentence of today's descriptions |
| `guide` | the long form; served by describe only, never paid for in a listing | the policy prose inside descriptions |
| `synonyms` | words an engineer would use ("bolt", "fasten", "hinge") | description stuffing; fixes the audit's discovery misses |
| `Args` | a Rust struct: `Deserialize + JsonSchema`, `deny_unknown_fields` | zod schema **and** the transcribed JSON **and** the untyped `Json<Value>` bodies |
| `Output` | a Rust struct: `Serialize + JsonSchema` | nothing today (no tool declares its result) |
| `examples` | `[{ title, args, note }]`, each checked by a test | prose examples |
| `effects` | `ReadOnly` / `Mutates` / `Destructive`, plus `idempotent` | `MUTATES_SOLIDS` in `gates.ts`; MCP annotations (none today) |
| `gates` | declared list: `RequiresIntent`, `SoundBase(|args| refs)`, `SheetCertified`, `PointRunLimit`, `VerifiedBeforeClose` | `gates.ts` tables and the backend route layers |
| `permission` | `Read` / `CreateGeometry` / `ModifyGeometry` / `Export` / `Settings` | per-route auth layers |
| `bench`, `resident`, `stability`, `since`, `deprecated` | surface placement and lifecycle | `BENCH_OF`, `CORE_SURFACE`, the registry columns |
| `long_running`, `cancellable` | whether the call streams progress | one global 120 s timeout |
| `handler` | `async fn(&ToolCtx, Args) -> Result<Output, ApiError>` | the TS handler **and** the route body |

Registration is an explicit list (`fn all() -> &'static [&'static dyn Tool]`), one module per bench. No linker-time collection: a tool that is defined but not listed fails a test, which is the disconnection gate applied to tools.

### Schema format

JSON Schema, derived from the `Args` / `Output` structs with `schemars`. Reasons:

- MCP requires JSON Schema for `inputSchema` and `outputSchema`; Claude and OpenAI tool definitions take the same. No conversion step.
- The struct the schema is derived from is the struct the handler receives, so "the schema says X, the server reads Y" cannot be written. The `conn.id` class is closed by construction on the request side; on the response side `Output` is the only thing the adapter can see.
- Field doc comments become the parameter descriptions. They are written once, next to the field, and reviewed in the same diff as the behaviour.
- The eight untyped parameters in the audit already have exact Rust types (`DocMateKind`, `Constraint`, each `Add*Request`). Deriving their schemas gives the model the grammar that prose carries today.

Rules the derivation enforces, each by a registry test:

- Units are types, not words: `Mm(f64)`, `Deg(f64)`, `Rad(f64)` newtypes carry `x-unit` into the schema. A bare `f64` named `angle` fails the test. (Today `revolve` takes degrees and `psketch_revolve` radians.)
- Ids are types: `PartRef` accepts the integer id or the uuid and resolves server-side. The two id spaces stop being the agent's problem and the scene-snapshot lookups disappear.
- Persistent ids have one wire form (a hex string), fixing audit 1.2.
- Every `Args` struct denies unknown fields, so a dropped argument (audit 1.3) is a typed refusal.

### Descriptions and examples

- `summary` and `guide` are string constants beside the def. Parameter text is the field's doc comment.
- Examples are data. A test deserializes every example into `Args` (it must validate) and, for tools marked `example_runs`, calls `dispatch` with it against the test router. That one test is also the production-call-site test the disconnection gate asks for: every tool is called at least once, through the real path.
- Steering text that exists to prevent a failure ("never loop this tool") is replaced by the gate that already enforces it; the gate's refusal message carries the instruction at the moment it matters.

### The result envelope

Every call returns one shape, whoever called:

```
{ "ok": true,
  "data":     <Output>,                       // typed by the def's output schema
  "evidence": { "certificate": {...} | null,  // the kernel's verdict for what this call changed
                "events": [seq, ...],         // the timeline steps it recorded
                "recording": "durable" | {...} },
  "content":  [ {image...} ],                 // optional pictures
  "gate":     null }
{ "ok": false,
  "error": { "code", "message", "retryable", "hint", "details" },   // the existing error catalogue
  "gate":  { "name", "bypass": "acknowledge_unsound" } | null }
```

`evidence` is filled by `dispatch`, not by each handler: it knows which solids the call touched and reads their certificate once. A mutating tool therefore cannot forget its verdict (audit 1.6) and every result names the history steps it made. A handler that returns HTTP 200 with `success: false` has no way to express that; failure is `Err(ApiError)` (audit 1.4, 1.5). The 63 handlers that return a bare status are migrated to `ApiError` as their tool moves.

### Dispatch, for the in-app agent and for REST

- In-process: the agent runtime calls `tools::dispatch(&ctx, name, json_args)` and gets the envelope. The LLM tool definitions it sends are built from the same registry.
- REST: `POST /api/agent/tools/{name}/call` with `{ "args": {...}, "idempotency_key": "..." }` calls the same function. This is the only route the MCP adapter needs.
- The UI's existing routes stay. As each tool moves, its route body becomes a call to the tool's handler with the same `Args`, so the engineer's click and the agent's call run one piece of code. Routes are not removed in this plan.

`ToolCtx` carries the session id, the bound document and part, the open intent, the caller's permissions and a progress sink. The headers `core.ts` sends today (`X-Roshera-Agent`, document and part pins, intent) map onto it unchanged.

### Gates

All six gates move behind `dispatch`, keyed by session id, declared on the def:

| Gate | Today | After |
|---|---|---|
| Refusal cache | MCP process memory | per-session table in the backend; cleared by the same rule |
| Intent required | `gates.ts` **and** the `require_declared_intent` layer on the mutating routes | one check, driven by `effects` |
| Sound base | `BASE_REFS` in `gates.ts` **and** `refuse_unsound_base` call sites | one check, refs from the def's closure |
| Sheet export | both sides | one check |
| Single-point run | MCP only | per-session counters in the backend |
| Verification before close | MCP only | per-session ledger in the backend |

The in-app agent gets the same constraints as an external one without loading the MCP. `gate3_drift_set_equality_tests.rs` becomes unnecessary because there is one set.

### The MCP adapter

What `roshera-mcp` becomes (a few hundred lines):

1. On start, `GET /api/agent/tools`. Register every tool with the SDK's low-level `Server` (`tools/list` returns the served JSON Schema verbatim; no zod).
2. `tools/call` → `POST …/call` → map the envelope to a `CallToolResult`: `structuredContent` = `data` + `evidence`, a short text rendering, images from `content`, `isError` from `ok`.
3. Annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`) from `effects`; `outputSchema` from the def.
4. Client-side concerns that stay, because they are about the client: the credential and pin headers, the surface mode (which tools are listed up front, from `resident` / `bench`), the image-freshness policy, and writing exported files to the client's disk.
5. If the backend is unreachable there is no compiled fallback table (that would be a second copy). The adapter lists one tool, `roshera_status`, that says so.

`find_tool` and `describe_tool` become `GET /api/agent/tools/search?q=` and the registry entry, so the in-app agent and external agents search the same index, ranked over `summary` + `synonyms`. A query that matches nothing above a floor answers "no tool does this" and names the nearest ones. `invoke`, `workbench` and `cad_program` stay as adapter-side composition over `dispatch` at first; `cad_program` moves server-side later as a transaction (`/api/tx` already exists).

End state worth keeping open: `api-server` serves MCP itself over Streamable HTTP at `/mcp`, and the npm package shrinks to a stdio bridge for clients that need one.

### Versioning

- **Registry format**: `registry_version` (semver). The adapter states the format it speaks; a major mismatch is a clear startup error, never a partial listing.
- **Per tool**: `since`, `revision` (a hash of the schema and summary), `deprecated { since, replaced_by, remove_after }`.
- **Compatibility rule**, enforced by a snapshot test over the served registry: adding an optional parameter or an output field is free; removing or renaming a parameter, tightening a type, or changing a unit needs a new tool name, with the old one kept as a deprecated alias for a stated window. The snapshot diff is reviewed like an API change because it is one.
- **Staleness**: the adapter sends the registry hash it listed with on each call. If the tool's revision has changed, the backend answers `registry_stale`; the adapter refetches and emits `tools/list_changed`. A long-lived client never calls a tool with a schema the server no longer has.

### Auth

- The credential is unchanged: stdio adapters read `ROSHERA_API_KEY` (or `~/.roshera/api_key`) and send it; the registry never contains secrets.
- Permission is a field on the def and is checked in `dispatch`, so a new tool cannot be added without one (today a route gets its auth layer by where it is placed in `main.rs`).
- The registry listing is filtered by the caller's permissions: an agent with read access is not shown `clear_parts`.
- Destructive tools require `confirm: true` in `Args` (generated from `effects`), which gives MCP clients a real `destructiveHint` and the app a place to ask the human.
- Remote MCP (HTTP) uses OAuth as the MCP spec requires; that arrives with the `/mcp` end state, not before.

### Long operations: progress, cancel, retry

- A def marked `long_running` may be called with `Accept: text/event-stream`. Events: `progress { done, total, message }`, then `result { envelope }`. The handler reports through `ToolCtx::progress`; a STEP import reports solids resolved, a `drill_pattern` reports holes cut.
- The adapter forwards these as MCP `notifications/progress` when the client supplied a progress token, and otherwise just waits on the stream. The per-call timeout comes from the def, not one global number.
- Cancel: MCP `notifications/cancelled` → `DELETE /api/agent/calls/{call_id}`. A def says whether it is `cancellable`; one that is not refuses the cancel honestly instead of pretending.
- Retry: `idempotency_key` is recorded with the result per session for a bounded time. A repeat with the same key returns the recorded envelope. This removes the duplicate-geometry risk behind today's "may still be computing" timeout message.
- Composite tools run inside a transaction so a failure leaves nothing behind (`drill_pattern`'s cutters, `assembly_mate`'s connectors).

### Resources and subscriptions (including `user_context`)

Some things an agent needs are state, not actions: the open document, the parts list, a part's certificate, the timeline, and what the user is looking at and has selected.

- The registry gains a second list, `resources`: `{ uri_template, title, summary, Output schema, subscribable, also_tool }`.
  - `roshera://document/current`, `roshera://parts`, `roshera://part/{id}`, `roshera://part/{id}/certificate`, `roshera://timeline/{branch}`, `roshera://user/context`.
- Read: `GET /api/agent/resources?uri=…`. Change feed: one SSE stream, `GET /api/agent/events?since=<cursor>`, fed by the broadcasts the WebSocket layer already produces.
- The adapter maps these to `resources/list`, `resources/read`, `resources/subscribe` and `notifications/resources/updated`.
- Many MCP clients read tools well and resources poorly. A resource marked `also_tool` is projected as a read-only tool with the same name and schema. So the `user_context` tool in the other worktree becomes **one** `ResourceDef` with `also_tool: true`: external agents call `user_context`, clients that support subscriptions get told when the selection changes, and the in-app agent reads the same struct in-process. Until this stage lands, `user_context` as a hand-written tool is fine; it migrates like any other.

## How the existing tests and gates migrate

| Today | Becomes |
|---|---|
| `agent_registry_tests.rs` (shape, completeness against `BENCH_OF`, object-typed schemas) | Registry tests over `tools::all()`: unique names, schema derives, every example validates and runs, every def has a permission and effects, snapshot for compatibility. The completeness pin disappears: there is no second list. |
| `ontology_coverage.mjs`, `BENCH_OF` | `bench` is a field; the test is "every def has one". |
| `gate3_drift_set_equality_tests.rs` | Deleted when the last gated tool has moved. |
| `constraint_gates`, `single_point_gate`, `verification_scope_gate`, `refusal_stops_program`, `batch_step_certification`, `drawing_gates` (MCP suites against stubs) | Rust router tests against the real `dispatch`, same cases, ported one suite per stage. The MCP suites are deleted as their gate moves, not before. |
| `structured_errors`, `unsound_forwarding`, `recording_failures_disclosure`, `perception_fidelity`, `durability_disclosure` | Envelope tests in Rust (the facts are produced there now) plus one adapter test that the envelope maps to a `CallToolResult` faithfully. |
| `find_tool_recall`, `find_tool_heldout` | Run against the backend search endpoint; the held-out set stays held out. |
| `scale_s2`, `scale_s3s4` (token bill, surface modes) | Adapter tests fed a fixture registry; the bill is computed from the served registry. |
| `part_pin`, `document_pin`, `intent_header`, `raw_fetch_scope` | Stay as adapter tests (they are about headers the adapter sends). |
| New in this task: `assembly_mate_wire`, `create_sphere_name` | Deleted when their tool moves; the def's example test covers the same ground against the real route. |

Rule during migration: a tool is deleted from TypeScript in the same change that its def's example test goes green, never earlier.

## Staged plan

Each stage ships on its own and leaves the system working.

| Stage | What lands | Proves | Size |
|---|---|---|---|
| **0. Make the drift visible** | A test that compares the two copies' schemas (not just names), with today's 17 differences as a ratchet list that may only shrink. Plus the rule that every MCP stub is pinned to the Rust source. | Nothing new can drift unnoticed while the rest is built. | half a day |
| **1. One bench, end to end** | `tools/` module, `ToolDef`, `dispatch`, the envelope, `POST …/call`, registry v2 listing these tools. The 12 **assembly** tools become defs (their request structs are already typed). The adapter learns to register served tools generically; `tools/assembly.ts` is deleted. | The whole path, on the bench where the failure happened, with no effect on the resident surface or its token bill. | about a week |
| **2. Gates behind dispatch** | Intent, sound-base and refusal-cache in `dispatch`; the per-session state they need. | The in-app agent is constrained without MCP. | days |
| **3. Read tools** | Inspect, queries, labels, lens, perception as defs (pass-throughs; `PartRef` lands here). | Volume: about 45 tools with almost no logic. | days |
| **4. Mutating core** | Create, modify, sketch, timeline, io. Typed `Args` replace `Json<Value>`; composites become transactions; remaining gates move. | The id and unit rules; `gates.ts` is deleted. | 1–2 weeks |
| **5. Drawing and GD&T** | After the drawing campaign's files are quiet. | | days |
| **6. Search, resources, progress** | Backend search; `resources` with `user_context` first; SSE progress and cancel; idempotency keys. | The things MCP offers that are unused today. | about a week |
| **7. In-app agent in-process** | The app's agent builds its tool list from `tools::all()` and calls `dispatch` directly. | The primary product no longer depends on a child MCP process. | days, once 1–4 are in |
| **8. Optional** | `/mcp` served by `api-server`; the npm package becomes a bridge. | | later |

### The smallest first step (Stage 0)

One new test file, no production code, no edits to files another worktree owns:

- `roshera-mcp/test/registry_schema_parity.mjs`. For every tool, compare the compiled MCP schema with the backend registry row: property names, `required`, types, enums, defaults.
- Known differences live in a short ratchet list in the same file, each with a reason. A difference not on the list fails; a listed difference that has closed also fails until its line is removed (the `KNOWN_REDS.md` rule).
- Add it to `npm run test:drift`.

A working prototype is `.scratch/drift.mjs` in this worktree: it already finds the 17 differences by reading `agent_registry.rs` as text. The production version should read the served registry JSON instead (dump it from a backend test), so it compares what clients receive.

It should land after the other worktree's `user_context` change merges, because that change touches both copies.

## Risks

| Risk | Answer |
|---|---|
| The adapter is useless when the backend is down | It already is: every tool call needs the backend. Listing a table of tools that cannot run was the only thing lost. |
| Token bill changes when descriptions move to `summary` / `guide` | Intended, and measured per stage by the existing bill test. Resident tools are migrated last so the minimal surface is stable until then. |
| `find_tool` rankings shift | The held-out suite is the judge. Synonyms are data, so a miss is fixed by adding a word to one def. |
| Big-bang rewrite of 12,000 lines of `main.rs` | Not proposed. Handlers move one tool at a time; a route body shrinks to a call into its def. |
| `schemars` output differs from what a model handles well (deep `oneOf`, `$ref`) | A registry test bounds schema depth and forbids unresolved `$ref` in the served form; the generator inlines definitions. |
| Session state for gates must survive a backend restart | It does not today either (MCP process memory). Stated in the envelope when a session is new, never silently reset mid-task. |

## Open questions for Varun

1. Should the app's own UI routes be allowed to bypass gates the agent cannot (today they can)? The design gives one function; whether a human click passes `RequiresIntent` is a product decision.
2. Is a breaking rename of tool parameters acceptable once, at Stage 4 (ids and units), or must every current name survive as an alias?
3. `kb_lookup` reads bundled data in the MCP package. Does the knowledge base move to the backend with everything else, or stay as the one adapter-side tool?
