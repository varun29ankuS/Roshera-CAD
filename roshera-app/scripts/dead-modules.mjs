#!/usr/bin/env node
/**
 * DEAD MODULE CHECK
 * =================
 * Fails when a module under `src/` is imported by nothing. `tsc` will not
 * catch this: an unreferenced file still typechecks perfectly, so dead code
 * accumulates silently and is only ever found by someone remembering to look.
 * This makes it a check instead of a habit.
 *
 * Detection is by specifier, not by symbol: a file is live if some other
 * module imports its path. Entry points and type-only declaration files are
 * roots by definition and are exempt.
 *
 *   node scripts/dead-modules.mjs        # exits 1 if anything is unreferenced
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative, basename, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const SRC = join(fileURLToPath(new URL('..', import.meta.url)), 'src')

/** Roots: nothing imports these, and nothing should. */
const ENTRY_POINTS = new Set(['main.tsx', 'vite-env.d.ts'])

function walk(dir) {
  const out = []
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    if (statSync(full).isDirectory()) out.push(...walk(full))
    else if (/\.(ts|tsx)$/.test(entry)) out.push(full)
  }
  return out
}

const files = walk(SRC)
const sources = new Map(files.map((f) => [f, readFileSync(f, 'utf8')]))

const dead = []
for (const file of files) {
  const name = basename(file)
  if (ENTRY_POINTS.has(name) || name.endsWith('.d.ts')) continue

  // The specifier others would write: the basename without extension. Index
  // modules are additionally reachable by their directory name.
  const stem = name.replace(/\.(tsx?|d\.ts)$/, '')
  const dirName = basename(join(file, '..'))
  const candidates = stem === 'index' ? [stem, dirName] : [stem]

  const referenced = [...sources].some(([other, text]) => {
    if (other === file) return false
    return candidates.some((c) =>
      new RegExp(`(from|import\\()\\s*['"][^'"]*[/']${c}['"]`).test(text),
    )
  })

  if (!referenced) dead.push(relative(SRC, file).split(sep).join('/'))
}

if (dead.length > 0) {
  console.error(`dead-modules: ${dead.length} module(s) under src/ are imported by nothing:\n`)
  for (const d of dead) console.error(`  src/${d}`)
  console.error(
    '\nDelete them, or wire them up. If one is a deliberate entry point, add it to ENTRY_POINTS in scripts/dead-modules.mjs.',
  )
  process.exit(1)
}

console.log(`dead-modules: clean — every module under src/ is reachable (${files.length} checked).`)
