/**
 * The URL of a drawing's SVG.
 *
 * `review: true` is the on-screen sheet: the server's preview route
 * (`preview.svg`) renders the sheet as it stands, gates nothing, records
 * nothing, and serves the same bytes again until the sheet changes. Its
 * layout and soundness findings are read separately and listed beside it in
 * the sheet panel. Through the export route every redraw took 7-8 s and
 * recorded an export in the history (2026-11-18).
 *
 * `review: false` is a download: the export route brings the sheet up to
 * date, certifies it, and refuses until the layout passes. Neither ever
 * sends `acknowledge_unsound`.
 */
export function drawingSvgUrl(apiHost: string, id: string, opts: { review: boolean }): string {
  const base = `${apiHost}/api/drawings/${encodeURIComponent(id)}`
  return opts.review ? `${base}/preview.svg` : `${base}/svg`
}
