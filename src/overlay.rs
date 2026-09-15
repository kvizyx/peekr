//! Fullscreen overlay showing a frozen screenshot where the user drags a selection.

use std::cell::Cell;
use std::rc::Rc;

use anyhow::{Result, anyhow};
use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Key, Pos2, Rect, Stroke, StrokeKind, TextureOptions, ViewportCommand,
};
use image::RgbaImage;

use crate::capture::Screenshot;

/// Selection rectangle in screenshot pixels: x, y, width, height.
type PixelRect = [u32; 4];

const DIM: Color32 = Color32::from_black_alpha(120);
const ACCENT: Color32 = Color32::from_rgb(64, 156, 255);
const MIN_SELECTION_PX: u32 = 4;

/// Shows the overlay and returns the selected part of the screenshot, or `None` if cancelled.
pub fn select_region(shot: &Screenshot) -> Result<Option<RgbaImage>> {
    let result: Rc<Cell<Option<PixelRect>>> = Rc::default();

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

    let app_result = result.clone();
    let started = std::time::Instant::now();
    eframe::run_native(
        "ochco-overlay",
        options,
        Box::new(|cc| {
            let size = [shot.image.width() as usize, shot.image.height() as usize];
            let pixels = egui::ColorImage::from_rgba_unmultiplied(size, shot.image.as_raw());
            let texture = cc.egui_ctx.load_texture("screenshot", pixels, TextureOptions::NEAREST);
            log::debug!("overlay window created in {:?}", started.elapsed());
            Ok(Box::new(Overlay { texture, image_size: size, drag_start: None, result: app_result }))
        }),
    )
    .map_err(|e| anyhow!("overlay window failed: {e}"))?;

    Ok(result.get().map(|[x, y, w, h]| image::imageops::crop_imm(&shot.image, x, y, w, h).to_image()))
}

struct Overlay {
    texture: egui::TextureHandle,
    image_size: [usize; 2],
    drag_start: Option<Pos2>,
    result: Rc<Cell<Option<PixelRect>>>,
}

impl Overlay {
    fn to_pixels(&self, screen: Rect, sel: Rect) -> PixelRect {
        let sx = self.image_size[0] as f32 / screen.width();
        let sy = self.image_size[1] as f32 / screen.height();
        let x0 = ((sel.min.x - screen.min.x) * sx).round().clamp(0.0, self.image_size[0] as f32) as u32;
        let y0 = ((sel.min.y - screen.min.y) * sy).round().clamp(0.0, self.image_size[1] as f32) as u32;
        let x1 = ((sel.max.x - screen.min.x) * sx).round().clamp(0.0, self.image_size[0] as f32) as u32;
        let y1 = ((sel.max.y - screen.min.y) * sy).round().clamp(0.0, self.image_size[1] as f32) as u32;
        [x0, y0, x1 - x0, y1 - y0]
    }
}

impl eframe::App for Overlay {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let screen = ui.max_rect();
        let painter = ui.painter();
        ctx.set_cursor_icon(CursorIcon::Crosshair);

        let (pressed, released, pointer, cancel) = ctx.input(|i| {
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
            self.drag_start = pointer;
        }
        let selection = match (self.drag_start, pointer) {
            (Some(start), Some(end)) => Some(Rect::from_two_pos(start, end).intersect(screen)),
            _ => None,
        };

        painter.image(self.texture.id(), screen, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);

        match selection {
            Some(sel) => {
                // Dim everything around the selection.
                for r in [
                    Rect::from_min_max(screen.min, Pos2::new(screen.max.x, sel.min.y)),
                    Rect::from_min_max(Pos2::new(screen.min.x, sel.max.y), screen.max),
                    Rect::from_min_max(Pos2::new(screen.min.x, sel.min.y), Pos2::new(sel.min.x, sel.max.y)),
                    Rect::from_min_max(Pos2::new(sel.max.x, sel.min.y), Pos2::new(screen.max.x, sel.max.y)),
                ] {
                    painter.rect_filled(r, 0.0, DIM);
                }
                painter.rect_stroke(sel, 0.0, Stroke::new(1.5, ACCENT), StrokeKind::Outside);

                let [_, _, w, h] = self.to_pixels(screen, sel);
                let label_pos = Pos2::new(sel.min.x, (sel.min.y - 4.0).max(screen.min.y + 16.0));
                painter.text(label_pos, Align2::LEFT_BOTTOM, format!("{w} × {h}"), FontId::proportional(13.0), Color32::WHITE);
            }
            None => {
                painter.rect_filled(screen, 0.0, DIM);
            }
        }

        let hint = "Drag to select text  ·  Esc to cancel";
        let hint_pos = Pos2::new(screen.center().x, screen.min.y + 24.0);
        let galley = painter.layout_no_wrap(hint.to_owned(), FontId::proportional(15.0), Color32::WHITE);
        let bg = Rect::from_center_size(hint_pos, galley.size()).expand2(egui::vec2(14.0, 8.0));
        painter.rect_filled(bg, 8.0, Color32::from_black_alpha(190));
        painter.galley(bg.min + egui::vec2(14.0, 8.0), galley, Color32::WHITE);

        if released {
            if let Some(sel) = selection {
                let px = self.to_pixels(screen, sel);
                if px[2] >= MIN_SELECTION_PX && px[3] >= MIN_SELECTION_PX {
                    self.result.set(Some(px));
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            self.drag_start = None;
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}
