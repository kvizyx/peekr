//! PP-OCRv5 pipeline: detect text boxes, crop them, recognize each line, assemble text.

mod det;
mod geometry;
mod preprocess;
mod rec;
mod text;

use std::path::{Path, PathBuf};

use anyhow::Result;
use image::RgbaImage;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

use self::det::Detector;
use self::geometry::Quad;
use self::rec::Recognizer;
pub use self::text::assemble_text;

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
        Self {
            det: root.join("det").join("det.onnx"),
            rec: root.join("rec").join(lang).join("rec.onnx"),
        }
    }
}

/// A recognized piece of text and where it was found.
#[derive(Debug, Clone)]
pub struct TextLine {
    pub text: String,
    quad: Quad,
}

pub struct OcrEngine {
    detector: Detector,
    recognizer: Recognizer,
}

impl OcrEngine {
    pub fn load(paths: &ModelPaths) -> Result<Self> {
        Ok(Self {
            detector: Detector::load(&paths.det)?,
            recognizer: Recognizer::load(&paths.rec)?,
        })
    }

    pub fn recognize(&mut self, img: &RgbaImage) -> Result<Vec<TextLine>> {
        let img = preprocess::pad_with_border_color(img, PADDING);
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

fn load_session(path: &Path) -> ort::Result<Session> {
    Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .commit_from_file(path)
}
