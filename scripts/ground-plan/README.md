# Ground plan

An isometric plan of the whole repository — every crate and package, drawn from what is actually
on disk. Districts are subsystems, buildings are structures, and walking inside a structure turns
its files into buildings of their own.

```
node scripts/ground-plan/build.mjs            # → scripts/ground-plan/ground-plan.html
node scripts/ground-plan/build.mjs /tmp/x.html
node scripts/ground-plan/extract-plan.mjs > plan.json   # data only, no page
```

The output is one self-contained HTML file. No network calls, no build step, no dependencies —
open it in a browser.

## The rule this obeys

**Nothing on the page is hand-entered except prose that carries no numbers.** Every figure comes
from `extract-plan.mjs`: line counts by walking the tree, churn by reading `git log`, and the
links between structures by parsing every `use` and `import` statement. The viewer is a renderer.

That is the whole point. A hand-maintained architecture diagram is out of date the week it is
drawn and quietly lies thereafter. Re-run the build and the plan is current.

## What is measured

| On the page | Comes from |
|---|---|
| Footprint | number of files in the structure |
| Height | lines of code, on a compressed `^0.55` scale stated in the legend |
| Hatch density | commits in all history that touched that code |
| Solid link | files naming another structure in a `use` / `import` |
| Dashed link | an HTTP/WebSocket call — **not** a link-time dependency |

The solid/dashed split is load-bearing rather than decorative. `roshera-mcp`, `roshera-app`,
`roshera-eval` and `roshera-rl` reach the server over the wire and link the kernel nowhere, which
is exactly why the agent surface can be replaced without rebuilding anything.

## What is authored

Two things, both in `viewer.html`:

- `ANN` — the per-structure prose: what it does, how it is built, and its **Condition**, the list
  of recorded ceilings. Conditions are transcribed from the `#[ignore]` registry, which the
  workspace's bare-ignore gate requires to carry a reason. That gate is what makes the list
  trustworthy rather than flattering.
- `FLOW` — the eight hops of a single agent request, for the walkthrough.

Both are keyed by structure letter. **Neither contains a number**, so prose cannot drift out of
step with the measurements.

## Adding or re-cutting a structure

Edit `STRUCTURES` in `extract-plan.mjs`. Order matters — first match wins, so a specific module
must appear before the crate root that would otherwise swallow its files. Add matching prose to
`ANN` in `viewer.html` under the same key; a structure with no entry still renders, with its
measured figures and no description.

## Known limits

The extractor states these in `meta.caveats`, so they travel with the data rather than living
only here:

- churn does not follow renames, so a moved file reads younger than it is
- Rust edges are module-level — the module tree is not the file tree, so a file-to-file arrow
  would be a guess
- a use of the `geometry-engine` crate from outside it is attributed to `A`, the kernel's largest
  structure: crate-level granularity, not module-level
- `loc` counts every line, blanks and comments included
- reference edges below three referencing files are dropped to keep the plan legible

## For agents

The complete dataset is embedded in the page as `<script type="application/json"
id="roshera-plan">`, unmodified generator output. Read it without touching the DOM:

```js
const html = await (await fetch(url)).text();
const plan = JSON.parse(html.match(/id="roshera-plan"[^>]*>([\s\S]*?)<\/script>/)[1]);

plan.meta        // commit, totals, caveats
plan.districts   // 7
plan.structures  // loc, files, churn, tests
plan.edges       // { from, to, w, kind }
plan.files       // { p, k, loc, c, t, r[] }
```

In a browser it is also on `window.__ROSHERA_PLAN__`.
