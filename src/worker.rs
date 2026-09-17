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
use crate::platform;

/// Models are unloaded after this long without captures. Loading takes ~0.3 s and starts when
/// the capture hotkey is pressed, so it is done before the user finishes selecting.
const IDLE_UNLOAD_AFTER: Duration = Duration::from_secs(60);

enum Job {
    /// A capture has started: load the models while the user is selecting.
    Prepare,
    /// Recognize the image and send the text back.
    Recognize(RgbaImage, Sender<Result<String>>),
}

pub struct OcrWorker {
    jobs: Sender<Job>,
}

impl OcrWorker {
    pub fn spawn(store: ModelStore, config: OcrConfig) -> Self {
        let (jobs, job_rx) = channel();

        let run = move || {
            let mut engine: Option<OcrEngine> = None;

            while let Some(job) = next_job(&job_rx, &mut engine) {
                if engine.is_none() {
                    match OcrEngine::load(&store, &config) {
                        Ok(loaded) => engine = Some(loaded),
                        Err(e) => {
                            log::error!("failed to load OCR models: {e:#}");

                            if let Job::Recognize(_, reply) = job {
                                let _ = reply.send(Err(e.context("loading the OCR models")));
                            }
                            continue;
                        }
                    }
                }

                let (Some(loaded), Job::Recognize(image, reply)) = (&mut engine, job) else {
                    continue;
                };

                let result = recognize(loaded, &image);
                if let Err(e) = &result {
                    log::error!("recognition failed: {e:#}");
                }

                // Nobody is waiting if the overlay was closed in the meantime.
                let _ = reply.send(result);
            }
        };

        std::thread::Builder::new()
            .name("ocr".into())
            .spawn(run)
            .expect("the OS refused to spawn the OCR thread");

        Self { jobs }
    }

    /// Starts loading the models in the background, if they are not loaded yet.
    pub fn prepare(&self) {
        self.send(Job::Prepare);
    }

    /// Queues the image for recognition; the text arrives on the returned channel.
    pub fn recognize(&self, image: RgbaImage) -> Receiver<Result<String>> {
        let (reply, result) = channel();
        self.send(Job::Recognize(image, reply));

        result
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

                // Freed model and screenshot buffers stay committed to the process;
                // idle is the moment to hand those pages back to the system.
                platform::trim_working_set();
                log::info!("unloaded OCR models after {IDLE_UNLOAD_AFTER:?} of inactivity");
            }
        }
    }
}

fn recognize(engine: &mut OcrEngine, image: &RgbaImage) -> Result<String> {
    let started = Instant::now();
    let text = ocr::assemble_text(&engine.recognize(image)?);
    log::info!("recognized {} chars in {:?}", text.chars().count(), started.elapsed());

    Ok(text)
}
