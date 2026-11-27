/**
 * Applies the plot (lib/sheet-plot.ts) to a sheet's SVG.
 *
 * Every mark is given its moment (`--plot-at`) and its kind (`data-plot`);
 * the keyframes in index.css do the drawing, so nothing runs per frame in
 * script. A stroked mark is given `pathLength="1"` so one dash pattern draws
 * any line, long or short. When the plot ends -- or the moment a key or a
 * pointer is pressed -- every attribute it added is taken off again, and the
 * SVG is exactly the kernel's markup.
 */

import { PLOT_TOTAL_MS, plotAt, plotKind, plotLayer, plotPlan, type PlotLayer, type PlotMark } from './sheet-plot.ts'

const DRAWN = 'rect,line,polyline,polygon,circle,ellipse,path,text,image'

function describe(el: Element): PlotMark {
  return {
    tag: el.tagName.toLowerCase(),
    classes: Array.from(el.classList),
    inView: el.closest('g.view') !== null,
    inTitleBlock: el.closest('g.title-block') !== null,
    inZones: el.closest('g.zones') !== null,
  }
}

/** Plot `svg` now. Returns a function that finishes it at once (safe to call
 *  more than once, and after it has finished by itself). */
export function plotSheet(svg: SVGSVGElement): () => void {
  const plan = plotPlan()
  const byLayer = new Map<PlotLayer, { el: SVGElement; kind: 'stroke' | 'show' }[]>()
  for (const node of Array.from(svg.querySelectorAll<SVGElement>(DRAWN))) {
    // A <text>'s own <tspan>s arrive with it.
    const mark = describe(node)
    const layer = plotLayer(mark)
    if (layer === null) continue
    const list = byLayer.get(layer) ?? []
    list.push({ el: node, kind: plotKind(mark) })
    byLayer.set(layer, list)
  }

  const touched: { el: SVGElement; addedPathLength: boolean }[] = []
  for (const [layer, marks] of byLayer) {
    marks.forEach(({ el, kind }, k) => {
      const { atMs, strokeMs } = plotAt(plan[layer], k, marks.length)
      let addedPathLength = false
      if (kind === 'stroke' && !el.hasAttribute('pathLength')) {
        el.setAttribute('pathLength', '1')
        addedPathLength = true
      }
      el.style.setProperty('--plot-at', `${atMs}ms`)
      el.style.setProperty('--plot-ms', `${strokeMs}ms`)
      el.setAttribute('data-plot', kind)
      touched.push({ el, addedPathLength })
    })
  }

  let finished = false
  const finish = () => {
    if (finished) return
    finished = true
    window.clearTimeout(timer)
    window.removeEventListener('pointerdown', finish, true)
    window.removeEventListener('keydown', finish, true)
    for (const { el, addedPathLength } of touched) {
      el.removeAttribute('data-plot')
      el.style.removeProperty('--plot-at')
      el.style.removeProperty('--plot-ms')
      if (el.getAttribute('style') === '') el.removeAttribute('style')
      if (addedPathLength) el.removeAttribute('pathLength')
    }
  }
  const timer = window.setTimeout(finish, PLOT_TOTAL_MS + 60)
  window.addEventListener('pointerdown', finish, true)
  window.addEventListener('keydown', finish, true)
  return finish
}
