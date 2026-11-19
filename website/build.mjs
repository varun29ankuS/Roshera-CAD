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

import { readFileSync, writeFileSync, mkdirSync, readdirSync, copyFileSync, rmSync, existsSync } from 'node:fs'
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
${extra}
<section class="sec"><div><div class="mono kicker">From the blog</div><h3>Engineering notes from building the kernel.</h3><p><a href="/blog">All posts →</a></p></div><div class="cards">${latest}</div></section>
</main>
${footer()}
<script>try{localStorage.setItem("roshera-theme","light")}catch(e){}</script>
<script src="/hero.js" defer></script>
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

for (const f of readdirSync(join(ROOT, 'public'))) copyFileSync(join(ROOT, 'public', f), join(OUT, f))

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
