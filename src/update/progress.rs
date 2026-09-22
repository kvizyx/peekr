//! The window shown while an update is being installed.
//!
//! The app has nothing else on screen at that point: it is still starting, and the executable it
//! is about to run is the one being replaced. So this window is all there is to say that
//! something is happening, and it closes itself once the update is in place.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use egui::{
    Align, Button, ColorImage, CornerRadius, Frame, Id, Image, Layout, Margin, ProgressBar, RichText, Sense, Stroke,
    TextureHandle, TextureOptions, ViewportCommand, vec2,
};
use winit::window::{Icon, WindowLevel};

use crate::{icon, theme, window};

const WINDOW_SIZE: [f32; 2] = [340.0, 128.0];
const WINDOW_MARGIN: Margin = Margin::same(18);
const LOGO_SIZE: f32 = 32.0;
/// Rounded like the app's icon, which is a square with a corner radius of 112 of its 512 points.
const WINDOW_RADIUS: f32 = 12.0;
/// How often the window looks at how far the update has got.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How far the update has got, shared between the thread installing it and the window.
pub struct Progress {
    /// Bytes to download in total, and how many have arrived.
    total: u64,
    /// Kept apart, because an attempt that starts over starts the file it is on over, not the
    /// files that are already down.
    completed: AtomicU64,
    current: AtomicU64,
    /// What is being done right now, in the words the window shows.
    step: Mutex<String>,
    /// Set when there is nothing left to wait for, so that the window closes itself.
    done: AtomicBool,
    /// Set when the user has had enough of waiting. The download gives up at its first chance,
    /// and the app goes on running the version it has.
    cancelled: AtomicBool,
}

impl Progress {
    pub fn new(total: u64) -> Self {
        Self {
            total,
            completed: AtomicU64::new(0),
            current: AtomicU64::new(0),
            step: Mutex::new("Starting".to_owned()),
            done: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        }
    }

    pub fn downloading(&self, file: &str) {
        self.set_step(format!("Downloading {file}"));
    }

    /// Counts bytes as they come off the network, which is what the download is waiting for.
    pub fn downloaded(&self, bytes: u64) {
        self.current.fetch_add(bytes, Ordering::Relaxed);
    }

    /// One file is down; what it brought stays on the bar whatever the next one does.
    pub fn completed(&self, bytes: u64) {
        self.completed.fetch_add(bytes, Ordering::Relaxed);
        self.current.store(0, Ordering::Relaxed);
    }

    /// A new attempt is starting: the bar goes back to whatever is really on disk, which the
    /// next [`Self::downloaded`] reports.
    pub fn restart(&self) {
        self.current.store(0, Ordering::Relaxed);
    }

    pub fn installing(&self) {
        self.set_step("Installing".to_owned());
    }

    pub fn finish(&self) {
        self.done.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Counts down to the next attempt, unless the user gives up first. Reports whether the wait
    /// was seen through.
    pub fn waiting(&self, how_long: Duration) -> bool {
        let until = std::time::Instant::now() + how_long;

        loop {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() || self.is_cancelled() {
                return !self.is_cancelled();
            }

            self.set_step(countdown(left));
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    pub fn cancel(&self) {
        self.set_step("Cancelling".to_owned());
        self.cancelled.store(true, Ordering::Release);
    }

    fn set_step(&self, step: String) {
        // A panicking thread cannot leave the step unreadable: it is only ever shown.
        *self.step.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = step;
    }

    fn step(&self) -> String {
        self.step
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    fn arrived(&self) -> u64 {
        self.completed.load(Ordering::Relaxed) + self.current.load(Ordering::Relaxed)
    }

    fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 1.0;
        }

        (self.arrived() as f32 / self.total as f32).clamp(0.0, 1.0)
    }

    /// How much of the download is in, as `4.2 of 12.1 MB`.
    fn size(&self) -> String {
        let megabytes = |bytes: u64| bytes as f64 / f64::from(1 << 20);
        let downloaded = self.arrived().min(self.total);

        format!("{:.1} of {:.1} MB", megabytes(downloaded), megabytes(self.total))
    }
}

/// What the window says while it waits for the next attempt.
///
/// Rounded rather than truncated, so that a five second wait counts 5, 4, 3, 2, 1, 0 instead of
/// starting at 4, or sitting on 1 for the whole of the last second.
fn countdown(left: Duration) -> String {
    format!("No connection, trying again in {:.0}s", left.as_secs_f32().round())
}

/// Shows the window until `progress` reports the update is done.
pub fn show(progress: &Progress, version: &str) -> Result<()> {
    window::run(
        |event_loop| {
            let icon = Icon::from_rgba(icon::rgba(), icon::SIZE, icon::SIZE)
                .inspect_err(|e| log::warn!("invalid window icon: {e}"))
                .ok();

            // No system frame: the window draws and rounds its own, and is dragged by its face.
            let window = window::centered(event_loop, WINDOW_SIZE)
                .with_title("Peekr")
                .with_resizable(false)
                .with_decorations(false)
                .with_window_level(WindowLevel::AlwaysOnTop)
                .with_window_icon(icon);

            vec![window]
        },
        |contexts| {
            for ctx in contexts {
                theme::apply(ctx);
            }

            Updating {
                progress,
                version: version.to_owned(),
                logo: None,
            }
        },
    )
}

struct Updating<'a> {
    progress: &'a Progress,
    version: String,
    logo: Option<TextureHandle>,
}

impl window::App for Updating<'_> {
    fn ui(&mut self, _window: usize, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        if self.progress.is_done() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        // The download reports its progress from another thread, which no event wakes this one for.
        ctx.request_repaint_after(POLL_INTERVAL);

        // With no title bar to take hold of, the window is dragged by its face. The cancel button
        // sits on top of this and takes its own clicks first.
        let handle = ui.interact(ui.max_rect(), Id::new("drag-handle"), Sense::drag());
        if handle.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        Frame::new()
            .fill(theme::BACKGROUND)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(WINDOW_RADIUS as u8))
            .inner_margin(WINDOW_MARGIN)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(ui.available_height());

                self.show_title(ui);
                ui.add_space(18.0);

                ui.add(
                    ProgressBar::new(self.progress.fraction())
                        .desired_height(6.0)
                        .corner_radius(CornerRadius::same(3))
                        .fill(theme::ACCENT)
                        .animate(false),
                );
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.progress.step()).size(12.0).color(theme::MUTED));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(self.progress.size()).size(12.0).color(theme::MUTED));
                    });
                });
            });
    }

    fn corner_radius(&self, _window: usize) -> f32 {
        WINDOW_RADIUS
    }
}

impl Updating<'_> {
    /// The app's icon and what is being installed, as a window of its own would have in its title.
    fn show_title(&mut self, ui: &mut egui::Ui) {
        let logo = self.logo(ui.ctx()).clone();

        ui.horizontal(|ui| {
            ui.add(Image::new(&logo).fit_to_exact_size(vec2(LOGO_SIZE, LOGO_SIZE)));
            ui.add_space(6.0);

            ui.vertical(|ui| {
                ui.label(RichText::new("Peekr").size(15.0).strong().color(theme::TEXT));
                ui.label(
                    RichText::new(format!("Updating to {}", self.version))
                        .size(12.0)
                        .color(theme::MUTED),
                );
            });

            // A download that never gives up needs a way out, and this window is the only one
            // the app has open to offer it.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let cancelled = self.progress.is_cancelled();
                let button =
                    Button::new(RichText::new("Cancel").size(12.0).color(theme::MUTED)).min_size(vec2(64.0, 26.0));

                if ui.add_enabled(!cancelled, button).clicked() {
                    self.progress.cancel();
                }
            });
        });
    }

    fn logo(&mut self, ctx: &egui::Context) -> &TextureHandle {
        self.logo.get_or_insert_with(|| {
            let size = [icon::SIZE as usize; 2];
            let image = ColorImage::from_rgba_unmultiplied(size, &icon::rgba());

            ctx.load_texture("logo", image, TextureOptions::LINEAR)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_follows_the_download() {
        let progress = Progress::new(1000);
        assert!((progress.fraction() - 0.0).abs() < f32::EPSILON);

        progress.downloaded(250);
        assert!((progress.fraction() - 0.25).abs() < f32::EPSILON);

        // More than was promised still reads as done rather than overflowing the bar.
        progress.downloaded(5000);
        assert!((progress.fraction() - 1.0).abs() < f32::EPSILON);
        assert_eq!(progress.size(), "0.0 of 0.0 MB");
    }

    #[test]
    fn the_countdown_starts_at_the_whole_wait_and_reaches_zero() {
        let at = |seconds: f32| countdown(Duration::from_secs_f32(seconds));

        assert_eq!(at(5.0), "No connection, trying again in 5s");
        assert_eq!(at(4.6), "No connection, trying again in 5s");
        assert_eq!(at(4.4), "No connection, trying again in 4s");
        assert_eq!(at(1.4), "No connection, trying again in 1s");
        assert_eq!(at(0.4), "No connection, trying again in 0s", "it does reach zero");
        assert_eq!(at(0.0), "No connection, trying again in 0s");
    }

    #[test]
    fn starting_a_file_over_does_not_lose_the_files_before_it() {
        let progress = Progress::new(1000);

        progress.downloaded(400);
        progress.completed(400);

        progress.downloaded(100);
        assert!((progress.fraction() - 0.5).abs() < f32::EPSILON);

        // The second file goes back to nothing; the first one stays down.
        progress.restart();
        assert!(
            (progress.fraction() - 0.4).abs() < f32::EPSILON,
            "the bar keeps what earlier files brought"
        );
    }

    #[test]
    fn an_empty_download_is_already_done() {
        let progress = Progress::new(0);

        assert!((progress.fraction() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_window_waits_until_the_update_says_it_is_done() {
        let progress = Progress::new(10);
        assert!(!progress.is_done());

        progress.finish();
        assert!(progress.is_done());
    }

    #[test]
    fn the_step_says_what_is_being_downloaded() {
        let progress = Progress::new(10);

        progress.downloading("peekr.exe");
        assert_eq!(progress.step(), "Downloading peekr.exe");

        progress.installing();
        assert_eq!(progress.step(), "Installing");
    }
}
