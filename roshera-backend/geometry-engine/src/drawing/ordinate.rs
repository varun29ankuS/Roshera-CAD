//! Ordinate dimensioning (ISO 129-1): a ladder of locations measured from
//! one datum drawn as values written at the ends of their extension lines,
//! in one band, from a `0` at the datum -- no dimension lines, no
//! arrowheads. Design: `docs/research/2026-11-21-ordinate-dimensioning.md`.
//!
//! This module holds the pure geometry of one band: where each value is
//! written along the band when locations are closer together than a value
//! is tall ([`spread`]), and the extension line that reaches it, jogged when
//! the value had to move ([`extension_line`]).

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
