//! Command-line arguments.

use anyhow::{Context, Result, bail};

use crate::ocr::models::{OcrConfig, RecognitionMode};

pub const USAGE: &str = "usage: ochco [--capture | --image <file> | --list-models] \
                         [--det <detector id>] [--rec <recognizer id> | auto]";

/// What the process should do.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum Command {
    /// Run in the system tray and capture on the global hotkey.
    #[default]
    Tray,
    /// Capture a region once, copy the recognized text and exit.
    Capture,
    /// Recognize an image file and print the text.
    Image(String),
    /// Print the known models.
    ListModels,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    pub command: Command,
    pub ocr: OcrConfig,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut parsed = Self::default();
        let mut args = args.into_iter();

        while let Some(flag) = args.next() {
            let mut value = || args.next().with_context(|| format!("{flag} needs a value\n{USAGE}"));

            let command = match flag.as_str() {
                "--capture" => Command::Capture,
                "--image" => Command::Image(value()?),
                "--list-models" => Command::ListModels,
                "--det" => {
                    parsed.ocr.detector = Some(value()?);
                    continue;
                }
                "--rec" => {
                    parsed.ocr.recognition = match value()?.as_str() {
                        "auto" => RecognitionMode::Auto,
                        id => RecognitionMode::Single(id.to_owned()),
                    };
                    continue;
                }
                _ => bail!("unknown argument {flag:?}\n{USAGE}"),
            };

            if parsed.command != Command::Tray {
                bail!("{flag} cannot be combined with another command\n{USAGE}");
            }
            parsed.command = command;
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

        assert_eq!(args.command, Command::Tray);
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

        assert_eq!(args.command, Command::Image("a.png".into()));
        assert_eq!(args.ocr.detector.as_deref(), Some("pp-ocrv5-mobile"));
        assert_eq!(args.ocr.recognition, RecognitionMode::Single("pp-ocrv6-small".into()));
    }

    #[test]
    fn parses_one_shot_capture() {
        let args = parse(&["--capture", "--rec", "pp-ocrv5-eslav"]).expect("valid arguments");

        assert_eq!(args.command, Command::Capture);
    }

    #[test]
    fn rejects_unknown_flags_missing_values_and_conflicting_commands() {
        assert!(parse(&["--verbose"]).is_err());
        assert!(parse(&["--image"]).is_err());
        assert!(parse(&["--capture", "--list-models"]).is_err());
    }
}
