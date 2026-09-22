//! Settings window: shows the capture hotkey and records a new one.

use std::time::Duration;

use anyhow::Result;
use egui::{Align, Button, Event, Frame, Key, Layout, Margin, RichText, Spinner, ViewportCommand, vec2};
use winit::window::{Icon, WindowButtons};

use crate::config::Config;
use crate::shortcut::Shortcut;
use crate::{icon, theme, window};

const WINDOW_SIZE: [f32; 2] = [440.0, 268.0];
const WINDOW_MARGIN: Margin = Margin::same(20);
const SECTION_MARGIN: Margin = Margin::same(16);
const BUTTON_HEIGHT: f32 = 30.0;
/// How often the window checks whether the app needs it closed.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// What the window does with the settings it changes. The app owns the config, so every change
/// goes back to it instead of being written here.
pub struct Handlers<'a> {
    /// Called with every recorded hotkey; reports why it cannot be used, which the window shows.
    pub apply: &'a mut dyn FnMut(Shortcut) -> Result<()>,
    /// Called when the window starts and stops waiting for a new shortcut, so that the app can
    /// release the current hotkey meanwhile.
    pub recording: &'a mut dyn FnMut(bool),
    /// Called when automatic updates are turned on or off.
    pub set_updates: &'a mut dyn FnMut(bool),
    /// Whether the app has something else to do, such as a capture.
    pub interrupted: &'a dyn Fn() -> bool,
}

/// Shows the settings window until the user closes it, or until the app needs it gone.
pub fn open(current: &Config, handlers: Handlers<'_>) -> Result<()> {
    let current = current.clone();

    window::run(
        |event_loop| {
            let icon = Icon::from_rgba(icon::rgba(), icon::SIZE, icon::SIZE)
                .inspect_err(|e| log::warn!("invalid window icon: {e}"))
                .ok();

            let window = window::centered(event_loop, WINDOW_SIZE)
                .with_title("Settings")
                .with_resizable(false)
                .with_enabled_buttons(WindowButtons::CLOSE | WindowButtons::MINIMIZE)
                .with_window_icon(icon);

            vec![window]
        },
        |contexts| {
            for ctx in contexts {
                theme::apply(ctx);
            }

            SettingsApp {
                hotkey: current.hotkey,
                updates: current.updates,
                recording: false,
                super_held: false,
                message: None,
                handlers,
            }
        },
    )
}

enum Message {
    Info(String),
    Error(String),
}

struct SettingsApp<'a> {
    hotkey: Shortcut,
    updates: bool,
    /// Waiting for the user to press the new shortcut.
    recording: bool,
    /// egui's `Modifiers` has no Windows / Super key, so its presses are tracked separately.
    super_held: bool,
    message: Option<Message>,
    handlers: Handlers<'a>,
}

impl window::App for SettingsApp<'_> {
    fn ui(&mut self, _window: usize, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.track_super_key(&ctx);

        if self.recording {
            self.record(&ctx);
        }

        // Nothing here animates, so without this the window would not notice the app waiting.
        ctx.request_repaint_after(POLL_INTERVAL);
        if (self.handlers.interrupted)() {
            self.set_recording(false);
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        Frame::new()
            .fill(theme::BACKGROUND)
            .inner_margin(WINDOW_MARGIN)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(ui.available_height());

                self.show_hotkey(ui);
                ui.add_space(12.0);

                self.show_updates(ui);
                ui.add_space(10.0);

                self.show_message(ui);
            });
    }
}

impl SettingsApp<'_> {
    /// The hotkey as key caps, with the button that records a new one.
    fn show_hotkey(&mut self, ui: &mut egui::Ui) {
        let border = if self.recording { theme::ACCENT } else { theme::BORDER };

        theme::section(SECTION_MARGIN)
            .stroke(egui::Stroke::new(1.0, border))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Capture hotkey").size(14.0).strong().color(theme::TEXT));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if self.recording { "Cancel" } else { "Change" };
                        let button =
                            Button::new(RichText::new(label).color(theme::TEXT)).min_size(vec2(84.0, BUTTON_HEIGHT));

                        if ui.add(button).clicked() {
                            let recording = !self.recording;

                            self.set_recording(recording);
                            self.message = None;
                        }
                    });
                });

                ui.add_space(12.0);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.set_min_height(BUTTON_HEIGHT);

                    if self.recording {
                        ui.add(Spinner::new().size(14.0).color(theme::ACCENT));
                        ui.label(
                            RichText::new("Press the shortcut you want to use")
                                .size(14.0)
                                .color(theme::MUTED),
                        );
                        return;
                    }

                    for (index, key) in self.hotkey.to_string().split('+').enumerate() {
                        if index > 0 {
                            ui.label(RichText::new("+").size(13.0).color(theme::MUTED));
                        }

                        theme::key_cap(ui, key, 14.0);
                    }
                });
            });
    }

    /// The switch for automatic updates, and what turning it on means.
    fn show_updates(&mut self, ui: &mut egui::Ui) {
        theme::section(SECTION_MARGIN).show(ui, |ui| {
            ui.set_width(ui.available_width());

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Enable automatic updates")
                        .size(14.0)
                        .strong()
                        .color(theme::TEXT),
                );

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::toggle(ui, &mut self.updates).changed() {
                        (self.handlers.set_updates)(self.updates);
                    }
                });
            });

            ui.add_space(8.0);
            ui.label(
                RichText::new("New releases are downloaded and installed when application starts.")
                    .size(13.0)
                    .color(theme::MUTED),
            );
        });
    }

    /// The result of the last change, or what to do when nothing has been changed yet.
    fn show_message(&self, ui: &mut egui::Ui) {
        let (text, color) = match &self.message {
            Some(Message::Info(text)) => (text.as_str(), theme::ACCENT),
            Some(Message::Error(text)) => (text.as_str(), ui.visuals().error_fg_color),
            None => return,
        };

        ui.label(RichText::new(text).size(13.0).color(color));
    }

    /// Starts or stops waiting for a new shortcut, telling the app so that it can release the
    /// current hotkey while the next press is being recorded.
    fn set_recording(&mut self, recording: bool) {
        if self.recording == recording {
            return;
        }

        self.recording = recording;
        (self.handlers.recording)(recording);
    }

    fn track_super_key(&mut self, ctx: &egui::Context) {
        ctx.input(|input| {
            for event in &input.events {
                if let Event::Key {
                    key: Key::SuperLeft | Key::SuperRight,
                    pressed,
                    ..
                } = event
                {
                    self.super_held = *pressed;
                }
            }
        });
    }

    /// Takes the next non-modifier key press out of the input (so buttons don't react to Space
    /// or Enter) and applies it, with the modifiers held at that moment, as the new hotkey.
    fn record(&mut self, ctx: &egui::Context) {
        let pressed = ctx.input_mut(|input| {
            let index = input
                .events
                .iter()
                .position(|event| matches!(event, Event::Key { key, pressed: true, .. } if !is_modifier(*key)))?;

            match input.events.remove(index) {
                Event::Key { key, modifiers, .. } => Some((key, modifiers)),
                _ => None,
            }
        });

        let Some((key, modifiers)) = pressed else {
            return;
        };

        // The hotkey is applied while it is still released, so that registering it can tell
        // whether another application holds it.
        if key != Key::Escape {
            match Shortcut::from_key_press(key, modifiers, self.super_held) {
                Some(shortcut) => self.apply(shortcut),
                None => {
                    self.message = Some(Message::Error(format!(
                        "{} cannot be used; end the shortcut with a letter, digit, F-key or navigation key.",
                        key.name()
                    )));
                }
            }
        }

        self.set_recording(false);
    }

    fn apply(&mut self, shortcut: Shortcut) {
        if let Some(problem) = shortcut.problem() {
            self.message = Some(Message::Error(problem.to_owned()));
            return;
        }

        // A saved hotkey speaks for itself: the key caps above show it. Only a warning about it
        // is worth a line.
        self.message = match (self.handlers.apply)(shortcut) {
            Ok(()) => {
                self.hotkey = shortcut;
                shortcut.warning().map(|warning| Message::Info(warning.to_owned()))
            }
            Err(e) => Some(Message::Error(format!("{e:#}"))),
        };
    }
}

fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}
