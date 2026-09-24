// Run: node --experimental-strip-types --test test/client-messages.test.mjs
//
// Wiring checks against the source itself. Two independently maintained
// surfaces must agree: every frame the app sends over the main WebSocket
// must be a variant of the backend `ClientMessage` enum
// (api-server/src/protocol/protocol.rs, `#[serde(tag = "type", content =
// "data")]`), and the production call sites for the transform gizmo, New
// Project, the server Error frame and the transform tools must be the ones
// that go through the kernel.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, dirname, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const appRoot = join(here, '..')
const srcRoot = join(appRoot, 'src')
const protocolRs = join(
  appRoot,
  '..',
  'roshera-backend',
  'api-server',
  'src',
  'protocol',
  'protocol.rs',
)

function read(rel) {
  return readFileSync(join(srcRoot, rel), 'utf8')
}

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) walk(p, out)
    else if (/\.(ts|tsx)$/.test(name)) out.push(p)
  }
  return out
}

function clientMessageVariants() {
  const rs = readFileSync(protocolRs, 'utf8')
  const start = rs.indexOf('pub enum ClientMessage {')
  assert.ok(start >= 0, 'ClientMessage enum not found in protocol.rs')
  // The enum body ends at the first line that is exactly `}`.
  const end = rs.indexOf('\n}', start)
  const body = rs.slice(start, end)
  const variants = new Set()
  for (const m of body.matchAll(/^ {4}([A-Z]\w*)\s*[{(,]/gm)) variants.add(m[1])
  assert.ok(variants.has('GeometryCommand') && variants.has('Ping'), [...variants].join(','))
  return variants
}

test('every WebSocket frame the app sends is a ClientMessage variant with its body under `data`', () => {
  const variants = clientMessageVariants()
  const offenders = []
  let seen = 0
  for (const file of walk(srcRoot)) {
    const text = readFileSync(file, 'utf8')
    const re = /\.send\(\s*(?:JSON\.stringify\(\s*)?\{\s*type:\s*'(\w+)'\s*,\s*(\w+)\s*:/g
    for (const m of text.matchAll(re)) {
      seen += 1
      const [, type, bodyKey] = m
      if (!variants.has(type) || bodyKey !== 'data') {
        offenders.push(`${relative(appRoot, file)}: type '${type}', body key '${bodyKey}'`)
      }
    }
  }
  assert.ok(seen >= 3, `expected to find the Authenticate/Ping/Query sends, found ${seen}`)
  assert.deepEqual(offenders, [])
})

test('the transform gizmo commits through the kernel transform route, not the WebSocket', () => {
  const src = read('components/viewport/TransformGizmo.tsx')
  assert.match(src, /from '@\/lib\/gizmo-commit'/)
  // Every commit goes through the session, which refuses a drag composed on
  // a mesh the kernel has already moved past.
  assert.match(src, /createGizmoSession\(/)
  assert.match(src, /\.drag\(/)
  assert.doesNotMatch(src, /commitGizmoDrag\(/)
  assert.doesNotMatch(src, /wsClient/)
})

test('the gizmo listeners attach when the gizmo mounts, and one commit runs at a time', () => {
  const src = read('components/viewport/TransformGizmo.tsx')
  const start = src.indexOf("addEventListener('mouseUp'")
  assert.ok(start >= 0)
  // The dependency array of the effect that registers the listeners.
  const deps = src.slice(start).match(/\}, \[([^\]]*)\]\)/)
  assert.ok(deps, 'listener effect dependency array not found')
  assert.match(deps[1], /\bgizmoMounted\b/)
  const up = src.slice(src.indexOf('const handleMouseUp'), start)
  const disable = up.indexOf('controls.enabled = false')
  assert.ok(disable >= 0 && disable < up.indexOf('.drag('))
  // Re-armed only from the session's verdict, never unconditionally.
  assert.doesNotMatch(src, /controls\.enabled = true/)
  assert.match(up, /\.finally\(\(\) => \{\s*rearmMountedGizmo\?\.\(\)/)
  assert.match(src, /controls\.enabled = !session\.isBusy\(\) && !session\.isStale\(selectedId\)/)
})

test("document create/open failures carry the backend's refusal text, not a bare status", () => {
  const src = read('lib/documents-api.ts')
  for (const fn of ['createDocument', 'openDocument']) {
    const start = src.indexOf(`export async function ${fn}`)
    const block = src.slice(start, src.indexOf('\n}', start))
    assert.match(block, /throw await refusal\(/, fn)
    assert.doesNotMatch(block, /\$\{resp\.status\}/, fn)
  }
})

function commandBlock(src, id) {
  const start = src.indexOf(`id: '${id}'`)
  assert.ok(start >= 0, `command ${id} not found`)
  const next = src.indexOf('cmds.push(', start)
  return src.slice(start, next < 0 ? undefined : next)
}

test('palette New Project opens a backend document and does not clear the scene first', () => {
  const block = commandBlock(read('components/CommandPalette.tsx'), 'file.new')
  assert.match(block, /runNewProject\(/)
  assert.doesNotMatch(block, /clearScene\(/)
})

test('TopBar New Project reports the backend refusal, not a blanket "unreachable"', () => {
  const src = read('components/layout/TopBar.tsx')
  const start = src.indexOf('const handleNewProject')
  const block = src.slice(start, src.indexOf('}, [])', start))
  assert.match(block, /runNewProject\(/)
})

test('a server Error frame is shown to the user, not only logged', () => {
  const src = read('lib/ws-bridge.ts')
  const start = src.indexOf("case 'Error':")
  assert.ok(start >= 0)
  const block = src.slice(start, src.indexOf('break', start))
  assert.match(block, /serverErrorText\(/)
  assert.match(block, /addLine\(/)
})

test('there is no scale tool: the kernel transform route carries no scale', () => {
  assert.doesNotMatch(read('stores/scene-store.ts'), /export type TransformTool = [^\n]*'scale'/)
  assert.doesNotMatch(read('lib/shortcuts.ts'), /setActiveTool\('scale'\)/)
  assert.doesNotMatch(read('components/layout/ToolBar.tsx'), /handleToolChange\('scale'\)/)
  assert.doesNotMatch(read('components/CommandPalette.tsx'), /\['scale',/)
  // The properties panel shows scale but cannot set it.
  const panel = read('components/panels/PropertiesPanel.tsx')
  assert.doesNotMatch(panel, /handleChange\('scale'/)
  const scl = panel.slice(panel.indexOf('label="Scl"'), panel.indexOf('/>', panel.indexOf('label="Scl"')))
  assert.doesNotMatch(scl, /onChange=/)
  assert.match(panel, /readOnly=\{!onChange\}/)
})

test('an ObjectCreated for a part the scene already has is merged, not announced', () => {
  const src = read('lib/ws-bridge.ts')
  const start = src.indexOf("case 'ObjectCreated':")
  const block = src.slice(start, src.indexOf('break', start))
  assert.match(block, /mergeObjectCreated\(/)
  assert.doesNotMatch(block, /queueUpsert\([^,]+,\s*true/)
})
