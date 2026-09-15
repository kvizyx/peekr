//! Text detection with a PP-OCR DB (Differentiable Binarization) model.

use std::path::Path;

use anyhow::{Context, Result};
use image::{GrayImage, Luma, RgbImage, imageops};
use imageproc::contours::{BorderType, find_contours};
use ort::session::Session;
use ort::value::TensorRef;

use super::geometry::{Point, Quad, min_area_rect};

/// Pixels of the probability map above this value are considered text.
const BIN_THRESHOLD: f32 = 0.3;
/// Boxes whose mean probability is below this are dropped.
const BOX_THRESHOLD: f32 = 0.5;
/// How much to expand a shrunk text kernel back to the full text box.
const UNCLIP_RATIO: f32 = 1.6;
const MIN_BOX_SIDE: f32 = 3.0;
const MAX_CANDIDATES: usize = 1000;

/// Images whose shorter side is smaller than this are upscaled before detection.
/// Screen text is rendered crisp, so larger upscaling only costs time without improving results.
const MIN_SIDE: u32 = 128;
const MAX_SIDE: u32 = 2048;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

pub struct Detector {
    session: Session,
}

impl Detector {
    pub fn load(path: &Path) -> Result<Self> {
        let session = super::load_session(path)
            .with_context(|| format!("loading detection model {}", path.display()))?;
        Ok(Self { session })
    }

    /// Returns text boxes in `img` coordinates.
    pub fn detect(&mut self, img: &RgbImage) -> Result<Vec<Quad>> {
        let (w, h) = img.dimensions();
        let (rw, rh) = detection_size(w, h);
        let resized = imageops::resize(img, rw, rh, imageops::FilterType::Triangle);

        let input = to_bgr_chw(&resized, |c, v| (v / 255.0 - MEAN[c]) / STD[c]);
        let shape = [1usize, 3, rh as usize, rw as usize];
        let outputs = self.session.run(ort::inputs![TensorRef::from_array_view((shape, &*input))?])?;
        let (out_shape, pred) = outputs[0].try_extract_tensor::<f32>()?;
        let (mh, mw) = (out_shape[2] as u32, out_shape[3] as u32);

        let quads = boxes_from_map(pred, mw, mh);
        let (sx, sy) = (w as f32 / mw as f32, h as f32 / mh as f32);
        Ok(quads
            .into_iter()
            .map(|q| q.map(|p| Point::new(p.x * sx, p.y * sy)))
            .collect())
    }
}

/// Picks a model input size: both sides are multiples of 32, small images are upscaled.
fn detection_size(w: u32, h: u32) -> (u32, u32) {
    let (w, h) = (w as f32, h as f32);
    let mut ratio = 1.0f32;
    if w.min(h) < MIN_SIDE as f32 {
        ratio = MIN_SIDE as f32 / w.min(h);
    }
    if w.max(h) * ratio > MAX_SIDE as f32 {
        ratio = MAX_SIDE as f32 / w.max(h);
    }
    let round32 = |v: f32| (((v * ratio) / 32.0).round() as u32).max(1) * 32;
    (round32(w), round32(h))
}

/// Converts an RGB image into a planar BGR float tensor (PaddleOCR models are trained on BGR).
pub(super) fn to_bgr_chw(img: &RgbImage, normalize: impl Fn(usize, f32) -> f32) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let plane = (w * h) as usize;
    let mut data = vec![0f32; plane * 3];
    for (i, px) in img.pixels().enumerate() {
        // Output channel 0 is blue, 2 is red; normalization constants follow the output order.
        data[i] = normalize(0, px[2] as f32);
        data[plane + i] = normalize(1, px[1] as f32);
        data[2 * plane + i] = normalize(2, px[0] as f32);
    }
    data
}

fn boxes_from_map(pred: &[f32], w: u32, h: u32) -> Vec<Quad> {
    let is_text = |x: u32, y: u32| pred[(y * w + x) as usize] > BIN_THRESHOLD;
    // Binarize with a 2x2 dilation: merges characters separated by thin gaps into one line.
    let bitmap = GrayImage::from_fn(w, h, |x, y| {
        let hit = is_text(x, y)
            || (x > 0 && is_text(x - 1, y))
            || (y > 0 && is_text(x, y - 1))
            || (x > 0 && y > 0 && is_text(x - 1, y - 1));
        Luma([if hit { 255 } else { 0 }])
    });

    let mut quads = Vec::new();
    for contour in find_contours::<i32>(&bitmap)
        .into_iter()
        .filter(|c| c.border_type == BorderType::Outer)
        .take(MAX_CANDIDATES)
    {
        let points: Vec<Point> = contour
            .points
            .iter()
            .map(|p| Point::new(p.x as f32, p.y as f32))
            .collect();
        let rect = min_area_rect(&points);
        if rect.width.min(rect.height) < MIN_BOX_SIDE {
            continue;
        }
        if box_score(pred, w, h, &rect.corners()) < BOX_THRESHOLD {
            continue;
        }

        // Unclipping a rectangle by distance d grows each side by 2d (the rounded corners
        // produced by polygon offsetting do not change its minimum area rectangle).
        let d = rect.width * rect.height * UNCLIP_RATIO / (2.0 * (rect.width + rect.height));
        let grown = rect.grow(d);
        if grown.width.min(grown.height) < MIN_BOX_SIDE + 2.0 {
            continue;
        }
        let quad = grown.corners().map(|p| {
            Point::new(p.x.clamp(0.0, w as f32), p.y.clamp(0.0, h as f32))
        });
        quads.push(super::geometry::order_quad(quad));
    }
    quads
}

/// Mean probability inside the quad.
fn box_score(pred: &[f32], w: u32, h: u32, quad: &Quad) -> f32 {
    let min_x = quad.iter().map(|p| p.x).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let max_x = quad.iter().map(|p| p.x).fold(f32::MIN, f32::max).ceil().min((w - 1) as f32) as u32;
    let min_y = quad.iter().map(|p| p.y).fold(f32::MAX, f32::min).floor().max(0.0) as u32;
    let max_y = quad.iter().map(|p| p.y).fold(f32::MIN, f32::max).ceil().min((h - 1) as f32) as u32;

    let (mut sum, mut count) = (0f32, 0u32);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if super::geometry::quad_contains(quad, Point::new(x as f32, y as f32)) {
                sum += pred[(y * w + x) as usize];
                count += 1;
            }
        }
    }
    if count == 0 { 0.0 } else { sum / count as f32 }
}
