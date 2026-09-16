//! PaddleOCR pipeline: detect text boxes, crop them, recognize each line, assemble text.

mod det;
mod geometry;
pub mod models;
mod preprocess;
mod rec;
mod text;

use std::path::Path;

use anyhow::Result;
use image::RgbaImage;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

use self::det::Detector;
use self::geometry::Quad;
use self::models::{ModelStore, OcrConfig, RecognizerSpec};
use self::rec::Recognizer;
pub use self::text::assemble_text;

/// Lines recognized with lower confidence are dropped.
const MIN_LINE_SCORE: f32 = 0.5;
/// Border added around the selection so text touching the edges is still detected.
const PADDING: u32 = 24;
/// In auto mode, a reading at least this share as long as the longest one is not penalized.
const COVERAGE_TOLERANCE: f32 = 0.9;
/// In auto mode, a reading in a recognizer's exclusive script needs this confidence to be final.
/// Stylized fonts (e.g. Impact in video thumbnails) score around 0.75, while a model never
/// produced a real word in an alphabet the text is not written in, so the bar can be low.
const DECISIVE_MIN_SCORE: f32 = 0.6;
/// Single-symbol readings are often noise from picture edges, so they need more confidence.
const MIN_SINGLE_SYMBOL_SCORE: f32 = 0.9;

/// A recognized piece of text and where it was found.
#[derive(Debug, Clone)]
pub struct TextLine {
    pub text: String,
    quad: Quad,
}

pub struct OcrEngine {
    detector: Detector,
    /// Recognizers in auto-mode order, with the specs that drive early decisions.
    recognizers: Vec<(&'static RecognizerSpec, Recognizer)>,
}

impl OcrEngine {
    pub fn load(store: &ModelStore, config: &OcrConfig) -> Result<Self> {
        let detector_spec = store.select_detector(config)?;
        let detector = Detector::load(&store.detector_path(detector_spec.id), detector_spec.params)?;

        let recognizers = store
            .select_recognizers(config)?
            .into_iter()
            .map(|spec| Ok((spec, Recognizer::load(&store.recognizer_path(spec.id))?)))
            .collect::<Result<Vec<_>>>()?;

        let recognizer_ids: Vec<_> = recognizers.iter().map(|(spec, _)| spec.id).collect();
        log::info!(
            "OCR models: detector {}, recognizers {recognizer_ids:?}",
            detector_spec.id
        );

        Ok(Self { detector, recognizers })
    }

    pub fn recognize(&mut self, img: &RgbaImage) -> Result<Vec<TextLine>> {
        let img = preprocess::pad_with_border_color(img, PADDING);
        let quads = geometry::merge_line_fragments(self.detector.detect(&img)?);

        let mut lines = Vec::with_capacity(quads.len());

        for quad in quads {
            let crop = geometry::crop_quad(&img, &quad);
            let (text, score) = self.recognize_line(&crop)?;

            if is_plausible_line(&text, score) {
                lines.push(TextLine { text, quad });
            }
        }

        Ok(lines)
    }

    /// Runs the loaded recognizers on the line and keeps the best result. Stops early once a
    /// reading is decisive (see [`is_decisive`]), so e.g. Russian text is read by one model only.
    fn recognize_line(&mut self, crop: &image::RgbImage) -> Result<(String, f32)> {
        let mut candidates = Vec::with_capacity(self.recognizers.len());

        for (spec, recognizer) in &mut self.recognizers {
            let (text, score) = recognizer.recognize(crop)?;
            log::debug!("{:>16} {score:.3} {text:?}", spec.id);

            if is_decisive(spec, &text, score) {
                return Ok((text, score));
            }

            candidates.push((text, score));
        }

        Ok(pick_best_reading(candidates).unwrap_or_default())
    }
}

/// Filters out unconfident readings and likely noise.
fn is_plausible_line(text: &str, score: f32) -> bool {
    match text.chars().filter(|c| !c.is_whitespace()).count() {
        0 => false,
        1 => score >= MIN_SINGLE_SYMBOL_SCORE,
        _ => score >= MIN_LINE_SCORE,
    }
}

/// A reading is final when it is confident and has real words in a script that only its
/// recognizer supports.
fn is_decisive(spec: &RecognizerSpec, text: &str, score: f32) -> bool {
    let Some(script) = spec.exclusive_script else {
        return false;
    };

    score >= DECISIVE_MIN_SCORE && text::contains_word_in(text, script)
}

/// Chooses between readings of the same line by different models.
///
/// Confidence alone is misleading: a model that lacks an alphabet emits blanks for its letters
/// and confidently reads only the punctuation around them. Such readings are much shorter,
/// so each confidence is scaled down by how much shorter the reading is than the longest one.
fn pick_best_reading(candidates: Vec<(String, f32)>) -> Option<(String, f32)> {
    let visible_chars = |text: &str| text.chars().filter(|c| !c.is_whitespace()).count() as f32;
    let longest = candidates
        .iter()
        .map(|(text, _)| visible_chars(text))
        .fold(0.0, f32::max);

    let rank = |(text, score): &(String, f32)| {
        let coverage = if longest > 0.0 {
            (visible_chars(text) / (COVERAGE_TOLERANCE * longest)).min(1.0)
        } else {
            1.0
        };

        score * coverage
    };

    candidates.into_iter().max_by(|a, b| rank(a).total_cmp(&rank(b)))
}

fn load_session(path: &Path) -> ort::Result<Session> {
    Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .commit_from_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(text: &str, score: f32) -> (String, f32) {
        (text.to_owned(), score)
    }

    #[test]
    fn prefers_the_model_that_reads_the_whole_line() {
        let candidates = vec![
            reading("Привет, мир! Съешь же ещё этих мягких булок.", 0.97),
            reading(", !      .", 0.99),
        ];

        let (text, _) = pick_best_reading(candidates).expect("there are candidates");

        assert!(text.starts_with("Привет"));
    }

    #[test]
    fn prefers_confidence_when_readings_have_similar_length() {
        let candidates = vec![
            reading("L'été dernier, nous sommes allés a la plage prés de Nimes.", 0.976),
            reading("L'été dernier, nous sommes allés à la plage près de Nîmes", 0.987),
        ];

        let (text, _) = pick_best_reading(candidates).expect("there are candidates");

        assert!(text.ends_with("Nîmes"));
    }

    #[test]
    fn empty_readings_fall_back_to_confidence() {
        let candidates = vec![reading("", 0.2), reading("", 0.6)];

        let (_, score) = pick_best_reading(candidates).expect("there are candidates");

        assert!((score - 0.6).abs() < f32::EPSILON);
    }

    #[test]
    fn confident_cyrillic_reading_is_decisive() {
        let eslav = &models::RECOGNIZERS[0];

        assert!(is_decisive(eslav, "Сохранить изменения?", 0.95));
        assert!(is_decisive(eslav, "CША?", 0.757));
        assert!(!is_decisive(eslav, "Сохранить изменения?", 0.5));
        assert!(!is_decisive(eslav, "Еrгог: 404", 0.95));
    }

    #[test]
    fn single_symbols_need_high_confidence() {
        assert!(!is_plausible_line("à", 0.71));
        assert!(is_plausible_line("5", 0.95));
        assert!(is_plausible_line("OK", 0.71));
        assert!(!is_plausible_line("   ", 0.99));
    }

    #[test]
    fn recognizers_without_exclusive_script_are_never_decisive() {
        let latin = &models::RECOGNIZERS[1];

        assert!(!is_decisive(latin, "Größe und Qualität", 0.99));
    }

    #[test]
    fn no_candidates_means_no_reading() {
        assert!(pick_best_reading(Vec::new()).is_none());
    }
}
