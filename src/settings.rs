//! Settings window: shows the capture hotkey and records a new one.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use eframe::egui::{self, Align, Event, Key, Layout, RichText, ViewportCommand};

use crate::icon;
use crate::shortcut::Shortcut;

const WINDOW_SIZE: [f32; 2] = [420.0, 170.0];

/// Shows the settings window until the user closes it. `apply` is called with every recorded
/// hotkey and reports why it cannot be used; the window shows the result.
pub fn edit_hotkey(current: Shortcut, apply: &mut dyn FnMut(Shortcut) -> Result<()>) -> Result<()> {
    let icon = egui::IconData {
        rgba: icon::rgba(),
        width: icon::SIZE,
        height: icon::SIZE,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Settings")
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(Arc::new(icon)),
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "ochco-settings",
        options,
        Box::new(move |_| {
            Ok(Box::new(SettingsApp {
                hotkey: current,
                recording: false,
                super_held: false,
                message: None,
                apply,
            }))
        }),
    )
    .map_err(|e| anyhow!("settings window failed: {e}"))
}

enum Message {
    Info(String),
    Error(String),
}

struct SettingsApp<'a> {
    hotkey: Shortcut,
    /// Waiting for the user to press the new shortcut.
    recording: bool,
    /// egui's `Modifiers` has no Windows / Super key, so its presses are tracked separately.
    super_held: bool,
    message: Option<Message>,
    apply: &'a mut dyn FnMut(Shortcut) -> Result<()>,
}

impl eframe::App for SettingsApp<'_> {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.track_super_key(ui.ctx());

        if self.recording {
            self.record(ui.ctx());
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Capture hotkey");
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                let shortcut = if self.recording {
                    RichText::new("Press the new shortcut…").italics()
                } else {
                    RichText::new(self.hotkey.to_string()).strong()
                };
                ui.label(shortcut.size(18.0));

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if self.recording { "Cancel" } else { "Record hotkey" };
                    if ui.button(label).clicked() {
                        self.recording = !self.recording;
                        self.message = None;
                    }
                });
            });

            ui.add_space(8.0);
            self.show_message(ui);

            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Reset to default").clicked() {
                        self.recording = false;
                        self.apply(Shortcut::default());
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                        }
                    });
                });
            });
        });
    }
}

impl SettingsApp<'_> {
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

        self.recording = false;

        if key == Key::Escape {
            return;
        }

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

    fn apply(&mut self, shortcut: Shortcut) {
        if let Some(problem) = shortcut.problem() {
            self.message = Some(Message::Error(problem.to_owned()));
            return;
        }

        self.message = Some(match (self.apply)(shortcut) {
            Ok(()) => {
                self.hotkey = shortcut;
                let saved = format!("Saved. Press {shortcut} to capture text.");
                Message::Info(shortcut.warning().map_or(saved, |warning| format!("Saved. {warning}")))
            }
            Err(e) => Message::Error(format!("{e:#}")),
        });
    }

    fn show_message(&self, ui: &mut egui::Ui) {
        match &self.message {
            Some(Message::Info(text)) => {
                ui.label(text);
            }
            Some(Message::Error(text)) => {
                ui.colored_label(ui.visuals().error_fg_color, text);
            }
            None => {
                ui.weak("Click “Record hotkey” and press the shortcut you want to use.");
            }
        }
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
