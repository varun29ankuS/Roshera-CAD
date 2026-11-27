import { test } from 'node:test'
import assert from 'node:assert/strict'

const { cubeView, squareOnFace, rollView, adjacentView, CUBE_FACE_LABELS, FACE_VIEWS } = await import(
  '../src/lib/view-cube.ts'
)

const v = (x, y, z) => ({ x, y, z })
const ZERO = v(0, 0, 0)
const near = (a, b) => a.every((x, i) => Math.abs(x - b[i]) < 1e-9)
const unit = (a) => {
  const n = Math.hypot(...a)
  return a.map((x) => x / n)
}

test('a face click looks from that side (Z-up: +Z top, -Y front, +X right)', () => {
  assert.equal(cubeView(ZERO, v(0, 0, 1)).preset, 'top')
  assert.equal(cubeView(ZERO, v(0, 0, -1)).preset, 'bottom')
  assert.equal(cubeView(ZERO, v(0, -1, 0)).preset, 'front')
  assert.equal(cubeView(ZERO, v(0, 1, 0)).preset, 'back')
  assert.equal(cubeView(ZERO, v(1, 0, 0)).preset, 'right')
  assert.equal(cubeView(ZERO, v(-1, 0, 0)).preset, 'left')
})

test('the camera stands on the side it looks from', () => {
  assert.ok(near(cubeView(ZERO, v(0, -1, 0)).direction, [0, -1, 0]), 'front: the camera is on -Y')
  assert.ok(near(cubeView(ZERO, v(1, 0, 0)).direction, [1, 0, 0]))
  assert.ok(near(cubeView(ZERO, v(0, 0, 1)).direction, [0, 0, 1]))
})

test('each of the 8 corners is its own corner view, by where the corner sits', () => {
  const seen = new Set()
  for (const x of [-1, 1]) for (const y of [-1, 1]) for (const z of [-1, 1]) {
    // whichever side of the little corner box was hit
    const view = cubeView(v(0.38 * x, 0.38 * y, 0.38 * z), v(1, 0, 0))
    assert.equal(view.kind, 'corner')
    assert.ok(near(view.direction, unit([x, y, z])), `corner ${x},${y},${z}`)
    assert.ok(near(view.up, [0, 0, 1]), 'Z stays up')
    seen.add(view.direction.join())
  }
  assert.equal(seen.size, 8)
})

test('the front-right-top corner is the home isometric view', () => {
  assert.equal(cubeView(v(0.38, -0.38, 0.38), v(0, 0, 1)).preset, 'isometric')
  assert.equal(cubeView(v(-0.38, -0.38, 0.38), v(0, 0, 1)).preset, undefined)
})

test('each of the 12 edges is its own edge view, halfway between its two faces', () => {
  const seen = new Set()
  const edges = []
  for (const a of [-1, 1]) for (const b of [-1, 1]) edges.push([a, b, 0], [a, 0, b], [0, a, b])
  for (const [x, y, z] of edges) {
    const view = cubeView(v(0.38 * x, 0.38 * y, 0.38 * z), v(0, -1, 0))
    assert.equal(view.kind, 'edge')
    assert.ok(near(view.direction, unit([x, y, z])), `edge ${x},${y},${z}`)
    assert.ok(near(view.up, [0, 0, 1]))
    seen.add(view.direction.join())
  }
  assert.equal(seen.size, 12)
})

test('6 faces + 12 edges + 8 corners: 26 views, all different', () => {
  const all = new Set()
  for (const x of [-1, 0, 1]) for (const y of [-1, 0, 1]) for (const z of [-1, 0, 1]) {
    if (x === 0 && y === 0 && z === 0) continue
    const off = [x, y, z].filter((c) => c !== 0).length
    const view = off === 1 ? cubeView(ZERO, v(x, y, z)) : cubeView(v(0.38 * x, 0.38 * y, 0.38 * z), v(1, 0, 0))
    all.add(view.direction.map((c) => c.toFixed(6)).join())
  }
  assert.equal(all.size, 26)
})

test('looking straight down or up, the screen-up is the Top and Bottom views\' own', () => {
  assert.ok(near(cubeView(ZERO, v(0, 0, 1)).up, [0, 1, 0]))
  assert.ok(near(cubeView(ZERO, v(0, 0, -1)).up, [0, -1, 0]))
})

test('the word on each face is the view a click on it gives', () => {
  // drei draws the faces in the order +X, -X, +Y, -Y, +Z, -Z.
  const normals = [v(1, 0, 0), v(-1, 0, 0), v(0, 1, 0), v(0, -1, 0), v(0, 0, 1), v(0, 0, -1)]
  assert.deepEqual(CUBE_FACE_LABELS, ['RIGHT', 'LEFT', 'BACK', 'FRONT', 'TOP', 'BOTTOM'])
  normals.forEach((n, i) => {
    assert.equal(cubeView(ZERO, n).preset.toUpperCase(), CUBE_FACE_LABELS[i])
  })
})

test('square-on to a face is recognised, and nothing else is', () => {
  assert.equal(squareOnFace([0, -1, 0]), 'front')
  assert.equal(squareOnFace([0, 0, 1]), 'top')
  assert.equal(squareOnFace([0.01, -0.9999, 0.005]), 'front', 'within a degree')
  assert.equal(squareOnFace(unit([1, -1, 1])), null)
  assert.equal(squareOnFace(unit([1, -1, 0])), null)
  assert.equal(squareOnFace([0, 0, 0]), null)
})

test('rolling turns the picture a quarter turn about the line of sight', () => {
  // Front view, Z up. Clockwise on screen: +X (screen right) ends up pointing down,
  // so the new screen-up is -X.
  const cw = rollView([0, -1, 0], [0, 0, 1], 1)
  assert.ok(near(cw.direction, [0, -1, 0]), 'still looking from the front')
  assert.ok(near(cw.up, [-1, 0, 0]))
  const ccw = rollView([0, -1, 0], [0, 0, 1], -1)
  assert.ok(near(ccw.up, [1, 0, 0]))
  // four quarter turns is where it started
  let view = { direction: [0, -1, 0], up: [0, 0, 1] }
  for (let k = 0; k < 4; k++) view = rollView(view.direction, view.up, 1)
  assert.ok(near(view.up, [0, 0, 1]))
})

test('an arrow steps to the neighbouring face, keeping the picture the right way up', () => {
  const front = FACE_VIEWS.front
  const right = adjacentView(front.direction, front.up, 'right')
  assert.ok(near(right.direction, [1, 0, 0]), 'right of Front is Right')
  assert.ok(near(right.up, [0, 0, 1]))
  const left = adjacentView(front.direction, front.up, 'left')
  assert.ok(near(left.direction, [-1, 0, 0]))
  const up = adjacentView(front.direction, front.up, 'up')
  assert.ok(near(up.direction, FACE_VIEWS.top.direction), 'above Front is Top')
  assert.ok(near(up.up, FACE_VIEWS.top.up), 'and it arrives as the Top view is drawn')
  const down = adjacentView(front.direction, front.up, 'down')
  assert.ok(near(down.direction, [0, 0, -1]))
})

test('four steps the same way go all the way round', () => {
  let view = { ...FACE_VIEWS.front }
  const visited = []
  for (let k = 0; k < 4; k++) {
    view = adjacentView(view.direction, view.up, 'right')
    visited.push(squareOnFace(view.direction))
  }
  assert.deepEqual(visited, ['right', 'back', 'left', 'front'])
})

const { CUBE_FACE_TEXT_TURNS } = await import('../src/lib/view-cube.ts')

test('every face has a text rotation, and the front needs none', () => {
  assert.equal(CUBE_FACE_TEXT_TURNS.length, 6)
  assert.equal(CUBE_FACE_TEXT_TURNS[3], 0, 'FRONT (-Y) is drawn as the texture lies')
  assert.equal(CUBE_FACE_TEXT_TURNS[4], 0, 'TOP (+Z) too')
  for (const t of CUBE_FACE_TEXT_TURNS) assert.ok([0, 1, 2, 3].includes(t))
})

const { neighbourFace, axisEndOn } = await import('../src/lib/view-cube.ts')

test('an arrow names the standard view it goes to: above any side view is THE Top view', () => {
  for (const side of ['front', 'back', 'left', 'right']) {
    const v = FACE_VIEWS[side]
    assert.equal(neighbourFace(v.direction, v.up, 'up'), 'top', `above ${side}`)
    assert.equal(neighbourFace(v.direction, v.up, 'down'), 'bottom', `below ${side}`)
  }
  assert.equal(neighbourFace(FACE_VIEWS.front.direction, FACE_VIEWS.front.up, 'right'), 'right')
  assert.equal(neighbourFace(FACE_VIEWS.front.direction, FACE_VIEWS.front.up, 'left'), 'left')
  // From the Top view (Y up the screen): right is Right, above is Back.
  assert.equal(neighbourFace(FACE_VIEWS.top.direction, FACE_VIEWS.top.up, 'right'), 'right')
  assert.equal(neighbourFace(FACE_VIEWS.top.direction, FACE_VIEWS.top.up, 'up'), 'back')
})

test('after a roll the arrows follow the picture, not the world', () => {
  // Front rolled a quarter turn clockwise: screen-up is -X, so "above" is Left.
  const rolled = rollView([0, -1, 0], [0, 0, 1], 1)
  assert.equal(neighbourFace(rolled.direction, rolled.up, 'up'), 'left')
})

test('an axis pointing at the viewer (or straight away) is end-on; the others are not', () => {
  // Front view: the camera stands on -Y, so the Y axis is end-on.
  assert.equal(axisEndOn([0, 1, 0], [0, -1, 0]), true)
  assert.equal(axisEndOn([1, 0, 0], [0, -1, 0]), false)
  assert.equal(axisEndOn([0, 0, 1], [0, -1, 0]), false)
  // Isometric: none is.
  const iso = unit([1, -1, 1])
  for (const a of [[1, 0, 0], [0, 1, 0], [0, 0, 1]]) assert.equal(axisEndOn(a, iso), false)
})
