/**
 * LAYOUT AUDIT — structural UI defects, measured across widths and states.
 *
 * WHY THIS EXISTS. Every layout regression in the docked-rail work was found
 * by a human looking at one state after it shipped: chips clipped at 224px but
 * not 236, a render that became a second viewport at 560 but was fine at 224,
 * five readout items flung across a panel that had been compact a width ago,
 * a collapsed bar stranded at the top of an empty column. Each was a real
 * defect and each was found the same way — the founder looked, and said so.
 *
 * They are not taste failures. They are UNMEASURED CASES, and cases are
 * countable: a component lives at N widths × M states, and shipping after
 * checking one of them is how the same complaint arrives repeatedly.
 *
 * This walks the cases and reports STRUCTURE, never opinion. It cannot tell
 * you a panel is ugly. It can tell you that text is being destroyed with
 * nothing admitting it, that something sits past its container's edge, that a
 * row is distributing peers across a void, that a panel covers the geometry
 * the app exists to show, or that type has fallen through the floor.
 *
 * Run it against the dev server with Chrome DevTools attached; `PROBE` is the
 * expression to evaluate in the page, and `CASES` names the states worth
 * driving. Deliberately dependency-free so it can be pasted into any console.
 */

/** Below this, type is unreadable. The floor is 11px; 10.5 allows rounding. */
export const TYPE_FLOOR_PX = 10.5;

/**
 * A flex row is "distributing peers" when the widest gap between adjacent
 * children exceeds both the widest child and this many pixels. Under it, a
 * `justify-between` row reads as a normal control cluster.
 */
export const VOID_GAP_PX = 64;

export const PROBE = `(() => {
  const FLOOR = ${TYPE_FLOOR_PX};
  const VOID = ${VOID_GAP_PX};
  const out = { clipped: [], overflow: [], voids: [], verticalVoids: [], occluding: [], tiny: [] };
  const canvas = document.querySelector('canvas');
  const cr = canvas ? canvas.getBoundingClientRect() : null;
  const label = (el) =>
    (el.getAttribute('aria-label') || el.title || (el.textContent || '').trim()).slice(0, 44);

  for (const el of document.querySelectorAll('div,span,button,dl,p,input,a')) {
    const r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) continue;
    const cs = getComputedStyle(el);

    // Text cut with nothing admitting it. An ellipsis or a tooltip is an
    // honest cut; overflow:hidden with neither silently destroys characters,
    // which is how six parts rendered as two strings in the model tree.
    // Visually-hidden text (sr-only) is clipped to a 1px box ON PURPOSE so a
    // screen reader still reaches it. It is the opposite of a defect, and it
    // trips every naive scrollWidth check — this probe reported a correctly
    // hidden rail label as destroyed text until that was measured.
    const srOnly = el.clientWidth <= 1 || el.clientHeight <= 1;
    if (!srOnly && el.children.length === 0 && el.scrollWidth > el.clientWidth + 2) {
      const admits = cs.textOverflow === 'ellipsis' || !!el.title || !!el.closest('[title]');
      if (!admits) out.clipped.push({ text: label(el), width: Math.round(r.width), needs: el.scrollWidth });
    }

    // Past the container's right edge — the camera chips at 224px.
    const host = el.offsetParent;
    if (host) {
      const hr = host.getBoundingClientRect();
      if (r.right > hr.right + 1 && getComputedStyle(host).overflow !== 'visible') {
        out.overflow.push({ text: label(el), by: Math.round(r.right - hr.right) });
      }
    }

    // Peers distributed across a void. dt/dd pairs are EXCLUDED: a label
    // against its value is two different roles spanning one column — a spec
    // sheet — and bookending is correct there. The defect is same-kind
    // siblings (tabs, glyph buttons, stat chips) pushed to opposite edges.
    //
    // Only gaps BETWEEN siblings count. Trailing slack after the last child is
    // not a void — every left-aligned toolbar has it, and a detector that
    // cannot tell the difference reports the fix for this defect as a new
    // instance of it.
    // dl is the semantic form; data-role="pair" is the declaration for a
    // label/value row not marked up as one. Bookending is correct in both, and
    // the probe cannot infer intent from a plain div, so it is stated.
    const inPair = !!el.closest('dl,[data-role=pair]');
    const kids = [...el.children].map((k) => k.getBoundingClientRect()).filter((k) => k.width > 0 && k.height > 0);
    const isRow = cs.flexDirection.startsWith('row');
    if (!inPair && cs.display === 'flex' && isRow && cs.justifyContent === 'space-between' && kids.length >= 2) {
      let gap = 0;
      for (let i = 1; i < kids.length; i++) gap = Math.max(gap, kids[i].left - kids[i - 1].right);
      const widest = Math.max(...kids.map((k) => k.width));
      if (gap > Math.max(widest, VOID)) out.voids.push({ text: label(el), gap: Math.round(gap) });
    }

    // The same walker, rotated. A panel tightened horizontally tends to
    // accumulate vertical rot — a column scattering three fields down a tall
    // panel, a spacer nobody owns — and a row-only detector is blind to the
    // entire axis.
    //
    // A LAST child sitting flush against the container's bottom edge is a
    // deliberate anchor (a footer bar, a settings button at the foot of a nav
    // rail), and the gap above it is the anchor working, not rot.
    //
    // Detected structurally rather than by reading margin-top:auto, because
    // getComputedStyle RESOLVES that to a pixel value — an mt-auto footer
    // reports marginTop "285px", never "auto", so a check for the keyword
    // silently never fires. This probe called the toolbar's pinned button a
    // 285px defect until that was measured.
    //
    // Feeds are exempt for the same reason: trailing emptiness in a
    // transcript is expected.
    if (
      cs.display === 'flex' &&
      cs.flexDirection.startsWith('column') &&
      kids.length >= 2 &&
      !el.closest('[data-role=feed]')
    ) {
      const children = [...el.children].filter((c) => {
        const b = c.getBoundingClientRect();
        return b.width > 0 && b.height > 0;
      });
      const sorted = children.sort(
        (a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top,
      );
      const floor = r.bottom - (parseFloat(cs.paddingBottom) || 0);
      let gap = 0;
      for (let i = 1; i < sorted.length; i++) {
        const kb = sorted[i].getBoundingClientRect();
        const pinnedFooter = i === sorted.length - 1 && Math.abs(kb.bottom - floor) <= 2;
        if (pinnedFooter) continue; // the anchor earns its gap
        gap = Math.max(gap, kb.top - sorted[i - 1].getBoundingClientRect().bottom);
      }
      if (gap > Math.max(48, r.height * 0.2)) {
        out.verticalVoids.push({ text: label(el), gap: Math.round(gap), height: Math.round(r.height) });
      }
    }

    // Covering the model. \`pointer-events: none\` decorations are skipped —
    // a frame or a hint overlay is not occlusion — as is the canvas's own
    // wrapper, which IS the viewport rather than something on top of it.
    if (
      cr &&
      (cs.position === 'absolute' || cs.position === 'fixed') &&
      cs.pointerEvents !== 'none' &&
      r.width > 200 &&
      r.height > 150 &&
      !el.contains(canvas)
    ) {
      const ox = Math.min(r.right, cr.right) - Math.max(r.left, cr.left);
      const oy = Math.min(r.bottom, cr.bottom) - Math.max(r.top, cr.top);
      if (ox > 40 && oy > 40) out.occluding.push({ text: label(el), area: Math.round(ox * oy) });
    }

    // Through the type floor.
    if (el.children.length === 0 && (el.textContent || '').trim()) {
      const px = parseFloat(cs.fontSize);
      if (px && px < FLOOR) out.tiny.push({ text: label(el), px });
    }
  }

  const dedupe = (a) =>
    [...new Map(a.map((x) => [x.text + ':' + (x.gap ?? x.by ?? x.px ?? x.area ?? ''), x])).values()];
  const rail = document.querySelector('[role=separator][aria-orientation=horizontal]');
  return {
    railWidth: rail ? Math.round(rail.parentElement.getBoundingClientRect().width) : null,
    clipped: dedupe(out.clipped).slice(0, 12),
    overflow: dedupe(out.overflow).slice(0, 12),
    voids: dedupe(out.voids).sort((a, b) => b.gap - a.gap).slice(0, 12),
    verticalVoids: dedupe(out.verticalVoids).sort((a, b) => b.gap - a.gap).slice(0, 12),
    occluding: dedupe(out.occluding).sort((a, b) => b.area - a.area).slice(0, 8),
    tiny: dedupe(out.tiny).slice(0, 8),
  };
})()`;

/**
 * The states worth driving, and how. Each is a real configuration a user
 * reaches, not a synthetic one — the rail's two widths crossed with whether
 * anything is selected, plus the collapsed panels, because collapsed × empty
 * is where a bar stranded itself at the top of an empty column.
 *
 * `verify` must be checked after driving: a case that silently failed to
 * change state reports the previous case's numbers, and three identical
 * results look like a clean sweep rather than a broken harness.
 */
export const CASES = [
  {
    name: 'idle — nothing selected, board collapsed, rail narrow',
    drive: `(() => {
      const m = [...document.querySelectorAll('button')].find(b => b.getAttribute('aria-label') === 'Minimize Blackboard');
      if (m) m.click();
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    })()`,
    verify: `!document.querySelector('button[aria-label="Minimize Blackboard"]')`,
  },
  {
    name: 'board open — rail wide',
    drive: `(() => {
      const s = [...document.querySelectorAll('button')].find(b => b.getAttribute('aria-label') === 'Open Blackboard');
      if (s) s.click();
    })()`,
    verify: `!!document.querySelector('button[aria-label="Minimize Blackboard"]')`,
  },
  {
    name: 'selection + board open — all three tenants live',
    drive: `(() => {
      const el = [...document.querySelectorAll('*')].find(e => e.children.length === 0 && /^solid_/.test((e.textContent||'').trim()));
      if (!el) return;
      let n = el;
      for (let i = 0; i < 6 && n; i++) { if (/cursor-pointer/.test(n.className || '')) break; n = n.parentElement; }
      (n || el).click();
    })()`,
    verify: `!!document.querySelector('dl')`,
  },
  {
    name: 'agent view collapsed — the empty-column case',
    drive: `(() => {
      const m = [...document.querySelectorAll('button')].find(b => b.title === 'Minimize' && (b.innerText||'').trim() === '▁');
      if (m) m.click();
    })()`,
    verify: `!!document.querySelector('button[aria-label="Open the agent-eye view"]')`,
  },
];

/** A run is clean when nothing structural fired in any case. */
export function isClean(results) {
  return results.every(
    (r) =>
      r.report.clipped.length === 0 &&
      r.report.overflow.length === 0 &&
      r.report.voids.length === 0 &&
      r.report.verticalVoids.length === 0 &&
      r.report.tiny.length === 0,
  );
}
