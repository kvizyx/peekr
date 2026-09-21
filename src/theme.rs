//! Colors and shapes shared by the overlay and the settings window.

use egui::{Button, Color32, CornerRadius, Frame, Margin, RichText, Shadow, Stroke, Visuals, vec2};

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
pub const BUTTON_HEIGHT: f32 = 32.0;
/// Space between a button's label and its edges.
pub const BUTTON_PADDING: egui::Vec2 = egui::Vec2::new(14.0, 6.0);

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

/// The button for the main action of a window, such as copying the recognized text.
pub fn accent_button(label: &str) -> Button<'static> {
    Button::new(RichText::new(label.to_owned()).color(Color32::WHITE).strong())
        .fill(ACCENT)
        .corner_radius(CornerRadius::same(SMALL_RADIUS))
        .min_size(vec2(0.0, BUTTON_HEIGHT))
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
