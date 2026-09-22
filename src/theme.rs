//! Colors and shapes shared by the overlay and the settings window.

use egui::{Color32, CornerRadius, Frame, Margin, RichText, Sense, Shadow, Stroke, Visuals, pos2};

pub const ACCENT: Color32 = Color32::from_rgb(214, 104, 38);
pub const BACKGROUND: Color32 = Color32::from_rgb(30, 30, 33);
/// Areas set into the background, such as the recognized text.
pub const SURFACE: Color32 = Color32::from_rgb(22, 22, 25);
/// Areas raised above the background, such as key caps and buttons.
pub const RAISED: Color32 = Color32::from_rgb(44, 44, 48);
pub const BORDER: Color32 = Color32::from_rgb(48, 48, 52);
pub const TEXT: Color32 = Color32::from_rgb(236, 236, 240);
pub const MUTED: Color32 = Color32::from_rgb(150, 150, 158);

pub const RADIUS: u8 = 10;
/// Corner radius of the smaller shapes inside a panel: buttons and key caps.
pub const SMALL_RADIUS: u8 = 6;
pub const TOGGLE_SIZE: egui::Vec2 = egui::Vec2::new(40.0, 22.0);
/// How far the knob of a switch stays from the edge of its track.
const TOGGLE_KNOB_INSET: f32 = 3.0;

/// A panel in the app's colors: the result card and the usage hint.
pub fn card(margin: Margin) -> Frame {
    Frame::new()
        .fill(BACKGROUND)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(margin)
        .shadow(Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(110),
        })
}

/// An area set into the background of a window, such as the hotkey section of the settings.
pub fn section(margin: Margin) -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(margin)
}

/// Gives a window's own widgets, such as buttons, the colors of the rest of the app.
pub fn apply(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();

    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = BACKGROUND;
    visuals.extreme_bg_color = SURFACE;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, TEXT);

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = SURFACE;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);

    for widget in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.bg_fill = RAISED;
        widget.weak_bg_fill = RAISED;
        widget.bg_stroke = Stroke::new(1.0, BORDER);
        widget.fg_stroke = Stroke::new(1.0, TEXT);
        widget.corner_radius = CornerRadius::same(SMALL_RADIUS);
        widget.expansion = 0.0;
    }

    widgets.hovered.bg_fill = RAISED.gamma_multiply(1.3);
    widgets.hovered.weak_bg_fill = RAISED.gamma_multiply(1.3);
    widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER.gamma_multiply(1.4));
    widgets.active.bg_fill = ACCENT;
    widgets.active.weak_bg_fill = ACCENT;
    widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);

    ctx.set_visuals(visuals);
}

/// A switch for a setting that is either on or off. egui's checkbox reads as one item of a list;
/// a switch reads as something that stays the way it is put.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(TOGGLE_SIZE, Sense::click());

    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }

    let radius = rect.height() / 2.0;
    let (track, knob) = if *on { (ACCENT, Color32::WHITE) } else { (RAISED, MUTED) };
    let border = if response.hovered() {
        TEXT.gamma_multiply(0.3)
    } else {
        BORDER
    };

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(radius as u8), track);
    painter.rect_stroke(
        rect,
        CornerRadius::same(radius as u8),
        Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );

    let travelled = if *on { rect.width() - rect.height() } else { 0.0 };
    let center = pos2(rect.left() + radius + travelled, rect.center().y);
    painter.circle_filled(center, radius - TOGGLE_KNOB_INSET, knob);

    response
}

/// Draws one key of a shortcut as a key cap.
pub fn key_cap(ui: &mut egui::Ui, key: &str, size: f32) {
    Frame::new()
        .fill(RAISED)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(SMALL_RADIUS))
        .inner_margin(Margin::symmetric(9, 5))
        .show(ui, |ui| {
            ui.label(RichText::new(key).size(size).color(TEXT));
        });
}
