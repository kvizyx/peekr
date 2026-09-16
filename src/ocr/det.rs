//! Text detection with a PP-OCR DB (Differentiable Binarization) model.

use std::path::Path;

use anyhow::{Context, Result};
use image::{GrayImage, Luma, RgbImage, imageops};
use imageproc::contours::{BorderType, find_contours};

use super::geometry::{self, BoundingBox, Point, Quad};
use super::models::DetectorParams;
use super::onnx::OnnxModel;
use super::preprocess::to_bgr_chw;

const MIN_BOX_SIDE: f32 = 3.0;
const MAX_CANDIDATES: usize = 1000;

/// Images whose shorter side is smaller than this are upscaled before detection.
/// Screen text is rendered crisp, so larger upscaling only costs time without improving results.
const MIN_SIDE: f32 = 128.0;
const MAX_SIDE: f32 = 2048.0;
/// The model downsamples by 32, so input sides must be multiples of it.
const SIDE_MULTIPLE: f32 = 32.0;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

pub struct Detector {
    model: OnnxModel,
    params: DetectorParams,
}

impl Detector {
    pub fn load(path: &Path, params: DetectorParams) -> Result<Self> {
        let model = OnnxModel::load(path).with_context(|| format!("loading detection model {}", path.display()))?;

        Ok(Self { model, params })
    }

    /// Returns text boxes in `img` coordinates.
    pub fn detect(&mut self, img: &RgbImage) -> Result<Vec<Quad>> {
        let (w, h) = img.dimensions();
        let (input_w, input_h) = input_size(w, h);
        let resized = imageops::resize(img, input_w, input_h, imageops::FilterType::Triangle);

        let input = to_bgr_chw(&resized, |c, v| (v / 255.0 - MEAN[c]) / STD[c]);
        let shape = [1, 3, input_h as usize, input_w as usize];
        let params = self.params;

        let quads = self.model.run(shape, &input, |map_shape, probabilities| {
            let map = ProbabilityMap {
                data: probabilities,
                params,
                width: map_shape[3] as u32,
                height: map_shape[2] as u32,
            };

            let (sx, sy) = (w as f32 / map.width as f32, h as f32 / map.height as f32);
            map.text_boxes()
                .into_iter()
                .map(|quad| quad.map(|p| Point::new(p.x * sx, p.y * sy)))
                .collect()
        })?;

        Ok(quads)
    }
}

/// Picks the model input size: small images are upscaled, huge ones downscaled.
fn input_size(w: u32, h: u32) -> (u32, u32) {
    let (w, h) = (w as f32, h as f32);

    let mut ratio = 1.0;
    if w.min(h) < MIN_SIDE {
        ratio = MIN_SIDE / w.min(h);
    }
    if w.max(h) * ratio > MAX_SIDE {
        ratio = MAX_SIDE / w.max(h);
    }

    let round = |side: f32| ((side * ratio / SIDE_MULTIPLE).round().max(1.0) * SIDE_MULTIPLE) as u32;
    (round(w), round(h))
}

/// Per-pixel text probability produced by the detection model.
struct ProbabilityMap<'a> {
    data: &'a [f32],
    params: DetectorParams,
    width: u32,
    height: u32,
}

impl ProbabilityMap<'_> {
    fn at(&self, x: u32, y: u32) -> f32 {
        self.data[(y * self.width + x) as usize]
    }

    /// DB post-processing: binarize, trace contours, fit and expand rectangles.
    fn text_boxes(&self) -> Vec<Quad> {
        let mut quads = Vec::new();

        let contours = find_contours::<i32>(&self.binarize())
            .into_iter()
            .filter(|c| c.border_type == BorderType::Outer)
            .take(MAX_CANDIDATES);

        for contour in contours {
            let points: Vec<Point> = contour
                .points
                .iter()
                .map(|p| Point::new(p.x as f32, p.y as f32))
                .collect();

            let Some(rect) = geometry::min_area_rect(&points) else {
                continue;
            };

            if rect.width.min(rect.height) < MIN_BOX_SIDE
                || self.mean_inside(&rect.corners()) < self.params.box_threshold
            {
                continue;
            }

            // The model predicts shrunk text kernels. Unclipping a rectangle by distance d grows each
            // side by 2d (the rounded corners of polygon offsetting do not change its bounding rectangle).
            let area = rect.width * rect.height;
            let distance = area * self.params.unclip_ratio / (2.0 * (rect.width + rect.height));
            let grown = rect.grow(distance);
            if grown.width.min(grown.height) < MIN_BOX_SIDE + 2.0 {
                continue;
            }

            let (max_x, max_y) = (self.width as f32, self.height as f32);
            let quad = grown
                .corners()
                .map(|p| Point::new(p.x.clamp(0.0, max_x), p.y.clamp(0.0, max_y)));
            quads.push(geometry::order_quad(quad));
        }

        quads
    }

    /// Thresholds the map, optionally with a 2x2 dilation (see [`DetectorParams::dilate`]).
    fn binarize(&self) -> GrayImage {
        let is_text = |x: u32, y: u32| self.at(x, y) > self.params.bin_threshold;

        GrayImage::from_fn(self.width, self.height, |x, y| {
            let dilated = || {
                (x > 0 && is_text(x - 1, y))
                    || (y > 0 && is_text(x, y - 1))
                    || (x > 0 && y > 0 && is_text(x - 1, y - 1))
            };
            let hit = is_text(x, y) || (self.params.dilate && dilated());

            Luma([if hit { 255 } else { 0 }])
        })
    }

    /// Mean probability of the pixels inside the quad.
    fn mean_inside(&self, quad: &Quad) -> f32 {
        let bounds = BoundingBox::of_quad(quad);
        let x_range = pixel_range(bounds.min.x, bounds.max.x, self.width);
        let y_range = pixel_range(bounds.min.y, bounds.max.y, self.height);

        let (mut sum, mut count) = (0.0, 0u32);
        for y in y_range {
            for x in x_range.clone() {
                if geometry::quad_contains(quad, Point::new(x as f32, y as f32)) {
                    sum += self.at(x, y);
                    count += 1;
                }
            }
        }

        if count == 0 { 0.0 } else { sum / count as f32 }
    }
}

/// Pixel indices covering `min..=max`, clamped to `0..len`.
fn pixel_range(min: f32, max: f32, len: u32) -> std::ops::RangeInclusive<u32> {
    let first = min.floor().max(0.0) as u32;
    let last = max.ceil().min((len - 1) as f32) as u32;

    first..=last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_size_is_a_multiple_of_32() {
        assert_eq!(input_size(1000, 300), (992, 288));
    }

    #[test]
    fn small_images_are_upscaled_and_huge_ones_downscaled() {
        assert_eq!(input_size(200, 40), (640, 128));
        assert_eq!(input_size(8000, 1000), (2048, 256));
    }

    #[test]
    fn finds_a_box_around_a_bright_blob() {
        let (width, height) = (64, 32);
        let data: Vec<f32> = (0..width * height)
            .map(|i| {
                if (8..24).contains(&(i / width)) && (10..50).contains(&(i % width)) {
                    0.9
                } else {
                    0.0
                }
            })
            .collect();

        let params = super::super::models::DETECTORS[0].params;
        let map = ProbabilityMap {
            data: &data,
            params,
            width,
            height,
        };

        let boxes = map.text_boxes();

        assert_eq!(boxes.len(), 1);
        let bounds = BoundingBox::of_quad(&boxes[0]);
        assert!(bounds.min.x < 10.0 && bounds.max.x > 50.0, "{bounds:?}");
    }
}
