use std::ffi::c_void;
use std::sync::mpsc::{Receiver, TryRecvError};

use anyhow::{Result, bail};
use objc2::rc::{Retained, autoreleasepool};
use objc2::{ClassType as _, MainThreadMarker, Message as _, msg_send};
use objc2_app_kit::{
    NSApplication, NSColor, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSPopUpMenuWindowLevel, NSView,
    NSWindow, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
};
use objc2_core_graphics::{CGEvent, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint};
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::EventLoopBuilder;
use winit::monitor::MonitorHandle;
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS as _, WindowAttributesExtMacOS as _};
use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use winit::window::{Window, WindowAttributes};

use crate::capture::Screenshot;

pub use super::unix::{attach_parent_console, single_instance};

pub const SUPER_KEY: &str = "Cmd";
pub const ALT_KEY: &str = "Option";
pub const COMMAND_KEY: &str = "Cmd";

/// AppKit works in points and scales every window to its screen by itself.
pub fn init_dpi_awareness() {}

/// Makes the app a menu bar app: no Dock icon and no menu bar of its own. It does not take focus
/// from the app the user is in when it starts, only when it shows a window.
pub fn configure_event_loop(builder: &mut EventLoopBuilder<()>) {
    builder
        .with_activation_policy(ActivationPolicy::Accessory)
        .with_default_menu(false)
        .with_activate_ignoring_other_apps(false);
}

/// Returns pages the allocator holds on to back to the system.
pub fn trim_working_set() {
    unsafe extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
    }

    // SAFETY: a null zone stands for every zone, and a zero goal for as much as can be freed.
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
}

/// Directory for per-user settings (`~/Library/Application Support`).
pub fn config_dir() -> Option<std::path::PathBuf> {
    Some(super::unix::home_dir()?.join("Library").join("Application Support"))
}

/// Cursor position in desktop coordinates: points, from the top left corner of the main display.
pub fn cursor_position() -> Option<(i32, i32)> {
    let event = CGEvent::new(None)?;
    let point = CGEvent::location(Some(&event));

    Some((point.x.floor() as i32, point.y.floor() as i32))
}

/// Whether the monitor contains a point in desktop coordinates. winit reports monitors in
/// physical pixels, scaled from the points the system places them in.
pub fn monitor_contains(monitor: &MonitorHandle, (x, y): (i32, i32)) -> bool {
    let scale = monitor.scale_factor();
    let origin: LogicalPosition<f64> = monitor.position().to_logical(scale);
    let size: LogicalSize<f64> = monitor.size().to_logical(scale);
    let (x, y) = (f64::from(x), f64::from(y));

    (origin.x..origin.x + size.width).contains(&x) && (origin.y..origin.y + size.height).contains(&y)
}

/// Checks that the user allowed the app to record the screen. Without that, macOS hands out
/// screenshots with the desktop background and nothing else.
///
/// The first time, this asks the system to show its prompt, which also lists the app in System
/// Settings. macOS applies the permission to processes started after it was given.
pub fn screen_capture_access() -> Result<()> {
    if CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() {
        return Ok(());
    }

    bail!(
        "peekr is not allowed to record the screen; allow it in System Settings > Privacy & Security > \
         Screen & System Audio Recording, then restart peekr"
    )
}

/// Makes the window cover the monitor the screenshot was taken from: a borderless window of the
/// monitor's size, which [`float_over_screen`] raises above the menu bar and the Dock. Native
/// fullscreen would move the window to a Space of its own, sliding the whole screen over.
///
/// Desktop coordinates are winit's logical coordinates on macOS, so the window is placed without
/// looking for the monitor among winit's.
pub fn cover_monitor(
    attributes: WindowAttributes,
    _: &[MonitorHandle],
    _: usize,
    _: usize,
    shot: &Screenshot,
) -> WindowAttributes {
    attributes
        .with_position(LogicalPosition::new(shot.origin.0, shot.origin.1))
        .with_inner_size(LogicalSize::new(shot.size.0, shot.size.1))
        .with_has_shadow(false)
}

/// Raises a window that covers a monitor above the menu bar and the Dock, on every Space,
/// including the Space of an app in fullscreen.
pub fn float_over_screen(window: &Window) {
    let Some(ns_window) = ns_window(window) else {
        return;
    };

    ns_window.setLevel(NSPopUpMenuWindowLevel);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
}

/// Turns off the zoom animation of new windows, and brings the app back from hiding (see
/// [`return_focus`]), as a hidden app's windows stay hidden.
pub fn prepare_window(window: &Window) {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).unhideWithoutActivation();
    }

    if let Some(ns_window) = ns_window(window) {
        ns_window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    }
}

/// Hands focus back to the app the user was in. An app whose windows closed otherwise stays
/// active, and keys the user presses next, such as the paste shortcut, go nowhere.
pub fn return_focus() {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).hide(None);
    }
}

/// Rounds the corners of a window that draws its own frame by `radius` physical pixels: the
/// window is made transparent, and its content clipped to a rounded rectangle.
pub fn use_own_frame(window: &Window, radius: u32) {
    let Some(view) = ns_view(window) else {
        return;
    };

    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(f64::from(radius) / window.scale_factor());
        layer.setMasksToBounds(true);
    }

    if let Some(ns_window) = view.window() {
        ns_window.setOpaque(false);
        ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
        // The shadow follows the shape of what the window draws, which just changed.
        ns_window.invalidateShadow();
    }
}

/// The AppKit view behind a winit window.
fn ns_view(window: &Window) -> Option<Retained<NSView>> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };

    // SAFETY: an AppKit window handle points to the window's content view, which lives as long as
    // the window does.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };

    Some(view.retain())
}

fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
    ns_view(window)?.window()
}

/// Blocks the main thread until the app has an event to handle, dispatching the AppKit events
/// that drive the menu bar icon and the global hotkey meanwhile. Their callbacks run from here.
///
/// winit has launched the app by then (see `window::init`), and runs it itself while a window
/// is open.
pub struct EventLoop {
    mtm: MainThreadMarker,
}

impl EventLoop {
    /// Must be created on the main thread, the one AppKit delivers events to.
    pub fn new() -> Self {
        let mtm = MainThreadMarker::new().expect("the event loop runs on the main thread");

        Self { mtm }
    }

    #[expect(clippy::unused_self, reason = "same API as the other platforms' event loops")]
    pub fn waker(&self) -> Waker {
        Waker
    }

    /// Returns the next event, or `None` when the channel is closed.
    pub fn next<T>(&self, events: &Receiver<T>) -> Option<T> {
        let app = NSApplication::sharedApplication(self.mtm);

        loop {
            match events.try_recv() {
                Ok(event) => return Some(event),
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => {}
            }

            autoreleasepool(|_| {
                // SAFETY: the mode is a constant string AppKit defines.
                let mode = unsafe { NSDefaultRunLoopMode };
                let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&NSDate::distantFuture()),
                    mode,
                    true,
                );

                if let Some(event) = event {
                    app.sendEvent(&event);
                }
            });
        }
    }
}

/// Wakes up [`EventLoop::next`] after an event was sent, by posting an empty AppKit event.
#[derive(Debug, Clone, Copy)]
pub struct Waker;

impl Waker {
    #[expect(clippy::unused_self, reason = "same API as the other platforms' wakers")]
    pub fn wake(self) {
        autoreleasepool(|_| {
            let event =
                NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                    NSEventType::ApplicationDefined,
                    NSPoint::new(0.0, 0.0),
                    NSEventModifierFlags(0),
                    0.0,
                    0,
                    None,
                    0,
                    0,
                    0,
                );
            let Some(event) = event else {
                return;
            };

            // SAFETY: `sharedApplication` returns the application object winit created on the main
            // thread, and `postEvent:atStart:` is one of the few AppKit methods that may be called
            // from any thread; tray and hotkey callbacks run on the main thread anyway.
            let app: Retained<NSApplication> = unsafe { msg_send![NSApplication::class(), sharedApplication] };
            app.postEvent_atStart(&event, false);
        });
    }
}
