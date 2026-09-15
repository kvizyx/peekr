//! Text line recognition with a PP-OCR CTC model.

use std::path::Path;

use anyhow::{Context, Result, bail};
use image::{RgbImage, imageops};
use ort::session::Session;
use ort::value::TensorRef;

use super::det::to_bgr_chw;

const INPUT_HEIGHT: u32 = 48;
/// PaddleOCR pads narrow lines to at least this width.
const MIN_INPUT_WIDTH: u32 = 320;

pub struct Recognizer {
    session: Session,
    /// Output class index -> text. Index 0 is the CTC blank.
    charset: Vec<String>,
}

impl Recognizer {
    /// Loads `rec.onnx`; the dictionary comes from the model metadata or `dict.txt` next to it.
    pub fn load(model: &Path) -> Result<Self> {
        let session = super::load_session(model)
            .with_context(|| format!("loading recognition model {}", model.display()))?;

        let embedded = session.metadata().ok().and_then(|m| m.custom("character"));
        let dict = match embedded {
            Some(chars) => chars,
            None => {
                let path = model.with_file_name("dict.txt");
                std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
            }
        };
        let mut charset: Vec<String> = std::iter::once(String::new())
            .chain(dict.lines().map(|l| l.trim_end_matches('\r').to_owned()))
            .collect();

        let classes = output_classes(&session)?;
        match classes.checked_sub(charset.len()) {
            Some(0) => {}
            Some(1) => charset.push(" ".to_owned()),
            _ => bail!("dictionary has {} symbols but the model outputs {classes} classes", charset.len() - 1),
        }
        Ok(Self { session, charset })
    }

    /// Recognizes a single cropped text line. Returns the text and its mean confidence.
    pub fn recognize(&mut self, line: &RgbImage) -> Result<(String, f32)> {
        let (w, h) = line.dimensions();
        let scaled_w = ((INPUT_HEIGHT as f32 * w as f32 / h as f32).ceil() as u32).max(1);
        let input_w = scaled_w.max(MIN_INPUT_WIDTH);
        let resized = imageops::resize(line, scaled_w, INPUT_HEIGHT, imageops::FilterType::Triangle);

        let pixels = to_bgr_chw(&resized, |_, v| v / 127.5 - 1.0);
        // Right-pad with zeros (mid-gray after normalization), like PaddleOCR.
        let (iw, ih) = (input_w as usize, INPUT_HEIGHT as usize);
        let mut input = vec![0f32; 3 * ih * iw];
        let src_w = scaled_w as usize;
        for c in 0..3 {
            for y in 0..ih {
                let src = c * ih * src_w + y * src_w;
                let dst = c * ih * iw + y * iw;
                input[dst..dst + src_w].copy_from_slice(&pixels[src..src + src_w]);
            }
        }

        let shape = [1usize, 3, ih, iw];
        let outputs = self.session.run(ort::inputs![TensorRef::from_array_view((shape, &*input))?])?;
        let (out_shape, probs) = outputs[0].try_extract_tensor::<f32>()?;
        let (steps, classes) = (out_shape[1] as usize, out_shape[2] as usize);
        Ok(ctc_decode(probs, steps, classes, &self.charset))
    }
}

fn output_classes(session: &Session) -> Result<usize> {
    let output = session.outputs().first().context("model has no outputs")?;
    match output.dtype().tensor_shape() {
        Some(shape) if shape.len() == 3 && shape[2] > 0 => Ok(shape[2] as usize),
        _ => bail!("unexpected recognition output shape {:?}", output.dtype()),
    }
}

/// Greedy CTC decoding: argmax per step, collapse repeats, drop blanks.
fn ctc_decode(probs: &[f32], steps: usize, classes: usize, charset: &[String]) -> (String, f32) {
    let mut text = String::new();
    let (mut score_sum, mut count) = (0f32, 0u32);
    let mut prev = 0usize;
    for t in 0..steps {
        let row = &probs[t * classes..(t + 1) * classes];
        let (idx, &p) = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
        if idx != 0
            && idx != prev
            && let Some(s) = charset.get(idx)
        {
            text.push_str(s);
            score_sum += p;
            count += 1;
        }
        prev = idx;
    }
    let score = if count == 0 { 0.0 } else { score_sum / count as f32 };
    (text.trim().to_owned(), score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctc_collapses_repeats_and_blanks() {
        let charset: Vec<String> = ["", "a", "b"].iter().map(|s| s.to_string()).collect();
        // steps: a a blank a b
        let probs = [
            0.1, 0.8, 0.1, //
            0.1, 0.8, 0.1, //
            0.9, 0.05, 0.05, //
            0.1, 0.8, 0.1, //
            0.1, 0.1, 0.8,
        ];
        let (text, score) = ctc_decode(&probs, 5, 3, &charset);
        assert_eq!(text, "aab");
        assert!((score - 0.8).abs() < 1e-5);
    }
}
