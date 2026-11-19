---
title: Roshera
description: A geometry kernel whose parts and drawings check their own numbers.
path: /
---

<section class="sec"><div><div class="mono kicker">Why it is different</div><h3>CAD you can delegate, because the kernel <span style="color:var(--blue)">proves every step.</span></h3><p>For forty years CAD has trusted the person driving it to check by eye. AI designs faster than anyone can check. Roshera moves the checking into the geometry engine: every solid certified, every number on a drawing re-measured, every tolerance verified on the real faces, and anything it cannot verify refused.</p><p><a href="/why">Why that changes how parts are made →</a></p></div><div class="cards">
<div class="card"><span class="mono">conventional CAD</span><b>Prints what it is told</b><span>A drawing that says 40 on a part that measures 42 goes to the shop.</span></div>
<div class="card"><span class="mono">AI on top of CAD</span><b>Guesses plausibly, fast</b><span>Right most of the time, confidently wrong the rest, faster than any checker.</span></div>
<div class="card"><span class="mono">Roshera</span><b>Proves, or refuses</b><span>Each result comes with its measurement. A wrong sheet cannot be released; a wrong tolerance reads out of spec.</span></div>
</div></section>

<section class="sec"><div><div class="mono kicker">What it does</div><h3>From a sentence or a STEP file to a drawing the shop can use.</h3><p>Roshera is a geometry kernel with its own drawing bench and design history. Engineers work in the app; AI agents use the same commands and the same checks.</p></div><div class="cards">
<a class="card" href="/drawings"><span class="mono">Drawings</span><b>Production drawings that certify themselves</b><span>Views, sections, fits from ISO 286, hole tables, GD&T, profile ordinates. Every printed value re-measured against the part.</span></a>
<a class="card" href="/kernel"><span class="mono">Kernel</span><b>A kernel that refuses instead of guessing</b><span>Native B-Rep in Rust, exact predicates, a timeline of every change, and a certificate with every result.</span></a>
<a class="card" href="/fields"><span class="mono">Fields</span><b>Exact distance fields on the real solid</b><span>Signed distance to the true surface, refined where the surface is: the base for lattices, walls and clearances.</span></a>
<a class="card" href="/agents"><span class="mono">Agents</span><b>Your AI tools drive it; the kernel checks the work</b><span>REST and the Model Context Protocol, with manufacturing playbooks the agent reads before it acts.</span></a>
</div></section>

<section class="sec"><div><div class="mono kicker">Why it matters</div><h3>A drawing that disagrees with its part costs a scrapped batch.</h3><p>Most drawing tools print what they were told. Roshera prints what it measured, says what it left out, and refuses to release a sheet that no longer matches the part.</p></div><div class="cards">
<div class="card"><span class="mono">re-measured</span><b>Every value on the sheet</b><span>A sheet certificate checks each printed dimension against the live solid.</span></div>
<div class="card"><span class="mono">complete</span><b>Nothing left out silently</b><span>Dimensions the sheet does not state are listed, until someone states or signs for them.</span></div>
<div class="card"><span class="mono">gated</span><b>No release on a wrong sheet</b><span>A sheet that disagrees with its part cannot be exported as a release drawing.</span></div>
</div></section>
