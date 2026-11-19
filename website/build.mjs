// roshera.tech -- build the site from Markdown into static HTML.
//
// Every page is a Markdown file in content/pages (or a post in content/blog)
// with a small front matter block. The build writes, into dist/:
//   - one HTML page per file (clean URL: /drawings -> dist/drawings/index.html)
//   - the same page as Markdown (/drawings.md), for AI agents and crawlers
//   - llms.txt (an index for language models) and llms-full.txt (everything)
//   - sitemap.xml, rss.xml (the blog), robots.txt (AI crawlers welcome)
//   - schema.org JSON-LD in every page
// Run: npm run build   (Vercel runs the same; see vercel.json)

import { readFileSync, writeFileSync, mkdirSync, readdirSync, cpSync, rmSync, existsSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { marked } from 'marked'

const ROOT = dirname(fileURLToPath(import.meta.url))
const OUT = join(ROOT, 'dist')
const SITE = 'https://roshera.tech'
const EMAIL = 'info@roshera.tech'
const NAME = 'Roshera'
const TAGLINE = 'A geometry kernel whose parts and drawings check their own numbers.'

// ── Content ──────────────────────────────────────────────────────────────

/** Parse `---\nkey: value\n---\nbody` front matter. */
function parse(file) {
  const raw = readFileSync(file, 'utf8').replace(/\r\n/g, '\n')
  const m = raw.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/)
  if (!m) throw new Error(`${file}: missing front matter`)
  const meta = {}
  for (const line of m[1].split('\n')) {
    const i = line.indexOf(':')
    if (i > 0) meta[line.slice(0, i).trim()] = line.slice(i + 1).trim()
  }
  for (const key of ['title', 'description']) {
    if (!meta[key]) throw new Error(`${file}: front matter needs "${key}"`)
  }
  return { meta, body: m[2].trim() }
}

const read = (dir) =>
  readdirSync(join(ROOT, 'content', dir))
    .filter((f) => f.endsWith('.md'))
    .map((f) => ({ slug: f.replace(/\.md$/, ''), ...parse(join(ROOT, 'content', dir, f)) }))

const pages = read('pages').map((p) => ({ ...p, path: p.meta.path ?? `/${p.slug}` }))
const posts = read('blog')
  .map((p) => ({ ...p, path: `/blog/${p.slug}` }))
  .sort((a, b) => (a.meta.date < b.meta.date ? 1 : -1))
for (const p of posts) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(p.meta.date ?? '')) throw new Error(`blog/${p.slug}: date must be YYYY-MM-DD`)
}

// ── Templates ────────────────────────────────────────────────────────────

const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')

const NAV = [
  ['/why', 'Why Roshera'],
  ['/drawings', 'Drawings'],
  ['/kernel', 'Kernel'],
  ['/fields', 'Fields'],
  ['/agents', 'Agents'],
  ['/use-cases', 'Use cases'],
  ['/blog', 'Blog'],
  ['/contact', 'Contact'],
]

function head({ title, description, path, jsonld, type = 'website' }) {
  const url = SITE + (path === '/' ? '' : path)
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>${esc(title)}</title>
<meta name="description" content="${esc(description)}">
<link rel="canonical" href="${url}">
<link rel="alternate" type="text/markdown" href="${path === '/' ? '/index.md' : path + '.md'}" title="This page as Markdown">
<link rel="alternate" type="application/rss+xml" href="/rss.xml" title="${NAME} blog">
<link rel="icon" type="image/png" href="/logo.png">
<meta property="og:type" content="${type}">
<meta property="og:title" content="${esc(title)}">
<meta property="og:description" content="${esc(description)}">
<meta property="og:url" content="${url}">
<meta property="og:image" content="${SITE}/logo.png">
<meta name="twitter:card" content="summary">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Geist:wght@400;500;600;700&family=JetBrains+Mono:wght@400;500&display=swap">
<link rel="stylesheet" href="/site.css">
<script type="application/ld+json">${JSON.stringify(jsonld)}</script>
</head>`
}

const ORG = {
  '@type': 'Organization',
  name: NAME,
  url: SITE,
  email: EMAIL,
  logo: `${SITE}/logo.png`,
}

function header(current) {
  const links = NAV.map(
    ([href, label]) => `<a href="${href}"${current.startsWith(href) ? ' aria-current="page"' : ''}>${label}</a>`,
  ).join('')
  return `<header class="bar"><div class="wrap bar-in">
  <a class="brand" href="/"><img src="/logo.png" alt="" width="26" height="26">${NAME}</a>
  <nav class="mono" aria-label="Main">${links}</nav>
</div></header>`
}

const footer = () => `<footer class="foot"><div class="wrap foot-in">
  <div><a class="brand" href="/"><img src="/logo.png" alt="" width="22" height="22">${NAME}</a><p>${esc(TAGLINE)}</p></div>
  <div class="mono"><span>Contact</span><a href="mailto:${EMAIL}">${EMAIL}</a></div>
  <div class="mono"><span>For machines</span><a href="/llms.txt">llms.txt</a> · <a href="/sitemap.xml">sitemap</a> · <a href="/rss.xml">rss</a></div>
</div></footer>`

function page({ title, description, path, html, jsonld, type, wide }) {
  return `${head({ title, description, path, jsonld, type })}
<body>
${header(path)}
<main class="wrap ${wide ? '' : 'prose'}">
${html}
</main>
${footer()}
</body>
</html>
`
}

// ── Home sections ────────────────────────────────────────────────────────

// The whole loop. "today" ships now; "building" is in development and says so.
const STAGES = [
  ['Describe', 'Words, a spec or a STEP file', 'today'],
  ['Build', 'Native B-Rep, recorded as history', 'today'],
  ['Certify', 'Sound solid, with the measurements', 'today'],
  ['Draw', 'Production drawing, re-measured', 'today'],
  ['Mesh', 'Meshes traced to their faces', 'building'],
  ['Simulate', 'Stress, heat, flow on the real part', 'building'],
  ['Improve', 'Results feed the next design step', 'building'],
]
const PIPELINE = `<section class="pipe reveal" aria-label="The design loop">
<div class="mono kicker">The whole loop, one engine</div>
<ol data-pipeline>${STAGES.map(([t, d, s], i) => `<li data-state="${s}"><span class="mono">${String(i + 1).padStart(2, '0')} · ${s === 'today' ? 'today' : 'building now'}</span><b>${t}</b><em>${d}</em></li>`).join('')}</ol>
</section>`

const CLIPS = [
  ['import-step', 'Import a STEP file', 'Pick the file; the kernel checks the solid is sound before anything else.'],
  ['drawing-appears', 'Ask for the drawing', 'The agent reads the part, asks what it must not guess, and the sheet appears.'],
  ['zoom-sheet', 'Read it closely', 'Hole table, fits from ISO 286, datums: every value re-measured on the solid.'],
  ['edit-on-sheet', 'Edit on the sheet', 'Move a dimension, move a view, set a tolerance; each edit is one undoable step.'],
  ['wing-rib-build', 'A wing rib from its spec', 'NACA 2412, 300 mm chord: the agent generates the section, builds and draws it.'],
]
const PLAYER = `<section class="sec watch reveal" data-player>
<div><div class="mono kicker">See it work</div><h3>Real recordings, not renders.</h3><p>Screen recordings of Roshera at work this week, sped up. Nothing staged: what the agent said and what the kernel checked is what you see.</p>
<ol class="chapters">${CLIPS.map(([id, t, d], i) => `<li><button type="button" data-src="/video/${id}.mp4" data-poster="/video/${id}.jpg" aria-selected="${i === 0}"><span class="mono">${String(i + 1).padStart(2, '0')}</span><b>${t}</b><em>${d}</em></button></li>`).join('')}</ol></div>
<figure class="screen"><div class="chrome"><i></i><i></i><i></i><span class="mono">Roshera</span></div><video muted playsinline preload="metadata" aria-label="Recording of Roshera"></video><div class="progress"><i></i></div></figure>
</section>`

const LOOP = `<section class="sec loop reveal" data-loop>
<div><div class="mono kicker">Design · mesh · simulate · improve</div><h3>Simulation that feeds back into the design.</h3><p>The next stage closes the loop: mesh the certified part, simulate it, and turn the result into a change the kernel can make and check, with every result traced to the face that caused it.</p>
<ol class="loop-steps"><li data-step>Part</li><li data-step>Mesh</li><li data-step>Simulate</li><li data-step>Feedback</li><li data-step>Improve</li></ol>
<p class="note" data-note aria-live="polite"></p>
<p class="fine">Illustration of the loop we are building. The physics is real: stress around a hole in a plate under tension (the Kirsch solution, peak 3× nominal), and a slot aligned with the load (peak 1 + 2b/a, Inglis).</p></div>
<figure class="loop-canvas"><canvas aria-label="A plate with a hole is meshed, its stress field computed, the peak found, and the hole reshaped to lower it"></canvas><figcaption class="mono"><span class="scale"></span><span>low</span><span>stress</span><span>high</span></figcaption></figure>
</section>`

const CERT_ROWS = [
  ['Ø40 H7', 'main bore', '40.000'],
  ['3× Ø13 H7', 'mounting holes', '13.000'],
  ['198.8', 'overall length', '198.800'],
  ['28.5', 'boss height', '28.500'],
  ['⌖ Ø0.05 | A | B', 'position, B1–B3', '0.031'],
  ['R65', 'outer arc', '65.000'],
]
const CERT = `<section class="sec cert reveal" data-cert>
<div><div class="mono kicker">The sheet checks itself</div><h3>Change the part, and the drawing knows.</h3><p>Every value on a Roshera drawing is re-measured on the solid. When the part changes and the sheet does not, the sheet stops being sound, and release is refused until it states what the part really is.</p><p class="note" data-caption aria-live="polite"></p><p><a href="/drawings">How drawings certify themselves →</a></p></div>
<div class="cert-panel">
<svg viewBox="0 0 320 150" aria-hidden="true"><rect x="10" y="40" width="300" height="70" rx="35" class="part"/><circle data-bore cx="60" cy="75" r="20" class="part"/><circle cx="170" cy="75" r="6.5" class="part"/><circle cx="215" cy="75" r="6.5" class="part"/><circle cx="260" cy="75" r="6.5" class="part"/><g class="dim"><line x1="40" y1="22" x2="80" y2="22"/><text data-bore-text x="60" y="16">Ø40 H7</text><text x="215" y="132">3× Ø13 H7</text></g><g data-mark></g></svg>
<ul>${CERT_ROWS.map(([v, w, m]) => `<li data-row data-state="wait"><code>${esc(v)}</code><span>${w}</span><span class="m mono">${m}</span><span class="v mono">…</span></li>`).join('')}</ul>
<div class="cert-foot"><span class="mono" data-status data-kind="wait">Checking the sheet…</span><span class="mono rel" data-release data-kind="wait">Release: checking</span></div>
</div>
</section>`

const SHOTS = `<section class="shots reveal" aria-label="Screenshots">
<div class="mono kicker">Inside the app</div>
<div class="shot-grid">
<figure><img src="/shots/part-imported.jpg" alt="A STEP part imported into Roshera with its soundness certificate" loading="lazy"><figcaption>Imported, and certified sound before anything else.</figcaption></figure>
<figure><img src="/shots/drawing-sheet.jpg" alt="A production drawing generated by Roshera" loading="lazy"><figcaption>The drawing: views, sections, hole table, title block.</figcaption></figure>
<figure><img src="/shots/sheet-closeup.jpg" alt="Close-up of datums, hole tags and dimensions on a Roshera drawing" loading="lazy"><figcaption>Datums, hole tags and the agent's account of the sheet.</figcaption></figure>
<figure><img src="/shots/wing-rib-part.jpg" alt="An aerofoil wing rib built by the agent in Roshera" loading="lazy"><figcaption>A NACA 2412 wing rib, built from its specification.</figcaption></figure>
</div>
</section>`

// ── Home ─────────────────────────────────────────────────────────────────

function home() {
  const latest = posts
    .slice(0, 3)
    .map(
      (p) => `<a class="card post" href="${p.path}"><span class="mono">${p.meta.date}</span><b>${esc(p.meta.title)}</b><span>${esc(p.meta.description)}</span></a>`,
    )
    .join('')
  const intro = pages.find((p) => p.path === '/')
  const extra = intro ? marked.parse(intro.body) : ''
  return `${head({
    title: `${NAME} · Describe the part. Get a verified model.`,
    description: TAGLINE,
    path: '/',
    jsonld: {
      '@context': 'https://schema.org',
      '@graph': [
        ORG,
        {
          '@type': 'SoftwareApplication',
          name: NAME,
          applicationCategory: 'DesignApplication',
          operatingSystem: 'Web, Windows, macOS',
          description: TAGLINE,
          url: SITE,
        },
      ],
    },
  })}
<body class="home">
<div class="door">
  <div id="hero" aria-hidden="true"></div>
  <a class="brand door-brand" href="/"><img src="/logo.png" alt="" width="30" height="30">${NAME}</a>
  <nav class="links mono" aria-label="Main">${NAV.map(([h, l]) => `<a href="${h}">${l}</a>`).join('')}</nav>
  <div class="spec mono" aria-label="What Roshera checks">
    <div><i></i>every dimension re-measured<b>✓</b></div>
    <div><i></i>fits from ISO 286 tables<b>H7</b></div>
    <div><i></i>datums and position verified<b>⌖</b></div>
    <div><i></i>exact distance field<b>SDF</b></div>
  </div>
  <div class="claim">
    <h1>Describe the part.<span>Get a verified model.</span></h1>
    <p>Tell Roshera what you need in plain words, or give it a STEP file. It builds the 3D part and its manufacturing drawing, and checks every dimension against the geometry before you see it.</p>
  </div>
  <div class="contact-card">
    <div class="in">
      <h2><img src="/logo.png" alt="" width="22" height="22">Start a pilot</h2>
      <p>Send us a part you make today, your process and what your drawings must show.</p>
      <label class="mono" for="mail">Write to</label>
      <div class="addr"><span id="mail">${EMAIL}</span></div>
      <a class="go" href="mailto:${EMAIL}?subject=Roshera%20pilot">Email us</a>
    </div>
    <div class="status mono"><span><i></i>Kernel online</span><span>pilots open</span></div>
  </div>
</div>
<main class="wrap">
${PIPELINE}
${PLAYER}
${extra}
${LOOP}
${CERT}
${SHOTS}
<section class="sec"><div><div class="mono kicker">From the blog</div><h3>Engineering notes from building the kernel.</h3><p><a href="/blog">All posts →</a></p></div><div class="cards">${latest}</div></section>
</main>
${footer()}
<script>try{localStorage.setItem("roshera-theme","light")}catch(e){}</script>
<script src="/hero.js" defer></script>
<script src="/site.js" defer></script>
</body>
</html>
`
}

// ── Build ────────────────────────────────────────────────────────────────

if (existsSync(OUT)) rmSync(OUT, { recursive: true })
mkdirSync(OUT, { recursive: true })
const write = (rel, text) => {
  const file = join(OUT, rel)
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, text)
}
const htmlPath = (path) => (path === '/' ? 'index.html' : `${path.slice(1)}/index.html`)
const mdPath = (path) => (path === '/' ? 'index.md' : `${path.slice(1)}.md`)

cpSync(join(ROOT, 'public'), OUT, { recursive: true })

write('index.html', home())
for (const p of pages) {
  const md = `# ${p.meta.title}\n\n${p.body}\n`
  write(mdPath(p.path), md)
  if (p.path === '/') continue
  write(
    htmlPath(p.path),
    page({
      title: `${p.meta.title} · ${NAME}`,
      description: p.meta.description,
      path: p.path,
      html: `<h1>${esc(p.meta.title)}</h1>\n<p class="lede">${esc(p.meta.description)}</p>\n${marked.parse(p.body)}`,
      jsonld: { '@context': 'https://schema.org', '@type': 'WebPage', name: p.meta.title, description: p.meta.description, publisher: ORG },
    }),
  )
}
for (const p of posts) {
  write(mdPath(p.path), `# ${p.meta.title}\n\n${p.meta.date}\n\n${p.body}\n`)
  write(
    htmlPath(p.path),
    page({
      title: `${p.meta.title} · ${NAME}`,
      description: p.meta.description,
      path: p.path,
      type: 'article',
      html: `<p class="mono kicker"><a href="/blog">Blog</a> · ${p.meta.date}</p><h1>${esc(p.meta.title)}</h1>\n<p class="lede">${esc(p.meta.description)}</p>\n${marked.parse(p.body)}\n<p class="end">Questions, or a part you want to try? <a href="mailto:${EMAIL}">${EMAIL}</a></p>`,
      jsonld: {
        '@context': 'https://schema.org',
        '@type': 'BlogPosting',
        headline: p.meta.title,
        description: p.meta.description,
        datePublished: p.meta.date,
        author: ORG,
        publisher: ORG,
        mainEntityOfPage: SITE + p.path,
      },
    }),
  )
}
const blogIndex = posts
  .map((p) => `<a class="card post" href="${p.path}"><span class="mono">${p.meta.date}</span><b>${esc(p.meta.title)}</b><span>${esc(p.meta.description)}</span></a>`)
  .join('\n')
write(
  'blog/index.html',
  page({
    title: `Blog · ${NAME}`,
    description: 'Engineering notes from building a geometry kernel that checks its own work.',
    path: '/blog',
    wide: true,
    html: `<h1>Blog</h1><p class="lede">Engineering notes from building a geometry kernel that checks its own work. Each post is about one real problem, with the numbers.</p><div class="cards list">${blogIndex}</div>`,
    jsonld: { '@context': 'https://schema.org', '@type': 'Blog', name: `${NAME} blog`, publisher: ORG },
  }),
)
write('blog.md', `# ${NAME} blog\n\n${posts.map((p) => `- [${p.meta.title}](${SITE}${p.path}.md) (${p.meta.date}): ${p.meta.description}`).join('\n')}\n`)

// llms.txt: the index for language models (llmstxt.org format).
const section = (items) => items.map((p) => `- [${p.meta.title}](${SITE}${mdPath(p.path) === 'index.md' ? '/index.md' : '/' + mdPath(p.path)}): ${p.meta.description}`).join('\n')
write(
  'llms.txt',
  `# ${NAME}

> ${TAGLINE} Roshera is a native B-Rep geometry kernel with a drawing bench, an event-sourced design timeline and exact signed-distance fields. People use it in a CAD workspace; AI agents use the same commands over REST and the Model Context Protocol. Every result carries a certificate: sheet values are re-measured against the solid, and operations outside what the kernel can verify are refused with a reason.

Contact: ${EMAIL}

## Pages
${section(pages.filter((p) => p.path !== '/'))}

## Blog
${section(posts)}

## Optional
- [Everything on this site in one file](${SITE}/llms-full.txt)
`,
)
write(
  'llms-full.txt',
  [
    `# ${NAME}\n\n> ${TAGLINE}\n\nContact: ${EMAIL}`,
    ...pages.map((p) => `# ${p.meta.title}\n\nURL: ${SITE}${p.path}\n\n${p.body}`),
    ...posts.map((p) => `# ${p.meta.title}\n\nURL: ${SITE}${p.path} (${p.meta.date})\n\n${p.body}`),
  ].join('\n\n---\n\n') + '\n',
)

const today = new Date().toISOString().slice(0, 10)
const urls = ['/', '/blog', ...pages.filter((p) => p.path !== '/').map((p) => p.path), ...posts.map((p) => p.path)]
write(
  'sitemap.xml',
  `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
${urls.map((u) => `  <url><loc>${SITE}${u === '/' ? '' : u}</loc><lastmod>${posts.find((p) => p.path === u)?.meta.date ?? today}</lastmod></url>`).join('\n')}
</urlset>
`,
)
write(
  'rss.xml',
  `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel>
<title>${NAME} blog</title><link>${SITE}/blog</link><description>Engineering notes from building a geometry kernel that checks its own work.</description>
${posts.map((p) => `<item><title>${esc(p.meta.title)}</title><link>${SITE}${p.path}</link><guid>${SITE}${p.path}</guid><pubDate>${new Date(p.meta.date + 'T09:00:00Z').toUTCString()}</pubDate><description>${esc(p.meta.description)}</description></item>`).join('\n')}
</channel></rss>
`,
)
write(
  'robots.txt',
  `# Roshera welcomes search engines and AI crawlers.
User-agent: *
Allow: /

User-agent: GPTBot
Allow: /
User-agent: ClaudeBot
Allow: /
User-agent: Claude-Web
Allow: /
User-agent: anthropic-ai
Allow: /
User-agent: PerplexityBot
Allow: /
User-agent: Google-Extended
Allow: /
User-agent: CCBot
Allow: /

Sitemap: ${SITE}/sitemap.xml
`,
)

console.log(`built ${pages.length} pages and ${posts.length} posts into dist/`)
