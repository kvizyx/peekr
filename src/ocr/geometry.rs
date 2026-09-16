//! Planar geometry for text boxes: rotated rectangles, quads and cropping.

use image::{Rgb, RgbImage, imageops};

/// Quads tilted less than this (tangent of the angle) are treated as horizontal.
const MAX_HORIZONTAL_SLOPE: f32 = 0.08;
/// Horizontal fragments closer than this many line heights are merged.
const MAX_WORD_GAP: f32 = 1.2;
/// Fragments whose heights differ more than this factor are never merged.
const MAX_HEIGHT_RATIO: f32 = 1.6;
/// Minimal vertical overlap of two fragments, relative to the shorter one.
const MIN_LINE_OVERLAP: f32 = 0.6;
/// Crops at least this much taller than wide are treated as vertical text.
const VERTICAL_TEXT_RATIO: f32 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }

    fn dist(self, other: Self) -> f32 {
        let d = self.sub(other);
        d.x.hypot(d.y)
    }
}

/// Four corners ordered top-left, top-right, bottom-right, bottom-left.
pub type Quad = [Point; 4];

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    pub min: Point,
    pub max: Point,
}

impl BoundingBox {
    pub fn of_quad(quad: &Quad) -> Self {
        let mut min = Point::new(f32::MAX, f32::MAX);
        let mut max = Point::new(f32::MIN, f32::MIN);

        for p in quad {
            min = Point::new(min.x.min(p.x), min.y.min(p.y));
            max = Point::new(max.x.max(p.x), max.y.max(p.y));
        }

        Self { min, max }
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    /// Length of the vertical range shared with `other`; negative if they do not overlap.
    pub fn vertical_overlap(&self, other: &Self) -> f32 {
        self.max.y.min(other.max.y) - self.min.y.max(other.min.y)
    }

    fn union(&self, other: &Self) -> Self {
        Self {
            min: Point::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Point::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    fn to_quad(self) -> Quad {
        [
            self.min,
            Point::new(self.max.x, self.min.y),
            self.max,
            Point::new(self.min.x, self.max.y),
        ]
    }
}

/// A rotated rectangle.
#[derive(Debug, Clone, Copy)]
pub struct RotatedRect {
    pub center: Point,
    pub width: f32,
    pub height: f32,
    /// Unit vector along the `width` side.
    pub axis: Point,
}

impl RotatedRect {
    /// Moves every side outwards by `distance`.
    pub fn grow(self, distance: f32) -> Self {
        Self {
            width: self.width + 2.0 * distance,
            height: self.height + 2.0 * distance,
            ..self
        }
    }

    pub fn corners(&self) -> Quad {
        let (ux, uy) = (self.axis.x * self.width / 2.0, self.axis.y * self.width / 2.0);
        let (vx, vy) = (-self.axis.y * self.height / 2.0, self.axis.x * self.height / 2.0);
        let c = self.center;

        [
            Point::new(c.x - ux - vx, c.y - uy - vy),
            Point::new(c.x + ux - vx, c.y + uy - vy),
            Point::new(c.x + ux + vx, c.y + uy + vy),
            Point::new(c.x - ux + vx, c.y - uy + vy),
        ]
    }
}

fn cross(o: Point, a: Point, b: Point) -> f32 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// Andrew's monotone chain; returns the hull counter-clockwise (in y-up terms).
fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    sorted.dedup();

    if sorted.len() < 3 {
        return sorted;
    }

    let mut hull: Vec<Point> = Vec::with_capacity(sorted.len() * 2);

    // First pass builds the lower hull, second pass (over reversed points) the upper one.
    for pass in 0..2 {
        let start = hull.len();

        for &p in &sorted {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }

        // The last point of each chain is the first point of the other one.
        hull.pop();
        if pass == 0 {
            sorted.reverse();
        }
    }

    hull
}

/// Minimum area enclosing rectangle (rotating calipers over the convex hull).
/// Returns `None` for an empty point set.
pub fn min_area_rect(points: &[Point]) -> Option<RotatedRect> {
    let hull = convex_hull(points);

    if hull.len() < 3 {
        // Degenerate contour: a point or a line segment.
        let (&a, &b) = (hull.first()?, hull.last()?);
        let len = a.dist(b);
        let axis = if len > 0.0 {
            Point::new((b.x - a.x) / len, (b.y - a.y) / len)
        } else {
            Point::new(1.0, 0.0)
        };
        let center = Point::new(f32::midpoint(a.x, b.x), f32::midpoint(a.y, b.y));

        return Some(RotatedRect {
            center,
            width: len,
            height: 0.0,
            axis,
        });
    }

    let mut best: Option<(f32, RotatedRect)> = None;

    for (i, &a) in hull.iter().enumerate() {
        let b = hull[(i + 1) % hull.len()];
        let len = a.dist(b);
        if len == 0.0 {
            continue;
        }

        // Project the hull onto the edge direction `u` and its normal.
        let u = Point::new((b.x - a.x) / len, (b.y - a.y) / len);
        let (mut min_u, mut max_u, mut min_v, mut max_v) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);

        for p in &hull {
            let d = p.sub(a);
            let pu = d.x * u.x + d.y * u.y;
            let pv = -d.x * u.y + d.y * u.x;

            min_u = min_u.min(pu);
            max_u = max_u.max(pu);
            min_v = min_v.min(pv);
            max_v = max_v.max(pv);
        }

        let area = (max_u - min_u) * (max_v - min_v);
        if best.as_ref().is_none_or(|(best_area, _)| area < *best_area) {
            let (cu, cv) = (f32::midpoint(min_u, max_u), f32::midpoint(min_v, max_v));
            let center = Point::new(a.x + u.x * cu - u.y * cv, a.y + u.y * cu + u.x * cv);
            let rect = RotatedRect {
                center,
                width: max_u - min_u,
                height: max_v - min_v,
                axis: u,
            };

            best = Some((area, rect));
        }
    }

    best.map(|(_, rect)| rect)
}

/// Orders corners as top-left, top-right, bottom-right, bottom-left (same as PaddleOCR).
pub fn order_quad(mut quad: Quad) -> Quad {
    quad.sort_by(|a, b| a.x.total_cmp(&b.x));

    let (mut left, mut right) = ([quad[0], quad[1]], [quad[2], quad[3]]);
    left.sort_by(|a, b| a.y.total_cmp(&b.y));
    right.sort_by(|a, b| a.y.total_cmp(&b.y));

    [left[0], right[0], right[1], left[1]]
}

pub fn quad_contains(quad: &Quad, p: Point) -> bool {
    let (mut positive, mut negative) = (false, false);

    for i in 0..4 {
        let side = cross(quad[i], quad[(i + 1) % 4], p);
        positive |= side > 0.0;
        negative |= side < 0.0;
    }

    !(positive && negative)
}

/// The detector often splits one line into several boxes at wide word gaps, sometimes with
/// overlaps that make the recognizer output a character twice. Merges horizontal fragments
/// of the same line into a single box.
pub fn merge_line_fragments(quads: Vec<Quad>) -> Vec<Quad> {
    let (horizontal, mut result): (Vec<Quad>, Vec<Quad>) = quads.into_iter().partition(|q| {
        let dx = q[1].x - q[0].x;
        dx > 0.0 && ((q[1].y - q[0].y) / dx).abs() < MAX_HORIZONTAL_SLOPE
    });

    let mut boxes: Vec<BoundingBox> = horizontal.iter().map(BoundingBox::of_quad).collect();
    boxes.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));

    let mut merged: Vec<BoundingBox> = Vec::new();

    for fragment in boxes {
        // Boxes are sorted by left edge, so a fragment can only continue a box to its left.
        let line = merged.iter_mut().rev().find(|line| {
            let (shorter, taller) = (
                line.height().min(fragment.height()),
                line.height().max(fragment.height()),
            );
            let similar_height = taller <= MAX_HEIGHT_RATIO * shorter;
            let same_line = line.vertical_overlap(&fragment) >= MIN_LINE_OVERLAP * shorter;
            let close = fragment.min.x - line.max.x <= MAX_WORD_GAP * taller;

            similar_height && same_line && close
        });

        match line {
            Some(line) => *line = line.union(&fragment),
            None => merged.push(fragment),
        }
    }

    result.extend(merged.into_iter().map(BoundingBox::to_quad));
    result
}

/// Cuts the quad out of the image into an upright rectangle.
///
/// Detected quads are rotated rectangles, so an affine mapping from three corners is exact.
pub fn crop_quad(img: &RgbImage, quad: &Quad) -> RgbImage {
    let [top_left, top_right, bottom_right, bottom_left] = *quad;

    let width = top_left
        .dist(top_right)
        .max(bottom_left.dist(bottom_right))
        .round()
        .max(1.0) as u32;
    let height = top_left
        .dist(bottom_left)
        .max(top_right.dist(bottom_right))
        .round()
        .max(1.0) as u32;
    let x_axis = top_right.sub(top_left);
    let y_axis = bottom_left.sub(top_left);

    let crop = RgbImage::from_fn(width, height, |u, v| {
        let fu = (u as f32 + 0.5) / width as f32;
        let fv = (v as f32 + 0.5) / height as f32;
        let x = top_left.x + x_axis.x * fu + y_axis.x * fv - 0.5;
        let y = top_left.y + x_axis.y * fu + y_axis.y * fv - 0.5;

        sample_bilinear(img, x, y)
    });

    // Vertical text: rotate so characters run left to right.
    if height as f32 >= width as f32 * VERTICAL_TEXT_RATIO {
        imageops::rotate270(&crop)
    } else {
        crop
    }
}

fn sample_bilinear(img: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (w, h) = img.dimensions();
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);

    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);

    let (p00, p10) = (img.get_pixel(x0, y0), img.get_pixel(x1, y0));
    let (p01, p11) = (img.get_pixel(x0, y1), img.get_pixel(x1, y1));

    Rgb(std::array::from_fn(|c| {
        let top = f32::from(p00[c]) * (1.0 - fx) + f32::from(p10[c]) * fx;
        let bottom = f32::from(p01[c]) * (1.0 - fx) + f32::from(p11[c]) * fx;
        (top * (1.0 - fy) + bottom * fy).round() as u8
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_quad(x0: f32, y0: f32, x1: f32, y1: f32) -> Quad {
        BoundingBox {
            min: Point::new(x0, y0),
            max: Point::new(x1, y1),
        }
        .to_quad()
    }

    #[test]
    fn min_area_rect_of_axis_aligned_box() {
        let points = [
            Point::new(1.0, 2.0),
            Point::new(11.0, 2.0),
            Point::new(11.0, 6.0),
            Point::new(1.0, 6.0),
            Point::new(5.0, 4.0),
        ];

        let rect = min_area_rect(&points).expect("non-empty input");

        let (long, short) = (rect.width.max(rect.height), rect.width.min(rect.height));
        assert!((long - 10.0).abs() < 1e-4 && (short - 4.0).abs() < 1e-4, "{rect:?}");
        assert!((rect.center.x - 6.0).abs() < 1e-4 && (rect.center.y - 4.0).abs() < 1e-4);
    }

    #[test]
    fn min_area_rect_of_nothing() {
        assert!(min_area_rect(&[]).is_none());
    }

    #[test]
    fn order_quad_starts_top_left() {
        let quad = order_quad([
            Point::new(10.0, 5.0),
            Point::new(0.0, 5.0),
            Point::new(10.0, 0.0),
            Point::new(0.0, 0.0),
        ]);

        assert_eq!(quad, rect_quad(0.0, 0.0, 10.0, 5.0));
    }

    #[test]
    fn merges_fragments_of_one_line() {
        let fragments = vec![rect_quad(60.0, 10.0, 120.0, 30.0), rect_quad(0.0, 11.0, 50.0, 31.0)];

        assert_eq!(merge_line_fragments(fragments), vec![rect_quad(0.0, 10.0, 120.0, 31.0)]);
    }

    #[test]
    fn keeps_separate_lines_and_distant_columns() {
        let boxes = vec![
            rect_quad(0.0, 0.0, 100.0, 20.0),
            rect_quad(0.0, 40.0, 100.0, 60.0),
            rect_quad(300.0, 0.0, 400.0, 20.0),
        ];

        assert_eq!(merge_line_fragments(boxes).len(), 3);
    }
}
