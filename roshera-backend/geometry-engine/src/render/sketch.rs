//! EYE-SKETCH: the agent eye for 2D sketches.
//!
//! A deterministic, GPU-free rasterizer that turns a [`Sketch`] into a PNG a
//! vision-capable agent (or a human) can actually look at — the 2D analogue of
//! [`super::dimensioned`] for solids. Today only *solids* render
//! (`render_part`/`scene_view`); a raw sketch was invisible to the agent. This is
//! the precondition for semantic recognition ("is this a gear?"): the agent
//! cannot recognise what it cannot see.
//!
//! Lines, polylines, and circles are drawn; standalone points get a small marker.
//! The view auto-frames the sketch's content with a uniform scale and a flipped
//! Y (image Y points down). Deterministic: the same sketch yields identical bytes,
//! so renders can be snapshot-tested like volumes.

use super::dimensioned::draw_line_pub;
use crate::sketch2d::line2d::LineGeometry;
use crate::sketch2d::{Point2d, Sketch};

const BG: [u8; 3] = [250, 250, 250];
/// Geometry ink.
const INK: [u8; 3] = [25, 35, 60];
/// Standalone-point marker ink.
const POINT_INK: [u8; 3] = [180, 40, 40];
const WIDTH: usize = 800;
const HEIGHT: usize = 800;
const MARGIN: f64 = 48.0;
/// Circle tessellation for drawing (a closed N-gon).
const CIRCLE_SEGS: usize = 96;

/// A rendered sketch image: an RGB framebuffer that encodes to PNG.
pub struct SketchFrame {
    pub width: usize,
    pub height: usize,
    /// `width * height * 3` RGB bytes.
    pub pixels: Vec<u8>,
}

impl SketchFrame {
    /// Encode the framebuffer as a PNG (RGB, 8-bit) — what the agent eye returns.
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().map_err(|e| format!("png header: {e}"))?;
            w.write_image_data(&self.pixels)
                .map_err(|e| format!("png data: {e}"))?;
        }
        Ok(out)
    }
}

/// Collect every drawable segment of the sketch as world-space endpoint pairs,
/// plus the standalone points (drawn as markers).
fn collect_geometry(sketch: &Sketch) -> (Vec<(Point2d, Point2d)>, Vec<Point2d>) {
    let mut segs: Vec<(Point2d, Point2d)> = Vec::new();
    let mut pts: Vec<Point2d> = Vec::new();

    // Lines — draw the segment variant over its endpoints. Infinite lines and
    // rays have no finite extent to frame, so they are skipped (a sketch profile
    // is built from segments).
    for entry in sketch.lines().iter() {
        if let LineGeometry::Segment(s) = &entry.value().geometry {
            segs.push((s.start, s.end));
        }
    }
    // Polylines — each segment (handles the closing edge for closed polylines).
    for entry in sketch.polylines().iter() {
        for s in entry.value().polyline.segments() {
            segs.push((s.start, s.end));
        }
    }
    // Circles — a closed N-gon approximation.
    for entry in sketch.circles().iter() {
        let c = &entry.value().circle;
        let mut prev = Point2d::new(c.center.x + c.radius, c.center.y);
        for k in 1..=CIRCLE_SEGS {
            let t = (k as f64) / (CIRCLE_SEGS as f64) * std::f64::consts::TAU;
            let cur = Point2d::new(
                c.center.x + c.radius * t.cos(),
                c.center.y + c.radius * t.sin(),
            );
            segs.push((prev, cur));
            prev = cur;
        }
    }
    // Standalone points.
    for entry in sketch.points().iter() {
        pts.push(entry.value().position);
    }

    (segs, pts)
}

/// Render a sketch to a deterministic PNG framebuffer. Returns `None` when the
/// sketch has no drawable geometry (nothing to frame).
pub fn render_sketch(sketch: &Sketch) -> Option<SketchFrame> {
    let (segs, pts) = collect_geometry(sketch);
    if segs.is_empty() && pts.is_empty() {
        return None;
    }

    // World bounds over every endpoint + standalone point.
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (a, b) in &segs {
        for p in [a, b] {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
    }
    for p in &pts {
        min_x = min_x.min(p.x);
        min_y = min_y.min(p.y);
        max_x = max_x.max(p.x);
        max_y = max_y.max(p.y);
    }
    if !min_x.is_finite() || !max_x.is_finite() {
        return None;
    }

    let span_x = (max_x - min_x).max(1e-9);
    let span_y = (max_y - min_y).max(1e-9);
    // Uniform scale so the drawing fits inside the margins.
    let sx = (WIDTH as f64 - 2.0 * MARGIN) / span_x;
    let sy = (HEIGHT as f64 - 2.0 * MARGIN) / span_y;
    let scale = sx.min(sy);
    let cx = (min_x + max_x) * 0.5;
    let cy = (min_y + max_y) * 0.5;
    let to_px = |p: &Point2d| -> (f64, f64) {
        let px = WIDTH as f64 * 0.5 + (p.x - cx) * scale;
        let py = HEIGHT as f64 * 0.5 - (p.y - cy) * scale; // image Y points down
        (px, py)
    };

    let mut pixels = vec![0u8; WIDTH * HEIGHT * 3];
    for chunk in pixels.chunks_exact_mut(3) {
        chunk.copy_from_slice(&BG);
    }

    for (a, b) in &segs {
        let (x0, y0) = to_px(a);
        let (x1, y1) = to_px(b);
        draw_line_pub(&mut pixels, WIDTH, HEIGHT, x0, y0, x1, y1, INK);
    }
    // Points as small plus-markers so vertices are visible against the lines.
    for p in &pts {
        let (px, py) = to_px(p);
        draw_line_pub(
            &mut pixels,
            WIDTH,
            HEIGHT,
            px - 3.0,
            py,
            px + 3.0,
            py,
            POINT_INK,
        );
        draw_line_pub(
            &mut pixels,
            WIDTH,
            HEIGHT,
            px,
            py - 3.0,
            px,
            py + 3.0,
            POINT_INK,
        );
    }

    Some(SketchFrame {
        width: WIDTH,
        height: HEIGHT,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch2d::{Point2d, Sketch, SketchAnchor};

    #[test]
    fn renders_a_triangle_to_nonblank_png() {
        let sketch = Sketch::new("tri".to_string(), SketchAnchor::xy());
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(20.0, 0.0));
        let c = sketch.add_point(Point2d::new(10.0, 15.0));
        sketch.add_line(a, b).expect("a-b");
        sketch.add_line(b, c).expect("b-c");
        sketch.add_line(c, a).expect("c-a");

        let frame = render_sketch(&sketch).expect("triangle renders");
        assert_eq!(frame.pixels.len(), frame.width * frame.height * 3);
        let ink = frame.pixels.chunks_exact(3).filter(|c| *c != BG).count();
        assert!(
            ink > 100,
            "expected ink on the canvas, got {ink} non-bg pixels"
        );

        let png = frame.to_png().expect("png encodes");
        assert!(
            png.len() > 8 && &png[1..4] == b"PNG",
            "output must be a valid PNG"
        );
    }

    #[test]
    fn determinism_same_sketch_same_bytes() {
        let make = || {
            let s = Sketch::new("c".to_string(), SketchAnchor::xy());
            s.add_circle(Point2d::new(0.0, 0.0), 5.0).expect("circle");
            render_sketch(&s).expect("renders").pixels
        };
        assert_eq!(make(), make(), "the sketch eye must be deterministic");
    }

    #[test]
    fn empty_sketch_renders_nothing() {
        let sketch = Sketch::new("empty".to_string(), SketchAnchor::xy());
        assert!(render_sketch(&sketch).is_none());
    }

    /// Writes a PNG into `target/` (gitignored) for visual inspection — a house
    /// outline (square base + peaked roof). Run with `-- --ignored`.
    #[test]
    #[ignore = "writes a PNG to target/ for visual inspection"]
    fn demo_render_house_to_target_png() {
        let s = Sketch::new("house".to_string(), SketchAnchor::xy());
        s.add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(10.0, 0.0),
                Point2d::new(10.0, 8.0),
                Point2d::new(5.0, 13.0),
                Point2d::new(0.0, 8.0),
            ],
            true,
        )
        .expect("house outline");
        let png = render_sketch(&s).expect("renders").to_png().expect("png");
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/target/sketch_house_demo.png");
        std::fs::write(path, &png).expect("write png");
    }
}
