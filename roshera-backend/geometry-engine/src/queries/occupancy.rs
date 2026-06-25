//! Occupancy spatial query — the "SDF X-ray" perception channel.
//!
//! Samples the solid's EXACT signed-distance field (`field::signed_distance`,
//! built on `nearest_on_solid` + ray-parity — never a tessellation lookup) at
//! the centre of every cell of a coarse `N×N×N` grid placed over the part's
//! world bounding box. A cell is INSIDE the material when its centre's signed
//! distance is `<= 0`.
//!
//! Where the shaded render (the "eye") can OCCLUDE internal structure and the
//! validity certificate reports a single verdict, the occupancy grid is a
//! non-deceivable structural X-ray: it reveals internal cavities, wall
//! thickness, gaps and through-holes directly, because each cell is decided by
//! the analytic SDF in isolation, not by what a camera can see.
//!
//! [`to_slice_stack`] serialises the grid as an ASCII slice-stack — one block
//! per z-layer, `'#'` for inside and `'.'` for outside, rows indexed by y and
//! columns by x. The format is fixed (validated in a perception experiment) so
//! it stays directly comparable across parts and across timeline states.

use super::field::signed_distance;
use super::region::face_world_box;
use crate::math::Point3;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// A coarse occupancy grid over a solid's (margin-expanded) world bounding box.
///
/// Cells are stored z-major → y → x (`cell(x, y, z)` at index
/// `(z * n + y) * n + x`), each `true` when the cell centre lies inside the
/// material (signed distance `<= 0`). The grid is cubic: `n × n × n` cells
/// spanning `[bbox_min, bbox_max]`, cell `(x, y, z)` centred at the midpoint of
/// its cell.
#[derive(Debug, Clone)]
pub struct OccupancyGrid {
    /// Cells per axis (`n × n × n` total).
    pub n: usize,
    /// Min corner of the sampled (margin-expanded) world box.
    pub bbox_min: Point3,
    /// Max corner of the sampled (margin-expanded) world box.
    pub bbox_max: Point3,
    /// Inside/outside flag per cell, z-major → y → x.
    pub cells: Vec<bool>,
    /// Fraction of cells inside the material, in `[0, 1]`.
    pub fill_fraction: f64,
}

impl OccupancyGrid {
    /// Flat index of cell `(x, y, z)` in z-major → y → x order.
    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.n + y) * self.n + x
    }

    /// Whether cell `(x, y, z)` is inside the material.
    #[inline]
    pub fn is_inside(&self, x: usize, y: usize, z: usize) -> bool {
        self.cells[self.idx(x, y, z)]
    }

    /// World position of the CENTRE of cell `(x, y, z)`.
    pub fn cell_center(&self, x: usize, y: usize, z: usize) -> Point3 {
        let span = |a: f64, b: f64, t: usize| a + (b - a) * ((t as f64 + 0.5) / self.n as f64);
        Point3::new(
            span(self.bbox_min.x, self.bbox_max.x, x),
            span(self.bbox_min.y, self.bbox_max.y, y),
            span(self.bbox_min.z, self.bbox_max.z, z),
        )
    }
}

/// World AABB of every face of `solid_id`, unioned. Immutable — uses
/// `region::face_world_box` (exact trim-edge sampling + analytic sphere
/// envelope) rather than the `&mut`-requiring `Solid::bounding_box`, so the
/// whole query stays on a read lock. `None` if the solid contributes no
/// geometry.
fn solid_world_box(model: &BRepModel, solid_id: SolidId) -> Option<(Point3, Point3)> {
    let solid = model.solids.get(solid_id)?;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);

    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut any = false;
    for sh in shells {
        let shell = match model.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for &fid in &shell.faces {
            if let Some(bb) = face_world_box(model, fid) {
                min[0] = min[0].min(bb.min.x);
                min[1] = min[1].min(bb.min.y);
                min[2] = min[2].min(bb.min.z);
                max[0] = max[0].max(bb.max.x);
                max[1] = max[1].max(bb.max.y);
                max[2] = max[2].max(bb.max.z);
                any = true;
            }
        }
    }
    if any {
        Some((
            Point3::new(min[0], min[1], min[2]),
            Point3::new(max[0], max[1], max[2]),
        ))
    } else {
        None
    }
}

/// Build an `n × n × n` occupancy grid over `solid_id`'s world bounding box,
/// expanded by `margin_frac` of each axis extent on every side (so the boundary
/// shows as a `'.'` border rather than being clipped). Each cell centre is
/// classified by the EXACT signed distance — inside when `sd <= 0`.
///
/// `n` is clamped to at least 1. A degenerate solid (no reachable boundary
/// face) yields an all-empty grid of the requested size, with the bbox
/// collapsed at the origin.
pub fn occupancy_grid(
    model: &BRepModel,
    solid: SolidId,
    n: usize,
    margin_frac: f64,
) -> OccupancyGrid {
    let n = n.max(1);
    let (raw_min, raw_max) = solid_world_box(model, solid).unwrap_or((Point3::ZERO, Point3::ZERO));

    // Expand by margin_frac of each extent, with a small floor so a perfectly
    // flat axis (e.g. a degenerate or planar extent) still has a non-zero span
    // to sample across.
    let margin = margin_frac.max(0.0);
    let pad = |lo: f64, hi: f64| -> (f64, f64) {
        let extent = hi - lo;
        let m = if extent > 0.0 {
            extent * margin
        } else {
            // No extent on this axis: give it a token half-width so the grid
            // has volume to sample (still centred on the slab).
            0.5
        };
        (lo - m, hi + m)
    };
    let (min_x, max_x) = pad(raw_min.x, raw_max.x);
    let (min_y, max_y) = pad(raw_min.y, raw_max.y);
    let (min_z, max_z) = pad(raw_min.z, raw_max.z);
    let bbox_min = Point3::new(min_x, min_y, min_z);
    let bbox_max = Point3::new(max_x, max_y, max_z);

    let mut grid = OccupancyGrid {
        n,
        bbox_min,
        bbox_max,
        cells: vec![false; n * n * n],
        fill_fraction: 0.0,
    };

    let mut inside = 0usize;
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let p = grid.cell_center(x, y, z);
                // Inside when the exact signed distance is <= 0. A solid with no
                // reachable boundary face yields `None` → treated as outside.
                let is_in = matches!(signed_distance(model, solid, p), Some((sd, _)) if sd <= 0.0);
                if is_in {
                    let idx = grid.idx(x, y, z);
                    grid.cells[idx] = true;
                    inside += 1;
                }
            }
        }
    }
    grid.fill_fraction = inside as f64 / (n * n * n) as f64;
    grid
}

/// Serialise an [`OccupancyGrid`] as the fixed ASCII slice-stack: for each
/// z-layer `k`, a header line `z=k`, then `n` rows of `n` characters, `'#'` for
/// inside and `'.'` for outside. Rows are indexed by y (top row `y=0`), columns
/// by x (left column `x=0`). Layers are emitted in ascending `z`.
pub fn to_slice_stack(grid: &OccupancyGrid) -> String {
    let n = grid.n;
    // Header line "z=k\n" + n rows of (n chars + "\n") per layer.
    let mut out = String::with_capacity(n * (6 + n * (n + 1)));
    for z in 0..n {
        out.push_str("z=");
        out.push_str(&z.to_string());
        out.push('\n');
        for y in 0..n {
            for x in 0..n {
                out.push(if grid.is_inside(x, y, z) { '#' } else { '.' });
            }
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector3;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    #[test]
    fn occupancy_box_is_a_filled_block() {
        // A 20³ box centred at origin. With a 0.1 margin the interior fills a
        // dense central block; the centre cell is inside; the corner cells (in
        // the margin border) are outside.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let g = occupancy_grid(&m, b, 12, 0.1);

        // A solid box is mostly filled: the 0.1 margin trims roughly the outer
        // ring, leaving a high fill fraction.
        assert!(
            g.fill_fraction > 0.45,
            "solid box should fill most of its bbox, got {}",
            g.fill_fraction
        );
        // Centre cells are inside.
        let c = g.n / 2;
        assert!(g.is_inside(c, c, c), "box centre cell must be inside");
        // Far corner (in the margin border) is outside.
        assert!(
            !g.is_inside(0, 0, 0),
            "box bbox corner cell must be outside (margin border)"
        );
        // Slice stack is well-formed: n layers, each "z=k" + n rows of n chars.
        let stack = to_slice_stack(&g);
        let lines: Vec<&str> = stack.lines().collect();
        assert_eq!(lines.len(), g.n * (g.n + 1), "stack line count");
        assert_eq!(lines[0], "z=0");
        assert!(lines[1].len() == g.n && lines[1].chars().all(|ch| ch == '#' || ch == '.'));
    }

    #[test]
    fn occupancy_hollow_tube_has_empty_core() {
        // A 50×50×16 plate with a Ø20 through-bore on the Z axis. On a mid-Z
        // slice the bore axis reads EMPTY ('.') even though it sits deep inside
        // the bbox — the X-ray sees through the hole a render could occlude.
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 50.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore difference");

        let g = occupancy_grid(&m, part, 21, 0.1);
        let mid = g.n / 2;
        // Centre column (on the bore axis) on the mid slice is EMPTY.
        assert!(
            !g.is_inside(mid, mid, mid),
            "bore axis cell must be empty (through-hole)"
        );
        // The material ring around it is SOLID: a cell well off-axis but still
        // inside the plate footprint, on the mid slice, is inside.
        // Cell index near x≈+18mm: bbox spans ~[-27.5,27.5] over n cells.
        let off = g.n - 4; // a column out toward +x, inside the 50mm plate
        assert!(
            g.is_inside(off, mid, mid),
            "material ring cell must be inside"
        );
        // The empty core must persist as a column across the mid slices, not be
        // a single stray cell: every interior z-layer reads empty on the axis.
        for z in (mid - 1)..=(mid + 1) {
            assert!(
                !g.is_inside(mid, mid, z),
                "bore must be an empty core column at z={z}"
            );
        }
    }

    #[test]
    fn occupancy_sphere_is_round() {
        // A radius-12 sphere at the origin: the centre is inside, the bbox
        // corners are outside (a box's corners would be filled — roundness is
        // the signature). The fill fraction sits near a sphere's volume ratio
        // (π/6 ≈ 0.524 of the tight bbox, lower once the 0.1 margin is added).
        let mut m = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ZERO, 12.0)
            .expect("sphere"));
        let g = occupancy_grid(&m, s, 16, 0.1);

        let c = g.n / 2;
        assert!(g.is_inside(c, c, c), "sphere centre must be inside");
        // All eight bbox corners are outside a round body.
        let last = g.n - 1;
        for &(x, y, z) in &[
            (0, 0, 0),
            (last, 0, 0),
            (0, last, 0),
            (0, 0, last),
            (last, last, 0),
            (last, 0, last),
            (0, last, last),
            (last, last, last),
        ] {
            assert!(
                !g.is_inside(x, y, z),
                "sphere bbox corner ({x},{y},{z}) must be empty"
            );
        }
        // Roundness sanity: filled, but clearly less than a box would be.
        assert!(
            g.fill_fraction > 0.2 && g.fill_fraction < 0.5,
            "sphere fill fraction {} should be sub-box (round)",
            g.fill_fraction
        );
    }
}
