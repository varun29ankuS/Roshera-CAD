# Workflows, identities, modes: direction agreed so far

2026-11-20. These are notes from a conversation with Varun. They are not a design yet. They record what was decided and what is parked, so the design can start from here.

Related notes:
- `2026-11-20-cad-workflow-variations.md`
- `2026-11-20-cad-roles-in-industry.md`
- `2026-11-20-workflow-catalog.md`

## Decided by Varun

- **The first identity is the aerospace supplier engineer, out of at least five.** The six proposed are:
  1. aerospace supplier engineer (D7, M0, D4);
  2. aerospace quality / first-article inspector (M4, M6, E3);
  3. automotive Tier-1/2 supplier engineer (D7, D4, M4, run the PPAP way);
  4. drawing checker (Check mode, D4 re-check);
  5. design engineer at an OEM or startup (D1, D2, D3, E1, E2);
  6. manufacturing engineer / job shop (M0, M1, M2, M6).
- **Workflows are suggested from what is asked.** The catalog lists them all.
- **A check is not enough; it has to be conveyed.** Every gate shows on the board as a card with four parts:
  - what was checked;
  - the verdict;
  - what was not checked;
  - why it matters for this part.

  A job ends with a summary pack, such as a release pack or a first-article pack.
- **Modes, adapted to mechanical engineering:**

  | Mode | Behaviour |
  |---|---|
  | Consult | Read-only |
  | Plan | Proposes the workflow, stages, questions and assumptions; builds nothing until approved |
  | Step | Stops at every checkpoint |
  | Automatic | Passes the kernel's gates itself; stops at person gates, refusals and any value it would have to guess |
  | Explore | Options on branches with no gates; one gate on promotion |
  | Check | Acts as a checker and produces redlines |

  In every mode, a gate a person must sign never passes by itself.
- **Datums and key characteristics are proposed, never decided.** In every mode, including Automatic, the agent proposes them, carries on with the proposal marked as proposed, and release waits on a person's confirmation.
- **The identity is inferred, not picked, and it belongs to the job rather than the person.** Varun: "using keywords to decide is a slippery 2000's mistake".
  - The kernel gathers the facts as structured data: what arrived, what the data pack states, the part's state and revision, who made what, and what the project confirmed before.
  - The model judges the workflow, role, industry rules and its confidence, and it must cite the evidence it relied on. A judgment with no evidence counts as unsure, and the agent then asks the one question that separates the candidates.
  - Code enforces what judgment must never decide:
    - the signer comes from the login;
    - the checker is never the drafter;
    - an inferred industry adds gates, never removes them;
    - a person gate never passes by itself.
  - Routing is scored on an eval set of realistic requests, with a held-out part. A wrong route on that set is a red test.

## The checkpoint definition

A checkpoint is a named state, plus the evidence at that state, plus who may pass it. The kernel can sign "sound", "matches the standard", "sheet complete" and "no interference". A person signs "spec agreed", "checked", "released", "change approved" and dispositions.

Today `timeline_checkpoint` stores a named intent over an event range, with its author, and two rules are enforced:
- no geometry change without an open checkpoint;
- no next checkpoint while the last one is unverified.

It does not yet store the gate evidence, cannot be jumped back to, and does not appear on the board.

## Parked: Omnigraph as the work-graph store

Varun: "this is why we need omnigraph", then: record it, no trial now.

- **The fit.** Omnigraph would hold the work around the geometry: jobs, gates, signatures, characteristics, roles, revisions, agent runs and evidence. It brings:
  - a typed schema that refuses bad writes ("no release without every gate signed");
  - Cedar policies enforced in the engine ("the checker is not the drafter");
  - search that follows links, similarity and keywords together, for "find past jobs like this" when grounding the routing judgment;
  - a branch per agent, with an author on every write;
  - time travel, for the aerospace need to show revision A as released;
  - a built-in MCP server.
- **The timeline stays the record of geometry.** The two are linked by event and checkpoint ids.
- **This reverses the earlier "knowledge graph = Postgres, not a graph DB" decision**, so it is gated on a trial:
  1. install it on Windows as a separate process, since Windows is only confirmed for the desktop app;
  2. write the aerospace supplier schema;
  3. prove a release missing a signature is refused, the checker-is-drafter policy is refused, and "similar past jobs" retrieval returns sensible matches;
  4. route one real request through its MCP server.

  If the trial fails, keep the schema and policy design and build them on Postgres.
- **Lance and Arrow.** Lance is the storage format underneath Omnigraph. It is also the candidate format for the "Roshera episodes" dataset for AI labs: one record per agent run, holding tool calls, timeline events, parts, renders and certificate verdicts. Arrow and DuckDB are candidates for eval analytics. Keep all three out of the kernel and the api-server builds.

## Strategy context (Varun: "think big")

- The goal is to be the lab that ships the CAD model, with Roshera as the de facto kernel. Lean is the precedent: the kernel is the reward function.
- **The sequence:**
  1. kernel trust (product gaps, 81/81);
  2. the environment: episode record, task generator, adversarial reward suite, sold to labs;
  3. post-train an open-weights model, "Roshera-1";
  4. model plus kernel as the product;
  5. the simulation design loop.
- **The model changes the API-only rule deliberately.** That is Varun's call, extending #16.
- **The Map tab** gets agent and workflow layers over the design graph, all fed by the same episode record.

## Next

Finish the remaining product gaps first. Then design: workflow and gate templates, modes, routing with its eval set, and the Blackboard rebuild (stage strip and checkpoint cards).
