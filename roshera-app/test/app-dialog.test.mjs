import { test } from 'node:test'
import assert from 'node:assert/strict'

const { useAppDialog, confirmDialog, promptDialog, alertDialog, answerDialog } = await import('../src/lib/app-dialog.ts')

const open = () => useAppDialog.getState().current

test('a confirm resolves true when confirmed and false when cancelled', async () => {
  const yes = confirmDialog({ title: 'Delete 3 parts?' })
  assert.equal(open().kind, 'confirm')
  assert.equal(open().title, 'Delete 3 parts?')
  answerDialog(true)
  assert.equal(await yes, true)
  assert.equal(open(), null)

  const no = confirmDialog({ title: 'Delete 3 parts?' })
  answerDialog(false)
  assert.equal(await no, false)
})

test('a prompt resolves the trimmed text, or null when cancelled or left empty', async () => {
  const named = promptDialog({ title: 'Rename', initial: 'bracket' })
  assert.equal(open().kind, 'prompt')
  assert.equal(open().initial, 'bracket')
  answerDialog(true, '  cover plate ')
  assert.equal(await named, 'cover plate')

  const cancelled = promptDialog({ title: 'Rename', initial: 'bracket' })
  answerDialog(false, 'ignored')
  assert.equal(await cancelled, null)

  const empty = promptDialog({ title: 'Rename', initial: 'bracket' })
  answerDialog(true, '   ')
  assert.equal(await empty, null)
})

test('an alert resolves however it is closed', async () => {
  const a = alertDialog({ title: 'Rename failed', body: 'The kernel refused (409).' })
  assert.equal(open().kind, 'alert')
  answerDialog(false)
  await a
  assert.equal(open(), null)
})

test('dialogs asked for at once are shown one at a time, in order', async () => {
  const first = confirmDialog({ title: 'first' })
  const second = confirmDialog({ title: 'second' })
  assert.equal(open().title, 'first')
  answerDialog(true)
  assert.equal(await first, true)
  assert.equal(open().title, 'second')
  answerDialog(false)
  assert.equal(await second, false)
  assert.equal(open(), null)
})

test('answering with nothing open does nothing', () => {
  answerDialog(true)
  assert.equal(open(), null)
})
