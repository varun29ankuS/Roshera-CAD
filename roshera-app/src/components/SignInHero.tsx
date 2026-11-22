import { useEffect, useRef } from 'react'
import * as THREE from 'three'
import {
  FUSELAGE,
  LENGTH,
  NOZZLE,
  WING,
  nacaSection,
  nacaThickness,
  tableAt,
  wakeSeeds,
  wingLeadingEdgeX,
  wingTrailingEdgeX,
} from '@/lib/fighter-model'
import { signInScenePalette } from '@/lib/sign-in-palette'
import { useThemeStore } from '@/stores/theme-store'

/**
 * The sign-in hero (Varun 2026-11-18, after nTop's): a stealth fighter
 * concept drawn like a technical illustration -- toon-shaded paper, a bold
 * ink silhouette, fine creases and panel lines -- with a wake of ~110k GPU
 * particles shed from the trailing edges, the nozzles and the wingtips. The
 * camera drifts slowly and leans toward the pointer; under reduced motion
 * everything stands still. The numbers are in `lib/fighter-model`.
 */
export function SignInHero() {
  const host = useRef<HTMLDivElement>(null)
  // Drawn in the app's theme, so signing in and the app look like one place.
  const theme = useThemeStore((s) => s.theme)

  useEffect(() => {
    const el = host.current
    if (!el) return
    const pal = signInScenePalette(theme)
    const still = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false
    const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true })
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2))
    el.appendChild(renderer.domElement)
    renderer.domElement.style.display = 'block'

    const scene = new THREE.Scene()
    scene.add(new THREE.HemisphereLight(pal.skyLight, pal.groundLight, 0.9))
    const sun = new THREE.DirectionalLight(pal.skyLight, theme === 'dark' ? 1.1 : 1.6)
    sun.position.set(-4, 10, 6)
    scene.add(sun)
    const camera = new THREE.PerspectiveCamera(28, 1, 0.5, 400)
    // The picture sits a little left of centre (Varun: "move it about 5 cm").
    const SHIFT = 0.07
    const frame = () => {
      const w = el.clientWidth || window.innerWidth
      const h = el.clientHeight || window.innerHeight
      camera.aspect = w / h
      camera.setViewOffset(w, h, w * SHIFT, 0, w, h)
      camera.updateProjectionMatrix()
      renderer.setSize(w, h)
      flowMat.uniforms.uScale.value = h * Math.min(window.devicePixelRatio, 2)
    }

    const disposables: { dispose: () => void }[] = []
    const keep = <T extends { dispose: () => void }>(x: T) => (disposables.push(x), x)

    // ---- the look: toon-shaded paper, bold silhouette, fine creases ----
    const ramp = keep(new THREE.DataTexture(new Uint8Array([168, 214, 246]), 3, 1, THREE.RedFormat))
    ramp.minFilter = ramp.magFilter = THREE.NearestFilter
    ramp.needsUpdate = true
    const offset = { polygonOffset: true, polygonOffsetFactor: 1, polygonOffsetUnits: 1 }
    const skin = keep(new THREE.MeshToonMaterial({ color: pal.skin, gradientMap: ramp, side: THREE.DoubleSide, ...offset }))
    const glass = keep(new THREE.MeshToonMaterial({ color: pal.glass, gradientMap: ramp, transparent: true, opacity: 0.9, ...offset }))
    const black = keep(new THREE.MeshBasicMaterial({ color: pal.void, side: THREE.DoubleSide }))
    const dark = keep(new THREE.MeshBasicMaterial({ color: pal.shade, side: THREE.DoubleSide }))
    const crease = keep(new THREE.LineBasicMaterial({ color: pal.crease, transparent: true, opacity: 0.75 }))
    const panel = keep(new THREE.LineBasicMaterial({ color: pal.crease, transparent: true, opacity: 0.35 }))
    const cs = keep(new THREE.LineBasicMaterial({ color: pal.crease, transparent: true, opacity: 0.6 }))
    const rimInk = keep(new THREE.LineBasicMaterial({ color: pal.rim }))
    const edgeBand = keep(new THREE.LineBasicMaterial({ color: pal.band }))
    const edgeBand2 = keep(new THREE.LineBasicMaterial({ color: pal.band, transparent: true, opacity: 0.55 }))
    // The silhouette: back faces pushed out along their normals, drawn in the
    // theme's ink (dark on paper, light on graphite).
    const hullMat = keep(
      new THREE.ShaderMaterial({
        side: THREE.BackSide,
        uniforms: { uWidth: { value: 0.035 }, uInk: { value: new THREE.Color(pal.silhouette) } },
        vertexShader: 'uniform float uWidth; void main() { vec3 p = position + normal * uWidth; gl_Position = projectionMatrix * modelViewMatrix * vec4(p, 1.0); }',
        fragmentShader: 'uniform vec3 uInk; void main() { gl_FragColor = vec4(uInk, 1.0); }',
      }),
    )

    const jet = new THREE.Group()
    jet.position.x = -LENGTH / 2
    scene.add(jet)
    const add = (o: THREE.Object3D) => jet.add(o)
    const geo = <T extends THREE.BufferGeometry>(g: T) => keep(g)
    function part(g: THREE.BufferGeometry, mat: THREE.Material = skin, angle = 32) {
      const grp = new THREE.Group()
      grp.add(new THREE.Mesh(geo(g), mat))
      grp.add(new THREE.Mesh(g, hullMat))
      grp.add(new THREE.LineSegments(geo(new THREE.EdgesGeometry(g, angle)), crease))
      return grp
    }
    function loft(rings: THREE.Vector3[][], capStart = true, capEnd = true) {
      const K = rings[0].length
      const pos: number[] = []
      const idx: number[] = []
      for (const r of rings) for (const p of r) pos.push(p.x, p.y, p.z)
      for (let i = 0; i < rings.length - 1; i++)
        for (let k = 0; k < K; k++) {
          const a = i * K + k, b = i * K + ((k + 1) % K), c = (i + 1) * K + k, d = (i + 1) * K + ((k + 1) % K)
          idx.push(a, c, b, b, c, d)
        }
      const cap = (ri: number, flip: boolean) => {
        const base = pos.length / 3
        const cen = new THREE.Vector3()
        rings[ri].forEach((p) => cen.add(p))
        cen.multiplyScalar(1 / K)
        pos.push(cen.x, cen.y, cen.z)
        for (let k = 0; k < K; k++) {
          const a = ri * K + k, b = ri * K + ((k + 1) % K)
          if (flip) idx.push(base, b, a)
          else idx.push(base, a, b)
        }
      }
      if (capStart) cap(0, false)
      if (capEnd) cap(rings.length - 1, true)
      const g = new THREE.BufferGeometry()
      g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3))
      g.setIndex(idx)
      g.computeVertexNormals()
      return g
    }
    const lineOf = (pts: THREE.Vector3[], mat: THREE.LineBasicMaterial, loop = false) =>
      add(loop ? new THREE.LineLoop(geo(new THREE.BufferGeometry().setFromPoints(pts)), mat) : new THREE.Line(geo(new THREE.BufferGeometry().setFromPoints(pts)), mat))

    // ---- fuselage: a blended, chined body ----
    const F = FUSELAGE
    const W = (x: number) => tableAt(F.xs, F.halfWidth, x)
    {
      const rings: THREE.Vector3[][] = []
      for (let i = 0; i <= 90; i++) {
        const x = (LENGTH * (1 - Math.cos((Math.PI * i) / 90))) / 2
        const w = W(x), ht = tableAt(F.xs, F.top, x), hb = tableAt(F.xs, F.belly, x), zc = tableAt(F.xs, F.chine, x)
        const half: [number, number][] = [
          [0, ht], [0.3 * w, ht * 0.98], [0.62 * w, ht * 0.72], [0.9 * w, zc + 0.12 * (ht - zc)], [w, zc],
          [0.88 * w, zc + 0.35 * (hb - zc)], [0.55 * w, hb * 0.97], [0, hb],
        ]
        const ring = half.map(([z, y]) => new THREE.Vector3(x, y, z))
        for (let k = half.length - 2; k >= 1; k--) ring.push(new THREE.Vector3(x, half[k][1], -half[k][0]))
        rings.push(ring)
      }
      add(part(loft(rings), skin, 24))
    }
    // canopy, its frame line, and the spine behind it
    {
      const c = part(new THREE.SphereGeometry(1, 48, 20, 0, Math.PI * 2, 0, Math.PI / 2), glass, 40)
      c.scale.set(1.9, 0.5, 0.48)
      c.position.set(4.3, 0.42, 0)
      add(c)
      const frameLine = new THREE.EllipseCurve(0, 0, 1.9, 0.48, 0, Math.PI * 2).getPoints(80).map((p) => new THREE.Vector3(4.3 + p.x, 0.43, p.y))
      lineOf(frameLine, crease, true)
      const s = part(new THREE.SphereGeometry(1, 40, 16, 0, Math.PI * 2, 0, Math.PI / 2), skin, 50)
      s.scale.set(4.2, 0.22, 0.55)
      s.position.set(9.4, 0.47, 0)
      add(s)
    }

    const X = new THREE.Vector3(1, 0, 0)
    for (const side of [-1, 1]) {
      // ---- intakes under the chine ----
      const iring = (x: number, k: number) => {
        const w = W(x)
        return [
          new THREE.Vector3(x, -0.02, side * (w * 0.72)), new THREE.Vector3(x, -0.02, side * (w * 0.72 + 0.5 * k)),
          new THREE.Vector3(x, -0.62, side * (w * 0.72 + 0.38 * k)), new THREE.Vector3(x, -0.62, side * (w * 0.6)),
        ]
      }
      const front = iring(5.6, 1).map((p, k) => (k === 1 || k === 2 ? p.clone().add(new THREE.Vector3(0.7, 0, 0)) : p))
      add(part(loft([front, iring(7.2, 0.9), iring(9.4, 0.2)], false, false), skin, 18))
      add(new THREE.Mesh(geo(new THREE.BufferGeometry().setFromPoints([front[0], front[1], front[2], front[0], front[2], front[3]])), dark))

      // ---- wing: cranked trailing edge, taper, anhedral, control surfaces ----
      const anh = Math.tan((WING.anhedralDeg * Math.PI) / 180)
      const station = (z: number, t: number) => ({
        le: new THREE.Vector3(wingLeadingEdgeX(z), -0.05 - (z - WING.rootZ) * anh, side * z),
        c: wingTrailingEdgeX(z) - wingLeadingEdgeX(z),
        t,
      })
      const W0 = station(WING.rootZ, WING.thickness.root), W1 = station(WING.crankZ, WING.thickness.crank), W2 = station(WING.tipZ, WING.thickness.tip)
      const unit = nacaSection(1)
      const wring = (s: ReturnType<typeof station>) => unit.map(([u, v]) => s.le.clone().add(new THREE.Vector3(u * s.c, v * s.c * s.t, 0)))
      add(part(loft([wring(W0), wring(W1), wring(W2)]), skin, 20))
      const onTop = (A: ReturnType<typeof station>, B: ReturnType<typeof station>, u: number, f: number) => {
        const c = A.c + (B.c - A.c) * u, t = A.t + (B.t - A.t) * u
        return A.le.clone().lerp(B.le, u).add(new THREE.Vector3(f * c, nacaThickness(t, f) * c + 0.012, 0))
      }
      const box = (A: ReturnType<typeof station>, B: ReturnType<typeof station>, u0: number, u1: number, f0: number, f1: number) =>
        lineOf([onTop(A, B, u0, f0), onTop(A, B, u1, f0), onTop(A, B, u1, f1), onTop(A, B, u0, f1)], cs, true)
      box(W0, W1, 0.08, 0.95, 0.78, 0.995) // inner flaperon
      box(W1, W2, 0.05, 0.85, 0.75, 0.99) // outer aileron
      box(W0, W1, 0.15, 0.97, 0.005, 0.1) // leading-edge flaps
      box(W1, W2, 0.03, 0.9, 0.005, 0.14)
      const band: THREE.Vector3[] = []
      for (let i = 0; i <= 24; i++) {
        const u = i / 24
        band.push(u < 0.5 ? onTop(W0, W1, u * 2, 0.004) : onTop(W1, W2, (u - 0.5) * 2, 0.004))
      }
      lineOf(band, edgeBand)
      lineOf(band.map((q) => q.clone().add(new THREE.Vector3(0.09, -0.004, 0))), edgeBand2)

      // ---- all-moving tailplane and canted fin ----
      const surf = (a: { le: THREE.Vector3; c: number; th: THREE.Vector3 }, b: { le: THREE.Vector3; c: number; th: THREE.Vector3 }, t: number) => {
        const sec = nacaSection(t)
        const r = (s: typeof a) => sec.map(([u, v]) => s.le.clone().addScaledVector(X, u * s.c).addScaledVector(s.th, v * s.c))
        return loft([r(a), r(b)])
      }
      const Y = new THREE.Vector3(0, 1, 0)
      add(part(surf({ le: new THREE.Vector3(14.6, -0.06, side * 1.45), c: 2.9, th: Y }, { le: new THREE.Vector3(14.6 + 2.6 * Math.tan((46 * Math.PI) / 180), -0.06, side * 4.05), c: 0.95, th: Y }, 0.04), skin, 20))
      const cant = (28 * Math.PI) / 180
      const up = new THREE.Vector3(0, Math.cos(cant), side * Math.sin(cant))
      const n = new THREE.Vector3().crossVectors(X, up).normalize()
      const finRoot = { le: new THREE.Vector3(12.9, 0.4, side * 1.25), c: 3.6, th: n }
      const finTip = { le: finRoot.le.clone().addScaledVector(up, 2.7).add(new THREE.Vector3(2.7 * Math.tan((44 * Math.PI) / 180), 0, 0)), c: 1.35, th: n }
      add(part(surf(finRoot, finTip, 0.05), skin, 20))
      const hinge = (fr: number) => [finRoot.le.clone().addScaledVector(X, fr * finRoot.c), finTip.le.clone().addScaledVector(X, fr * finTip.c)]
      const [h0, h1] = hinge(0.72)
      lineOf([h0.clone().lerp(h1, 0.1), h0.clone().lerp(h1, 0.85)], panel)

      // ---- exhaust: boat-tail, serrated nozzle, black throat ----
      const nzY = NOZZLE.y, nzZ = side * NOZZLE.z
      const lathe = (pts: [number, number][], seg = 48) => {
        const g = new THREE.LatheGeometry(pts.map(([r, x]) => new THREE.Vector2(r, x)), seg)
        g.rotateZ(-Math.PI / 2)
        g.translate(0, nzY, nzZ)
        return g
      }
      add(part(lathe([[0.74, 15.0], [0.76, 16.2], [0.72, 17.0], [0.66, NOZZLE.startX]]), skin, 50))
      const { petals: P, innerRadius: rIn, exitRadius: rOut, startX: x0, seamX, tipX } = NOZZLE
      const pos: number[] = []
      const idx: number[] = []
      for (let j = 0; j <= 2 * P; j++) {
        const a = (j / (2 * P)) * Math.PI * 2
        const tipJ = j % 2 === 1
        const xe = tipJ ? tipX : seamX, re = tipJ ? rOut - 0.02 : rOut
        pos.push(x0, nzY + rIn * Math.sin(a), nzZ + rIn * Math.cos(a), xe, nzY + re * Math.sin(a), nzZ + re * Math.cos(a))
        if (j < 2 * P) {
          const b = 2 * j
          idx.push(b, b + 1, b + 2, b + 1, b + 3, b + 2)
        }
      }
      const petals = geo(new THREE.BufferGeometry())
      petals.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3))
      petals.setIndex(idx)
      petals.computeVertexNormals()
      add(new THREE.Mesh(petals, skin))
      add(new THREE.Mesh(geo(lathe([[rOut - 0.012, seamX], [rIn - 0.012, x0]])), black))
      const throat = geo(new THREE.CircleGeometry(rIn - 0.01, 48))
      throat.rotateY(Math.PI / 2)
      throat.translate(x0 + 0.05, nzY, nzZ)
      add(new THREE.Mesh(throat, black))
      const rim: THREE.Vector3[] = []
      const seams: THREE.Vector3[] = []
      for (let j = 0; j <= 2 * P; j++) rim.push(new THREE.Vector3(pos[j * 6 + 3], pos[j * 6 + 4], pos[j * 6 + 5]))
      for (let j = 0; j < 2 * P; j += 2)
        seams.push(new THREE.Vector3(pos[j * 6], pos[j * 6 + 1], pos[j * 6 + 2]), new THREE.Vector3(pos[j * 6 + 3], pos[j * 6 + 4], pos[j * 6 + 5]))
      lineOf(rim, rimInk)
      add(new THREE.LineSegments(geo(new THREE.BufferGeometry().setFromPoints(seams)), panel))
      lineOf(new THREE.EllipseCurve(0, 0, rIn, rIn, 0, Math.PI * 2).getPoints(64).map((q) => new THREE.Vector3(x0, nzY + q.y, nzZ + q.x)), crease, true)
    }

    // ---- the wake, as GPU particles ----
    const wake = wakeSeeds(110000, Math.random)
    const wakeGeo = geo(new THREE.BufferGeometry())
    wakeGeo.setAttribute('position', new THREE.BufferAttribute(wake.origin, 3))
    wakeGeo.setAttribute('seed', new THREE.BufferAttribute(wake.seed, 4))
    const flowMat = keep(
      new THREE.ShaderMaterial({
        transparent: true,
        depthWrite: false,
        uniforms: {
          uTime: { value: 0 },
          uScale: { value: window.innerHeight },
          uPale: { value: new THREE.Color(pal.wakePale) },
          uDeep: { value: new THREE.Color(pal.wakeDeep) },
          uAlpha: { value: pal.wakeAlpha },
        },
        vertexShader: `
          attribute vec4 seed;
          uniform float uTime; uniform float uScale;
          varying float vAlpha; varying float vCore;
          float h(float n) { return fract(sin(n * 91.345) * 47453.53); }
          void main() {
            float kind = seed.z, side = seed.w, lane = seed.x;
            float s = fract(seed.y + uTime * (kind == 1.0 ? 0.07 : 0.05) * (0.85 + 0.3 * lane));
            vec3 p = position;
            p.x += s * (kind == 1.0 ? 36.0 : 30.0);
            float grow = (kind == 1.0 ? 0.06 : 0.1) + 2.3 * pow(s, 1.3);
            float ph = lane * 60.0;
            p.y += grow * 0.3 * (0.6 * sin(p.x * 0.33 + ph + uTime * 0.45) + 0.3 * sin(p.x * 0.91 + ph * 1.7 - uTime * 0.7) + 0.1 * sin(p.x * 2.3 + ph * 3.1));
            p.z += grow * 0.4 * (0.6 * sin(p.x * 0.27 + ph * 1.3 - uTime * 0.35) + 0.3 * sin(p.x * 0.83 + ph * 2.3 + uTime * 0.6));
            float j1 = h(seed.y * 311.0 + 1.0) - 0.5, j2 = h(seed.y * 173.0 + 2.0) - 0.5;
            // The wing and tail wake widens downstream as sqrt(x), x metres
            // from the trailing edge; the exhaust jets stay a narrow, dense
            // core (Varun 11-22: the central darkening from the exhausts
            // reads better than a jet spread to its real 5 degrees); a tip
            // vortex's core grows below.
            float x = s * 30.0;
            float wakeD = 0.04 + 0.11 * sqrt(x);
            if (kind == 1.0) {
              p.y += j1 * (0.04 + 0.3 * s); p.z += j2 * (0.04 + 0.4 * s);
            } else if (kind == 2.0) {
              p.y += j1 * 0.04; p.z += j2 * 0.04;
            } else {
              p.y += j1 * 2.0 * wakeD; p.z += j2 * (0.04 + 0.08 * sqrt(x));
            }
            if (kind == 2.0) {
              float a = lane * 6.2831 + s * 26.0 * side;
              float r = 0.07 + 0.5 * pow(s, 0.8);
              p.y += r * sin(a); p.z += side * r * cos(a) - side * 0.5 * s;
            }
            vCore = kind == 1.0 ? 1.0 : (kind == 2.0 ? 0.85 : 0.35 + 0.55 * lane);
            vAlpha = smoothstep(0.0, 0.025, s) * (1.0 - smoothstep(0.4, 1.0, s)) * (kind == 1.0 ? 0.75 : 0.55);
            vec4 mv = modelViewMatrix * vec4(p, 1.0);
            gl_PointSize = clamp(uScale * 0.11 / -mv.z, 1.5, 6.0);
            gl_Position = projectionMatrix * mv;
          }`,
        fragmentShader: `
          uniform vec3 uPale; uniform vec3 uDeep; uniform float uAlpha;
          varying float vAlpha; varying float vCore;
          void main() {
            vec2 q = gl_PointCoord - 0.5; float d = dot(q, q);
            if (d > 0.25) discard;
            gl_FragColor = vec4(mix(uPale, uDeep, vCore), uAlpha * vAlpha * (1.0 - d * 3.0));
          }`,
      }),
    )
    const points = new THREE.Points(wakeGeo, flowMat)
    points.frustumCulled = false
    add(points)

    // ---- camera: high, slow drift, leaning to the pointer ----
    const pointer = { x: 0, y: 0 }
    const lean = { x: 0, y: 0 }
    const onMove = (e: PointerEvent) => {
      pointer.x = (e.clientX / window.innerWidth) * 2 - 1
      pointer.y = (e.clientY / window.innerHeight) * 2 - 1
    }
    const onLeave = () => {
      pointer.x = 0
      pointer.y = 0
    }
    window.addEventListener('pointermove', onMove)
    document.addEventListener('pointerleave', onLeave)
    window.addEventListener('resize', frame)
    frame()

    const orbit = new THREE.Vector3()
    const target = new THREE.Vector3(7, -9, 3)
    const clock = new THREE.Clock()
    renderer.setAnimationLoop(() => {
      const dt = Math.min(clock.getDelta(), 0.05)
      const t = clock.elapsedTime
      flowMat.uniforms.uTime.value = still ? 6 : t
      jet.rotation.x = still ? 0 : Math.sin(t * 0.3) * 0.04
      const k = 1 - Math.exp(-dt * 3)
      lean.x += (pointer.x - lean.x) * k
      lean.y += (pointer.y - lean.y) * k
      const az = (still ? -1.35 : -1.35 + 0.09 * Math.sin(t * ((Math.PI * 2) / 70))) - lean.x * 0.14
      const el = ((50 + 5 * Math.sin(still ? 0 : t * 0.15)) * Math.PI) / 180 + lean.y * 0.08
      orbit.set(Math.cos(az) * Math.cos(el), Math.sin(el), Math.sin(az) * Math.cos(el)).multiplyScalar(56)
      camera.position.copy(target).add(orbit)
      camera.lookAt(target)
      renderer.render(scene, camera)
    })

    return () => {
      renderer.setAnimationLoop(null)
      window.removeEventListener('pointermove', onMove)
      document.removeEventListener('pointerleave', onLeave)
      window.removeEventListener('resize', frame)
      for (const d of disposables) d.dispose()
      renderer.dispose()
      renderer.domElement.remove()
    }
  }, [theme])

  return <div ref={host} className="absolute inset-0" aria-hidden="true" />
}
