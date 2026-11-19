// roshera.tech -- the moving parts: the chapter player, the pipeline strip,
// the live sheet certificate, and reveal-on-scroll. No dependencies.
;(() => {
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches
  const onView = (el, enter, leave) => {
    if (!('IntersectionObserver' in window)) return enter()
    new IntersectionObserver((es) => es.forEach((e) => (e.isIntersecting ? enter() : leave && leave())), { threshold: 0.35 }).observe(el)
  }

  // Reveal sections as they scroll in.
  document.querySelectorAll('.reveal').forEach((el) => {
    if (reduce) return el.classList.add('in')
    onView(el, () => el.classList.add('in'))
  })

  // Chapter player: real recordings, one at a time, advancing on end.
  const player = document.querySelector('[data-player]')
  if (player) {
    const video = player.querySelector('video')
    const chapters = [...player.querySelectorAll('[data-src]')]
    const bar = player.querySelector('.progress i')
    let current = 0
    let visible = false
    const select = (i, play) => {
      current = (i + chapters.length) % chapters.length
      chapters.forEach((c, j) => c.setAttribute('aria-selected', String(j === current)))
      const c = chapters[current]
      video.poster = c.dataset.poster
      video.src = c.dataset.src
      if (play && visible && !reduce) video.play().catch(() => {})
    }
    chapters.forEach((c, i) => c.addEventListener('click', () => select(i, true)))
    video.addEventListener('ended', () => select(current + 1, true))
    video.addEventListener('timeupdate', () => {
      if (bar && video.duration) bar.style.width = `${(100 * video.currentTime) / video.duration}%`
    })
    select(0, false)
    onView(
      player,
      () => {
        visible = true
        if (!reduce) video.play().catch(() => {})
      },
      () => {
        visible = false
        video.pause()
      },
    )
  }

  // Pipeline strip: each step lights in turn.
  const strip = document.querySelector('[data-pipeline]')
  if (strip) {
    const steps = [...strip.querySelectorAll('li')]
    let i = 0
    let timer = 0
    const tick = () => {
      steps.forEach((s, j) => {
        s.classList.toggle('done', j < i)
        s.classList.toggle('now', j === i)
      })
      i = (i + 1) % (steps.length + 1)
    }
    if (reduce) steps.forEach((s) => s.classList.add('done'))
    else
      onView(
        strip,
        () => {
          if (!timer) timer = setInterval(tick, 1100)
        },
        () => {
          clearInterval(timer)
          timer = 0
        },
      )
  }

  // The design loop, drawn with real physics: a plate in tension with a hole.
  // Stress is the Kirsch solution (peak 3x nominal at the hole's edge);
  // the redesign is a slot along the load, whose peak is 1 + 2b/a (Inglis).
  const loop = document.querySelector('[data-loop]')
  if (loop) {
    const canvas = loop.querySelector('canvas')
    const ctx = canvas.getContext('2d')
    const steps = [...loop.querySelectorAll('[data-step]')]
    const note = loop.querySelector('[data-note]')
    const W = 2.6, H = 1.3, R = 0.3            // plate half-extents and hole radius, units
    const SLOT_A = 0.46, SLOT_B = 0.2           // slot semi-axes along / across the load
    const KT_SLOT = 1 + (2 * SLOT_B) / SLOT_A   // Inglis: 1.87
    const NT = 64, NR = 12
    const ray = (c, s) => Math.min(Math.abs(c) > 1e-9 ? W / 2 / Math.abs(c) : 1e9, Math.abs(s) > 1e-9 ? H / 2 / Math.abs(s) : 1e9)
    const kirsch = (r, th) => {
      const q = (R * R) / (r * r), q2 = q * q, c2 = Math.cos(2 * th), s2 = Math.sin(2 * th)
      const sr = 0.5 * (1 - q) + 0.5 * (1 - 4 * q + 3 * q2) * c2
      const st = 0.5 * (1 + q) - 0.5 * (1 + 3 * q2) * c2
      const tau = -0.5 * (1 + 2 * q - 3 * q2) * s2
      return Math.sqrt(sr * sr - sr * st + st * st + 3 * tau * tau)
    }
    // Structured mesh: rays from the hole to the plate edge, graded toward the hole.
    const mesh = (m) => {
      const nodes = []
      for (let i = 0; i < NT; i++) {
        const th = (2 * Math.PI * i) / NT, c = Math.cos(th), s = Math.sin(th)
        const ix = (1 - m) * R + m * SLOT_A, iy = (1 - m) * R + m * SLOT_B
        const ox = c * ray(c, s), oy = s * ray(c, s)
        const row = []
        for (let j = 0; j <= NR; j++) {
          const t = Math.pow(j / NR, 1.7)
          const x = ix * c + (ox - ix * c) * t, y = iy * s + (oy - iy * s) * t
          const rc = R + (ray(c, s) - R) * t                 // same node on the round-hole mesh
          const base = kirsch(rc, th)
          const kt = 3 + (KT_SLOT - 3) * m
          row.push({ x, y, v: 1 + (base - 1) * ((kt - 1) / 2) })
        }
        nodes.push(row)
      }
      return nodes
    }
    const stops = [[0.5, [236, 241, 252]], [1.1, [176, 198, 244]], [1.6, [31, 85, 214]], [2.1, [242, 162, 58]], [2.7, [214, 64, 43]]]
    const colour = (v) => {
      for (let k = 1; k < stops.length; k++) {
        if (v <= stops[k][0] || k === stops.length - 1) {
          const [a, ca] = stops[k - 1], [b, cb] = stops[k]
          const f = Math.max(0, Math.min(1, (v - a) / (b - a)))
          return `rgb(${ca.map((x, n) => Math.round(x + (cb[n] - x) * f)).join(',')})`
        }
      }
    }
    const ease = (x) => (x <= 0 ? 0 : x >= 1 ? 1 : x * x * (3 - 2 * x))
    const CYCLE = 15
    let t0 = 0, raf = 0
    const draw = (now) => {
      const rect = canvas.getBoundingClientRect(), dpr = Math.min(2, devicePixelRatio || 1)
      if (canvas.width !== Math.round(rect.width * dpr)) { canvas.width = Math.round(rect.width * dpr); canvas.height = Math.round(rect.height * dpr) }
      const t = reduce ? 14 : ((now - t0) / 1000) % CYCLE
      const scale = Math.min(canvas.width / (W + 0.9), canvas.height / (H + 0.5))
      const cx = canvas.width / 2, cy = canvas.height / 2
      const P = (p) => [cx + p.x * scale, cy - p.y * scale]
      ctx.clearRect(0, 0, canvas.width, canvas.height)
      const morph = ease((t - 9.5) / 2)
      const nodes = mesh(morph)
      const meshIn = ease((t - 1.5) / 2.5), fieldIn = ease((t - 4.2) / 1.8)
      // field
      if (fieldIn > 0) {
        ctx.globalAlpha = fieldIn
        ctx.fillStyle = colour(1) // far field: nominal stress, fills the plate's corners
        ctx.fillRect(cx - (W / 2) * scale, cy - (H / 2) * scale, W * scale, H * scale)
        for (let i = 0; i < NT; i++) for (let j = 0; j < NR; j++) {
          const a = nodes[i][j], b = nodes[(i + 1) % NT][j], c = nodes[(i + 1) % NT][j + 1], d = nodes[i][j + 1]
          ctx.fillStyle = colour((a.v + b.v + c.v + d.v) / 4)
          ctx.beginPath(); [a, b, c, d].forEach((p, n) => ctx[n ? 'lineTo' : 'moveTo'](...P(p))); ctx.closePath(); ctx.fill(); ctx.strokeStyle = ctx.fillStyle; ctx.lineWidth = 0.6; ctx.stroke()
        }
        ctx.globalAlpha = 1
        ctx.fillStyle = getComputedStyle(loop).getPropertyValue('--card') || '#fbfbfa' // the hole is empty
        ctx.beginPath(); nodes.forEach((row, i) => ctx[i ? 'lineTo' : 'moveTo'](...P(row[0]))); ctx.closePath(); ctx.fill()
      }
      // mesh, ring by ring
      if (meshIn > 0) {
        ctx.strokeStyle = fieldIn > 0.5 ? 'rgba(255,255,255,0.45)' : 'rgba(31,85,214,0.55)'
        ctx.lineWidth = Math.max(0.6, dpr * 0.6)
        const rings = Math.round(meshIn * NR)
        for (let j = 0; j <= rings; j++) { ctx.beginPath(); nodes.forEach((row, i) => ctx[i ? 'lineTo' : 'moveTo'](...P(row[j]))); ctx.closePath(); ctx.stroke() }
        for (let i = 0; i < NT; i++) { ctx.beginPath(); ctx.moveTo(...P(nodes[i][0])); ctx.lineTo(...P(nodes[i][rings])); ctx.stroke() }
        for (let i = 0; i < NT; i++) for (let j = 0; j < rings; j++) { ctx.beginPath(); ctx.moveTo(...P(nodes[i][j])); ctx.lineTo(...P(nodes[(i + 1) % NT][j + 1])); ctx.stroke() }
      }
      // outline: plate and hole
      ctx.strokeStyle = '#23272e'; ctx.lineWidth = 1.6 * dpr
      ctx.strokeRect(cx - (W / 2) * scale, cy - (H / 2) * scale, W * scale, H * scale)
      ctx.beginPath(); nodes.forEach((row, i) => ctx[i ? 'lineTo' : 'moveTo'](...P(row[0]))); ctx.closePath(); ctx.stroke()
      // load arrows
      ctx.fillStyle = ctx.strokeStyle = '#1f55d6'; ctx.lineWidth = 1.4 * dpr
      for (const side of [-1, 1]) for (const yy of [-0.4, 0, 0.4]) {
        const x0 = cx + side * (W / 2 + 0.06) * scale, x1 = cx + side * (W / 2 + 0.36) * scale, y = cy - yy * scale
        ctx.beginPath(); ctx.moveTo(x0, y); ctx.lineTo(x1, y); ctx.stroke()
        ctx.beginPath(); ctx.moveTo(x1, y); ctx.lineTo(x1 - side * 8 * dpr, y - 5 * dpr); ctx.lineTo(x1 - side * 8 * dpr, y + 5 * dpr); ctx.fill()
      }
      // hot spot
      if (t > 6.6 && t < 9.6) {
        const pulse = 0.5 + 0.5 * Math.sin((t - 6.6) * 5)
        for (const s of [1, -1]) { const [x, y] = P(nodes[NT / 4 * (s > 0 ? 1 : 3)][0]); ctx.strokeStyle = `rgba(214,64,43,${0.4 + 0.6 * pulse})`; ctx.lineWidth = 2 * dpr; ctx.beginPath(); ctx.arc(x, y, (10 + 6 * pulse) * dpr, 0, 7); ctx.stroke() }
      }
      const step = t < 1.5 ? 0 : t < 4.2 ? 1 : t < 6.6 ? 2 : t < 9.5 ? 3 : 4
      steps.forEach((s, i) => s.classList.toggle('now', i === step))
      const peak = (3 + (KT_SLOT - 3) * morph).toFixed(1)
      note.textContent = [
        'The part: a plate in tension with a hole.',
        'Meshed, finer where the stress will change fastest.',
        'Simulated: stress around the hole.',
        `Feedback: peak ${peak}× nominal at the hole edge, across the load.`,
        `Improved: a slot along the load path. Peak ${peak}× nominal.`,
      ][step]
      raf = reduce ? 0 : requestAnimationFrame(draw)
    }
    if (reduce) draw(0)
    else onView(loop, () => { if (!raf) { t0 = performance.now(); raf = requestAnimationFrame(draw) } }, () => { cancelAnimationFrame(raf); raf = 0 })
  }

  // Live sheet certificate: re-measure every value, change the part,
  // watch one go stale and release refuse, regenerate, release.
  const cert = document.querySelector('[data-cert]')
  if (cert) {
    const rows = [...cert.querySelectorAll('[data-row]')]
    const marks = [...cert.querySelectorAll('[data-mark]')]
    const status = cert.querySelector('[data-status]')
    const release = cert.querySelector('[data-release]')
    const bore = cert.querySelector('[data-bore]')
    const boreText = cert.querySelector('[data-bore-text]')
    const caption = cert.querySelector('[data-caption]')
    const set = (row, state, measured) => {
      row.dataset.state = state
      row.querySelector('.v').textContent = { wait: '…', ok: '✓ consistent', stale: '✗ stale' }[state]
      if (measured) row.querySelector('.m').textContent = measured
      const mark = marks[rows.indexOf(row)]
      if (mark) mark.dataset.state = state
    }
    const verdict = (text, kind) => {
      status.textContent = text
      status.dataset.kind = kind
      release.dataset.kind = kind
      release.textContent = kind === 'ok' ? 'Release: allowed' : kind === 'bad' ? 'Release: refused' : 'Release: checking'
    }
    const script = [
      [400, () => { rows.forEach((r) => set(r, 'wait')); bore.setAttribute('r', '20'); boreText.textContent = 'Ø40 H7'; rows[0].querySelector('.m').textContent = '40.000'; verdict('Checking the sheet…', 'wait'); caption.textContent = 'Each value on the sheet is re-measured on the solid.' }],
      ...rows.map((r) => [520, () => set(r, 'ok')]),
      [600, () => { verdict('Sound · 6 of 6 consistent', 'ok'); caption.textContent = 'Every printed value matches the part.' }],
      [2200, () => { bore.setAttribute('r', '21'); rows[0].querySelector('.m').textContent = '42.000'; set(rows[0], 'stale'); verdict('Not sound · 1 stale', 'bad'); caption.textContent = 'Someone opens the bore to Ø42. The sheet still says 40.' }],
      [2800, () => { boreText.textContent = 'Ø42 H7'; set(rows[0], 'ok', '42.000'); verdict('Sound · 6 of 6 consistent', 'ok'); caption.textContent = 'Regenerated: the sheet states what the part is. Release allowed again.' }],
      [3200, () => {}],
    ]
    let k = 0
    let timer = 0
    const run = () => {
      const [wait, fn] = script[k]
      fn()
      k = (k + 1) % script.length
      timer = setTimeout(run, wait)
    }
    if (reduce) { rows.forEach((r) => set(r, 'ok')); verdict('Sound · 6 of 6 consistent', 'ok') }
    else onView(cert, () => { if (!timer) run() }, () => { clearTimeout(timer); timer = 0 })
  }
})()
