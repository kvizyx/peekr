//! Fullscreen overlay showing a frozen screenshot. The user drags a selection, and the recognized
//! text appears next to it with a button to copy it.

use std::f64::consts::TAU;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::emath::GuiRounding as _;
use egui::epaint::RectShape;
use egui::{
    Align, Align2, Area, Button, Color32, CornerRadius, CursorIcon, Event, Frame, Id, Key, Label, Layout, Margin,
    Order, Painter, Pos2, Rect, RichText, ScrollArea, Sense, Shape, Stroke, StrokeKind, TextureOptions, UiBuilder,
    Vec2, ViewportCommand, pos2, vec2,
};
use image::RgbaImage;
use winit::monitor::MonitorHandle;
use winit::window::{Window, WindowAttributes, WindowLevel};

use crate::capture::Screenshot;
use crate::theme::{self, ACCENT};
use crate::window;

const DIM: Color32 = Color32::from_black_alpha(150);
/// Distance between the top of the screen and the usage hint.
const HINT_TOP: f32 = 16.0;
const HINT_PADDING: Margin = Margin::symmetric(18, 10);
/// Selections smaller than this (in screenshot pixels) are treated as accidental clicks.
const MIN_SELECTION_PX: u32 = 4;

const BUTTON_PADDING: Vec2 = Vec2::new(14.0, 6.0);
const BUTTON_HEIGHT: f32 = 32.0;
/// Space between a key name and the edges of its key cap.
const KEY_PADDING: Vec2 = Vec2::new(6.0, 3.0);
const CARD_WIDTH: (f32, f32) = (300.0, 560.0);
/// Distance between the selection and the card, and between the card and the screen edges.
const CARD_GAP: f32 = 10.0;
/// Space the card needs below or above the selection; otherwise it goes inside the selection.
const CARD_ROOM: f32 = 240.0;
const TEXT_MAX_HEIGHT: f32 = 320.0;
/// The loader appears only when recognition takes longer than this.
const LOADER_DELAY: Duration = Duration::from_millis(300);
/// Smallest and largest size of the loader, which is scaled to the selection in between.
const LOADER_SIZE: (f32, f32) = (10.0, 36.0);
/// Width of the dark outline around the loader's arc.
const LOADER_OUTLINE: f32 = 2.0;
/// How often a pending recognition is checked for a result.
const POLL_INTERVAL: Duration = Duration::from_millis(30);

/// Selection rectangle in screenshot pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Recognizes a selected region; the text arrives on the returned channel.
pub type Recognize<'a> = &'a dyn Fn(RgbaImage) -> Receiver<Result<String>>;

/// Puts the text on the clipboard.
pub type CopyText<'a> = &'a mut dyn FnMut(&str) -> Result<()>;

/// Shows the overlay, one window per screenshot, until the user copies the recognized text or
/// cancels.
pub fn capture_text(screens: &[Screenshot], recognize: Recognize<'_>, copy: CopyText<'_>) -> Result<()> {
    let started = Instant::now();

    window::run(
        |event_loop| {
            let monitors: Vec<_> = event_loop.available_monitors().collect();

            screens
                .iter()
                .enumerate()
                .map(|(index, shot)| {
                    let attributes = Window::default_attributes()
                        .with_title("peekr")
                        .with_decorations(false)
                        .with_window_level(WindowLevel::AlwaysOnTop);

                    cover_monitor(attributes, &monitors, screens.len(), index, shot)
                })
                .collect()
        },
        |contexts| {
            let views = contexts
                .iter()
                .zip(screens)
                .map(|(ctx, shot)| {
                    ctx.set_visuals(egui::Visuals::dark());

                    let size = [shot.image.width() as usize, shot.image.height() as usize];
                    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, shot.image.as_raw());

                    ScreenView {
                        texture: ctx.load_texture("screenshot", pixels, TextureOptions::NEAREST),
                        image: &shot.image,
                    }
                })
                .collect();
            log::debug!("overlay windows created in {:?}", started.elapsed());

            Overlay {
                screens: views,
                stage: Stage::Selecting { drag: None },
                card: None,
                selections: 0,
                copy_error: None,
                repaint: vec![false; contexts.len()],
                recognize,
                copy,
            }
        },
    )
}

/// Makes the window cover the monitor the screenshot was taken from: a plain window of the
/// monitor's size, as switching the display in and out of fullscreen mode makes it blink.
#[cfg(windows)]
fn cover_monitor(
    attributes: WindowAttributes,
    _: &[MonitorHandle],
    _: usize,
    _: usize,
    shot: &Screenshot,
) -> WindowAttributes {
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::platform::windows::WindowAttributesExtWindows as _;

    attributes
        .with_position(PhysicalPosition::new(shot.origin.0, shot.origin.1))
        .with_inner_size(PhysicalSize::new(shot.image.width(), shot.image.height()))
        .with_skip_taskbar(true)
}

/// Makes the window cover the monitor the screenshot was taken from. Wayland does not let
/// clients position windows, so fullscreen is the only way to get there.
///
/// xcap and winit list monitors in different orders, so the monitor is found by its output name,
/// then by position, and only then by index when both see the same number of monitors.
#[cfg(not(windows))]
fn cover_monitor(
    attributes: WindowAttributes,
    monitors: &[MonitorHandle],
    screen_count: usize,
    index: usize,
    shot: &Screenshot,
) -> WindowAttributes {
    use winit::dpi::PhysicalPosition;
    use winit::window::Fullscreen;

    log::debug!(
        "captured monitor {:?} at {:?}; window system monitors: {:?}",
        shot.monitor_name,
        shot.origin,
        monitors.iter().map(|m| (m.name(), m.position())).collect::<Vec<_>>()
    );

    let monitor = monitors
        .iter()
        .find(|m| {
            m.name()
                .is_some_and(|name| shot.monitor_name.as_deref() == Some(&*name))
        })
        .or_else(|| {
            monitors.iter().find(|m| {
                let position = m.position();
                (position.x, position.y) == shot.origin
            })
        })
        .or_else(|| (monitors.len() == screen_count).then(|| monitors.get(index)).flatten())
        .cloned();

    if monitor.is_none() {
        log::warn!(
            "no monitor matches {:?} at {:?}; the overlay opens on the current one",
            shot.monitor_name,
            shot.origin
        );
    }

    // X11 window managers put a fullscreen window on the monitor it was mapped on, so it starts
    // out there as well.
    attributes
        .with_position(PhysicalPosition::new(shot.origin.0, shot.origin.1))
        .with_fullscreen(Some(Fullscreen::Borderless(monitor)))
}

/// One overlay window: a monitor's screenshot.
struct ScreenView<'a> {
    texture: egui::TextureHandle,
    image: &'a RgbaImage,
}

/// Progress of the capture, shared by all windows. `screen` is the window the selection is on.
enum Stage {
    Selecting {
        drag: Option<(usize, Pos2)>,
    },
    Recognizing {
        screen: usize,
        selection: Rect,
        result: Receiver<Result<String>>,
        started: Instant,
    },
    Done {
        screen: usize,
        selection: Rect,
        /// The recognized text, or why recognition failed.
        result: Result<String, String>,
    },
}

impl Stage {
    /// The selection on the given window; a selection being dragged ends at the cursor.
    fn selection(&self, screen: usize, cursor: Option<Pos2>) -> Option<Rect> {
        match *self {
            Self::Selecting {
                drag: Some((on, start)),
            } if on == screen => cursor.map(|end| Rect::from_two_pos(start, end)),
            Self::Recognizing {
                screen: on, selection, ..
            }
            | Self::Done {
                screen: on, selection, ..
            } if on == screen => Some(selection),
            _ => None,
        }
    }

    /// The window the selection is on, if there is one.
    fn screen(&self) -> Option<usize> {
        match *self {
            Self::Selecting { drag } => drag.map(|(on, _)| on),
            Self::Recognizing { screen, .. } | Self::Done { screen, .. } => Some(screen),
        }
    }
}

struct Overlay<'a> {
    screens: Vec<ScreenView<'a>>,
    stage: Stage,
    /// Window and area of the result card last frame, so clicks on it don't start a new selection.
    card: Option<(usize, Rect)>,
    /// Counts selections, so each one gets a new card placed next to it, even when the previous
    /// card was dragged away.
    selections: u64,
    copy_error: Option<String>,
    /// Windows to draw again because the shared stage changed while another window was drawn.
    repaint: Vec<bool>,
    recognize: Recognize<'a>,
    copy: CopyText<'a>,
}

impl window::App for Overlay<'_> {
    fn ui(&mut self, index: usize, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let screen = ui.max_rect();

        let (pressed, released, cursor, cancel, confirm) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.interact_pos().or(i.pointer.hover_pos()),
                i.key_pressed(Key::Escape) || i.pointer.secondary_pressed(),
                i.key_pressed(Key::Enter) || i.events.iter().any(|e| matches!(e, Event::Copy)),
            )
        });

        if cancel {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        self.poll_recognition();

        let over_card = self
            .card
            .zip(cursor)
            .is_some_and(|((on, card), cursor)| on == index && card.contains(cursor));
        if !over_card {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
        }

        // Dragging anywhere but on the card starts over, dropping the previous result.
        if pressed && !over_card {
            self.set_stage(Stage::Selecting {
                drag: cursor.map(|start| (index, start)),
            });
            self.card = None;
            self.selections += 1;
            self.copy_error = None;
        }

        let selection = self
            .stage
            .selection(index, cursor)
            .map(|sel| sel.intersect(screen))
            // Whole physical pixels, so the dimmed rectangles around the selection tile without
            // seams or overlapping edges.
            .map(|sel| sel.round_to_pixels(ctx.pixels_per_point()));
        let selecting = matches!(self.stage, Stage::Selecting { .. });

        let painter = ui.painter();
        self.paint_screenshot(index, painter, screen, selection);

        // The hint stays out of the way while the user drags.
        if !matches!(self.stage, Stage::Selecting { drag: Some(_) }) {
            show_hint(&ctx, screen);
        }

        if selecting && released && self.stage.screen() == Some(index) {
            self.finish_selection(index, screen, selection);
        }

        if let Stage::Recognizing {
            screen: on,
            selection,
            started,
            ..
        } = self.stage
            && on == index
        {
            // Most recognitions finish sooner, and the loader would only flash.
            if started.elapsed() >= LOADER_DELAY {
                paint_loader(ui, selection);
            }
            ctx.request_repaint_after(POLL_INTERVAL);
        }

        let copy_clicked = self.show_card(index, &ctx, screen);
        self.copy_text(&ctx, copy_clicked || confirm);
    }

    fn take_repaint(&mut self, window: usize) -> bool {
        self.repaint.get_mut(window).is_some_and(std::mem::take)
    }
}

impl Overlay<'_> {
    /// Changes the shared stage and redraws every window, as each shows part of it.
    fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
        self.repaint.fill(true);
    }

    /// Sends the selected region for recognition, or keeps selecting after an accidental click.
    fn finish_selection(&mut self, index: usize, screen: Rect, selection: Option<Rect>) {
        let region = selection.map(|sel| (sel, self.to_pixels(index, screen, sel)));

        let Some((selection, rect)) =
            region.filter(|(_, r)| r.width >= MIN_SELECTION_PX && r.height >= MIN_SELECTION_PX)
        else {
            self.set_stage(Stage::Selecting { drag: None });
            return;
        };

        let image = self.screens[index].image;
        let region = image::imageops::crop_imm(image, rect.x, rect.y, rect.width, rect.height).to_image();

        self.set_stage(Stage::Recognizing {
            screen: index,
            selection,
            result: (self.recognize)(region),
            started: Instant::now(),
        });
    }

    fn poll_recognition(&mut self) {
        let Stage::Recognizing {
            screen,
            selection,
            result,
            ..
        } = &self.stage
        else {
            return;
        };

        let result = match result.try_recv() {
            Ok(result) => result.map_err(|e| format!("Recognition failed: {e:#}")),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Recognition failed: the OCR worker stopped".to_owned()),
        };

        self.set_stage(Stage::Done {
            screen: *screen,
            selection: *selection,
            result,
        });
    }

    /// Copies the recognized text and closes the overlay when the user asks for it.
    fn copy_text(&mut self, ctx: &egui::Context, requested: bool) {
        let Stage::Done { result: Ok(text), .. } = &self.stage else {
            return;
        };
        if !requested || text.is_empty() {
            return;
        }

        match (self.copy)(text) {
            Ok(()) => ctx.send_viewport_cmd(ViewportCommand::Close),
            Err(e) => self.copy_error = Some(format!("Couldn't copy: {e:#}")),
        }
    }

    /// Draws the card with the recognition result when the selection is on this window. Returns
    /// whether Copy and Close was clicked.
    fn show_card(&mut self, index: usize, ctx: &egui::Context, screen: Rect) -> bool {
        let Stage::Done {
            screen: on,
            selection,
            result,
        } = &self.stage
        else {
            return false;
        };
        if *on != index {
            return false;
        }

        let selection = *selection;
        // Only the recognized text can be moved out of the way; the rest are short notes.
        let movable = matches!(result, Ok(text) if !text.is_empty());

        let width = selection
            .width()
            .clamp(CARD_WIDTH.0, CARD_WIDTH.1)
            .min(screen.width() - 2.0 * CARD_GAP);
        let (position, pivot) = card_position(screen, selection);
        let mut copy_clicked = false;

        // Egui keeps where the card was dragged to.
        let response = Area::new(Id::new(("ocr-result", self.selections)))
            .order(Order::Foreground)
            .default_pos(position)
            .movable(movable)
            .pivot(pivot)
            .constrain_to(screen.shrink(CARD_GAP))
            .show(ctx, |ui| {
                // The card is dragged by its background, which the labels on it are part of. The
                // recognized text opts back in to selection.
                ui.style_mut().interaction.selectable_labels = false;

                theme::card(Margin::same(14)).show(ui, |ui| {
                    // Short statuses shrink the card to fit, while the text gets the full width.
                    ui.set_max_width(width);

                    match result {
                        Err(message) => {
                            ui.label(RichText::new(message).color(ui.visuals().error_fg_color));
                        }
                        Ok(text) if text.is_empty() => {
                            ui.label(RichText::new("No text found").color(theme::MUTED));
                        }
                        Ok(text) => {
                            ui.set_width(width);
                            copy_clicked = show_text(ui, text);
                        }
                    }

                    if let Some(error) = &self.copy_error {
                        ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
                    }
                });
            });

        self.card = Some((index, response.response.rect));

        copy_clicked
    }

    /// Converts a selection in window points into pixels of the window's screenshot.
    fn to_pixels(&self, index: usize, screen: Rect, selection: Rect) -> PixelRect {
        let image = self.screens[index].image;
        let (image_w, image_h) = (image.width() as f32, image.height() as f32);
        let scale = Vec2::new(image_w / screen.width(), image_h / screen.height());

        let to_px = |pos: Pos2| {
            let p = (pos - screen.min) * scale;
            (
                p.x.round().clamp(0.0, image_w) as u32,
                p.y.round().clamp(0.0, image_h) as u32,
            )
        };
        let (x0, y0) = to_px(selection.min);
        let (x1, y1) = to_px(selection.max);

        PixelRect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }

    /// Draws the frozen screenshot, dimmed everywhere except the selection.
    fn paint_screenshot(&self, index: usize, painter: &Painter, screen: Rect, selection: Option<Rect>) {
        let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(self.screens[index].texture.id(), screen, full_uv, Color32::WHITE);

        let Some(sel) = selection else {
            painter.rect_filled(screen, 0.0, DIM);
            return;
        };

        let around = [
            Rect::from_min_max(screen.min, Pos2::new(screen.max.x, sel.min.y)),
            Rect::from_min_max(Pos2::new(screen.min.x, sel.max.y), screen.max),
            Rect::from_min_max(Pos2::new(screen.min.x, sel.min.y), Pos2::new(sel.min.x, sel.max.y)),
            Rect::from_min_max(Pos2::new(sel.max.x, sel.min.y), Pos2::new(screen.max.x, sel.max.y)),
        ];
        for rect in around {
            painter.rect_filled(rect, 0.0, DIM);
        }

        painter.rect_stroke(sel, 0.0, Stroke::new(2.0, ACCENT), StrokeKind::Outside);
    }
}

/// Shows the recognized text in its own area with the Copy and Close button below it. Returns
/// whether the button was clicked.
fn show_text(ui: &mut egui::Ui, text: &str) -> bool {
    // Drags anywhere on the text area select text rather than move the card.
    ui.scope_builder(UiBuilder::new().sense(Sense::drag()), |ui| {
        Frame::new()
            .fill(theme::SURFACE)
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .max_height(TEXT_MAX_HEIGHT)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let text = RichText::new(text).size(15.0).color(theme::TEXT);
                        ui.add(Label::new(text).wrap().selectable(true));
                    });
            });
    });

    ui.add_space(12.0);

    ui.horizontal(|ui| {
        // The row is as tall as the button from the start, so the hints are centered against it.
        ui.set_min_height(BUTTON_HEIGHT);

        key_hint(ui, "Enter", "copy");
        ui.add_space(8.0);
        key_hint(ui, "Esc", "close");

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().button_padding = BUTTON_PADDING;

            let copy = Button::new(RichText::new("Copy and Close").color(Color32::WHITE).strong())
                .fill(ACCENT)
                .corner_radius(6.0)
                .min_size(vec2(0.0, BUTTON_HEIGHT))
                // Takes drags too, so they don't move the card.
                .sense(Sense::click_and_drag());

            ui.add(copy).on_hover_text("Enter or Ctrl+C").clicked()
        })
        .inner
    })
    .inner
}

/// Draws a keyboard shortcut as a key cap followed by what it does.
fn key_hint(ui: &mut egui::Ui, key: &str, action: &str) {
    ui.spacing_mut().item_spacing.x = 6.0;

    // The key cap is painted by hand, as a Frame in a row centered against the button would
    // stretch to the height of the row.
    let background = ui.painter().add(Shape::Noop);

    ui.add_space(KEY_PADDING.x);
    let key = ui.label(RichText::new(key).size(11.0).color(theme::TEXT)).rect;
    ui.add_space(KEY_PADDING.x);

    ui.painter().set(
        background,
        RectShape::new(
            key.expand2(KEY_PADDING),
            CornerRadius::same(5),
            theme::RAISED,
            Stroke::new(1.0, theme::BORDER),
            StrokeKind::Inside,
        ),
    );

    ui.label(RichText::new(action).size(12.0).color(theme::MUTED));
}

/// Places the card below the selection, above it when there is no room below, and inside its
/// bottom edge when it fills the screen.
fn card_position(screen: Rect, selection: Rect) -> (Pos2, Align2) {
    if screen.max.y - selection.max.y >= CARD_ROOM {
        (pos2(selection.min.x, selection.max.y + CARD_GAP), Align2::LEFT_TOP)
    } else if selection.min.y - screen.min.y >= CARD_ROOM {
        (pos2(selection.min.x, selection.min.y - CARD_GAP), Align2::LEFT_BOTTOM)
    } else {
        (
            pos2(selection.min.x + CARD_GAP, selection.max.y - CARD_GAP),
            Align2::LEFT_BOTTOM,
        )
    }
}

/// Draws a spinning arc at the center of the selection being recognized, shrunk to fit small
/// selections. A dark outline keeps it visible on any screenshot.
fn paint_loader(ui: &egui::Ui, selection: Rect) {
    let center = selection.center();
    let size = (selection.size().min_elem() * 0.6).clamp(LOADER_SIZE.0, LOADER_SIZE.1);
    let radius = size / 2.0;
    let width = (size / 12.0).clamp(1.5, 3.0);

    // The same motion as egui's spinner.
    let time = ui.input(|i| i.time);
    let start = time * TAU;
    let end = start + 240_f64.to_radians() * time.sin();

    // The outline reaches a little past the ends of the arc, so they are outlined too.
    let overhang = f64::from(LOADER_OUTLINE / radius) * (end - start).signum();
    let outline = arc(center, radius, start - overhang, end + overhang);
    let painter = ui.painter();

    painter.add(Shape::line(
        outline,
        Stroke::new(width + 2.0 * LOADER_OUTLINE, theme::BACKGROUND),
    ));
    painter.add(Shape::line(arc(center, radius, start, end), Stroke::new(width, ACCENT)));
}

/// Points along a circular arc between two angles in radians.
fn arc(center: Pos2, radius: f32, start: f64, end: f64) -> Vec<Pos2> {
    let steps = (radius.round() as u32).clamp(8, 128);

    (0..=steps)
        .map(|step| {
            let angle = start + (end - start) * f64::from(step) / f64::from(steps);
            let (sin, cos) = angle.sin_cos();

            center + radius * vec2(cos as f32, sin as f32)
        })
        .collect()
}

/// Shows the usage hint on a card at the top center of the screen.
fn show_hint(ctx: &egui::Context, screen: Rect) {
    Area::new(Id::new("hint"))
        .order(Order::Foreground)
        .interactable(false)
        .fixed_pos(pos2(screen.center().x, screen.min.y + HINT_TOP))
        .pivot(Align2::CENTER_TOP)
        .show(ctx, |ui| {
            theme::card(HINT_PADDING).show(ui, |ui| {
                ui.label(RichText::new("Drag to select text").size(16.0).color(theme::TEXT));
            });
        });
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use egui::{Modifiers, PointerButton, RawInput};

    use super::*;
    use crate::window::App as _;

    const SCREEN: Vec2 = Vec2::new(1200.0, 800.0);

    fn frame(ctx: &egui::Context, overlay: &mut Overlay<'_>, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            events,
            ..RawInput::default()
        };

        ctx.run_ui(input, |ui| overlay.ui(0, ui)).textures_delta.clear();
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    /// Drags the result card by the point `grab` picks on it and returns how far the card moved.
    fn drag_card(text: &str, grab: impl Fn(Rect) -> Pos2) -> Vec2 {
        let ctx = egui::Context::default();
        let image = RgbaImage::new(SCREEN.x as u32, SCREEN.y as u32);
        let pixels = egui::ColorImage::filled([image.width() as usize, image.height() as usize], Color32::BLACK);
        let recognize = |_| mpsc::channel().1;
        let mut copy = |_: &str| Ok(());

        let mut overlay = Overlay {
            screens: vec![ScreenView {
                texture: ctx.load_texture("screenshot", pixels, TextureOptions::NEAREST),
                image: &image,
            }],
            stage: Stage::Done {
                screen: 0,
                selection: Rect::from_min_size(pos2(100.0, 100.0), vec2(400.0, 60.0)),
                result: Ok(text.to_owned()),
            },
            card: None,
            selections: 0,
            copy_error: None,
            repaint: vec![false],
            recognize: &recognize,
            copy: &mut copy,
        };

        for _ in 0..3 {
            frame(&ctx, &mut overlay, Vec::new());
        }

        let before = overlay.card.expect("the card is shown").1;

        let start = grab(before);
        let end = start + vec2(80.0, 50.0);

        frame(
            &ctx,
            &mut overlay,
            vec![Event::PointerMoved(start), button(start, true)],
        );

        for step in 1..=5 {
            let pos = start + (end - start) * (step as f32 / 5.0);
            frame(&ctx, &mut overlay, vec![Event::PointerMoved(pos)]);
        }

        frame(&ctx, &mut overlay, vec![button(end, false)]);
        frame(&ctx, &mut overlay, Vec::new());

        overlay.card.expect("the card is shown").1.min - before.min
    }

    const MOVED: Vec2 = Vec2::new(80.0, 50.0);
    const TEXT: &str = "Some recognized text
on two lines";

    #[test]
    fn the_card_is_dragged_by_its_background() {
        assert_eq!(drag_card(TEXT, |card| card.min + vec2(4.0, 4.0)), MOVED);
    }

    #[test]
    fn the_card_is_dragged_by_the_key_hints() {
        // The hints are at the bottom left, next to the button.
        assert_eq!(drag_card(TEXT, |card| card.left_bottom() + vec2(30.0, -30.0)), MOVED);
    }

    #[test]
    fn dragging_the_text_does_not_move_the_card() {
        assert_eq!(drag_card(TEXT, |card| card.min + vec2(40.0, 34.0)), Vec2::ZERO);
    }

    #[test]
    fn dragging_the_text_area_padding_does_not_move_the_card() {
        assert_eq!(drag_card(TEXT, |card| card.min + vec2(18.0, 18.0)), Vec2::ZERO);
    }

    #[test]
    fn dragging_the_button_does_not_move_the_card() {
        assert_eq!(
            drag_card(TEXT, |card| card.right_bottom() - vec2(30.0, 30.0)),
            Vec2::ZERO
        );
    }

    #[test]
    fn the_no_text_card_is_not_dragged() {
        assert_eq!(drag_card("", |card| card.min + vec2(4.0, 4.0)), Vec2::ZERO);
    }
}
