// An ACP connection the app is done with is released on the backend.
//
// The backend's transport ends a connection only on `DELETE /acp`: a dropped
// event stream does not end it and there is no idle timeout. Every reload,
// closed tab or rebuild used to leave its connection -- and the agent's MCP
// server process -- alive until the server died. The client now says DELETE
// when it closes, when it rebuilds a stale connection, and when the page is
// hidden for good (`pagehide`).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { AcpClient, installPageHideRelease } from '../src/lib/acp-client.ts'

/** Replace global fetch with a recorder; `respond` decides each answer. */
function recordFetch(respond = () => new Response(null, { status: 204 })) {
  const calls = []
  const original = globalThis.fetch
  globalThis.fetch = async (input, init = {}) => {
    calls.push({ input: String(input), init })
    return respond(String(input), init)
  }
  return { calls, restore: () => (globalThis.fetch = original) }
}

const deletes = (calls) => calls.filter((c) => (c.init.method ?? 'GET') === 'DELETE')

test('close() releases the connection with DELETE /acp, keepalive, carrying its id', () => {
  const f = recordFetch()
  try {
    const client = new AcpClient({ acpPath: '/acp', cwd: '/w' })
    client.connectionId = 'conn-1'
    client.close()
    const sent = deletes(f.calls)
    assert.equal(sent.length, 1, 'exactly one DELETE')
    assert.equal(sent[0].input, '/acp')
    assert.equal(sent[0].init.headers['Acp-Connection-Id'], 'conn-1')
    assert.equal(sent[0].init.keepalive, true, 'keepalive, so it survives page unload')
  } finally {
    f.restore()
  }
})

test('close() before any connection exists sends nothing', () => {
  const f = recordFetch()
  try {
    new AcpClient({ acpPath: '/acp', cwd: '/w' }).close()
    assert.equal(f.calls.length, 0)
  } finally {
    f.restore()
  }
})

test('a failed release never throws out of close()', () => {
  const original = globalThis.fetch
  globalThis.fetch = () => Promise.reject(new Error('offline'))
  try {
    const client = new AcpClient({ acpPath: '/acp', cwd: '/w' })
    client.connectionId = 'conn-x'
    assert.doesNotThrow(() => client.close())
  } finally {
    globalThis.fetch = original
  }
})

test('rebuilding a stale connection releases the old id before initializing a new one', async () => {
  const f = recordFetch((input, init) =>
    (init.method ?? 'GET') === 'DELETE'
      ? new Response(null, { status: 404 })
      : new Response('backend down', { status: 503 }),
  )
  try {
    const client = new AcpClient({ acpPath: '/acp', cwd: '/w' })
    client.connectionId = 'old-conn'
    client.sessionId = 'old-sess'
    await assert.rejects(client.reestablish())
    const firstDelete = f.calls.findIndex((c) => (c.init.method ?? 'GET') === 'DELETE')
    const firstPost = f.calls.findIndex((c) => c.init.method === 'POST')
    assert.ok(firstDelete >= 0, 'the old connection was released')
    assert.equal(f.calls[firstDelete].init.headers['Acp-Connection-Id'], 'old-conn')
    assert.ok(firstPost < 0 || firstDelete < firstPost, 'released before the new initialize')
  } finally {
    f.restore()
  }
})

test('pagehide releases the shared client; other events do not', () => {
  const listeners = new Map()
  const target = { addEventListener: (type, fn) => listeners.set(type, fn) }
  let released = 0
  installPageHideRelease(target, () => released++)
  assert.ok(listeners.has('pagehide'), 'listens for pagehide')
  listeners.get('pagehide')({ persisted: false })
  assert.equal(released, 1)
})

test('the app installs the pagehide release on its shared agent client', async () => {
  const { readFileSync } = await import('node:fs')
  const src = readFileSync(new URL('../src/lib/acp-blackboard.ts', import.meta.url), 'utf8')
  assert.match(src, /installPageHideRelease\(\s*window\s*,/, 'acp-blackboard wires pagehide to the shared client')
  assert.match(src, /resetAcpClient\(\)/, 'the release resets (closes) the shared client')
})
