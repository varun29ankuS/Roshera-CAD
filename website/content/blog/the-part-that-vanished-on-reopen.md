---
title: The part that vanished on reopen
description: An agent built a wing rib; after a reload the document came back without it. The cause was two sequence counters in one history, and the fix is a lesson about event-sourced CAD.
date: 2026-11-19
---

Roshera does not store parts. It stores their history: every operation is an event, and a document is rebuilt by replaying its events in order. That is what makes undo, branching and "change the dimension on the drawing and the part rebuilds" possible.

It also means order is everything.

## What happened

An agent built a wing rib and made its drawing. The page was reloaded, the document reopened, and the rib was gone. The drawing was still there, now reporting nineteen dangling dimensions.

The server log said why. On reopen, the replay refused the whole history: its first event took a solid as input that no earlier event had produced.

## Two counters

Reading the stored events showed the sketch-extrude that built the rib saved as **sequence 15**, and the naming step that followed it saved as **sequence 0**. Replay sorts by sequence, so it tried to name a solid that did not exist yet, and correctly refused to replay anything after that point.

Opening a document gives the timeline a fresh sequence counter. The recorder that numbers kernel operations had cached the counter of the first document it ever saw and kept numbering from it. Ordinary events used the new counter; kernel operations used the old one. Two counters, one history.

## The fix, and what we kept

The recorder now follows the open document's own counter. The test opens one document, records fifteen steps, opens another, and checks the next step is numbered 0. Before the fix it was numbered 15.

Two things we kept, because they behaved correctly:

- **The replay refused** rather than rebuilding a wrong part. A history it cannot replay faithfully is quarantined, and the document says so.
- **The drawing said its part was missing.** It now says that in one line ("the part this sheet was drawn from is not in the model") instead of a count of dimensions no one could fix on the sheet.

An event-sourced kernel is only as good as its event order, and a quarantine that refuses to guess is what turned a silent corruption into a ten-minute diagnosis.
