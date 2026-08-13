# roshera-rl

RL environment bridge: agent-driven, certificate-scored episodes against the
Roshera kernel. This package turns the MCP surface into a batch runner that
produces trajectories a trainer can consume — without ever pretending to know
more than the kernel actually certified.

## What an episode is

One episode = one document, one policy, one task. `runEpisode` (`lib/episode.mjs`):

1. Creates a fresh document on the backend.
2. Spawns an MCP session PINNED to that document (`ROSHERA_DOCUMENT`, see
   below).
3. Drives the policy step by step — `policy.act({task, observation, history})`
   returns either a `{tool, args}` action or `{done: true}` — until the policy
   declares done, the step or token budget is hit, or something breaks.
4. On completion, asks the session to verify the task's claims and fetch a
   recipe reference.
5. Closes the session and deletes the document (reaping), then writes the
   terminal trajectory record.

`runEpisode` never throws. Every failure mode — including a crashed MCP
process, a refused document creation, or a rate limit — is a named outcome,
not an exception a caller has to remember to catch. A worker that could die on
one episode would silently shrink the batch, so this property is exactly what
lets `runBatch` fan out N episodes without one bad episode taking the rest
down.

## The five outcomes

Every episode lands in exactly one of these (`lib/trajectory.mjs`, `OUTCOMES`):

| Outcome | Meaning |
|---|---|
| `COMPLETED` | The policy declared done. Claims were checked; a recipe ref was fetched if available. |
| `BUDGET_EXHAUSTED` | The step budget or token budget was hit before the policy declared done. |
| `CRASHED` | The MCP process died mid-episode (a tool call threw for a reason other than a 429). |
| `SETUP_FAILED` | Document creation or session spawn failed — no episode actually ran. |
| `RATE_LIMITED` | The backend's shared EvalHarness rate class (6000 req/min, shared across every concurrent episode) refused the call. |

The taxonomy is closed: `trajectory.close()` throws if handed an outcome name
outside this list, so a new category is a design change, not a typo.

`runBatch`'s `tally` always reports all five keys, zeros included. An absent
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

A component that could not be measured is reported **absent, with a stated
reason** (`gaps: [{name, reason}]`) — never silently defaulted to 0.

Weighting soundness against fidelity against refusal count is a training
choice with no kernel justification. Collapsing that into one scalar here
would assert a tradeoff Roshera cannot prove. So the environment reports the
vector, and scalarization — if any — happens downstream, in the trainer.

## Running a batch

```js
import { runBatch } from "./lib/runner.mjs";
import { scriptedPolicy } from "./lib/policy.mjs";
import { TASKS } from "./lib/task.mjs";

const { results, tally } = await runBatch({
  tasks: TASKS,
  policyFor: (task, seed) => scriptedPolicy([/* ... */]),
  seeds: [1],
  concurrency: 4,
  baseUrl: "http://127.0.0.1:8081",
  authHeader: { Authorization: "ApiKey ..." },
  outDir: "./out",
  kernelSha: "<git sha of the running api-server>",
});

console.log(tally); // { COMPLETED: 1, BUDGET_EXHAUSTED: 0, CRASHED: 0, SETUP_FAILED: 0, RATE_LIMITED: 0 }
```

`concurrency` is a real cap, not a suggestion — episodes are drained from a
shared queue by a fixed number of concurrent workers (`Promise.all` over
`Math.min(concurrency, tasks.length)` workers), so peak concurrency never
exceeds the number requested regardless of how many tasks are queued.

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
the load-bearing property this whole slice exists to establish — see the
isolation proof below.

### The isolation proof

`test/runner.test.mjs` asserts directly, not by inference, that N concurrent
episodes hold N distinct documents and that no call from one episode's
session ever carries another episode's document id. This is mutation-proven:
reverting the per-episode document creation to a module-level cached document
(a "create once, reuse" pattern — the same shape as the pre-pin
globally-active-document bug) makes the isolation check fail immediately,
with all episodes collapsed onto a single shared document id. Restoring the
per-episode creation restores the pass. That mutation and its recorded output
is the evidence this claim is tested, not merely asserted.

## Replay is recipe-level, not bit-stable

Every trajectory header states this explicitly: geometry reproduces to
~4e-8, not byte-for-byte. `recipe_ref` (from `session.recipeRef()`, backed by
the kernel's `GET /api/timeline/recipe/{ref}`) points at the timeline lineage
entry so a completed build is **re-issuable** — replaying it re-runs the
recipe and gets geometry within tolerance of the original, not an
identical byte stream. A consumer that assumes byte-identical replay is
assuming a determinism the kernel never promised.

## Tests

Each suite runs standalone (no test framework, no `node_modules` required
beyond what's already in the repo):

```
node test/trajectory.test.mjs
node test/reward.test.mjs
node test/task.test.mjs
node test/policy.test.mjs
node test/episode.test.mjs
node test/runner.test.mjs
```

`lib/mcp_session.mjs` imports `@modelcontextprotocol/sdk` lazily, inside
`spawnMcpSession`, specifically so these suites — which inject a fake
`spawn` and never touch a real MCP process — import and run cleanly even
where the SDK isn't installed.
