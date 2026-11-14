import { test } from 'node:test'
import assert from 'node:assert/strict'

const { formatMass, formatVolume, formatArea, formatSize, formatPoint3, diameterList } = await import('../src/lib/part-properties.ts')

test('mass reads in grams under a kilogram, kilograms above', () => {
  assert.equal(formatMass(0.5159677), '516 g')
  assert.equal(formatMass(2.34567), '2.35 kg')
  assert.equal(formatMass(0.0042), '4.2 g')
})

test('volume and area read in cm³ / cm² (the kernel reports mm)', () => {
  assert.equal(formatVolume(65728.37), '65.7 cm³')
  assert.equal(formatArea(18377.77), '183.8 cm²')
})

test('size reads as W × D × H in mm, no trailing zeros', () => {
  assert.equal(formatSize([80, 80, 32]), '80 × 80 × 32 mm')
  assert.equal(formatSize([12.5, 3.25, 0.1]), '12.5 × 3.25 × 0.1 mm')
})

test('a point drops float noise to two decimals and a true minus', () => {
  assert.equal(formatPoint3([-4.19e-6, 1.9e-15, 10.30106]), '(0, 0, 10.3)')
  assert.equal(formatPoint3([-12.345, 0, 1]), '(−12.35, 0, 1)')
})

test('diameters list biggest first, with counts', () => {
  assert.equal(
    diameterList([{ diameter: 8, count: 4 }, { diameter: 30, count: 1 }, { diameter: 80, count: 1 }]),
    'Ø80 · Ø30 · 4× Ø8',
  )
  assert.equal(diameterList([]), 'none')
})
