// Run: node --experimental-strip-types --test test/ws-schemas.test.mjs
//
// The backend `ServerMessage` enum is `#[serde(tag = "type", content = "data")]`
// (api-server/src/protocol/protocol.rs). Frames below are the exact shapes
// serde emits for the variants the app can receive; a frame the schema
// rejects is dropped at the boundary and the user never sees it.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { serverMessageSchema, serverErrorText } from '../src/lib/ws-schemas.ts'

test('a Rust ServerMessage::Error frame (content = "data") parses', () => {
  const frame = {
    type: 'Error',
    data: {
      error_code: 'PARSE_ERROR',
      message: 'Invalid message format: unknown variant `Command`',
    },
  }
  const r = serverMessageSchema.safeParse(frame)
  assert.ok(r.success, JSON.stringify(r.error?.issues))
  assert.equal(serverErrorText(r.data), 'Invalid message format: unknown variant `Command` (PARSE_ERROR)')
})

test('a Rust Error frame with details and request_id parses', () => {
  const frame = {
    type: 'Error',
    data: {
      error_code: 'auth_required',
      message: 'authentication required',
      details: { hint: 'send Authenticate' },
      request_id: 'r-1',
    },
  }
  assert.ok(serverMessageSchema.safeParse(frame).success)
})

test('the raw SubElementPick Error frame (payload.message) still parses', () => {
  const frame = { type: 'Error', payload: { message: 'SubElementPick(x): no faces' } }
  const r = serverMessageSchema.safeParse(frame)
  assert.ok(r.success)
  assert.equal(serverErrorText(r.data), 'SubElementPick(x): no faces')
})

test('an Error frame carrying no message is still reported, never silent', () => {
  const r = serverMessageSchema.safeParse({ type: 'Error' })
  assert.ok(r.success)
  assert.ok(serverErrorText(r.data).length > 0)
})

test('a Rust ServerMessage::Pong frame keeps its timestamp under data', () => {
  const r = serverMessageSchema.safeParse({ type: 'Pong', data: { timestamp: 42 } })
  assert.ok(r.success)
  assert.equal(r.data.data?.timestamp, 42)
})
