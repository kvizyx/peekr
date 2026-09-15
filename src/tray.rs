use anyhow::Result;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum TrayAction {
    Capture,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    capture: MenuItem,
    quit: MenuItem,
}

impl Tray {
    pub fn new(hotkey_label: &str) -> Result<Self> {
        let capture = MenuItem::new(format!("Capture text\t{hotkey_label}"), true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&capture, &PredefinedMenuItem::separator(), &quit])?;

        let icon = TrayIconBuilder::new()
            .with_tooltip(format!("ochco — {hotkey_label}"))
            .with_icon(app_icon())
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;
        Ok(Self { icon, capture, quit })
    }

    /// Drains pending tray and menu events.
    pub fn poll(&self) -> Option<TrayAction> {
        let mut action = None;
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                action = Some(TrayAction::Capture);
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == *self.capture.id() {
                action = Some(TrayAction::Capture);
            } else if event.id == *self.quit.id() {
                return Some(TrayAction::Quit);
            }
        }
        action
    }

    pub fn set_status(&self, status: &str) {
        if let Err(e) = self.icon.set_tooltip(Some(format!("ochco — {status}"))) {
            log::warn!("failed to update tray tooltip: {e}");
        }
    }
}

/// A 32x32 rounded square with three "text lines", drawn in code to avoid shipping assets.
fn app_icon() -> Icon {
    const S: u32 = 32;
    let mut rgba = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Signed distance to a rounded square with radius 7.
            let (qx, qy) = ((fx - 16.0).abs() - 9.0, (fy - 16.0).abs() - 9.0);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - 7.0;
            let alpha = (0.5 - outside).clamp(0.0, 1.0);
            if alpha == 0.0 {
                continue;
            }
            let line = [(9, 23), (15, 23), (21, 17)]
                .iter()
                .any(|&(row, end)| (row..row + 3).contains(&y) && (8..end).contains(&x));
            let color = if line { [255, 255, 255] } else { [40, 120, 230] };
            let i = ((y * S + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[color[0], color[1], color[2], (alpha * 255.0) as u8]);
        }
    }
    Icon::from_rgba(rgba, S, S).expect("valid icon dimensions")
}
