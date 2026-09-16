//! Catalog of supported models and the configuration that picks between them.
//!
//! Models live in `<root>/det/<id>/model.onnx` and `<root>/rec/<id>/model.onnx` (+ `dict.txt`),
//! as laid out by `cargo xtask models`.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

pub use super::text::Script;

/// Post-processing thresholds of a DB text detector; they are tuned per model.
#[derive(Debug, Clone, Copy)]
pub struct DetectorParams {
    /// Pixels of the probability map above this value are considered text.
    pub bin_threshold: f32,
    /// Boxes whose mean probability is below this are dropped.
    pub box_threshold: f32,
    /// How much to expand a shrunk text kernel back to the full text box.
    pub unclip_ratio: f32,
    /// Dilate the binary map by 2x2 so characters separated by thin gaps join into one box.
    pub dilate: bool,
}

#[derive(Debug)]
pub struct DetectorSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub params: DetectorParams,
}

#[derive(Debug)]
pub struct RecognizerSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub languages: &'static str,
    /// A script only this recognizer can read. In auto mode, a confident reading with words in
    /// this script is final: the other recognizers would only lose those letters.
    pub exclusive_script: Option<Script>,
}

/// Known detectors, best first.
pub const DETECTORS: &[DetectorSpec] = &[
    DetectorSpec {
        id: "pp-ocrv6-small",
        name: "PP-OCRv6 small",
        // Thresholds from the model's inference.yml; the larger unclip ratio and the dilation were
        // tuned on screen text, where the stock values crop punctuation and diacritics at box edges.
        params: DetectorParams {
            bin_threshold: 0.2,
            box_threshold: 0.45,
            unclip_ratio: 1.6,
            dilate: true,
        },
    },
    DetectorSpec {
        id: "pp-ocrv5-mobile",
        name: "PP-OCRv5 mobile",
        // RapidOCR defaults; the dilation keeps words of one line together.
        params: DetectorParams {
            bin_threshold: 0.3,
            box_threshold: 0.5,
            unclip_ratio: 1.6,
            dilate: true,
        },
    },
];

/// Known recognizers. Auto mode runs them in this order, so those with an exclusive script go first.
pub const RECOGNIZERS: &[RecognizerSpec] = &[
    RecognizerSpec {
        id: "pp-ocrv5-eslav",
        name: "PP-OCRv5 East Slavic",
        languages: "Russian, Ukrainian, Belarusian, Bulgarian, English",
        exclusive_script: Some(Script::Cyrillic),
    },
    RecognizerSpec {
        id: "pp-ocrv6-small",
        name: "PP-OCRv6 small",
        languages: "Chinese, Japanese, English and 46 Latin-script languages",
        exclusive_script: None,
    },
];

/// Which recognition models to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecognitionMode {
    /// Run the installed recognizers and keep the best reading of each line. A confident reading
    /// in a script only one recognizer supports (e.g. Cyrillic) skips the remaining ones.
    Auto,
    /// Always use the recognizer with this id.
    Single(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrConfig {
    /// Detector id; `None` picks the best installed one.
    pub detector: Option<String>,
    pub recognition: RecognitionMode,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            detector: None,
            recognition: RecognitionMode::Auto,
        }
    }
}

/// A directory with downloaded models.
#[derive(Debug, Clone)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn detector_path(&self, id: &str) -> PathBuf {
        self.root.join("det").join(id).join("model.onnx")
    }

    pub fn recognizer_path(&self, id: &str) -> PathBuf {
        self.root.join("rec").join(id).join("model.onnx")
    }

    pub fn installed_detectors(&self) -> impl Iterator<Item = &'static DetectorSpec> {
        DETECTORS.iter().filter(|spec| is_file(&self.detector_path(spec.id)))
    }

    pub fn installed_recognizers(&self) -> impl Iterator<Item = &'static RecognizerSpec> {
        RECOGNIZERS
            .iter()
            .filter(|spec| is_file(&self.recognizer_path(spec.id)))
    }

    /// Resolves the configured detector, falling back to the best installed one.
    pub fn select_detector(&self, config: &OcrConfig) -> Result<&'static DetectorSpec> {
        let Some(id) = &config.detector else {
            return match self.installed_detectors().next() {
                Some(spec) => Ok(spec),
                None => bail!("no detection model installed in {}", self.root.display()),
            };
        };

        let Some(spec) = DETECTORS.iter().find(|spec| spec.id == id) else {
            bail!(
                "unknown detector {id:?}; known: {}",
                ids(DETECTORS.iter().map(|s| s.id))
            );
        };

        if !is_file(&self.detector_path(spec.id)) {
            bail!("detector {id:?} is not installed in {}", self.root.display());
        }

        Ok(spec)
    }

    /// Resolves the recognizers to run for the configured mode.
    pub fn select_recognizers(&self, config: &OcrConfig) -> Result<Vec<&'static RecognizerSpec>> {
        let selected: Vec<_> = match &config.recognition {
            RecognitionMode::Auto => self.installed_recognizers().collect(),
            RecognitionMode::Single(id) => {
                let Some(spec) = RECOGNIZERS.iter().find(|spec| spec.id == id) else {
                    bail!(
                        "unknown recognizer {id:?}; known: {}",
                        ids(RECOGNIZERS.iter().map(|s| s.id))
                    );
                };

                vec![spec]
            }
        };

        if selected.is_empty() {
            bail!("no recognition model installed in {}", self.root.display());
        }

        if let Some(missing) = selected.iter().find(|spec| !is_file(&self.recognizer_path(spec.id))) {
            bail!(
                "recognizer {:?} is not installed in {}",
                missing.id,
                self.root.display()
            );
        }

        Ok(selected)
    }
}

fn is_file(path: &Path) -> bool {
    path.is_file()
}

fn ids(ids: impl Iterator<Item = &'static str>) -> String {
    ids.collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(files: &[&str]) -> (tempdir::TempDir, ModelStore) {
        let dir = tempdir::TempDir::new();

        for file in files {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().expect("file has a parent")).expect("create model dir");
            std::fs::write(&path, b"").expect("create model file");
        }

        let store = ModelStore::new(dir.path().to_path_buf());
        (dir, store)
    }

    #[test]
    fn default_detector_is_the_best_installed() {
        let (_dir, store) = store_with(&["det/pp-ocrv5-mobile/model.onnx"]);

        let spec = store
            .select_detector(&OcrConfig::default())
            .expect("a detector is installed");

        assert_eq!(spec.id, "pp-ocrv5-mobile");
    }

    #[test]
    fn auto_mode_uses_all_installed_recognizers() {
        let (_dir, store) = store_with(&["rec/pp-ocrv5-eslav/model.onnx", "rec/pp-ocrv6-small/model.onnx"]);

        let specs = store
            .select_recognizers(&OcrConfig::default())
            .expect("recognizers are installed");

        let ids: Vec<_> = specs.iter().map(|s| s.id).collect();
        assert_eq!(ids, ["pp-ocrv5-eslav", "pp-ocrv6-small"]);
    }

    #[test]
    fn single_mode_requires_the_model_to_be_installed() {
        let (_dir, store) = store_with(&["rec/pp-ocrv5-eslav/model.onnx"]);
        let config = OcrConfig {
            detector: None,
            recognition: RecognitionMode::Single("pp-ocrv6-small".into()),
        };

        let result = store.select_recognizers(&config);

        assert!(result.is_err());
    }

    /// Minimal self-deleting temporary directory, to avoid a dev-dependency.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};

        pub struct TempDir(PathBuf);

        impl TempDir {
            pub fn new() -> Self {
                static COUNTER: AtomicU32 = AtomicU32::new(0);

                let name = format!(
                    "ochco-test-{}-{}",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                );
                let path = std::env::temp_dir().join(name);
                std::fs::create_dir_all(&path).expect("create temp dir");

                Self(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
