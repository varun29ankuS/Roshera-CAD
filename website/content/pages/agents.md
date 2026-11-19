---
title: Agents
description: AI agents drive Roshera through REST and the Model Context Protocol, read manufacturing playbooks before they act, ask for what they must not invent, and get a certificate with every result.
---

<div class="facts"><div><b>MCP + REST</b><span>the same commands the app uses</span></div><div><b>Playbooks</b><span>part-family know-how read before acting</span></div><div><b>Certificates</b><span>on every build, drawing and tolerance</span></div></div>

## The problem

An agent that writes CAD commands into a conventional modeller gets no feedback about whether the result is right. It guesses, the modeller complies, and the error surfaces downstream.

## How agents work with Roshera

**Tools.** The kernel exposes over a hundred geometry, drawing and verification tools through the Model Context Protocol, plus REST for scripts. A tool finder lets an agent locate the right tool from a plain description.

**Playbooks.** Before building, the agent reads a playbook for the part family: a flange to its standard, an aerofoil from its NACA designation, a manufacturing drawing (purpose first, sheet and view rules, fits from tables, the review loop). Playbooks say which values come from the engineer and must never be invented.

**Feedback.** Every action returns a certificate. Edits on a drawing return what changed on the sheet: new issues, resolved issues, layout passes. Refusals say why. The agent reacts to facts, not to its own guess about the result.

**Asking.** Given "H7 on the Ø20 bores" on a part whose Ø20 features are boss outsides, the agent reports what it measured and asks which features take the fit.

## Proof

- An agent entered three hole positions with the wrong sign; the kernel measured all three out of spec by 74 to 144 mm; the agent fixed them before release.
- [Real recordings](/#watch) of an agent importing a part, drafting it and answering for each choice.

## Next step

Connect your agent, or talk to us about evals and environments: [For AI labs](/ai-labs) · [info@roshera.tech](mailto:info@roshera.tech).
