//! Fullscreen overlay showing a frozen screenshot. The user drags a selection, and the recognized
//! text appears next to it with a button to copy it.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::emath::GuiRounding as _;
use egui::{
    Align, Align2, Area, Button, Color32, CornerRadius, CursorIcon, Event, FontId, Frame, Id, Key, Label, Layout,
    Margin, Order, Painter, Pos2, Rect, RichText, ScrollArea, Shadow, Spinner, Stroke, StrokeKind, TextureOptions,
    Vec2, ViewportCommand, pos2, vec2,
};
use image::RgbaImage;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowLevel};

use crate::capture::Screenshot;
use crate::window;

const DIM: Color32 = Color32::from_black_alpha(150);
const ACCENT: Color32 = Color32::from_rgb(249, 115, 22);
const HINT: &str = "Drag to select text  ·  Esc to cancel";
const HINT_BACKGROUND: Color32 = Color32::from_black_alpha(190);
const HINT_PADDING: Vec2 = Vec2::new(14.0, 8.0);
/// Selections smaller than this (in screenshot pixels) are treated as accidental clicks.
const MIN_SELECTION_PX: u32 = 4;

const CARD_BACKGROUND: Color32 = Color32::from_rgb(30, 30, 33);
const CARD_BORDER: Color32 = Color32::from_rgb(48, 48, 52);
const CARD_TEXT: Color32 = Color32::from_rgb(236, 236, 240);
const CARD_MUTED: Color32 = Color32::from_rgb(150, 150, 158);
const CARD_WIDTH: (f32, f32) = (300.0, 560.0);
/// Distance between the selection and the card, and between the card and the screen edges.
const CARD_GAP: f32 = 10.0;
/// Space the card needs below or above the selection; otherwise it goes inside the selection.
const CARD_ROOM: f32 = 240.0;
const TEXT_MAX_HEIGHT: f32 = 320.0;
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

/// Shows the overlay until the user copies the recognized text or cancels.
pub fn capture_text(shot: &Screenshot, recognize: Recognize<'_>, copy: CopyText<'_>) -> Result<()> {
    let started = Instant::now();

    window::run(
        |event_loop| {
            let attributes = Window::default_attributes()
                .with_title("ochco")
                .with_decorations(false)
                .with_window_level(WindowLevel::AlwaysOnTop);

            cover_monitor(attributes, event_loop, shot)
        },
        |ctx| {
            ctx.set_visuals(egui::Visuals::dark());

            let size = [shot.image.width() as usize, shot.image.height() as usize];
            let pixels = egui::ColorImage::from_rgba_unmultiplied(size, shot.image.as_raw());
            let texture = ctx.load_texture("screenshot", pixels, TextureOptions::NEAREST);
            log::debug!("overlay window created in {:?}", started.elapsed());

            Overlay {
                texture,
                image: &shot.image,
                stage: Stage::Selecting { drag_start: None },
                card: None,
                copy_error: None,
                recognize,
                copy,
            }
        },
    )
}

/// Makes the window cover the monitor the screenshot was taken from: a plain window of the
/// monitor's size, as switching the display in and out of fullscreen mode makes it blink.
#[cfg(windows)]
fn cover_monitor(attributes: WindowAttributes, _: &ActiveEventLoop, shot: &Screenshot) -> WindowAttributes {
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::platform::windows::WindowAttributesExtWindows as _;

    attributes
        .with_position(PhysicalPosition::new(shot.origin.0, shot.origin.1))
        .with_inner_size(PhysicalSize::new(shot.image.width(), shot.image.height()))
        .with_skip_taskbar(true)
}

/// Makes the window cover the monitor the screenshot was taken from. Wayland does not let
/// clients position windows, so fullscreen is the only way to get there.
#[cfg(not(windows))]
fn cover_monitor(attributes: WindowAttributes, event_loop: &ActiveEventLoop, shot: &Screenshot) -> WindowAttributes {
    use winit::window::Fullscreen;

    let monitor = event_loop.available_monitors().nth(shot.monitor_index);

    attributes.with_fullscreen(Some(Fullscreen::Borderless(monitor)))
}

enum Stage {
    Selecting {
        drag_start: Option<Pos2>,
    },
    Recognizing {
        selection: Rect,
        result: Receiver<Result<String>>,
    },
    Done {
        selection: Rect,
        /// The recognized text, or why recognition failed.
        result: Result<String, String>,
    },
}

struct Overlay<'a> {
    texture: egui::TextureHandle,
    image: &'a RgbaImage,
    stage: Stage,
    /// Where the result card was drawn last frame, so clicks on it don't start a new selection.
    card: Option<Rect>,
    copy_error: Option<String>,
    recognize: Recognize<'a>,
    copy: CopyText<'a>,
}

impl window::App for Overlay<'_> {
    fn ui(&mut self, ui: &mut egui::Ui) {
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
            .is_some_and(|(card, cursor)| card.contains(cursor));
        if !over_card {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
        }

        // Dragging anywhere but on the card starts over, dropping the previous result.
        if pressed && !over_card {
            self.stage = Stage::Selecting { drag_start: cursor };
            self.card = None;
            self.copy_error = None;
        }

        let selection = match &self.stage {
            Stage::Selecting { drag_start } => drag_start
                .zip(cursor)
                .map(|(start, end)| Rect::from_two_pos(start, end).intersect(screen))
                // Whole physical pixels, so the dimmed rectangles around the selection tile
                // without seams or overlapping edges.
                .map(|sel| sel.round_to_pixels(ctx.pixels_per_point())),
            Stage::Recognizing { selection, .. } | Stage::Done { selection, .. } => Some(*selection),
        };
        let selecting = matches!(self.stage, Stage::Selecting { .. });

        let painter = ui.painter();
        self.paint_screenshot(painter, screen, selection, selecting);

        if selecting {
            paint_hint(painter, screen);

            if released {
                self.finish_selection(screen, selection);
            }
        }

        if matches!(self.stage, Stage::Recognizing { .. }) {
            ctx.request_repaint_after(POLL_INTERVAL);
        }

        let copy_clicked = self.show_card(&ctx, screen);
        self.copy_text(&ctx, copy_clicked || confirm);
    }
}

impl Overlay<'_> {
    /// Sends the selected region for recognition, or keeps selecting after an accidental click.
    fn finish_selection(&mut self, screen: Rect, selection: Option<Rect>) {
        let region = selection.map(|sel| (sel, self.to_pixels(screen, sel)));

        let Some((selection, rect)) =
            region.filter(|(_, r)| r.width >= MIN_SELECTION_PX && r.height >= MIN_SELECTION_PX)
        else {
            self.stage = Stage::Selecting { drag_start: None };
            return;
        };

        let image = image::imageops::crop_imm(self.image, rect.x, rect.y, rect.width, rect.height).to_image();
        self.stage = Stage::Recognizing {
            selection,
            result: (self.recognize)(image),
        };
    }

    fn poll_recognition(&mut self) {
        let Stage::Recognizing { selection, result } = &self.stage else {
            return;
        };

        let result = match result.try_recv() {
            Ok(result) => result.map_err(|e| format!("Recognition failed: {e:#}")),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Recognition failed: the OCR worker stopped".to_owned()),
        };

        self.stage = Stage::Done {
            selection: *selection,
            result,
        };
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

    /// Draws the card with the recognition progress or result. Returns whether Copy was clicked.
    fn show_card(&mut self, ctx: &egui::Context, screen: Rect) -> bool {
        let (selection, result) = match &self.stage {
            Stage::Selecting { .. } => return false,
            Stage::Recognizing { selection, .. } => (*selection, None),
            Stage::Done { selection, result } => (*selection, Some(result)),
        };

        let width = selection
            .width()
            .clamp(CARD_WIDTH.0, CARD_WIDTH.1)
            .min(screen.width() - 2.0 * CARD_GAP);
        let (position, pivot) = card_position(screen, selection);
        let mut copy_clicked = false;

        let response = Area::new(Id::new("ocr-result"))
            .order(Order::Foreground)
            .fixed_pos(position)
            .pivot(pivot)
            .constrain_to(screen.shrink(CARD_GAP))
            .show(ctx, |ui| {
                card_frame().show(ui, |ui| {
                    ui.set_width(width);

                    match result {
                        None => {
                            ui.horizontal(|ui| {
                                ui.add(Spinner::new().size(16.0).color(ACCENT));
                                ui.label(RichText::new("Recognizing…").color(CARD_MUTED));
                            });
                        }
                        Some(Err(message)) => {
                            ui.label(RichText::new(message).color(ui.visuals().error_fg_color));
                        }
                        Some(Ok(text)) if text.is_empty() => {
                            ui.label(RichText::new("No text found").color(CARD_MUTED));
                        }
                        Some(Ok(text)) => copy_clicked = show_text(ui, text),
                    }

                    if let Some(error) = &self.copy_error {
                        ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
                    }
                });
            });

        self.card = Some(response.response.rect);

        copy_clicked
    }

    /// Converts a selection in window points into screenshot pixels.
    fn to_pixels(&self, screen: Rect, selection: Rect) -> PixelRect {
        let (image_w, image_h) = (self.image.width() as f32, self.image.height() as f32);
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
    fn paint_screenshot(&self, painter: &Painter, screen: Rect, selection: Option<Rect>, selecting: bool) {
        let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(self.texture.id(), screen, full_uv, Color32::WHITE);

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

        if !selecting {
            return;
        }

        let size = self.to_pixels(screen, sel);
        let label = format!("{} × {}", size.width, size.height);
        let label_pos = Pos2::new(sel.min.x, (sel.min.y - 4.0).max(screen.min.y + 16.0));
        painter.text(
            label_pos,
            Align2::LEFT_BOTTOM,
            label,
            FontId::proportional(13.0),
            Color32::WHITE,
        );
    }
}

/// Shows the recognized text with the Copy button below it. Returns whether Copy was clicked.
fn show_text(ui: &mut egui::Ui, text: &str) -> bool {
    ScrollArea::vertical()
        .max_height(TEXT_MAX_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.add(Label::new(RichText::new(text).size(15.0).color(CARD_TEXT)).wrap());
        });

    ui.add_space(12.0);

    ui.horizontal(|ui| {
        ui.label(RichText::new("Esc to close").size(12.0).color(CARD_MUTED));

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let copy = Button::new(RichText::new("Copy").color(Color32::WHITE).strong())
                .fill(ACCENT)
                .corner_radius(6.0)
                .min_size(vec2(76.0, 28.0));

            ui.add(copy).on_hover_text("Enter or Ctrl+C").clicked()
        })
        .inner
    })
    .inner
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

fn card_frame() -> Frame {
    Frame::new()
        .fill(CARD_BACKGROUND)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .shadow(Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(110),
        })
}

/// Draws the usage hint at the top center of the screen.
fn paint_hint(painter: &Painter, screen: Rect) {
    let galley = painter.layout_no_wrap(HINT.to_owned(), FontId::proportional(15.0), Color32::WHITE);

    let center = Pos2::new(screen.center().x, screen.min.y + 24.0);
    let background = Rect::from_center_size(center, galley.size()).expand2(HINT_PADDING);

    painter.rect_filled(background, 8.0, HINT_BACKGROUND);
    painter.galley(background.min + HINT_PADDING, galley, Color32::WHITE);
}
