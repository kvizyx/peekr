use std::sync::Arc;

use anyhow::Result;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::icon;
use crate::shortcut::Shortcut;

const APP_NAME: &str = "Ochco";

pub enum TrayAction {
    Capture,
    OpenSettings,
    Quit,
}

pub struct Tray {
    /// Removed from the tray when dropped.
    _icon: TrayIcon,
    capture: MenuItem,
}

impl Tray {
    /// Creates the tray icon; `on_action` is called from the thread that delivers tray events.
    pub fn new(hotkey: Option<Shortcut>, on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Self> {
        let capture = MenuItem::new(capture_label(hotkey), true, None);
        let settings = MenuItem::new("Settings", true, None);
        let quit = MenuItem::new("Quit", true, None);

        let menu = Menu::new();
        menu.append_items(&[&capture, &settings, &PredefinedMenuItem::separator(), &quit])?;

        let icon = Icon::from_rgba(icon::rgba(), icon::SIZE, icon::SIZE)?;
        let icon = TrayIconBuilder::new()
            .with_tooltip(APP_NAME)
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        let on_action = Arc::new(on_action);

        TrayIconEvent::set_event_handler(Some({
            let on_action = Arc::clone(&on_action);
            move |event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    on_action(TrayAction::Capture);
                }
            }
        }));

        let (capture_id, settings_id, quit_id) = (capture.id().clone(), settings.id().clone(), quit.id().clone());
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == capture_id {
                on_action(TrayAction::Capture);
            } else if event.id == settings_id {
                on_action(TrayAction::OpenSettings);
            } else if event.id == quit_id {
                on_action(TrayAction::Quit);
            }
        }));

        Ok(Self { _icon: icon, capture })
    }

    /// Shows the active hotkey, or `None` when no hotkey is registered.
    pub fn set_hotkey(&self, hotkey: Option<Shortcut>) {
        self.capture.set_text(capture_label(hotkey));
    }
}

fn capture_label(hotkey: Option<Shortcut>) -> String {
    match hotkey {
        Some(hotkey) => format!("Capture text\t{hotkey}"),
        None => "Capture text".to_owned(),
    }
}
