//! Text line recognition with a PP-OCR CTC model.

use std::path::Path;

use anyhow::{Context, Result, bail};
use image::{RgbImage, imageops};
use ort::session::Session;
use ort::value::TensorRef;

use super::preprocess::to_bgr_chw;

const INPUT_HEIGHT: u32 = 48;
/// PaddleOCR pads narrow lines to at least this width.
const MIN_INPUT_WIDTH: u32 = 320;

pub struct Recognizer {
    session: Session,
    /// Output class index -> text. Index 0 is the CTC blank.
    charset: Vec<String>,
}

impl Recognizer {
    /// Loads the model; the dictionary comes from the model metadata or `dict.txt` next to it.
    pub fn load(model: &Path) -> Result<Self> {
        let session =
            super::load_session(model).with_context(|| format!("loading recognition model {}", model.display()))?;

        // RapidOCR exports embed the dictionary; official PaddlePaddle exports leave the key empty.
        let embedded = session.metadata().ok().and_then(|m| m.custom("character"));
        let dict = if let Some(embedded) = embedded.filter(|chars| !chars.trim().is_empty()) {
            embedded
        } else {
            let path = model.with_file_name("dict.txt");
            std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
        };

        let mut charset: Vec<String> = std::iter::once(String::new())
            .chain(dict.lines().map(|line| line.trim_end_matches('\r').to_owned()))
            .collect();

        // Models trained with `use_space_char` have one extra class for the space.
        let classes = output_classes(&session)?;
        match classes.checked_sub(charset.len()) {
            Some(0) => {}
            Some(1) => charset.push(" ".to_owned()),
            _ => bail!(
                "dictionary has {} symbols but the model outputs {classes} classes",
                charset.len() - 1
            ),
        }

        Ok(Self { session, charset })
    }

    /// Recognizes a single cropped text line. Returns the text and its mean confidence.
    pub fn recognize(&mut self, line: &RgbImage) -> Result<(String, f32)> {
        let input = LineTensor::new(line);
        let shape = [1, 3, INPUT_HEIGHT as usize, input.width];
        let outputs = self.session.run(ort::inputs![TensorRef::from_array_view((
            shape,
            input.data.as_slice()
        ))?])?;

        let (output_shape, probabilities) = outputs[0].try_extract_tensor::<f32>()?;
        let classes = output_shape[2] as usize;

        Ok(ctc_decode(probabilities, classes, &self.charset))
    }
}

/// A text line scaled to the model height and right-padded like in PaddleOCR.
struct LineTensor {
    data: Vec<f32>,
    width: usize,
}

impl LineTensor {
    fn new(line: &RgbImage) -> Self {
        let (w, h) = line.dimensions();
        let scaled_w = ((INPUT_HEIGHT as f32 * w as f32 / h as f32).ceil() as u32).max(1);
        let resized = imageops::resize(line, scaled_w, INPUT_HEIGHT, imageops::FilterType::Triangle);
        let pixels = to_bgr_chw(&resized, |_, v| v / 127.5 - 1.0);

        // Zero padding is mid-gray after normalization.
        let (src_w, height) = (scaled_w as usize, INPUT_HEIGHT as usize);
        let width = src_w.max(MIN_INPUT_WIDTH as usize);
        let mut data = vec![0f32; 3 * height * width];

        for (src_row, dst_row) in pixels.chunks_exact(src_w).zip(data.chunks_exact_mut(width)) {
            dst_row[..src_w].copy_from_slice(src_row);
        }

        Self { data, width }
    }
}

fn output_classes(session: &Session) -> Result<usize> {
    let output = session.outputs().first().context("model has no outputs")?;

    match output.dtype().tensor_shape() {
        Some(shape) if shape.len() == 3 && shape[2] > 0 => Ok(shape[2] as usize),
        _ => bail!("unexpected recognition output shape {:?}", output.dtype()),
    }
}

/// Greedy CTC decoding of `[steps, classes]` probabilities: argmax per step, collapse
/// repeats, drop blanks. Returns the text and the mean probability of its characters.
fn ctc_decode(probabilities: &[f32], classes: usize, charset: &[String]) -> (String, f32) {
    let mut text = String::new();
    let (mut score_sum, mut count) = (0.0, 0u32);
    let mut previous = 0;

    for step in probabilities.chunks_exact(classes) {
        let Some((class, &p)) = step.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)) else {
            continue;
        };

        if class != 0
            && class != previous
            && let Some(symbol) = charset.get(class)
        {
            text.push_str(symbol);
            score_sum += p;
            count += 1;
        }

        previous = class;
    }

    let score = if count == 0 { 0.0 } else { score_sum / count as f32 };
    (text.trim().to_owned(), score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctc_collapses_repeats_and_blanks() {
        let charset: Vec<String> = ["", "a", "b"].into_iter().map(String::from).collect();
        // Steps: a, a, blank, a, b.
        let probabilities = [
            0.1, 0.8, 0.1, //
            0.1, 0.8, 0.1, //
            0.9, 0.05, 0.05, //
            0.1, 0.8, 0.1, //
            0.1, 0.1, 0.8,
        ];

        let (text, score) = ctc_decode(&probabilities, 3, &charset);

        assert_eq!(text, "aab");
        assert!((score - 0.8).abs() < 1e-5);
    }

    #[test]
    fn narrow_lines_are_padded_to_the_minimum_width() {
        let line = RgbImage::new(20, 10);

        let tensor = LineTensor::new(&line);

        assert_eq!(tensor.width, MIN_INPUT_WIDTH as usize);
        assert_eq!(tensor.data.len(), 3 * INPUT_HEIGHT as usize * MIN_INPUT_WIDTH as usize);
    }
}
