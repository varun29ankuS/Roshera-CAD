// Mirror of roshera-backend/geometry-engine/examples/common/mod.rs
// `DemoManifestEntry` / `DemoManifest`. Keep these two in sync — the
// gallery loads `/demos/manifest.json` produced by the kernel demos
// (ROSHERA_DEMO_OUT=../roshera-app/public/demos cargo run --release
// --example demo_X).

export interface DemoEntry {
  category: string
  filename: string
  // Path relative to the manifest's directory, forward-slashed (e.g.
  // "primitives/box.stl").
  stl_path: string
  verts: number
  tris: number
  tess_ms: number
}

export interface DemoManifest {
  demos: DemoEntry[]
}

// Friendly display metadata for a category. Falls back to category name
// when not listed.
export interface CategoryInfo {
  title: string
  description: string
}

export const CATEGORY_INFO: Record<string, CategoryInfo> = {
  primitives: {
    title: 'Primitives',
    description: 'Box, sphere, cylinder, cone, torus — every B-Rep primitive the kernel ships.',
  },
  booleans: {
    title: 'Booleans',
    description: 'Union, difference, intersection across box / sphere / cylinder pairs.',
  },
  extrude_revolve: {
    title: 'Extrude & Revolve',
    description: 'Sketch-driven extrusion and revolution — rectangles, L-profiles, rings.',
  },
  sweep_loft: {
    title: 'Sweep & Loft',
    description: 'Profile sweep along a path; loft across multiple cross-sections.',
  },
  features: {
    title: 'Features',
    description: 'Edge fillet and chamfer — feature-based blending on existing solids.',
  },
  transforms: {
    title: 'Transforms',
    description: 'Translate, rotate, scale — bbox round-trips through the transform pipeline.',
  },
  pattern_draft: {
    title: 'Pattern & Draft',
    description: 'Linear and circular patterns; draft-angle face modification.',
  },
}

export function categoryInfo(category: string): CategoryInfo {
  return (
    CATEGORY_INFO[category] ?? {
      title: category,
      description: '',
    }
  )
}
