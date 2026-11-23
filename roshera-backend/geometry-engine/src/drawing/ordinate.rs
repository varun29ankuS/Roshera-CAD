//! Ordinate dimensioning (ISO 129-1): a ladder of locations measured from
//! one datum drawn as values written at the ends of their extension lines,
//! in one band, from a `0` at the datum -- no dimension lines, no
//! arrowheads. Design: `docs/research/2026-11-21-ordinate-dimensioning.md`.
//!
//! This module holds the pure geometry of one band: where each value is
//! written along the band when locations are closer together than a value
//! is tall ([`spread`]), and the extension line that reaches it, jogged when
//! the value had to move ([`extension_line`]).

/// Fewest locations from one datum, in one view and one direction, that are
/// drawn as an ordinate ladder rather than as a stack of dimensions (Varun
/// 2026-11-22: "10 or more locations from one datum in a view"; about 20 of
/// the 44 bench parts). Fewer stay stacked baseline dimensions.
pub(crate) const ORDINATE_MIN_LADDER: usize = 10;

/// The ladder among `spans` (each a location's `(lo, hi)` along one
/// direction): the datum coordinate the most of them share as one end
/// (within `eps`) with their other ends all on ONE side of it, and those
/// spans' indices, when at least `min` do. A baseline ladder runs one way
/// from its datum; a span on the other side ends ON the datum's coordinate
/// at its feature and is measured from somewhere else (CADGenBench 124's
/// TOP: "1.5" from -30 to -28.5, beside nine locations from -28.5), so it
/// is not a member. A span whose two ends both lie on the datum locates
/// nothing. Ties go to the lower datum coordinate, so the answer does not
/// depend on the order of `spans`.
pub(crate) fn find_ladder(spans: &[(f64, f64)], min: usize, eps: f64) -> Option<(f64, Vec<usize>)> {
    let on = |x: f64, d: f64| (x - d).abs() <= eps;
    let members_of = |d: f64| -> Vec<usize> {
        let (mut above, mut below) = (Vec::new(), Vec::new());
        for (i, &(lo, hi)) in spans.iter().enumerate() {
            if on(lo, d) && !on(hi, d) {
                above.push(i);
            } else if on(hi, d) && !on(lo, d) {
                below.push(i);
            }
        }
        if below.len() > above.len() {
            below
        } else {
            above
        }
    };
    let mut best: Option<(f64, Vec<usize>)> = None;
    for &(lo, hi) in spans {
        for d in [lo, hi] {
            let m = members_of(d);
            let better = match &best {
                None => true,
                Some((bd, bm)) => m.len() > bm.len() || (m.len() == bm.len() && d < *bd - eps),
            };
            if better {
                best = Some((d, m));
            }
        }
    }
    best.filter(|(_, m)| m.len() >= min)
}

/// Where each value is written along the band, in the input order: as near
/// its feature as it can be, no two closer than `pitch` (a value's height
/// across the band plus a gap), the order of the features kept.
///
/// Least squares under the pitch constraint: with the features sorted and
/// `y_i = x_i - i * pitch`, the positions are `z_i + i * pitch` where `z` is
/// the best non-decreasing fit to `y` (pool adjacent violators). A run of
/// close features is spread about its own middle; features already a pitch
/// apart do not move.
pub(crate) fn spread(features: &[f64], pitch: f64) -> Vec<f64> {
    let n = features.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        features[a]
            .partial_cmp(&features[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Pool adjacent violators over y = x - i * pitch: blocks of (sum, count).
    let mut blocks: Vec<(f64, usize)> = Vec::with_capacity(n);
    for (rank, &i) in order.iter().enumerate() {
        blocks.push((features[i] - rank as f64 * pitch, 1));
        while blocks.len() >= 2 {
            let (s1, c1) = blocks[blocks.len() - 1];
            let (s0, c0) = blocks[blocks.len() - 2];
            if s0 / c0 as f64 <= s1 / c1 as f64 {
                break;
            }
            blocks.pop();
            if let Some(last) = blocks.last_mut() {
                *last = (s0 + s1, c0 + c1);
            }
        }
    }
    let mut out = vec![0.0; n];
    let mut rank = 0usize;
    for (sum, count) in blocks {
        let z = sum / count as f64;
        for _ in 0..count {
            out[order[rank]] = z + rank as f64 * pitch;
            rank += 1;
        }
    }
    out
}

/// The extension line of one value, along-the-band coordinate first: from
/// the part (`from_edge`, across the band) to the band (`to_band`), at the
/// feature's coordinate `feature`; jogged when the value is written at
/// `text_at` away from it -- straight to the knee `knee` (across), then
/// slanted to the value. `[straight, slant]`, the slant degenerate when not
/// jogged. Points are `[along, across]`.
pub(crate) fn extension_line(
    feature: f64,
    text_at: f64,
    from_edge: f64,
    knee: f64,
    to_band: f64,
) -> [[[f64; 2]; 2]; 2] {
    if (text_at - feature).abs() < 1e-9 {
        let foot = [feature, to_band];
        return [[[feature, from_edge], foot], [foot, foot]];
    }
    let bend = [feature, knee];
    [[[feature, from_edge], bend], [bend, [text_at, to_band]]]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ladder is the datum the most locations share, whichever end of
    /// their spans it is; fewer than the minimum is no ladder.
    ///
    /// Mutation: count members by `lo` only -> the datum at the high end
    /// (the second block) is missed -> RED.
    #[test]
    fn the_ladder_is_the_most_shared_datum_end() {
        // Ten locations measured from x = 0, and three from x = 100 written
        // with the datum as their HIGH end.
        let mut spans: Vec<(f64, f64)> = (1..=10).map(|k| (0.0, 5.0 * k as f64)).collect();
        spans.extend([(70.0, 100.0), (80.0, 100.0), (90.0, 100.0)]);
        let (datum, members) = find_ladder(&spans, 10, 1e-6).expect("a ladder of ten");
        assert_eq!(datum, 0.0);
        assert_eq!(members, (0..10).collect::<Vec<_>>());
        assert!(
            find_ladder(&spans[..9], 10, 1e-6).is_none(),
            "nine is no ladder"
        );
        // From the high end: ten spans ending on x = 100.
        let high: Vec<(f64, f64)> = (1..=10).map(|k| (100.0 - 5.0 * k as f64, 100.0)).collect();
        let (datum, members) = find_ladder(&high, 10, 1e-6).expect("a ladder from the high end");
        assert_eq!(datum, 100.0);
        assert_eq!(members.len(), 10);
    }

    /// A location measured from elsewhere TO the datum's coordinate is not a
    /// member: nine locations from -28.5 and "1.5" from -30 to -28.5
    /// (CADGenBench 124's TOP) are no ladder of ten.
    ///
    /// Mutation: count both sides of the datum together -> ten members, a
    /// band is drawn, and the dense-stack integration test of 124's TOP
    /// places one dimension too many -> RED.
    #[test]
    fn a_span_ending_on_the_datum_from_the_other_side_is_not_a_member() {
        let mut spans: Vec<(f64, f64)> = (1..=9).map(|k| (-28.5, -28.5 + 5.0 * k as f64)).collect();
        spans.push((-30.0, -28.5));
        assert!(find_ladder(&spans, 10, 1e-6).is_none(), "nine, not ten");
        spans.push((-28.5, 30.0));
        let (datum, members) = find_ladder(&spans, 10, 1e-6).expect("ten on one side");
        assert_eq!(datum, -28.5);
        assert!(!members.contains(&9), "the span from -30 is not a member");
    }

    /// Features a pitch or more apart are written at their features.
    #[test]
    fn values_far_apart_stay_on_their_features() {
        let x = [10.0, 30.0, 4.0, 50.0];
        assert_eq!(spread(&x, 5.0), x.to_vec());
    }

    /// Close features are spread a pitch apart, in their order, about the
    /// run's own middle (least displacement); others do not move.
    ///
    /// Mutation: skip the pooling (keep `y` as is) -> the run overlaps -> RED.
    #[test]
    fn close_values_are_spread_in_order_about_their_middle() {
        let x = [20.0, 21.0, 22.0, 60.0, 0.0];
        let t = spread(&x, 5.0);
        // The run 20, 21, 22 (middle 21) becomes 16, 21, 26.
        for (got, want) in t.iter().zip([16.0, 21.0, 26.0, 60.0, 0.0]) {
            assert!((got - want).abs() < 1e-9, "{t:?}");
        }
        let mut sorted = t.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for w in sorted.windows(2) {
            assert!(w[1] - w[0] >= 5.0 - 1e-9, "{t:?}");
        }
    }

    /// A run that, spread, reaches a neighbour pushes it too (the blocks
    /// pool): 0, 1 and 7 at a pitch of 5 are -2, 3, 8 -- not 7 left behind
    /// inside 3's pitch.
    #[test]
    fn a_spread_run_pushes_its_neighbour() {
        let t = spread(&[0.0, 1.0, 7.0], 5.0);
        for w in t.windows(2) {
            assert!(w[1] - w[0] >= 5.0 - 1e-9, "{t:?}");
        }
        let mean = t.iter().sum::<f64>() / 3.0;
        assert!(
            (mean - 8.0 / 3.0).abs() < 1e-9,
            "spread about the middle: {t:?}"
        );
    }

    /// The extension line runs straight to the band when the value is on its
    /// feature, and is jogged at the knee when the value moved.
    #[test]
    fn a_moved_value_gets_a_jogged_extension_line() {
        let straight = extension_line(10.0, 10.0, 100.0, 108.0, 112.0);
        assert_eq!(straight[0], [[10.0, 100.0], [10.0, 112.0]]);
        assert_eq!(straight[1][0], straight[1][1], "no slant");
        let jogged = extension_line(10.0, 14.0, 100.0, 108.0, 112.0);
        assert_eq!(jogged[0], [[10.0, 100.0], [10.0, 108.0]]);
        assert_eq!(jogged[1], [[10.0, 108.0], [14.0, 112.0]]);
    }
}
