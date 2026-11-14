//! Spatial index over a face's projected trim loops in (u, v).
//!
//! The curved-face mesher asks two questions of thousands of points per face:
//! "is this point inside the trimmed domain?" and "is it within `r` of a trim
//! segment?". Answered against every segment of every loop, each question
//! costs O(boundary): on an imported NURBS face with a 6 000-sample outline
//! and a 100 × 100 Steiner grid that is tens of millions of segment visits
//! (each with two `atan2` calls) per face, per refinement pass. The index
//! buckets segments into a uniform grid once per face, so a query looks only
//! at the segments near it.
//!
//! "Inside" uses the integer winding number (nonzero rule): inside the outer
//! loop, outside every hole, whatever each loop's orientation. That agrees
//! with the angle-sum winding test it replaces everywhere except exactly on
//! a segment, where both are ambiguous; callers keep points clear of the
//! boundary with [`TrimIndex::near`] (and the triangulator drops any free
//! point that still lands on a segment).

/// Segments of the trim loops bucketed into a uniform grid.
pub(crate) struct TrimIndex {
    segments: Vec<[(f64, f64); 2]>,
    /// Loop index of each segment: 0 = outer, 1.. = holes.
    segment_loop: Vec<u32>,
    loops: usize,
    lo: (f64, f64),
    cell: (f64, f64),
    nx: usize,
    ny: usize,
    /// Segments whose v-range meets each grid row (for the winding count).
    rows: Vec<Vec<u32>>,
    /// Segments whose bounding box meets each cell (for the distance query).
    cells: Vec<Vec<u32>>,
}

impl TrimIndex {
    /// Index the closed loops `outer` and `holes` (each given without the
    /// closing repeat of its first point).
    pub(crate) fn new(outer: &[(f64, f64)], holes: &[&[(f64, f64)]]) -> Self {
        let mut segments = Vec::new();
        let mut segment_loop = Vec::new();
        for (k, lp) in std::iter::once(outer)
            .chain(holes.iter().copied())
            .enumerate()
        {
            let n = lp.len();
            if n < 2 {
                continue;
            }
            for i in 0..n {
                let (a, b) = (lp[i], lp[(i + 1) % n]);
                if a != b {
                    segments.push([a, b]);
                    segment_loop.push(k as u32);
                }
            }
        }
        let (mut lo, mut hi) = (
            (f64::INFINITY, f64::INFINITY),
            (f64::NEG_INFINITY, f64::NEG_INFINITY),
        );
        for s in &segments {
            for p in s {
                lo = (lo.0.min(p.0), lo.1.min(p.1));
                hi = (hi.0.max(p.0), hi.1.max(p.1));
            }
        }
        if segments.is_empty()
            || !(lo.0.is_finite() && lo.1.is_finite() && hi.0.is_finite() && hi.1.is_finite())
        {
            return TrimIndex {
                segments: Vec::new(),
                segment_loop: Vec::new(),
                loops: 1 + holes.len(),
                lo: (0.0, 0.0),
                cell: (1.0, 1.0),
                nx: 1,
                ny: 1,
                rows: vec![Vec::new()],
                cells: vec![Vec::new()],
            };
        }
        // About one segment per cell on average; never degenerate.
        let side = ((segments.len() as f64).sqrt().ceil() as usize).clamp(1, 512);
        let (nx, ny) = (side, side);
        let span = ((hi.0 - lo.0).max(1e-300), (hi.1 - lo.1).max(1e-300));
        let cell = (span.0 / nx as f64, span.1 / ny as f64);
        let mut index = TrimIndex {
            segments,
            segment_loop,
            loops: 1 + holes.len(),
            lo,
            cell,
            nx,
            ny,
            rows: vec![Vec::new(); ny],
            cells: vec![Vec::new(); nx * ny],
        };
        for (id, s) in index.segments.iter().enumerate() {
            let (u0, u1) = (s[0].0.min(s[1].0), s[0].0.max(s[1].0));
            let (v0, v1) = (s[0].1.min(s[1].1), s[0].1.max(s[1].1));
            let (i0, i1) = (index.col(u0), index.col(u1));
            let (j0, j1) = (index.row(v0), index.row(v1));
            for j in j0..=j1 {
                index.rows[j].push(id as u32);
                for i in i0..=i1 {
                    index.cells[j * nx + i].push(id as u32);
                }
            }
        }
        index
    }

    fn col(&self, u: f64) -> usize {
        (((u - self.lo.0) / self.cell.0).floor().max(0.0) as usize).min(self.nx - 1)
    }

    fn row(&self, v: f64) -> usize {
        (((v - self.lo.1) / self.cell.1).floor().max(0.0) as usize).min(self.ny - 1)
    }

    /// Is `p` inside the trimmed domain: nonzero winding about the outer
    /// loop and zero about every hole?
    pub(crate) fn inside(&self, p: (f64, f64)) -> bool {
        if self.segments.is_empty() {
            return false;
        }
        let in_box = p.0 >= self.lo.0
            && p.1 >= self.lo.1
            && p.0 <= self.lo.0 + self.cell.0 * self.nx as f64
            && p.1 <= self.lo.1 + self.cell.1 * self.ny as f64;
        if !in_box {
            return false;
        }
        // Winding per loop from the segments crossing p's row only: a
        // segment that does not span p.v cannot contribute.
        let mut small = [0i32; 8];
        let mut large: Vec<i32>;
        let winding: &mut [i32] = if self.loops <= small.len() {
            &mut small[..self.loops]
        } else {
            large = vec![0; self.loops];
            &mut large
        };
        for &id in &self.rows[self.row(p.1)] {
            let [a, b] = self.segments[id as usize];
            let side = (b.0 - a.0) * (p.1 - a.1) - (p.0 - a.0) * (b.1 - a.1);
            let k = self.segment_loop[id as usize] as usize;
            if a.1 <= p.1 {
                if b.1 > p.1 && side > 0.0 {
                    winding[k] += 1;
                }
            } else if b.1 <= p.1 && side < 0.0 {
                winding[k] -= 1;
            }
        }
        winding[0] != 0 && winding[1..].iter().all(|&w| w == 0)
    }

    /// Is `p` closer than `r` to any trim segment?
    pub(crate) fn near(&self, p: (f64, f64), r: f64) -> bool {
        if self.segments.is_empty() || !(r > 0.0) {
            return false;
        }
        let (i0, i1) = (self.col(p.0 - r), self.col(p.0 + r));
        let (j0, j1) = (self.row(p.1 - r), self.row(p.1 + r));
        for j in j0..=j1 {
            for i in i0..=i1 {
                for &id in &self.cells[j * self.nx + i] {
                    let [a, b] = self.segments[id as usize];
                    if segment_distance(p, a, b) < r {
                        return true;
                    }
                }
            }
        }
        false
    }
}

impl TrimIndex {
    /// Every indexed segment whose bounding box, grown by `r`, contains `p`:
    /// a superset of the segments within `r` of `p`, each listed once.
    pub(crate) fn segments_near(&self, p: (f64, f64), r: f64) -> Vec<[(f64, f64); 2]> {
        if self.segments.is_empty() || !(r >= 0.0) {
            return Vec::new();
        }
        let (i0, i1) = (self.col(p.0 - r), self.col(p.0 + r));
        let (j0, j1) = (self.row(p.1 - r), self.row(p.1 + r));
        let mut ids: Vec<u32> = Vec::new();
        for j in j0..=j1 {
            for i in i0..=i1 {
                ids.extend_from_slice(&self.cells[j * self.nx + i]);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .map(|id| self.segments[id as usize])
            .collect()
    }

    /// Half the longest indexed segment: no point farther than this from
    /// every segment can lie in any segment's diametral circle.
    pub(crate) fn longest_half_segment(&self) -> f64 {
        self.segments
            .iter()
            .map(|[a, b]| 0.5 * ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt())
            .fold(0.0, f64::max)
    }
}

fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - qx).powi(2) + (p.1 - qy).powi(2)).sqrt()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::tessellation::surface::calculate_winding_number;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
    }

    fn star(rng: &mut Lcg, c: (f64, f64), r: f64, n: usize, ccw: bool) -> Vec<(f64, f64)> {
        let mut p: Vec<(f64, f64)> = (0..n)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / n as f64;
                let rr = r * (0.6 + 0.4 * rng.next());
                (c.0 + rr * a.cos(), c.1 + rr * a.sin())
            })
            .collect();
        if !ccw {
            p.reverse();
        }
        p
    }

    fn brute_near(p: (f64, f64), loops: &[&[(f64, f64)]], r: f64) -> f64 {
        let mut best = f64::INFINITY;
        for lp in loops {
            for i in 0..lp.len() {
                best = best.min(segment_distance(p, lp[i], lp[(i + 1) % lp.len()]));
            }
        }
        best - r
    }

    /// Against the brute-force angle-sum winding over every segment (the test
    /// the index replaces) and a brute-force distance: identical answers on
    /// 200 random domains x 400 random points, away from the ambiguous
    /// band right on a segment.
    #[test]
    fn the_index_answers_exactly_what_the_full_scan_answers() {
        let mut rng = Lcg(0xC0FFEE);
        let mut checked = 0;
        for case in 0..200 {
            let n = 3 + (rng.next() * 600.0) as usize;
            let outer = star(&mut rng, (0.0, 0.0), 10.0, n, case % 3 != 0);
            let mut holes = Vec::new();
            for h in 0..((rng.next() * 5.0) as usize).min(4) {
                let a = h as f64 * std::f64::consts::TAU / 4.0 + 0.3;
                let m = 3 + (rng.next() * 50.0) as usize;
                let ccw = rng.next() < 0.5;
                holes.push(star(&mut rng, (3.0 * a.cos(), 3.0 * a.sin()), 1.0, m, ccw));
            }
            let hole_refs: Vec<&[(f64, f64)]> = holes.iter().map(Vec::as_slice).collect();
            let index = TrimIndex::new(&outer, &hole_refs);
            let mut all: Vec<&[(f64, f64)]> = vec![&outer];
            all.extend(hole_refs.iter().copied());
            for _ in 0..400 {
                let p = (rng.next() * 26.0 - 13.0, rng.next() * 26.0 - 13.0);
                let r = rng.next() * 0.5;
                let gap = brute_near(p, &all, 0.0);
                if gap < 1e-9 {
                    continue; // on a segment: ambiguous by definition
                }
                let full = calculate_winding_number(&p, &outer).abs() > 0.5
                    && !holes
                        .iter()
                        .any(|h| calculate_winding_number(&p, h).abs() > 0.5);
                assert_eq!(index.inside(p), full, "case {case}: inside({p:?})");
                if (gap - r).abs() > 1e-12 {
                    assert_eq!(index.near(p, r), gap < r, "case {case}: near({p:?}, {r})");
                }
                checked += 1;
            }
        }
        assert!(checked > 70_000, "only {checked} points checked");
    }

    /// Many holes (more than the inline winding buffer) still classify.
    #[test]
    fn a_face_with_many_holes_classifies_every_hole() {
        let outer = vec![(0.0, 0.0), (100.0, 0.0), (100.0, 10.0), (0.0, 10.0)];
        let holes: Vec<Vec<(f64, f64)>> = (0..20)
            .map(|k| {
                let x = 2.0 + 5.0 * k as f64;
                vec![(x, 4.0), (x + 2.0, 4.0), (x + 2.0, 6.0), (x, 6.0)]
            })
            .collect();
        let refs: Vec<&[(f64, f64)]> = holes.iter().map(Vec::as_slice).collect();
        let index = TrimIndex::new(&outer, &refs);
        for k in 0..20 {
            let x = 3.0 + 5.0 * k as f64;
            assert!(
                !index.inside((x, 5.0)),
                "centre of hole {k} must be outside"
            );
            assert!(
                index.inside((x - 2.0, 5.0)),
                "gap before hole {k} must be inside"
            );
        }
        assert!(!index.inside((50.0, 11.0)) && !index.inside((-1.0, 5.0)));
    }
}
