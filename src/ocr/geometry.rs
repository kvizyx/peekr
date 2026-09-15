use image::{Rgb, RgbImage};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }

    fn dist(self, o: Point) -> f32 {
        let d = self.sub(o);
        (d.x * d.x + d.y * d.y).sqrt()
    }
}

/// Four corners ordered top-left, top-right, bottom-right, bottom-left.
pub type Quad = [Point; 4];

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
    pub fn grow(self, d: f32) -> Self {
        Self { width: self.width + 2.0 * d, height: self.height + 2.0 * d, ..self }
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
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut hull: Vec<Point> = Vec::with_capacity(pts.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        for &p in &pts {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
        if pass == 0 {
            pts.reverse();
        }
    }
    hull
}

/// Minimum area enclosing rectangle (rotating calipers over the convex hull).
pub fn min_area_rect(points: &[Point]) -> RotatedRect {
    let hull = convex_hull(points);
    if hull.len() < 3 {
        // Degenerate contour: a point or a line segment.
        let (a, b) = (hull[0], *hull.last().unwrap());
        let len = a.dist(b);
        let axis = if len > 0.0 { Point::new((b.x - a.x) / len, (b.y - a.y) / len) } else { Point::new(1.0, 0.0) };
        return RotatedRect { center: Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0), width: len, height: 0.0, axis };
    }

    let mut best: Option<(f32, RotatedRect)> = None;
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        let len = a.dist(b);
        if len == 0.0 {
            continue;
        }
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
            let (cu, cv) = ((min_u + max_u) / 2.0, (min_v + max_v) / 2.0);
            let center = Point::new(a.x + u.x * cu - u.y * cv, a.y + u.y * cu + u.x * cv);
            best = Some((area, RotatedRect { center, width: max_u - min_u, height: max_v - min_v, axis: u }));
        }
    }
    best.unwrap().1
}

/// Orders corners as top-left, top-right, bottom-right, bottom-left (same as PaddleOCR).
pub fn order_quad(mut q: Quad) -> Quad {
    q.sort_by(|a, b| a.x.total_cmp(&b.x));
    let (mut left, mut right) = ([q[0], q[1]], [q[2], q[3]]);
    left.sort_by(|a, b| a.y.total_cmp(&b.y));
    right.sort_by(|a, b| a.y.total_cmp(&b.y));
    [left[0], right[0], right[1], left[1]]
}

/// Quads tilted less than this (tan of the angle) are treated as horizontal.
const MAX_HORIZONTAL_SLOPE: f32 = 0.08;
/// Horizontal fragments closer than this many line heights are merged.
const MAX_WORD_GAP: f32 = 1.2;

/// The detector often splits one line into several boxes at wide word gaps, sometimes with
/// overlaps that make the recognizer output a character twice. Merges horizontal fragments
/// of the same line into a single box.
pub fn merge_line_fragments(quads: Vec<Quad>) -> Vec<Quad> {
    #[derive(Clone, Copy)]
    struct Bbox {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    }
    impl Bbox {
        fn height(&self) -> f32 {
            self.y1 - self.y0
        }
    }

    let (mut boxes, mut result) = (Vec::new(), Vec::new());
    for q in quads {
        let dx = q[1].x - q[0].x;
        let horizontal = dx > 0.0 && ((q[1].y - q[0].y) / dx).abs() < MAX_HORIZONTAL_SLOPE;
        if horizontal {
            let xs = q.map(|p| p.x);
            let ys = q.map(|p| p.y);
            boxes.push(Bbox {
                x0: xs.iter().copied().fold(f32::MAX, f32::min),
                y0: ys.iter().copied().fold(f32::MAX, f32::min),
                x1: xs.iter().copied().fold(f32::MIN, f32::max),
                y1: ys.iter().copied().fold(f32::MIN, f32::max),
            });
        } else {
            result.push(q);
        }
    }

    boxes.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    let mut merged: Vec<Bbox> = Vec::new();
    for b in boxes {
        // Boxes are sorted by left edge, so a fragment can only continue a box to its left.
        let target = merged.iter_mut().rev().find(|m| {
            let (hm, hb) = (m.height(), b.height());
            let overlap = m.y1.min(b.y1) - m.y0.max(b.y0);
            let similar_height = hm.max(hb) <= 1.6 * hm.min(hb);
            similar_height && overlap >= 0.6 * hm.min(hb) && b.x0 - m.x1 <= MAX_WORD_GAP * hm.max(hb)
        });
        match target {
            Some(m) => *m = Bbox { x0: m.x0, y0: m.y0.min(b.y0), x1: m.x1.max(b.x1), y1: m.y1.max(b.y1) },
            None => merged.push(b),
        }
    }

    result.extend(merged.into_iter().map(|b| {
        [Point::new(b.x0, b.y0), Point::new(b.x1, b.y0), Point::new(b.x1, b.y1), Point::new(b.x0, b.y1)]
    }));
    result
}

pub fn quad_contains(q: &Quad, p: Point) -> bool {
    let signs = (0..4).map(|i| cross(q[i], q[(i + 1) % 4], p));
    let (mut pos, mut neg) = (false, false);
    for s in signs {
        pos |= s > 0.0;
        neg |= s < 0.0;
    }
    !(pos && neg)
}

/// Cuts the quad out of the image into an upright rectangle.
///
/// Detected quads are rotated rectangles, so an affine mapping from three corners is exact.
pub fn crop_quad(img: &RgbImage, q: &Quad) -> RgbImage {
    let width = q[0].dist(q[1]).max(q[3].dist(q[2])).round().max(1.0) as u32;
    let height = q[0].dist(q[3]).max(q[1].dist(q[2])).round().max(1.0) as u32;
    let ex = q[1].sub(q[0]);
    let ey = q[3].sub(q[0]);

    let mut out = RgbImage::from_fn(width, height, |u, v| {
        let (fu, fv) = ((u as f32 + 0.5) / width as f32, (v as f32 + 0.5) / height as f32);
        let sx = q[0].x + ex.x * fu + ey.x * fv - 0.5;
        let sy = q[0].y + ex.y * fu + ey.y * fv - 0.5;
        sample_bilinear(img, sx, sy)
    });

    // Vertical text: rotate so characters run left to right.
    if height as f32 >= width as f32 * 1.5 {
        out = image::imageops::rotate270(&out);
    }
    out
}

fn sample_bilinear(img: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (w, h) = img.dimensions();
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let (p00, p10, p01, p11) = (img.get_pixel(x0, y0), img.get_pixel(x1, y0), img.get_pixel(x0, y1), img.get_pixel(x1, y1));
    Rgb(std::array::from_fn(|c| {
        let top = p00[c] as f32 * (1.0 - fx) + p10[c] as f32 * fx;
        let bottom = p01[c] as f32 * (1.0 - fx) + p11[c] as f32 * fx;
        (top * (1.0 - fy) + bottom * fy).round() as u8
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_area_rect_of_axis_aligned_box() {
        let pts = [Point::new(1.0, 2.0), Point::new(11.0, 2.0), Point::new(11.0, 6.0), Point::new(1.0, 6.0), Point::new(5.0, 4.0)];
        let r = min_area_rect(&pts);
        let (long, short) = (r.width.max(r.height), r.width.min(r.height));
        assert!((long - 10.0).abs() < 1e-4 && (short - 4.0).abs() < 1e-4, "{r:?}");
        assert!((r.center.x - 6.0).abs() < 1e-4 && (r.center.y - 4.0).abs() < 1e-4);
    }

    #[test]
    fn order_quad_starts_top_left() {
        let q = order_quad([Point::new(10.0, 5.0), Point::new(0.0, 5.0), Point::new(10.0, 0.0), Point::new(0.0, 0.0)]);
        assert_eq!(q, [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 5.0), Point::new(0.0, 5.0)]);
    }
}
