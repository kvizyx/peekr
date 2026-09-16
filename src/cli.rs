//! Command-line arguments.

use anyhow::{Context, Result, bail};

use crate::ocr::models::{OcrConfig, RecognitionMode};

pub const USAGE: &str =
    "usage: ochco [--image <file>] [--det <detector id>] [--rec <recognizer id> | auto] [--list-models]";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Recognize this file and print the text instead of starting the tray app.
    pub image: Option<String>,
    /// Print the known models and exit.
    pub list_models: bool,
    pub ocr: OcrConfig,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut parsed = Self::default();
        let mut args = args.into_iter();

        while let Some(flag) = args.next() {
            let mut value = || args.next().with_context(|| format!("{flag} needs a value\n{USAGE}"));

            match flag.as_str() {
                "--image" => parsed.image = Some(value()?),
                "--list-models" => parsed.list_models = true,
                "--det" => parsed.ocr.detector = Some(value()?),
                "--rec" => {
                    parsed.ocr.recognition = match value()?.as_str() {
                        "auto" => RecognitionMode::Auto,
                        id => RecognitionMode::Single(id.to_owned()),
                    };
                }
                _ => bail!("unknown argument {flag:?}\n{USAGE}"),
            }
        }

        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args> {
        Args::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn no_arguments_start_the_tray_app_in_auto_mode() {
        let args = parse(&[]).expect("valid arguments");

        assert_eq!(args, Args::default());
        assert_eq!(args.ocr.recognition, RecognitionMode::Auto);
    }

    #[test]
    fn parses_image_and_model_selection() {
        let args = parse(&[
            "--image",
            "a.png",
            "--det",
            "pp-ocrv5-mobile",
            "--rec",
            "pp-ocrv6-small",
        ])
        .expect("valid arguments");

        assert!(!args.list_models);
        assert_eq!(args.image.as_deref(), Some("a.png"));
        assert_eq!(args.ocr.detector.as_deref(), Some("pp-ocrv5-mobile"));
        assert_eq!(args.ocr.recognition, RecognitionMode::Single("pp-ocrv6-small".into()));
    }

    #[test]
    fn rejects_unknown_flags_and_missing_values() {
        assert!(parse(&["--verbose"]).is_err());
        assert!(parse(&["--image"]).is_err());
    }
}
