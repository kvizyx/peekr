//! The window shown while an update is being downloaded.
//!
//! The app has nothing else on screen at that point: it is still starting, and it is about to
//! restart into the version being downloaded. So this window is all there is to say that
//! something is happening, and it closes itself once the download is done.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
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
/// How often the window looks at how far the download has got.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How far the download has got, shared between the thread doing it and the window.
#[derive(Default)]
pub struct Progress {
    /// From 0 to 100, which is all Velopack reports.
    percent: AtomicU8,
    /// Set when there is nothing left to wait for, so that the window closes itself.
    done: AtomicBool,
    /// Set when the user would rather not wait. The download carries on without the window, and
    /// the app starts on the version it has.
    put_off: AtomicBool,
}

impl Progress {
    pub fn set_percent(&self, percent: i16) {
        self.percent.store(percent.clamp(0, 100) as u8, Ordering::Relaxed);
    }

    pub fn finish(&self) {
        self.done.store(true, Ordering::Release);
    }

    pub fn put_off(&self) {
        self.put_off.store(true, Ordering::Release);
    }

    pub fn is_put_off(&self) -> bool {
        self.put_off.load(Ordering::Acquire)
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    fn percent(&self) -> u8 {
        self.percent.load(Ordering::Relaxed)
    }

    fn fraction(&self) -> f32 {
        f32::from(self.percent()) / 100.0
    }
}

/// Shows the window until the download is done, or the user puts it off.
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

        if self.progress.is_done() || self.progress.is_put_off() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        // The download reports its progress from another thread, which no event wakes this one for.
        ctx.request_repaint_after(POLL_INTERVAL);

        // With no title bar to take hold of, the window is dragged by its face. The button sits on
        // top of this and takes its own clicks first.
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
                    ui.label(RichText::new("Downloading").size(12.0).color(theme::MUTED));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let percent = format!("{}%", self.progress.percent());
                        ui.label(RichText::new(percent).size(12.0).color(theme::MUTED));
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

            // A slow line should not hold the app up: the download goes on without the window,
            // and the update is installed on the next start.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let button =
                    Button::new(RichText::new("Later").size(12.0).color(theme::MUTED)).min_size(vec2(64.0, 26.0));

                if ui.add(button).clicked() {
                    self.progress.put_off();
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
        let progress = Progress::default();
        assert!((progress.fraction() - 0.0).abs() < f32::EPSILON);

        progress.set_percent(25);
        assert!((progress.fraction() - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn a_percentage_out_of_range_stays_on_the_bar() {
        let progress = Progress::default();

        progress.set_percent(250);
        assert_eq!(progress.percent(), 100);

        progress.set_percent(-1);
        assert_eq!(progress.percent(), 0);
    }

    #[test]
    fn the_window_waits_until_the_download_is_done_or_put_off() {
        let progress = Progress::default();
        assert!(!progress.is_done() && !progress.is_put_off());

        progress.put_off();
        assert!(progress.is_put_off());

        progress.finish();
        assert!(progress.is_done());
    }
}
