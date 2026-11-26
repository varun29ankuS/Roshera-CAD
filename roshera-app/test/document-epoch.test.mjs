// Run: node --experimental-strip-types --test test/document-epoch.test.mjs
//
// The document store's `epoch` is what every panel that loads itself from
// the server watches (history, tree, units, the Drawing workspace). It was
// bumped by hand in the tab-switch path only. When an agent opened another
// document over MCP, the list re-read (`refresh`) entered the new document
// -- the history caches emptied -- but `epoch` stayed, so no panel reloaded:
// the Drawing workspace kept the previous document's sheets (2026-11-26).
//
// The rule now: the open document changes in ONE place, `enterDocument`
// (lib/document-scope), whoever moved it; `epoch` is a count of the changes
// that place announces and is written nowhere else.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, dirname, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { currentDocument, enterDocument, onDocumentChange } from '../src/lib/document-scope.ts'

const here = dirname(fileURLToPath(import.meta.url))
const src = join(here, '..', 'src')
const read = (...p) => readFileSync(join(src, ...p), 'utf8').replace(/\r\n/g, '\n')
const store = read('stores', 'document-store.ts')

// One process per test file: the scope starts with no document.
test('learning which document is open is not a change; every move after it is', () => {
  const heard = []
  const stop = onDocumentChange((id, entry) => heard.push([id, entry]))
  // An epoch kept the way the store keeps it.
  let epoch = 0
  const stopCount = onDocumentChange((_id, entry) => {
    if (entry === 'switched') epoch += 1
  })

  assert.equal(currentDocument(), null)
  // The list loads: the server has doc-a open. Nothing on screen was about
  // another document, so nothing starts over.
  assert.equal(enterDocument('doc-a'), 'learned')
  assert.equal(epoch, 0)
  // The list is read again; nothing moved.
  assert.equal(enterDocument('doc-a'), 'same')
  assert.equal(epoch, 0)
  // The list is read again and the server is in doc-b: an agent moved it.
  // This is `refresh`'s call -- no `reopened`, no tab clicked.
  assert.equal(enterDocument('doc-b'), 'switched')
  assert.equal(epoch, 1, 'a document moved under the app reloads the panels')
  // A tab clicked here (`switchTo`), also onto the document already open:
  // its history was replayed.
  assert.equal(enterDocument('doc-c', { reopened: true }), 'switched')
  assert.equal(enterDocument('doc-c', { reopened: true }), 'switched')
  assert.equal(epoch, 3)

  assert.deepEqual(heard, [
    ['doc-a', 'learned'],
    ['doc-b', 'switched'],
    ['doc-c', 'switched'],
    ['doc-c', 'switched'],
  ])
  stop()
  stopCount()
})

test('the epoch is written in one place: where the scope announces a change', () => {
  const writes = [...store.matchAll(/epoch: state\.epoch \+ 1|epoch: s\.epoch \+ 1|epoch: get\(\)\.epoch \+ 1/g)]
  assert.equal(writes.length, 1, 'one writer')
  const foot = store.slice(store.lastIndexOf('onDocumentChange('))
  assert.ok(
    /^onDocumentChange\(\(_documentId, entry\) => \{\s*if \(entry === 'switched'\) useDocumentStore\.setState\(\(state\) => \(\{ epoch: state\.epoch \+ 1 \}\)\)\s*\}\)\s*$/.test(
      foot,
    ),
    'the scope subscription moves it, on a switch only',
  )
  // No action sets it: the store's own body mentions it once, as 0.
  const body = store.slice(store.indexOf('create<DocumentStoreState>('), store.lastIndexOf('onDocumentChange('))
  assert.deepEqual(body.match(/\bepoch\b[^\n]*/g)?.filter((l) => /epoch\s*:/.test(l)), ['epoch: 0,'])
})

test('a list re-read that finds the server elsewhere follows it', () => {
  const s = store.indexOf('refresh: async () => {')
  const refresh = store.slice(s, store.indexOf('switchTo: async', s))
  assert.ok(
    /if \(active && !get\(\)\.switchingId && enterDocument\(active\.id\) === 'switched'\) \{\s*await loadDocumentContents\(\)/.test(
      refresh,
    ),
    'the scene and the notebook are loaded as a tab switch loads them',
  )
  const load = store.slice(store.indexOf('function loadDocumentContents()'))
  assert.ok(/Promise\.all\(\[refreshSceneFromServer\(\), syncNotebook\(\)\]\)/.test(load.slice(0, 200)))
  // The tab switch uses the same loader and the same entry.
  const sw = store.slice(store.indexOf('switchTo: async'), store.indexOf('createAndSwitch: async'))
  assert.ok(sw.includes('enterDocument(id, { reopened: true })'))
  assert.ok(sw.includes('loadDocumentContents()'))
  assert.ok(
    sw.indexOf('active: d.id === id') < sw.indexOf('enterDocument(id'),
    'the tab is marked before the epoch moves: a panel reloading on it finds this document active',
  )
})

function sourceFiles(dir) {
  const out = []
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) out.push(...sourceFiles(p))
    else if (/\.(ts|tsx)$/.test(name)) out.push(p)
  }
  return out
}

test('every path that changes the open document goes through the store or reloads the page', () => {
  const files = sourceFiles(src)
  const rel = (p) => relative(src, p).split(sep).join('/')
  const calling = (re) => files.filter((p) => re.test(readFileSync(p, 'utf8'))).map(rel).sort()
  // Asking the server to open a document: the store, and the api module
  // that defines the call (whose own `newDocument` reloads the page).
  assert.deepEqual(calling(/\bopenDocument\(/), ['lib/documents-api.ts', 'stores/document-store.ts'])
  const api = read('lib', 'documents-api.ts')
  const fresh = api.slice(api.indexOf('export async function newDocument('))
  assert.ok(/await openDocument\(doc\.id\)\s*window\.location\.reload\(\)/.test(fresh))
  // Entering a document in the app: the store only.
  assert.deepEqual(calling(/\benterDocument\(/), ['lib/document-scope.ts', 'stores/document-store.ts'])
})

test('the app asks which document is open when someone else may have moved it', () => {
  const tabs = read('components', 'layout', 'DocumentTabs.tsx')
  const s = tabs.indexOf('void refresh()')
  const effect = tabs.slice(s, tabs.indexOf('}, [refresh])', s))
  assert.ok(effect.includes("window.addEventListener('roshera:agent-turn-ended', ask)"), "an agent's turn ended")
  assert.ok(effect.includes("document.addEventListener('visibilitychange', onVisible)"), 'the window is looked at again')
  assert.ok(effect.includes("window.removeEventListener('roshera:agent-turn-ended', ask)"))
  assert.ok(effect.includes("document.removeEventListener('visibilitychange', onVisible)"))
})
