// Run: node --experimental-strip-types --test test/assembly-tree.test.mjs
//
// The browser draws an imported assembly as its PRODUCT TREE: file → the
// sub-assemblies inside it → parts, with a part that is used several times
// in one place folded into one "×N" row. Built from each part's `assembly`
// occurrence (source file, assembly path, part name, use name).
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { buildAssemblyTree } from '../src/lib/assembly-tree.ts'

const part = (id, part, path, occurrence, sound = true, source = 'arm.stp') => ({
  id,
  name: occurrence ? `${part} [${occurrence}]` : part,
  sound,
  assembly: { source, path, part, occurrence },
})

const strip = (nodes) =>
  nodes.map((n) => ({
    kind: n.kind,
    name: n.name,
    ...(n.kind === 'part' ? { id: n.id } : { parts: n.partCount, sound: n.soundCount }),
    ...(n.children && n.children.length ? { children: strip(n.children) } : {}),
  }))

test('a part used twice in one assembly folds into one ×N row', () => {
  const { roots, rest } = buildAssemblyTree([
    part('a', 'cube', ['asm'], 'i1'),
    part('b', 'cube', ['asm'], 'i2'),
  ])
  assert.deepEqual(rest, [])
  // One file; its single top assembly is folded into the file row.
  assert.equal(roots.length, 1)
  assert.equal(roots[0].kind, 'file')
  assert.equal(roots[0].name, 'arm.stp')
  const [group] = roots[0].children
  assert.equal(group.kind, 'group')
  assert.equal(group.name, 'cube')
  assert.equal(group.count, 2)
  assert.deepEqual(
    group.children.map((c) => [c.id, c.name]),
    [['a', 'i1'], ['b', 'i2']],
  )
})

test('sub-assemblies nest in path order, under the file', () => {
  const { roots } = buildAssemblyTree([
    part('p1', 'bolt', ['arm', 'base'], 'bolt:1'),
    part('p2', 'plate', ['arm', 'base'], null),
    part('p3', 'cover', ['arm', 'link2'], null),
    part('p4', 'motor', ['arm'], null),
  ])
  assert.deepEqual(strip(roots), [
    {
      kind: 'file',
      name: 'arm.stp',
      parts: 4,
      sound: 4,
      children: [
        { kind: 'assembly', name: 'base', parts: 2, sound: 2, children: [
          { kind: 'part', name: 'bolt', id: 'p1' },
          { kind: 'part', name: 'plate', id: 'p2' },
        ] },
        { kind: 'assembly', name: 'link2', parts: 1, sound: 1, children: [
          { kind: 'part', name: 'cover', id: 'p3' },
        ] },
        { kind: 'part', name: 'motor', id: 'p4' },
      ],
    },
  ])
})

test('names sort naturally: 2 before 10', () => {
  const { roots } = buildAssemblyTree([
    part('x10', 'washer 10', ['asm'], null),
    part('x2', 'washer 2', ['asm'], null),
    part('x1', 'washer 1', ['asm'], null),
  ])
  assert.deepEqual(roots[0].children.map((n) => n.name), ['washer 1', 'washer 2', 'washer 10'])
})

test('instances inside a ×N row sort naturally by their use name', () => {
  const { roots } = buildAssemblyTree([
    part('c', 'pin', ['asm'], 'pin:10'),
    part('a', 'pin', ['asm'], 'pin:2'),
    part('b', 'pin', ['asm'], 'pin:1'),
  ])
  assert.deepEqual(roots[0].children[0].children.map((n) => n.name), ['pin:1', 'pin:2', 'pin:10'])
})

test('sound counts roll up; unknown is neither sound nor unsound', () => {
  const { roots } = buildAssemblyTree([
    part('a', 'pin', ['asm', 'sub'], 'p:1', true),
    part('b', 'pin', ['asm', 'sub'], 'p:2', false),
    part('c', 'plate', ['asm', 'sub'], null, null),
  ])
  const file = roots[0]
  assert.equal(file.partCount, 3)
  assert.equal(file.soundCount, 1)
  assert.equal(file.unsoundCount, 1)
  const group = file.children[0].children.find((n) => n.kind === 'group')
  assert.equal(group.soundCount, 1)
  assert.equal(group.unsoundCount, 1)
})

test('parts no assembly placed stay out of the tree', () => {
  const { roots, rest } = buildAssemblyTree([
    { id: 'box', name: 'Box', sound: true },
    part('a', 'cube', ['asm'], null),
  ])
  assert.deepEqual(rest, ['box'])
  assert.equal(roots.length, 1)
})

test('two files are two roots, sorted by name', () => {
  const { roots } = buildAssemblyTree([
    part('a', 'cube', ['asm'], null, true, 'vice.step'),
    part('b', 'cube', ['asm'], null, true, 'arm.stp'),
  ])
  assert.deepEqual(roots.map((r) => r.name), ['arm.stp', 'vice.step'])
})

test('a file whose parts sit under several top assemblies keeps them as rows', () => {
  const { roots } = buildAssemblyTree([
    part('a', 'cube', ['left'], null),
    part('b', 'cube', ['right'], null),
  ])
  assert.deepEqual(roots[0].children.map((n) => [n.kind, n.name]), [['assembly', 'left'], ['assembly', 'right']])
})

test('every node carries the ids of all parts beneath it (for hover / hide)', () => {
  const { roots } = buildAssemblyTree([
    part('a', 'pin', ['arm', 'base'], 'p:1'),
    part('b', 'pin', ['arm', 'base'], 'p:2'),
    part('c', 'plate', ['arm'], null),
  ])
  assert.deepEqual([...roots[0].partIds].sort(), ['a', 'b', 'c'])
  const base = roots[0].children.find((n) => n.name === 'base')
  assert.deepEqual([...base.partIds].sort(), ['a', 'b'])
})
