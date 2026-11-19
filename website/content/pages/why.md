---
title: Why Roshera is different
description: CAD has always trusted the person driving it. AI makes that trust impossible to keep by eye. Roshera is a geometry kernel that proves its own work, so the work can be handed to agents.
---

## CAD was built for a person checking by eye

For forty years CAD has been an instrument. A skilled person drives it, looks at the result and decides it is right. The software does what it is told and prints what it is given. If a drawing says 40 and the part measures 42, nothing in the tool notices; a checker might, or the shop floor will.

That arrangement held because a person made every change and looked at every result.

## AI breaks that arrangement

AI can now design parts and draw them in minutes. Most "AI for CAD" puts a language model in front of a conventional modeller: the model guesses the commands, the modeller executes them, and the result looks plausible. Plausible is the problem. A model that is right nine times in ten produces drawings that are wrong one time in ten, confidently, and at a speed no human checker can follow.

You cannot review your way out of that. The checking has to move into the geometry engine itself.

## Roshera proves its own work

Roshera is a geometry kernel built from scratch around one rule: **it cannot claim what it has not verified.**

| | Conventional CAD | AI on top of CAD | Roshera |
|---|---|---|---|
| Who checks a result | a person, by eye | nobody, or a person after the fact | the kernel, every time, with the measurement |
| A drawing value that no longer matches the part | prints anyway | prints anyway | marked stale, release refused |
| A feature the drawing leaves out | unnoticed | unnoticed | listed until stated or signed off |
| A tolerance on the wrong feature | possible | likely | measured on the real faces: in spec or out of spec |
| An operation it cannot do exactly | approximated silently | approximated silently | refused, with the reason |
| History | a feature tree you edit | a chat log | an event timeline: replay, undo, branch, compare |

## What that makes possible

Because every result carries proof, the work can be handed over:

- **An engineer describes the part** and gets a model whose soundness is certified, not one that merely looks right.
- **A supplier's STEP file becomes a production drawing** whose every number has been re-measured against the solid.
- **An agent builds and draws parts**, and when it is wrong the kernel says so. In one session an agent entered three hole positions with the wrong sign; the kernel measured all three out of spec by 74 to 144 mm, and the agent fixed them before anything was released.
- **A team keeps drawings and models in step**, because a sheet that disagrees with its part cannot be released.

## Built for agents and people together

Engineers use Roshera as a CAD workspace with a drawing bench and a design timeline. Agents use the same commands through REST and the Model Context Protocol, read manufacturing playbooks before they act, and ask for values they must not invent. Both get the same certificates.

The result is CAD you can delegate: fast because agents do the work, trustworthy because the kernel checks every step.

[Start a pilot](/contact) · [See how drawings certify themselves](/drawings) · [Read the engineering blog](/blog)
