---
title: A drawing that checks its own numbers
description: Every value on a Roshera sheet is re-measured against the solid. Here is what the sheet certificate checks, and what it refuses.
date: 2026-11-17
---

A manufacturing drawing makes thousands of small claims: this bore is 40, these holes are 13 H7, this face is flat to 0.05. In most CAD tools those numbers are correct when the sheet is made and then simply sit there. Change the part, forget to update the sheet, and the drawing goes to the shop saying something the part no longer is.

Roshera treats every number on a sheet as a claim to be checked.

## The sheet certificate

Each printed fact carries where it came from: the persistent identity of the faces or edges it measures. The certificate re-measures every one of them on the live solid and gives each a verdict:

- **consistent**: the sheet says what the part measures, within 0.1 mm;
- **stale**: the part changed after the sheet was made;
- **dangling**: the feature it describes no longer exists;
- **omitted**: the part carries a feature the sheet does not show.

A sheet is **sound** when nothing is stale, dangling or omitted. It is **complete** when every dimension the part has is either stated or signed off by a named person.

## Both directions

Checking what is printed is only half the job. A drawing can be perfectly true and still leave out a bore that the machinist then has to guess. So the certificate also walks the model: every bore, every pattern, every datum and tolerance frame the part carries must have a counterpart on the sheet. During a recording session this week, a part carried two datums and six position frames that the sheet did not show; the sheet read "Not sound: 8 to fix", each item named, until the sheet was regenerated.

## Release is gated

A sheet that is not sound cannot be exported as a release drawing. Not with a warning, not with an override buried in a menu: the export is refused, and the refusal says what to fix. A draft for review can still be printed, labelled as a draft.

## Why this matters for AI

When an agent makes drawings, the volume goes up and the human attention per sheet goes down. A drawing that verifies itself is what makes that safe. The agent can be fast; the certificate is what keeps it honest.
