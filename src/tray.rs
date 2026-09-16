use anyhow::Result;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const APP_NAME: &str = "ochco";

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
            .with_tooltip(format!("{APP_NAME} — {hotkey_label}"))
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
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
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
        if let Err(e) = self.icon.set_tooltip(Some(format!("{APP_NAME} — {status}"))) {
            log::warn!("failed to update tray tooltip: {e}");
        }
    }
}

/// A 32x32 rounded square with three "text lines", drawn in code to avoid shipping assets.
fn app_icon() -> Icon {
    const SIZE: u32 = 32;
    const BACKGROUND: [u8; 3] = [40, 120, 230];
    const FOREGROUND: [u8; 3] = [255, 255, 255];
    /// Text lines as (first row, end column); each is 3 px tall and starts at column 8.
    const LINES: [(u32, u32); 3] = [(9, 23), (15, 23), (21, 17)];

    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];

    for y in 0..SIZE {
        for x in 0..SIZE {
            // Signed distance from the pixel center to a rounded square with corner radius 7.
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (qx, qy) = ((fx - 16.0).abs() - 9.0, (fy - 16.0).abs() - 9.0);
            let distance = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - 7.0;

            let alpha = (0.5 - distance).clamp(0.0, 1.0);
            if alpha == 0.0 {
                continue;
            }

            let on_line = LINES
                .iter()
                .any(|&(row, end)| (row..row + 3).contains(&y) && (8..end).contains(&x));
            let [r, g, b] = if on_line { FOREGROUND } else { BACKGROUND };

            let i = ((y * SIZE + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[r, g, b, (alpha * 255.0) as u8]);
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).expect("icon buffer matches its dimensions")
}
