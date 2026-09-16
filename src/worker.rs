//! Background thread that owns the OCR engine, so recognition never blocks the UI thread.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use anyhow::Result;
use image::RgbaImage;

use crate::ocr::{self, ModelPaths, OcrEngine};
use crate::platform::Waker;

/// Characters of the recognized text shown in the tray tooltip.
const PREVIEW_CHARS: usize = 40;

pub struct OcrWorker {
    jobs: Sender<RgbaImage>,
    status: Receiver<String>,
}

impl OcrWorker {
    /// Models are loaded on the worker thread so startup stays instant.
    pub fn spawn(models: ModelPaths, waker: Waker) -> Self {
        let (jobs, job_rx) = channel::<RgbaImage>();
        let (status_tx, status) = channel();

        let run = move || {
            let report = |msg: String| {
                // The receiver is gone only when the app is shutting down.
                let _ = status_tx.send(msg);
                waker.wake();
            };

            let mut engine = match OcrEngine::load(&models) {
                Ok(engine) => engine,
                Err(e) => {
                    log::error!("failed to load OCR models: {e:#}");
                    report("failed to load models".into());
                    return;
                }
            };

            for image in job_rx {
                match process(&mut engine, &image) {
                    Ok(msg) => report(msg),
                    Err(e) => {
                        log::error!("recognition failed: {e:#}");
                        report("recognition failed".into());
                    }
                }
            }
        };

        std::thread::Builder::new()
            .name("ocr".into())
            .spawn(run)
            .expect("the OS refused to spawn the OCR thread");

        Self { jobs, status }
    }

    pub fn submit(&self, image: RgbaImage) {
        if self.jobs.send(image).is_err() {
            log::error!("OCR worker is not running");
        }
    }

    pub fn poll_status(&self) -> Option<String> {
        self.status.try_recv().ok()
    }
}

/// Recognizes the image, copies the text to the clipboard and returns a status line.
fn process(engine: &mut OcrEngine, image: &RgbaImage) -> Result<String> {
    let started = Instant::now();
    let text = ocr::assemble_text(&engine.recognize(image)?);
    log::info!("recognized {} chars in {:?}", text.chars().count(), started.elapsed());

    if text.is_empty() {
        return Ok("no text found".into());
    }

    arboard::Clipboard::new()?.set_text(text.as_str())?;
    log::info!("copied to clipboard:\n{text}");

    let preview: String = text.chars().take(PREVIEW_CHARS).collect();
    Ok(format!("copied: {preview}"))
}
