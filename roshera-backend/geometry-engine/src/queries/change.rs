//! Where material changed between two states of a model.
//!
//! "What did that cut remove? Did the boss land where I meant?" -- answered
//! from the solid itself, not a picture. Both states are sampled on ONE grid
//! over their combined bounds; each sample is the EXACT inside/outside of the
//! material (the analytic signed distance with the pseudonormal sign,
//! [`signed_distance`], which handles every surface kind -- a fillet's torus
//! included). Cells inside in one state and outside in the other are grouped
//! into connected regions, each `Added` or `Removed`.
//!
//! "Material" is the UNION of every solid in a state: a boolean consumes its
//! operands and mints new solids, so solids are never paired by id.
//!
//! What is exact and what is sampled is stated, never blurred: each sample's
//! inside/outside is exact; a region's extent, centre and volume are estimates
//! at the grid's cell size `cell`. A change thinner than a cell can fall
//! between samples, so this query alone never proves "no change" -- callers
//! pair it with the states' exact volumes (see the api-server route).

use super::field::signed_distance;
use super::occupancy::solid_world_box;
use crate::math::Point3;
use crate::primitives::topology_builder::BRepModel;

/// Whether a region gained or lost material from the first state to the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// Material in the second state where the first had none.
    Added,
    /// Material in the first state the second no longer has.
    Removed,
}

/// One connected region of change, at the grid's cell size.
#[derive(Debug, Clone)]
pub struct ChangeRegion {
    pub kind: ChangeKind,
    /// Grid cells in the region.
    pub cells: usize,
    /// `cells * cell^3` -- an estimate at the cell size, not an exact volume.
    pub volume_estimate: f64,
    /// Mean of the region's cell centres.
    pub center: Point3,
    /// Bounds of the region's cells.
    pub min: Point3,
    pub max: Point3,
    /// The kind of surface that bounds the change -- the nearer of the two
    /// states' surfaces to the region (see `naming_surface`): it names the
    /// feature ("Cylinder" for a drilled hole). A kind, never a face id: the
    /// states may be rebuilt scratch models.
    pub surface: Option<&'static str>,
}

/// The change between two states.
#[derive(Debug, Clone)]
pub struct MaterialChange {
    /// Edge length of a (cubic) grid cell.
    pub cell: f64,
    /// Cells per axis.
    pub dims: [usize; 3],
    /// The sampled box: both states' combined bounds, padded by one cell.
    pub min: Point3,
    pub max: Point3,
    /// Regions, largest first (then by centre x, y, z): deterministic.
    pub regions: Vec<ChangeRegion>,
}

/// Union of every solid's world box in `model`, or `None` when it has none.
fn model_box(model: &BRepModel) -> Option<(Point3, Point3)> {
    let mut acc: Option<(Point3, Point3)> = None;
    for (sid, _) in model.solids.iter() {
        if let Some((lo, hi)) = solid_world_box(model, sid) {
            acc = Some(match acc {
                None => (lo, hi),
                Some((a, b)) => (
                    Point3::new(a.x.min(lo.x), a.y.min(lo.y), a.z.min(lo.z)),
                    Point3::new(b.x.max(hi.x), b.y.max(hi.y), b.z.max(hi.z)),
                ),
            });
        }
    }
    acc
}

/// Whether `p` lies in the material of ANY solid of `model` (exact signed
/// distance <= 0, the rule the occupancy X-ray uses).
fn in_material(model: &BRepModel, p: Point3) -> bool {
    model
        .solids
        .iter()
        .any(|(sid, _)| matches!(signed_distance(model, sid, p), Some((sd, _)) if sd <= 0.0))
}

/// The kind of surface nearest `p` over every solid of `model`, with its
/// distance.
fn nearest_surface(model: &BRepModel, p: Point3) -> Option<(f64, &'static str)> {
    let mut best: Option<(f64, crate::primitives::face::FaceId)> = None;
    for (sid, _) in model.solids.iter() {
        if let Some((sd, face)) = signed_distance(model, sid, p) {
            if best.is_none_or(|(d, _)| sd.abs() < d) {
                best = Some((sd.abs(), face));
            }
        }
    }
    let (d, face) = best?;
    let face = model.faces.get(face)?;
    model
        .surfaces
        .get(face.surface_id)
        .map(|s| (d, s.type_name()))
}

/// The surface that names a change at `probe`: the nearer of the two states'
/// nearest surfaces -- the one that bounds the change. A drilled hole is
/// bounded by its cylinder wall in the state AFTER the cut (the block before
/// it has only distant planes there); a hole filled back in, by its wall in
/// the state BEFORE. On a tie, the state that has the material (`prefer`).
fn naming_surface(
    before: &BRepModel,
    after: &BRepModel,
    probe: Point3,
    prefer_after: bool,
) -> Option<&'static str> {
    match (
        nearest_surface(before, probe),
        nearest_surface(after, probe),
    ) {
        (None, None) => None,
        (Some((_, k)), None) | (None, Some((_, k))) => Some(k),
        (Some((db, kb)), Some((da, ka))) => {
            let tie = (db - da).abs() <= 1e-9 * (1.0 + db.max(da));
            if tie {
                Some(if prefer_after { ka } else { kb })
            } else if da < db {
                Some(ka)
            } else {
                Some(kb)
            }
        }
    }
}

/// Where material changed from `before` to `after`, sampled with about `n`
/// cells along the longest axis (clamped to 4..=48: the cost is cubic).
pub fn material_change(before: &BRepModel, after: &BRepModel, n: usize) -> MaterialChange {
    let n = n.clamp(4, 48);
    let empty = |cell: f64| MaterialChange {
        cell,
        dims: [0, 0, 0],
        min: Point3::ZERO,
        max: Point3::ZERO,
        regions: Vec::new(),
    };
    let (lo, hi) = match (model_box(before), model_box(after)) {
        (None, None) => return empty(0.0),
        (Some(a), None) | (None, Some(a)) => a,
        (Some((a0, a1)), Some((b0, b1))) => (
            Point3::new(a0.x.min(b0.x), a0.y.min(b0.y), a0.z.min(b0.z)),
            Point3::new(a1.x.max(b1.x), a1.y.max(b1.y), a1.z.max(b1.z)),
        ),
    };
    let extent = [hi.x - lo.x, hi.y - lo.y, hi.z - lo.z];
    let longest = extent.iter().cloned().fold(0.0_f64, f64::max);
    if longest <= 0.0 {
        return empty(0.0);
    }
    let cell = longest / n as f64;
    // One cell of padding on every side, so material at the bounds is sampled.
    let dims = [0, 1, 2].map(|i| ((extent[i] / cell).ceil() as usize).max(1) + 2);
    let origin = Point3::new(lo.x - cell, lo.y - cell, lo.z - cell);
    let center_of = |x: usize, y: usize, z: usize| {
        Point3::new(
            origin.x + (x as f64 + 0.5) * cell,
            origin.y + (y as f64 + 0.5) * cell,
            origin.z + (z as f64 + 0.5) * cell,
        )
    };
    let idx = |x: usize, y: usize, z: usize| (z * dims[1] + y) * dims[0] + x;
    let total = dims[0] * dims[1] * dims[2];

    // 0 = unchanged, 1 = added, 2 = removed.
    let mut diff = vec![0u8; total];
    for z in 0..dims[2] {
        for y in 0..dims[1] {
            for x in 0..dims[0] {
                let p = center_of(x, y, z);
                let a = in_material(before, p);
                let b = in_material(after, p);
                diff[idx(x, y, z)] = match (a, b) {
                    (false, true) => 1,
                    (true, false) => 2,
                    _ => 0,
                };
            }
        }
    }

    // Connected regions (6-neighbour), labelled in index order: deterministic.
    let mut seen = vec![false; total];
    let mut regions = Vec::new();
    for start in 0..total {
        if diff[start] == 0 || seen[start] {
            continue;
        }
        let tag = diff[start];
        let mut stack = vec![start];
        seen[start] = true;
        let mut cells = Vec::new();
        while let Some(i) = stack.pop() {
            cells.push(i);
            let x = i % dims[0];
            let y = (i / dims[0]) % dims[1];
            let z = i / (dims[0] * dims[1]);
            let mut visit = |nx: usize, ny: usize, nz: usize| {
                let j = idx(nx, ny, nz);
                if !seen[j] && diff[j] == tag {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(x - 1, y, z);
            }
            if x + 1 < dims[0] {
                visit(x + 1, y, z);
            }
            if y > 0 {
                visit(x, y - 1, z);
            }
            if y + 1 < dims[1] {
                visit(x, y + 1, z);
            }
            if z > 0 {
                visit(x, y, z - 1);
            }
            if z + 1 < dims[2] {
                visit(x, y, z + 1);
            }
        }

        let (mut sx, mut sy, mut sz) = (0.0, 0.0, 0.0);
        let mut rmin = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut rmax = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for &i in &cells {
            let c = center_of(
                i % dims[0],
                (i / dims[0]) % dims[1],
                i / (dims[0] * dims[1]),
            );
            sx += c.x;
            sy += c.y;
            sz += c.z;
            let h = cell / 2.0;
            rmin = Point3::new(
                rmin.x.min(c.x - h),
                rmin.y.min(c.y - h),
                rmin.z.min(c.z - h),
            );
            rmax = Point3::new(
                rmax.x.max(c.x + h),
                rmax.y.max(c.y + h),
                rmax.z.max(c.z + h),
            );
        }
        let k = cells.len() as f64;
        let center = Point3::new(sx / k, sy / k, sz / k);
        let kind = if tag == 1 {
            ChangeKind::Added
        } else {
            ChangeKind::Removed
        };
        // The cell nearest the centroid: a point that IS in the changed
        // material (the centroid of a ring-shaped change need not be).
        let probe = cells
            .iter()
            .map(|&i| {
                center_of(
                    i % dims[0],
                    (i / dims[0]) % dims[1],
                    i / (dims[0] * dims[1]),
                )
            })
            .min_by(|a, b| {
                let da =
                    (a.x - center.x).powi(2) + (a.y - center.y).powi(2) + (a.z - center.z).powi(2);
                let db =
                    (b.x - center.x).powi(2) + (b.y - center.y).powi(2) + (b.z - center.z).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(center);
        let surface = naming_surface(before, after, probe, kind == ChangeKind::Added);
        regions.push(ChangeRegion {
            kind,
            cells: cells.len(),
            volume_estimate: k * cell.powi(3),
            center,
            min: rmin,
            max: rmax,
            surface,
        });
    }
    regions.sort_by(|a, b| {
        b.cells
            .cmp(&a.cells)
            .then(
                a.center
                    .x
                    .partial_cmp(&b.center.x)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(
                a.center
                    .y
                    .partial_cmp(&b.center.y)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(
                a.center
                    .z
                    .partial_cmp(&b.center.z)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    MaterialChange {
        cell,
        dims,
        min: origin,
        max: Point3::new(
            origin.x + dims[0] as f64 * cell,
            origin.y + dims[1] as f64 * cell,
            origin.z + dims[2] as f64 * cell,
        ),
        regions,
    }
}
