/**
 * How a drawing sheet's line work is drawn ON SCREEN (Varun 2026-11-19:
 * "unable to understand what's hidden and what's not").
 *
 * The sheet inks ISO 128 widths -- visible 0.5 mm, hidden 0.25 mm dashed --
 * which is right on paper. At Fit an A1 sheet is drawn at about a third of
 * its size, so both widths fall below a pixel and render as the same black
 * hairline. On screen the strokes are fixed in screen pixels instead
 * (`vector-effect: non-scaling-stroke`): visible edges a firm 1.4 px in ink,
 * hidden edges 0.8 px, grey and clearly dashed, at every zoom. PDF and DXF
 * keep the ISO widths -- this only changes the on-screen preview.
 */
export const SCREEN_INK_CSS =
  '.view polyline, .view circle, .view ellipse { vector-effect: non-scaling-stroke; stroke-width: 1.4px; stroke: #15181d; }' +
  ' .view polyline.hidden, .view circle.hidden, .view ellipse.hidden { vector-effect: non-scaling-stroke; stroke-width: 0.8px; stroke: #8b929c; stroke-dasharray: 6 4; }' +
  ' .view polyline.hatch { vector-effect: non-scaling-stroke; stroke-width: 0.6px; stroke: #6b7280; }'

/** The sheet's SVG with the screen line styles added after its own styles. */
export function withScreenInk(svg: string): string {
  const close = svg.lastIndexOf('</svg>')
  if (close < 0) return svg
  return `${svg.slice(0, close)}<style>${SCREEN_INK_CSS}</style>${svg.slice(close)}`
}
