//! Fullscreen overlay showing a frozen screenshot where the user drags a selection.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use eframe::egui::emath::GuiRounding as _;
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Key, Painter, Pos2, Rect, Stroke, StrokeKind, TextureOptions, Vec2,
    ViewportCommand,
};
use image::RgbaImage;

use crate::capture::Screenshot;

const DIM: Color32 = Color32::from_black_alpha(120);
const ACCENT: Color32 = Color32::from_rgb(64, 156, 255);
const HINT: &str = "Drag to select text  ·  Esc to cancel";
const HINT_BACKGROUND: Color32 = Color32::from_black_alpha(190);
const HINT_PADDING: Vec2 = Vec2::new(14.0, 8.0);
/// Selections smaller than this (in screenshot pixels) are treated as accidental clicks.
const MIN_SELECTION_PX: u32 = 4;

/// Selection rectangle in screenshot pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Shows the overlay and returns the selected part of the screenshot, or `None` if cancelled.
pub fn select_region(shot: &Screenshot) -> Result<Option<RgbaImage>> {
    let selection: Rc<Cell<Option<PixelRect>>> = Rc::default();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ochco")
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(true)
            .with_monitor(shot.monitor_index),
        centered: false,
        ..Default::default()
    };

    let started = Instant::now();
    let app_selection = Rc::clone(&selection);

    eframe::run_native(
        "ochco-overlay",
        options,
        Box::new(|cc| {
            let size = [shot.image.width() as usize, shot.image.height() as usize];
            let pixels = egui::ColorImage::from_rgba_unmultiplied(size, shot.image.as_raw());
            let texture = cc.egui_ctx.load_texture("screenshot", pixels, TextureOptions::NEAREST);
            log::debug!("overlay window created in {:?}", started.elapsed());

            Ok(Box::new(Overlay {
                texture,
                image_size: size,
                drag_start: None,
                selection: app_selection,
            }))
        }),
    )
    .map_err(|e| anyhow!("overlay window failed: {e}"))?;

    let region = selection
        .get()
        .map(|r| image::imageops::crop_imm(&shot.image, r.x, r.y, r.width, r.height).to_image());

    Ok(region)
}

struct Overlay {
    texture: egui::TextureHandle,
    image_size: [usize; 2],
    drag_start: Option<Pos2>,
    /// Written once the user finishes a selection; read after the window closes.
    selection: Rc<Cell<Option<PixelRect>>>,
}

impl eframe::App for Overlay {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let screen = ui.max_rect();
        ctx.set_cursor_icon(CursorIcon::Crosshair);

        // The borderless window covers the whole monitor, so the GPU driver treats it as a
        // fullscreen app and enables variable refresh rate (G-Sync / FreeSync). Repainting only
        // on input makes the frame rate jump, which VRR monitors show as brightness flicker;
        // a steady vsynced frame rate avoids that.
        ctx.request_repaint();

        let (pressed, released, cursor, cancel) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.interact_pos().or(i.pointer.hover_pos()),
                i.key_pressed(Key::Escape) || i.pointer.secondary_pressed(),
            )
        });

        if cancel {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }

        if pressed {
            self.drag_start = cursor;
        }

        let selection = self
            .drag_start
            .zip(cursor)
            .map(|(start, end)| Rect::from_two_pos(start, end).intersect(screen))
            // Whole physical pixels, so the dimmed rectangles around the selection tile without
            // seams or overlapping edges.
            .map(|sel| sel.round_to_pixels(ctx.pixels_per_point()));

        let painter = ui.painter();
        self.paint_screenshot(painter, screen, selection);
        paint_hint(painter, screen);

        if released {
            if let Some(rect) = selection.map(|sel| self.to_pixels(screen, sel))
                && rect.width >= MIN_SELECTION_PX
                && rect.height >= MIN_SELECTION_PX
            {
                self.selection.set(Some(rect));
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }

            self.drag_start = None;
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}

impl Overlay {
    /// Converts a selection in window points into screenshot pixels.
    fn to_pixels(&self, screen: Rect, selection: Rect) -> PixelRect {
        let [image_w, image_h] = self.image_size.map(|v| v as f32);
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
    fn paint_screenshot(&self, painter: &Painter, screen: Rect, selection: Option<Rect>) {
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

/// Draws the usage hint at the top center of the screen.
fn paint_hint(painter: &Painter, screen: Rect) {
    let galley = painter.layout_no_wrap(HINT.to_owned(), FontId::proportional(15.0), Color32::WHITE);

    let center = Pos2::new(screen.center().x, screen.min.y + 24.0);
    let background = Rect::from_center_size(center, galley.size()).expand2(HINT_PADDING);

    painter.rect_filled(background, 8.0, HINT_BACKGROUND);
    painter.galley(background.min + HINT_PADDING, galley, Color32::WHITE);
}
