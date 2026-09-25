//! Native windows with egui content, drawn on the CPU.
//!
//! Windows presented through the GPU make the whole screen blink when they open and close
//! on some setups, so the overlay and the settings window render egui into a pixel buffer and
//! show it the way plain desktop apps do (GDI on Windows, shared memory on X11 and Wayland, a
//! Core Animation layer on macOS).

#[cfg(not(test))]
mod raster;
#[cfg(test)]
pub mod raster;

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use egui::{ViewportCommand, ViewportId, ViewportInfo};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::run_on_demand::EventLoopExtRunOnDemand as _;
use winit::window::{Window, WindowAttributes, WindowId};

use self::raster::{Frame, Renderer};
use crate::platform;

/// Content of one or more windows that share state.
pub trait App {
    /// Draws the window with the given index (in the order the windows were described).
    fn ui(&mut self, window: usize, ui: &mut egui::Ui);

    /// Whether the window has to be drawn again because another window changed shared state.
    /// Asked after every event, so it should also reset the flag.
    fn take_repaint(&mut self, _window: usize) -> bool {
        false
    }

    /// How far to round the window's own corners, in points; zero leaves it rectangular.
    /// Only a window that draws its own frame needs this, as the system shapes the others.
    fn corner_radius(&self, _window: usize) -> f32 {
        0.0
    }

    /// Whether the window covers a whole monitor, and has to be above everything on it.
    fn covers_screen(&self, _window: usize) -> bool {
        false
    }
}

thread_local! {
    /// winit allows one event loop per process, so it is created once and reused for every window.
    static EVENT_LOOP: RefCell<Option<EventLoop<()>>> = const { RefCell::new(None) };
}

/// Starts the window system ahead of the first window.
///
/// On macOS the app finishes launching the first time winit's event loop runs, and the menu bar
/// icon and the global hotkey need a launched app; elsewhere this only connects to the display
/// early. A window system that cannot start now is tried again when a window opens.
pub fn init() {
    /// Leaves the event loop as soon as it has started.
    struct Launch;

    impl ApplicationHandler for Launch {
        fn new_events(&mut self, event_loop: &ActiveEventLoop, _: StartCause) {
            event_loop.exit();
        }

        fn resumed(&mut self, _: &ActiveEventLoop) {}

        fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
    }

    let launched = take_event_loop().and_then(|mut event_loop| {
        let result = event_loop.run_app_on_demand(&mut Launch);
        EVENT_LOOP.set(Some(event_loop));

        result.context("starting the window event loop")
    });

    if let Err(e) = launched {
        log::warn!("{e:#}");
    }
}

/// The event loop created before, or a new one.
fn take_event_loop() -> Result<EventLoop<()>> {
    if let Some(event_loop) = EVENT_LOOP.take() {
        return Ok(event_loop);
    }

    let mut builder = EventLoop::builder();
    platform::configure_event_loop(&mut builder);

    builder.build().context("creating the window event loop")
}

/// Opens windows and runs `app` in them until one of them closes, which closes them all.
///
/// `attributes` describes the windows once the event loop is running (monitors are only known
/// then); `create` builds the app with each window's egui context, in the same order.
pub fn run<A: App>(
    attributes: impl FnOnce(&ActiveEventLoop) -> Vec<WindowAttributes>,
    create: impl FnOnce(&[egui::Context]) -> A,
) -> Result<()> {
    let mut event_loop = take_event_loop()?;

    let mut runner = Runner {
        attributes: Some(attributes),
        create: Some(create),
        app: None,
        windows: Vec::new(),
        closing: false,
        error: None,
    };
    let result = event_loop.run_app_on_demand(&mut runner);

    // Destroy the windows before the next ones can be opened.
    runner.windows.clear();
    EVENT_LOOP.set(Some(event_loop));
    platform::return_focus();

    result.context("window event loop failed")?;
    runner.error.map_or(Ok(()), Err)
}

/// Window attributes that center a window of `size` (in points) on the monitor under the mouse
/// cursor, or on the primary monitor when the cursor position is unknown.
pub fn centered(event_loop: &ActiveEventLoop, size: [f32; 2]) -> WindowAttributes {
    let attributes = Window::default_attributes().with_inner_size(LogicalSize::new(size[0], size[1]));

    let under_cursor = platform::cursor_position().and_then(|cursor| {
        event_loop
            .available_monitors()
            .find(|m| platform::monitor_contains(m, cursor))
    });

    let Some(monitor) = under_cursor.or_else(|| event_loop.primary_monitor()) else {
        return attributes;
    };

    let scale = monitor.scale_factor();
    let (origin, area) = (monitor.position(), monitor.size());
    let offset = |area: u32, size: f32| ((f64::from(area) - f64::from(size) * scale) / 2.0) as i32;

    attributes.with_position(PhysicalPosition::new(
        origin.x + offset(area.width, size[0]),
        origin.y + offset(area.height, size[1]),
    ))
}

struct Runner<A, Attributes, Create> {
    attributes: Option<Attributes>,
    create: Option<Create>,
    app: Option<A>,
    windows: Vec<AppWindow>,
    /// The windows were dropped; the loop exits on its next iteration.
    closing: bool,
    error: Option<anyhow::Error>,
}

struct AppWindow {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    ctx: egui::Context,
    input: egui_winit::State,
    viewport: ViewportInfo,
    renderer: Renderer,
    /// The frame as last drawn. softbuffer does not promise that a presented buffer keeps its
    /// contents, while the renderer only redraws what changed, so the frame is kept here.
    canvas: Vec<u32>,
    /// Where the buffer presented last time was; softbuffer hands out the same memory again as
    /// long as it is not double buffering, and then only the changed rows have to be copied.
    presented: usize,
    repaint_at: Option<Instant>,
    shown: bool,
    /// Corner radius for a window that draws its own frame, in physical pixels; zero for the rest.
    /// Applied when the window is first shown, as winit rewrites its styles until then.
    frame_radius: u32,
}

impl<A, Attributes, Create> ApplicationHandler for Runner<A, Attributes, Create>
where
    A: App,
    Attributes: FnOnce(&ActiveEventLoop) -> Vec<WindowAttributes>,
    Create: FnOnce(&[egui::Context]) -> A,
{
    fn new_events(&mut self, event_loop: &ActiveEventLoop, _: StartCause) {
        // Exiting in the iteration that dropped the windows would leave them on screen: winit
        // destroys dropped Wayland windows only in its next iteration.
        if self.closing {
            event_loop.exit();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(attributes), Some(create)) = (self.attributes.take(), self.create.take()) else {
            return;
        };

        let windows: Result<Vec<_>> = attributes(event_loop)
            .into_iter()
            .map(|attributes| AppWindow::open(event_loop, attributes))
            .collect();

        let mut windows = match windows {
            Ok(windows) => windows,
            Err(e) => return self.fail(e),
        };

        let contexts: Vec<_> = windows.iter().map(|w| w.ctx.clone()).collect();
        let mut app = create(&contexts);

        for (index, window) in windows.iter_mut().enumerate() {
            let radius = app.corner_radius(index) * window.window.scale_factor() as f32;
            window.frame_radius = radius.round() as u32;

            if app.covers_screen(index) {
                platform::float_over_screen(&window.window);
            }
        }

        let redrawn = windows
            .iter_mut()
            .enumerate()
            .try_fold(false, |close, (index, window)| {
                Ok::<_, anyhow::Error>(close || window.redraw(&mut app, index)?)
            });

        self.app = Some(app);
        self.windows = windows;

        match redrawn {
            Ok(false) => {}
            Ok(true) => self.close(),
            Err(e) => self.fail(e),
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let (Some(app), Some(index)) = (&mut self.app, self.windows.iter().position(|w| w.window.id() == id)) else {
            return;
        };
        let window = &mut self.windows[index];

        let redrawn = match event {
            WindowEvent::CloseRequested => Ok(true),
            WindowEvent::RedrawRequested => window.redraw(app, index),
            event => {
                if window.input.on_window_event(&window.window, &event).repaint {
                    window.window.request_redraw();
                }
                Ok(false)
            }
        };

        match redrawn {
            Ok(false) => {}
            Ok(true) => self.close(),
            Err(e) => self.fail(e),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.closing {
            // Run the next iteration right away rather than wait for input.
            event_loop.set_control_flow(ControlFlow::Poll);
            return;
        }

        let now = Instant::now();
        let mut wake_at = None;

        for (index, window) in self.windows.iter_mut().enumerate() {
            let requested = self.app.as_mut().is_some_and(|app| app.take_repaint(index));

            match window.repaint_at {
                _ if requested => window.window.request_redraw(),
                Some(at) if at <= now => {
                    window.repaint_at = None;
                    window.window.request_redraw();
                }
                Some(at) => wake_at = Some(wake_at.map_or(at, |earliest: Instant| earliest.min(at))),
                None => {}
            }
        }

        event_loop.set_control_flow(wake_at.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

impl<A, Attributes, Create> Runner<A, Attributes, Create> {
    fn close(&mut self) {
        for window in self.windows.drain(..) {
            window.window.set_visible(false);
        }

        self.closing = true;
    }

    fn fail(&mut self, error: anyhow::Error) {
        self.error = Some(error);
        self.close();
    }
}

impl AppWindow {
    fn open(event_loop: &ActiveEventLoop, attributes: WindowAttributes) -> Result<Self> {
        // Hidden until the first frame is drawn, so it never shows up blank.
        let window = event_loop
            .create_window(attributes.with_visible(false))
            .context("creating a window")?;
        let window = Rc::new(window);
        platform::prepare_window(&window);

        let context = softbuffer::Context::new(Rc::clone(&window)).map_err(|e| anyhow!("{e}"))?;
        let surface = softbuffer::Surface::new(&context, Rc::clone(&window)).map_err(|e| anyhow!("{e}"))?;

        let ctx = egui::Context::default();
        let input = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );

        Ok(Self {
            window,
            surface,
            ctx,
            input,
            viewport: ViewportInfo::default(),
            renderer: Renderer::default(),
            canvas: Vec::new(),
            presented: 0,
            repaint_at: None,
            shown: false,
            frame_radius: 0,
        })
    }

    /// Runs the app for one frame of this window and presents it. Returns whether the windows
    /// should close.
    fn redraw(&mut self, app: &mut impl App, index: usize) -> Result<bool> {
        let ctx = self.ctx.clone();
        egui_winit::update_viewport_info(&mut self.viewport, &ctx, &self.window, !self.shown);
        let mut input = self.input.take_egui_input(&self.window);
        input.viewports.insert(ViewportId::ROOT, self.viewport.clone());

        let mut output = ctx.run_ui(input, |ui| app.ui(index, ui));
        self.input.handle_platform_output(&self.window, output.platform_output);

        let (close, drag, repaint_delay) =
            output
                .viewport_output
                .get(&ViewportId::ROOT)
                .map_or((false, false, Duration::MAX), |viewport| {
                    let sent = |wanted: &ViewportCommand| {
                        viewport
                            .commands
                            .iter()
                            .any(|command| std::mem::discriminant(command) == std::mem::discriminant(wanted))
                    };

                    (
                        sent(&ViewportCommand::Close),
                        sent(&ViewportCommand::StartDrag),
                        viewport.repaint_delay,
                    )
                });

        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let size = self.window.inner_size();

        if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            let (width, height) = (width.get() as usize, height.get() as usize);
            self.canvas.resize(width * height, 0);

            let mut frame = Frame {
                pixels: &mut self.canvas,
                width,
                height,
            };
            let damage = self.renderer.render(
                &mut frame,
                &primitives,
                &mut output.textures_delta,
                output.pixels_per_point,
            );

            let (columns, rows) = (width as u32, height as u32);
            self.surface
                .resize(columns.try_into()?, rows.try_into()?)
                .map_err(|e| anyhow!("{e}"))?;
            let mut buffer = self.surface.buffer_mut().map_err(|e| anyhow!("{e}"))?;
            let same_buffer =
                std::mem::replace(&mut self.presented, buffer.as_ptr() as usize) == buffer.as_ptr() as usize;

            match damage.filter(|_| same_buffer) {
                Some(damage) => {
                    for row in damage.rows {
                        let at = row * width + damage.columns.start;
                        let end = row * width + damage.columns.end;

                        buffer[at..end].copy_from_slice(&self.canvas[at..end]);
                    }
                }
                None => buffer.copy_from_slice(&self.canvas),
            }

            if !self.shown {
                self.window.set_visible(true);
                self.window.focus_window();
                self.shown = true;

                // Only now: showing the window is the last thing winit rewrites its styles for.
                if self.frame_radius > 0 {
                    platform::use_own_frame(&self.window, self.frame_radius);
                }
            }

            buffer.present().map_err(|e| anyhow!("{e}"))?;
        } else {
            // Minimized: nothing to draw, but texture changes must not be lost.
            let mut frame = Frame {
                pixels: &mut [],
                width: 0,
                height: 0,
            };
            self.renderer
                .render(&mut frame, &[], &mut output.textures_delta, output.pixels_per_point);
            self.presented = 0;
        }

        self.repaint_at = match repaint_delay {
            Duration::ZERO => {
                self.window.request_redraw();
                None
            }
            delay => Instant::now().checked_add(delay),
        };

        // Last, because the system takes the thread over until the user lets the window go.
        if drag {
            let _ = self.window.drag_window();
        }

        Ok(close)
    }
}
