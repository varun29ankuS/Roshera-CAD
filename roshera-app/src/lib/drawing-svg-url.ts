/**
 * The URL of a drawing's SVG.
 *
 * `review: true` is the on-screen preview: it carries the server's
 * documented draft-for-human-review escape (`acknowledge_layout_issues`),
 * so a sheet whose LAYOUT check fails (label collisions, redundant
 * dimensions) is still shown, with its findings listed beside it in the
 * sheet panel. `review: false` is a download: the server refuses until the
 * layout passes. Neither ever bypasses soundness (`acknowledge_unsound`): a
 * sheet that disagrees with the model is refused everywhere.
 */
export function drawingSvgUrl(apiHost: string, id: string, opts: { review: boolean }): string {
  const base = `${apiHost}/api/drawings/${encodeURIComponent(id)}/svg`
  return opts.review ? `${base}?acknowledge_layout_issues=true` : base
}
