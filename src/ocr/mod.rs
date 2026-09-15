//! PP-OCRv5 pipeline: detect text boxes, crop them, recognize each line, assemble text.

mod det;
mod geometry;
mod rec;
mod text;

use std::path::{Path, PathBuf};

use anyhow::Result;
use image::{Rgb, RgbImage, RgbaImage};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

use geometry::Quad;

/// Lines recognized with lower confidence are dropped.
const MIN_LINE_SCORE: f32 = 0.5;
/// Border added around the selection so text touching the edges is still detected.
const PADDING: u32 = 24;

pub struct ModelPaths {
    pub det: PathBuf,
    pub rec: PathBuf,
}

impl ModelPaths {
    /// Layout: `<root>/det/det.onnx`, `<root>/rec/<lang>/rec.onnx` (+ `dict.txt`).
    pub fn in_dir(root: &Path, lang: &str) -> Self {
        Self { det: root.join("det").join("det.onnx"), rec: root.join("rec").join(lang).join("rec.onnx") }
    }
}

pub struct OcrEngine {
    detector: det::Detector,
    recognizer: rec::Recognizer,
}

#[derive(Debug, Clone)]
pub struct TextLine {
    pub text: String,
    quad: Quad,
}

impl OcrEngine {
    pub fn load(paths: &ModelPaths) -> Result<Self> {
        Ok(Self { detector: det::Detector::load(&paths.det)?, recognizer: rec::Recognizer::load(&paths.rec)? })
    }

    pub fn recognize(&mut self, img: &RgbaImage) -> Result<Vec<TextLine>> {
        let img = pad(img, PADDING);
        let quads = geometry::merge_line_fragments(self.detector.detect(&img)?);
        let mut lines = Vec::with_capacity(quads.len());
        for quad in quads {
            let crop = geometry::crop_quad(&img, &quad);
            let (text, score) = self.recognizer.recognize(&crop)?;
            log::debug!("line {score:.2} {text:?}");
            if score >= MIN_LINE_SCORE && !text.is_empty() {
                lines.push(TextLine { text, quad });
            }
        }
        Ok(lines)
    }
}

/// Joins recognized boxes into text in reading order: boxes on the same row are separated
/// by spaces, rows by newlines.
pub fn assemble_text(lines: &[TextLine]) -> String {
    struct Row<'a> {
        top: f32,
        bottom: f32,
        items: Vec<&'a TextLine>,
    }

    let bounds = |l: &TextLine| {
        let ys = l.quad.map(|p| p.y);
        (ys.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MIN, f32::max))
    };

    let mut sorted: Vec<&TextLine> = lines.iter().collect();
    sorted.sort_by(|a, b| bounds(a).0.total_cmp(&bounds(b).0));

    let mut rows: Vec<Row> = Vec::new();
    for line in sorted {
        let (top, bottom) = bounds(line);
        // Same row if the boxes overlap vertically by at least half of the shorter one.
        let same_row = rows.last_mut().filter(|row| {
            let overlap = bottom.min(row.bottom) - top.max(row.top);
            overlap >= 0.5 * (bottom - top).min(row.bottom - row.top)
        });
        match same_row {
            Some(row) => {
                row.top = row.top.min(top);
                row.bottom = row.bottom.max(bottom);
                row.items.push(line);
            }
            None => rows.push(Row { top, bottom, items: vec![line] }),
        }
    }

    rows.iter_mut()
        .map(|row| {
            row.items.sort_by(|a, b| a.quad[0].x.total_cmp(&b.quad[0].x));
            let joined = row.items.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
            text::fix_mixed_scripts(&joined)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Converts to RGB and pads with the average border color.
fn pad(img: &RgbaImage, pad: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for (x, y, px) in img.enumerate_pixels() {
        if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
            for c in 0..3 {
                sum[c] += px[c] as u64;
            }
            n += 1;
        }
    }
    let fill = Rgb(sum.map(|s| (s / n.max(1)) as u8));

    let mut out = RgbImage::from_pixel(w + 2 * pad, h + 2 * pad, fill);
    for (x, y, px) in img.enumerate_pixels() {
        out.put_pixel(x + pad, y + pad, Rgb([px[0], px[1], px[2]]));
    }
    out
}

fn load_session(path: &Path) -> ort::Result<Session> {
    Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .commit_from_file(path)
}
