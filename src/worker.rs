//! Background thread that owns the OCR engine, so recognition never blocks the UI thread.
//!
//! Models are loaded on demand when a capture starts and unloaded after a period of inactivity,
//! so the tray app stays small while idle.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use anyhow::Result;
use image::RgbaImage;

use crate::ocr::models::{ModelStore, OcrConfig};
use crate::ocr::{self, OcrEngine};
use crate::platform::{self, Waker};

/// Characters of the recognized text shown in the tray tooltip.
const PREVIEW_CHARS: usize = 40;
/// Models are unloaded after this long without captures. Loading takes ~0.3 s and starts when
/// the capture hotkey is pressed, so it is done before the user finishes selecting.
const IDLE_UNLOAD_AFTER: Duration = Duration::from_secs(120);

enum Job {
    /// A capture has started: load the models while the user is selecting.
    Prepare,
    Recognize(RgbaImage),
}

pub struct OcrWorker {
    jobs: Sender<Job>,
    status: Receiver<String>,
}

impl OcrWorker {
    pub fn spawn(store: ModelStore, config: OcrConfig, waker: Waker) -> Self {
        let (jobs, job_rx) = channel();
        let (status_tx, status) = channel();

        let run = move || {
            let report = |msg: String| {
                // The receiver is gone only when the app is shutting down.
                let _ = status_tx.send(msg);
                waker.wake();
            };

            let mut engine: Option<OcrEngine> = None;

            while let Some(job) = next_job(&job_rx, &mut engine) {
                let loaded = match &mut engine {
                    Some(loaded) => loaded,
                    None => match OcrEngine::load(&store, &config) {
                        Ok(loaded) => engine.insert(loaded),
                        Err(e) => {
                            log::error!("failed to load OCR models: {e:#}");
                            report("failed to load models".into());
                            continue;
                        }
                    },
                };

                let Job::Recognize(image) = job else {
                    continue;
                };

                match process(loaded, &image) {
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

    /// Starts loading the models in the background, if they are not loaded yet.
    pub fn prepare(&self) {
        self.send(Job::Prepare);
    }

    pub fn submit(&self, image: RgbaImage) {
        self.send(Job::Recognize(image));
    }

    pub fn poll_status(&self) -> Option<String> {
        self.status.try_recv().ok()
    }

    fn send(&self, job: Job) {
        if self.jobs.send(job).is_err() {
            log::error!("OCR worker is not running");
        }
    }
}

/// Waits for the next job, unloading the engine when it stays idle for too long.
/// Returns `None` when the app is shutting down.
fn next_job(jobs: &Receiver<Job>, engine: &mut Option<OcrEngine>) -> Option<Job> {
    loop {
        if engine.is_none() {
            return jobs.recv().ok();
        }

        match jobs.recv_timeout(IDLE_UNLOAD_AFTER) {
            Ok(job) => return Some(job),
            Err(RecvTimeoutError::Disconnected) => return None,
            Err(RecvTimeoutError::Timeout) => {
                *engine = None;

                // The overlay's OpenGL driver state stays committed after the window closes;
                // idle is the moment to hand those pages back to the system.
                platform::trim_working_set();
                log::info!("unloaded OCR models after {IDLE_UNLOAD_AFTER:?} of inactivity");
            }
        }
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
