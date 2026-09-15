//! Background thread that owns the OCR engine, so recognition never blocks the UI thread.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use anyhow::Result;
use image::RgbaImage;

use crate::ocr::{self, ModelPaths, OcrEngine};
use crate::platform::Waker;

pub struct OcrWorker {
    jobs: Sender<RgbaImage>,
    status: Receiver<String>,
}

impl OcrWorker {
    /// Models are loaded on the worker thread so startup stays instant.
    pub fn spawn(models: ModelPaths, waker: Waker) -> Self {
        let (jobs, job_rx) = channel::<RgbaImage>();
        let (status_tx, status) = channel();
        std::thread::Builder::new()
            .name("ocr".into())
            .spawn(move || {
                let report = |msg: String| {
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
            })
            .expect("spawning the OCR thread");
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

fn process(engine: &mut OcrEngine, image: &RgbaImage) -> Result<String> {
    let started = Instant::now();
    let text = ocr::assemble_text(&engine.recognize(image)?);
    log::info!("recognized {} chars in {:?}", text.chars().count(), started.elapsed());
    if text.is_empty() {
        return Ok("no text found".into());
    }
    arboard::Clipboard::new()?.set_text(text.clone())?;
    log::info!("copied to clipboard:\n{text}");
    let preview: String = text.chars().take(40).collect();
    Ok(format!("copied: {preview}"))
}
