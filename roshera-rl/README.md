# roshera-rl

RL environment bridge: agent-driven, certificate-scored episodes against the
Roshera kernel. This package turns the MCP surface into a batch runner that
produces trajectories a trainer can consume — without ever pretending to know
more than the kernel actually certified.

## What an episode is

One episode = one document, one policy, one task. `runEpisode` (`lib/episode.mjs`):

1. Opens the trajectory file and writes its header.
2. Creates a fresh document on the backend.
3. Spawns an MCP session PINNED to that document (`ROSHERA_DOCUMENT`, see
   below).
4. Drives the policy step by step — `policy.act({task, observation, history})`
   returns either a `{tool, args}` action or `{done: true}` — until the policy
   declares done, the step or token budget is hit, the policy leaves its own
   action space, or something breaks.
5. On completion, asks the session to verify the task's claims
   (`verify_claim`, against kernel ground truth) and fetch the build's recipe.
6. Closes the session and deletes the document (reaping), then writes the
   terminal trajectory record.

`runEpisode` reports failures as named outcomes, not exceptions. Every failure
mode it knows about — a crashed MCP process, a policy that throws, a refused
document creation, a rate limit, an unwritable trajectory path — lands in the
outcome taxonomy below with its reason recorded. That property is what lets
`runBatch` fan out N episodes without one bad episode taking the rest down.

The guarantee is stated honestly rather than absolutely. The one historical
counterexample — `openTrajectory`'s synchronous `writeFileSync` preceding any
try/catch, so an unwritable `outDir` threw out of `runEpisode` — is fixed: it
is now caught and reported as `SETUP_FAILED` with the reason. What remains
outside the try/catches is small but real: a policy whose `tokensUsed()`
throws, or a filesystem that fails an `appendFileSync` mid-episode, will still
throw. `runBatch`'s worker wraps each episode in a `.catch` that converts any
residual throw into a `CRASHED` result, so the batch survives either way — but
a `policyFor` factory that throws is evaluated before that backstop attaches
and will take the batch down (see Running a batch).

## The six outcomes

Every episode lands in exactly one of these (`lib/trajectory.mjs`, `OUTCOMES`):

| Outcome | Meaning |
|---|---|
| `COMPLETED` | The policy declared done. Claims were checked against kernel measurements; the recipe was fetched, or its absence stated. |
| `BUDGET_EXHAUSTED` | The step budget or token budget was hit before the policy declared done. Nothing failed — a limit was reached — so the terminal `error` is `null`. |
| `INVALID_ACTION` | The policy named a tool outside its own declared action space. The call never reaches the kernel: the harness records the step as a refusal (`gate: "harness_allowlist"`), with the same stated reward gaps a kernel refusal carries, counted in `refusals` like any other. An episode that ran zero real steps must not read the same as one that genuinely burned its budget. |
| `CRASHED` | Either the MCP transport died mid-episode (`session.call` threw — the child process is dead, the pipe broke; a thrown error carrying a 429 status is classed `RATE_LIMITED` instead), or the policy itself threw from `act()`. Both leave a recorded step carrying the reason, and the error is carried out in the terminal record and the returned object. |
| `SETUP_FAILED` | Setup failed before any step ran: opening the trajectory file, document creation, or the session spawn. The record names WHICH stage failed and carries the underlying error — a 401 from `POST /api/documents` and a spawn that died on a missing dependency used to write byte-identical records, and diagnosing them took a hand-run probe. A document created before a failed spawn is just as real as any other, and is still reaped rather than orphaned in PartManager's DashMap. |
| `RATE_LIMITED` | The backend's rate class refused the call — detected from what actually crossed the wire (the 429 body in the failure text; `ApiError.status` never crosses stdio), including a 429 on document creation, which is the first request every episode makes and therefore where concurrency hits the ceiling first. The episode stops at the ceiling instead of burning its budget against it. |

The taxonomy is closed: `trajectory.close()` throws if handed an outcome name
outside this list, so a new category is a design change, not a typo.

`runBatch`'s `tally` always reports all six keys, zeros included. An absent
key would read as "not measured," which is the one thing this taxonomy exists
to prevent — the same discipline the reward vector applies to its gaps.

## The reward vector — and why it is never scalarized

`rewardFromResult` / `mergeFinal` (`lib/reward.mjs`) report reward as a
**named vector** per step and a named terminal reading per episode — never a
single number.

- `sound` — the kernel's own soundness verdict for the geometry produced.
- `fidelity_signed` — the worst (largest-magnitude) signed relative deviation
  seen across the episode, not the mean and not the last value. A mean would
  let one good step hide one bad one — exactly the "certified sound at 9.97%"
  failure this signal exists to surface.
- `refused` (per step) / `refusals` (episode total) — a refusal is
  **recorded, not penalized**. It's information: the agent met a constraint
  and learned where it sits. Whether that's negative reward is the trainer's
  decision, not the environment's.
- `call_failed` (per step) / `call_failures` (episode total) — a call that
  failed without a gate naming it is a different fact from a refusal: a
  refusal is the kernel holding a line, a failure is the call not landing.
  Collapsing them would make "the moat held N times" unreadable.

A component that could not be measured is reported **absent, with a stated
reason** (`gaps: [{name, reason}]`) — never silently defaulted to 0.

Weighting soundness against fidelity against refusal count is a training
choice with no kernel justification. Collapsing that into one scalar here
would assert a tradeoff Roshera cannot prove. So the environment reports the
vector, and scalarization — if any — happens downstream, in the trainer.

### Fidelity arrives, measured live

The dense reward is real now, end to end. The api-server attaches a
requested-vs-measured fidelity block to the response of the ops that have a
measurable request — cylinder, box, revolve, loft (`attach_fidelity`,
`api-server/src/main.rs:1326`, call sites at 4237/4456/5361/6026).
`roshera-mcp` used to rebuild `perception` with a fixed key set that silently
dropped that block, so the signal never reached any agent; it now carries it
through verbatim (`perceptionFromBody`, `core.ts`, proven at the production
dispatch path by `roshera-mcp/test/perception_fidelity.test.mjs`). This
package reads it at `perception.fidelity.worst.signed_relative_deviation`.

A live run against a real kernel measured, on `create_cylinder`:

```
fidelity_signed: 1.4210854715202004e-16
```

— machine epsilon, consistent with what the kernel's own docs predict for the
calibration case: an analytic cylinder is exactly what was asked for, so the
block should read ~1e-16 and exists to prove the statistic is trustworthy
before a loft's number is believed (`main.rs`'s fidelity comment;
`geometry-engine/src/queries/fidelity.rs` puts analytic primitives at ~1e-15
against the loft residual at 0.19% and the octagon defect at 9.97%). The saved
trajectory is
`.superpowers/sdd/2026-08-13-rl-episode-loop-slice1/live-trajectory-after-gapfix.jsonl`.

A step whose response carries no fidelity block reports the component absent
with the reason: the op attached none. An op with nothing measurable attaches
nothing rather than a block of zeros, ops outside the four above never attach
one, and `verify_part` builds its own body from the read-side perception
endpoint, which has no fidelity producer — none of which is a dropped
measurement, and the gap text says so.

## Running a batch

### The entry point

`bin/run-batch.mjs` is the production call site — the reason this package
cannot be built and wired to nothing (`test/wiring.test.mjs` asserts it
actually drives `runBatch` over the seed tasks, behaviourally, not by regex).

```
cd roshera-rl
npm install          # once — the MCP SDK is a real dependency of a live run
ROSHERA_URL=http://127.0.0.1:8081 ROSHERA_API_KEY=<key> \
  npm run batch -- --concurrency 4 --out ./runs --repeats 1
```

What a live batch actually requires:

- **`npm install`, once.** `@modelcontextprotocol/sdk` is a declared
  dependency. The unit suites run without it (see Tests), but the spawned
  session is a real stdio client; a live run without `node_modules` fails
  every spawn, and it fails honestly — `SETUP_FAILED`, naming the spawn stage
  and the missing module.
- **`roshera-mcp/dist` must be built and current.** The child process is
  `node roshera-mcp/dist/index.js` — the compiled artifact, not the
  TypeScript. A stale `dist` runs stale tool code. The default entry resolves
  against this package's own location, never the CWD (`defaultMcpEntry`;
  `npm run batch` from the repo root used to make every spawn fail because a
  CWD-relative default pointed outside the repo). `ROSHERA_MCP_ENTRY`
  overrides it.
- **`ROSHERA_API_KEY`** becomes `Authorization: ApiKey <key>` on the
  harness's own fetches, and crosses into the child as `ROSHERA_API_KEY`
  (the child re-forms the same header itself). `ApiKey` is the only scheme
  this seam can carry: anything else is refused loudly at spawn rather than
  converted into a session that 401s on every call.
- **The rate class decides your throughput.** An identity on the server's
  `ROSHERA_EVAL_IDENTITIES` allowlist gets the EvalHarness class — 6000
  req/min — instead of the default Mutation class at 100 req/min
  (`auth_middleware.rs`). Either way the budget is shared across every
  concurrent episode of that identity, which is exactly why `RATE_LIMITED`
  is its own outcome: the ceiling shows up in the tally instead of being
  averaged into a lower score.
- **`ROSHERA_KERNEL_SHA`** stamps the trajectory header. Unset, the header
  says `"unknown"` — which is what both saved live trajectories say, because
  it wasn't set. Set it; a trajectory that can't name the kernel it ran
  against is a weaker artifact.

The spawned session also pins `ROSHERA_AMBIENT_PERCEPTION=cert`: the default
`compact` mode renders perception as one line of prose, in which
`perception.sound` does not exist and soundness is unreadable for every step.
`cert` returns the full perception object without paying for render images no
policy here looks at. A result arriving as prose anyway is reported as a
stated gap, never parsed into a boolean by guesswork.

Slice 1's batch drives the scripted reference policy (checkpoint the intent,
`create_cylinder`, then `verify_part` with the `part_id` read off the create
result — the last step cannot be a fixed script, because the id does not exist
until the kernel mints it). This proves the loop, not the agent; model-backed
policies arrive in slice 2 behind the same `policy.act` seam.

### The library API

```js
import { runBatch } from "./lib/runner.mjs";
import { scriptedPolicy } from "./lib/policy.mjs";
import { TASKS } from "./lib/task.mjs";

const { results, tally, orphans } = await runBatch({
  tasks: TASKS,
  policyFor: (task, seed) => scriptedPolicy([/* ... */]),
  seeds: [1],
  concurrency: 4,
  baseUrl: "http://127.0.0.1:8081",
  authHeader: { Authorization: "ApiKey ..." },
  outDir: "./out",
  kernelSha: "<git sha of the running api-server>",
});

console.log(tally);
// { COMPLETED: 1, BUDGET_EXHAUSTED: 0, INVALID_ACTION: 0,
//   CRASHED: 0, SETUP_FAILED: 0, RATE_LIMITED: 0 }
```

`concurrency` is a real cap, not a suggestion — episodes are drained from a
shared queue by a fixed number of concurrent workers
(`Math.max(1, Math.min(concurrency, queue.length))`), so peak concurrency
never exceeds the number requested regardless of how many tasks are queued.

One caveat, stated because it is the edge of the no-throw guarantee:
`policyFor` runs in the worker before the episode's `.catch` backstop exists.
A policy *instance* that throws is a `CRASHED` episode; a policy *factory*
that throws rejects the whole batch.

`orphans` is the reaper's honest remainder. Each episode attempts its own
DELETE and reports the result; `runBatch` retries every document the episode
could not drop (once — a document the backend refuses twice will not yield to
a third identical request), and returns the survivors rather than asserting
they were cleaned up. `bin/run-batch.mjs` prints them.

Each episode writes its own JSONL trajectory file under `outDir`, named
`{taskId}-{seed}-{index}.jsonl`.

## The `ROSHERA_DOCUMENT` pin, and why it exists

Without a pin, an MCP session discovers the backend's globally-`active`
document (`roshera-mcp/src/core.ts::bindSessionDocument`). That's fine for one
human driving one session. It is fatal for N concurrent episodes: every
process would land on the same document, so two episodes could see (and
mutate) each other's parts and gate state, and every trajectory produced in
parallel would be contaminated in a way no downstream consumer could detect.

`spawnMcpSession` (`lib/mcp_session.mjs`) sets `ROSHERA_DOCUMENT` in the
spawned process's environment to the document `runEpisode` just created for
that episode, and every tool call that process makes is scoped to it. This is
the load-bearing property this whole slice exists to establish.

### The isolation proof — what it proves, and how it was mutation-tested

`test/runner.test.mjs` asserts directly, not by inference, that four
concurrent episodes hold four distinct documents and that no call from one
episode's session ever carries another episode's document id.

The mutation that proves the test can fail is worth describing precisely,
because the obvious mutation proves nothing. Reverting per-episode document
creation to a module-level cached *document* — `if (CACHED) return CACHED;
CACHED = await fetch(...)` — does **not** fail this test: under Node's
run-to-completion scheduling, all four workers execute their `if (CACHED)`
check before any fetch has resolved, so all four still create their own
document and the test stays green against a mutant that shares state. The
recorded mutation instead caches the in-flight **promise**, assigned
synchronously before the first `await` — the realistic lazy-singleton shape.
Against that mutant every worker reuses the first creation and the isolation
check fails immediately: `["doc-0","doc-0","doc-0","doc-0"]`, `1 !== 4`.
Restoring per-episode creation restores the pass. That recorded failure is the
evidence this claim is tested, not merely asserted.

Its scope is one story, and it has three parts: the proof above drives
**injected fake sessions against a stub HTTP backend**, so it proves the
harness's per-episode document plumbing. The child process honouring the pin
is proven separately, by `roshera-mcp/test/document_pin.test.mjs`. The single
line that joins the two halves — the `ROSHERA_DOCUMENT: documentId` env
assignment in `spawnMcpSession` — is executed by **no automated test**; it has
been exercised only by two hand-run live smoke runs. See "What this does not
prove."

## Replay is recipe-level, not bit-stable

Every trajectory header states this explicitly: geometry reproduces to
~4e-8, not byte-for-byte. `recipe_ref` (from `session.recipeRef()`, backed by
the `recipe_get` tool over the kernel's timeline lineage) records what
`recipe_get` actually returns — `source`, `step_count`, `checkpoints`,
`certificate_summary`, and the `steps` themselves. The steps are embedded in
the trajectory because the address does not survive the episode: the document
is deleted at reap and `DELETE /api/documents` purges its timeline events, so
a descriptor alone would be a dangling pointer. Replaying means re-issuing
those steps and getting geometry within tolerance of the original, never an
identical byte stream. A consumer that assumes byte-identical replay is
assuming a determinism the kernel never promised.

For every non-`COMPLETED` outcome, `recipe_ref` is `{absent: "<reason>"}` —
never a bare `null`, which would read as "there was no recipe", a different
and false claim. The same rule holds for `claims`.

## Tests

`npm test` runs all eight suites; each also runs standalone:

```
node test/trajectory.test.mjs
node test/mcp_session.test.mjs
node test/reward.test.mjs
node test/task.test.mjs
node test/policy.test.mjs
node test/episode.test.mjs
node test/runner.test.mjs
node test/wiring.test.mjs
```

`npm run test:isolation` runs the isolation proof alone.

The unit suites need no `node_modules`: `lib/mcp_session.mjs` imports
`@modelcontextprotocol/sdk` lazily, inside `spawnMcpSession`, specifically so
suites that inject a fake `spawn` import and run cleanly where the SDK isn't
installed. A **live** batch is different — the SDK is a real dependency and
`npm install` is required first (see Running a batch).

`test/mcp_session.test.mjs` is the wire-contract proof: every tool result it
feeds through the real `readToolResult` is copied from the source that
produces it, cited line by line. That discipline exists because this package's
first version was written against an imagined wire contract, every test
injected a fake session, and the suite certified the mock — four criticals
survived seven reviews that way.

## What this does not prove

Everything below is a true limit of the current test evidence. Each is stated
here so the honest scoping lives with the code, not only in the execution
ledger.

- **The `ROSHERA_DOCUMENT` handoff line is executed by no test.** The
  isolation proof covers the harness half with fake sessions; roshera-mcp's
  document-pin tests cover the child half; the env assignment joining them
  (`spawnMcpSession`) runs only when a real child is spawned, which no suite
  in `npm test` does. It has been exercised exactly twice, by hand-run live
  smoke runs whose trajectories are saved in
  `.superpowers/sdd/2026-08-13-rl-episode-loop-slice1/`. An observation is
  not a regression test.
- **Recipe replay has never been demonstrated live.** Both saved live
  trajectories — including the one where everything else worked — carry
  `step_count: 0` with the stated absence: the durable log for the episode's
  document reported zero steps at scoring time, so the embedded-steps
  mechanism has never actually carried a real step out of a live episode. The
  design's replay test ("a recorded `recipe_ref` re-issues and re-certifies")
  remains unrun. What is proven is the *plumbing*, against response shapes
  copied from source: real recipes are recorded whole, refusals and empty
  logs become stated absences.
- **Checkpoint-then-mutate ordering is verified against `gates.ts` by
  inspection, not empirically.** The wiring test stubs `session.call`, so the
  real intent gate never fires in any suite. The reference policy's
  checkpoint-first ordering is asserted; that the gate would have refused the
  opposite order is read from source, not observed.
- **`RATE_LIMITED` has never been observed against a live backend.** The
  detection is proven against the 429 body copied verbatim from
  `auth_middleware.rs`, and it matches on that body's text — a stated
  compromise, since the rate-limit refusal is not in the typed error catalog
  and no typed field survives to the client. The 6000/min ceiling under real
  concurrency — the thing the outcome exists to measure — has not yet been
  measured. Note also the code does not back off and retry at the ceiling;
  it ends the episode there.
- **The refusal detector is a mirror, pinned at one line.** `typedRefusalOf`
  is copied from `gates.ts` (which does not export it), and
  `test/mcp_session.test.mjs` asserts the load-bearing line exists verbatim
  in both files. Drift in the surrounding semantics — anything beyond that
  one line — would not trip the pin.
- **No suite spawns a real MCP child.** Every session in `npm test` is
  injected. The real stdio transport, the SDK, and the compiled
  `roshera-mcp/dist` have run only in the live smokes. This is the same
  shadow that hid four criticals once already; the wire-contract suite
  narrows it, and does not close it.
