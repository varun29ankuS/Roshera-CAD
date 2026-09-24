/**
 * `New Project`, shared by the File menu and the command palette.
 *
 * A new project is a new backend document (`documents-api.ts::newDocument`:
 * create, open, reload). Nothing local is cleared first: the page reloads
 * onto the empty document only after the backend has switched to it, so a
 * refusal leaves the current scene on screen and says why.
 *
 * Pure module (dependencies passed in) so it runs under `node --test`.
 */
export async function runNewProject(
  newDocument: () => Promise<void>,
  report: (line: string) => void,
): Promise<boolean> {
  try {
    await newDocument()
    return true
  } catch (err) {
    // `fetch` rejects with a TypeError when the request never reached a
    // server; an HTTP refusal arrives as an Error carrying the server's text.
    const reason =
      err instanceof TypeError || !(err instanceof Error) || !err.message
        ? 'backend unreachable'
        : err.message.replace(/\.+$/, '')
    report(`New document failed: ${reason}. The current document is still open.`)
    return false
  }
}
