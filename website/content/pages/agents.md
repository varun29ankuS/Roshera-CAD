---
title: Agents
description: AI agents drive Roshera through REST and the Model Context Protocol, read manufacturing playbooks before they act, and get a certificate with every result.
---

## The same tools for people and agents

Everything the app does goes through the kernel's commands, and the same commands are available to software:

- a **REST API** for scripts and integrations,
- a **Model Context Protocol (MCP) server** that gives an AI agent over a hundred geometry, drawing and verification tools.

## Playbooks before actions

Agents read **playbooks** before they build: how to design a flange to its standard, how to lay out an aerofoil from its NACA designation, how to make a manufacturing drawing (purpose first, ISO sheet and view rules, fits from tables, the review loop). The playbooks say which values must come from the engineer and never be invented.

## A certificate with every result

Agents are wrong sometimes. Roshera is built so that it shows: every build returns a soundness certificate, every drawing a sheet certificate, every tolerance a measured verdict. When an agent entered hole positions with the wrong sign in a recent session, the kernel reported all three holes out of spec by tens of millimetres, and the agent corrected them.

## Agents ask instead of guessing

Given an ambiguous request ("H7 on the Ø20 bores" on a part whose Ø20 features are boss outsides, not bores), the agent says what it measured and asks which features take the fit. It does not ink a tolerance on a guess.

## Connect

Write to [info@roshera.tech](mailto:info@roshera.tech) for API access. This site publishes a machine-readable index at [/llms.txt](/llms.txt) and a Markdown copy of every page.
