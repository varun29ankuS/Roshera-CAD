// Rebuild public/hero.js from the app's own sign-in hero, so the website's
// front door and the app's sign-in page stay one drawing.
//
// Run from website/:  npm run hero
// Needs roshera-app's dependencies installed (it uses the app's Vite).
// The output is committed: Vercel serves public/hero.js as is.

import { writeFileSync, rmSync, existsSync, mkdirSync } from 'node:fs'
import { join, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'

const here = dirname(fileURLToPath(import.meta.url))
const app = resolve(here, '..', '..', 'roshera-app')
const outDir = resolve(here, '..', 'public')
const entry = join(app, 'src', '_site-hero-entry.tsx')
const config = join(app, '_vite.hero.config.mjs')

writeFileSync(
  entry,
  `import { createRoot } from 'react-dom/client'
import { createElement } from 'react'
import { SignInHero } from '@/components/SignInHero'
const el = document.getElementById('hero')
if (el) createRoot(el).render(createElement(SignInHero))
`,
)
writeFileSync(
  config,
  `import { defineConfig } from 'vite'
import path from 'node:path'
export default defineConfig({
  resolve: { alias: { '@': path.resolve(process.cwd(), 'src') } },
  define: { 'process.env.NODE_ENV': '"production"' },
  logLevel: 'warn',
  build: {
    outDir: ${JSON.stringify(outDir.replace(/\\/g, '/'))},
    emptyOutDir: false,
    copyPublicDir: false,
    lib: { entry: path.resolve(process.cwd(), 'src/_site-hero-entry.tsx'), name: 'RosheraHero', formats: ['iife'], fileName: () => 'hero.js' },
  },
})
`,
)
try {
  if (!existsSync(outDir)) mkdirSync(outDir, { recursive: true })
  execFileSync(process.execPath, [join(app, 'node_modules', 'vite', 'bin', 'vite.js'), 'build', '--config', config], {
    cwd: app,
    stdio: 'inherit',
  })
  console.log(`hero.js written to ${outDir}`)
} finally {
  rmSync(entry, { force: true })
  rmSync(config, { force: true })
}
