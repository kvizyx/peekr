//! Native windows with egui content, drawn on the CPU.
//!
//! Windows presented through the GPU make the whole screen blink when they open and close
//! on some setups, so the overlay and the settings window render egui into a pixel buffer and
//! show it the way plain desktop apps do (GDI on Windows, shared memory on X11 and Wayland).

mod raster;

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use egui::{ViewportCommand, ViewportId, ViewportInfo};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
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
}

thread_local! {
    /// winit allows one event loop per process, so it is created once and reused for every window.
    static EVENT_LOOP: RefCell<Option<EventLoop<()>>> = const { RefCell::new(None) };
}

/// Opens windows and runs `app` in them until one of them closes, which closes them all.
///
/// `attributes` describes the windows once the event loop is running (monitors are only known
/// then); `create` builds the app with each window's egui context, in the same order.
pub fn run<A: App>(
    attributes: impl FnOnce(&ActiveEventLoop) -> Vec<WindowAttributes>,
    create: impl FnOnce(&[egui::Context]) -> A,
) -> Result<()> {
    let mut event_loop = match EVENT_LOOP.take() {
        Some(event_loop) => event_loop,
        None => EventLoop::new().context("creating the window event loop")?,
    };

    let mut runner = Runner {
        attributes: Some(attributes),
        create: Some(create),
        app: None,
        windows: Vec::new(),
        error: None,
    };
    let result = event_loop.run_app_on_demand(&mut runner);

    // Destroy the windows before the next ones can be opened.
    runner.windows.clear();
    EVENT_LOOP.set(Some(event_loop));

    result.context("window event loop failed")?;
    runner.error.map_or(Ok(()), Err)
}

/// Window attributes that center a window of `size` (in points) on the monitor under the mouse
/// cursor, or on the primary monitor when the cursor position is unknown.
pub fn centered(event_loop: &ActiveEventLoop, size: [f32; 2]) -> WindowAttributes {
    let attributes = Window::default_attributes().with_inner_size(LogicalSize::new(size[0], size[1]));

    let under_cursor = platform::cursor_position().and_then(|(x, y)| {
        event_loop.available_monitors().find(|m| {
            let (origin, area) = (m.position(), m.size());
            (origin.x..origin.x + area.width as i32).contains(&x)
                && (origin.y..origin.y + area.height as i32).contains(&y)
        })
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
    error: Option<anyhow::Error>,
}

struct AppWindow {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    ctx: egui::Context,
    input: egui_winit::State,
    viewport: ViewportInfo,
    renderer: Renderer,
    repaint_at: Option<Instant>,
    shown: bool,
}

impl<A, Attributes, Create> ApplicationHandler for Runner<A, Attributes, Create>
where
    A: App,
    Attributes: FnOnce(&ActiveEventLoop) -> Vec<WindowAttributes>,
    Create: FnOnce(&[egui::Context]) -> A,
{
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
            Err(e) => return self.fail(event_loop, e),
        };

        let contexts: Vec<_> = windows.iter().map(|w| w.ctx.clone()).collect();
        let mut app = create(&contexts);

        for (index, window) in windows.iter_mut().enumerate() {
            match window.redraw(&mut app, index) {
                Ok(false) => {}
                Ok(true) => return event_loop.exit(),
                Err(e) => return self.fail(event_loop, e),
            }
        }

        self.app = Some(app);
        self.windows = windows;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
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
            Ok(true) => self.close(event_loop),
            Err(e) => self.fail(event_loop, e),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
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
    fn close(&mut self, event_loop: &ActiveEventLoop) {
        for window in self.windows.drain(..) {
            window.window.set_visible(false);
        }

        event_loop.exit();
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        self.close(event_loop);
    }
}

impl AppWindow {
    fn open(event_loop: &ActiveEventLoop, attributes: WindowAttributes) -> Result<Self> {
        // Hidden until the first frame is drawn, so it never shows up blank.
        let window = event_loop
            .create_window(attributes.with_visible(false))
            .context("creating a window")?;
        let window = Rc::new(window);
        platform::disable_window_animations(&window);

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
            repaint_at: None,
            shown: false,
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

        let (close, repaint_delay) =
            output
                .viewport_output
                .get(&ViewportId::ROOT)
                .map_or((false, Duration::MAX), |viewport| {
                    let close = viewport.commands.iter().any(|c| matches!(c, ViewportCommand::Close));
                    (close, viewport.repaint_delay)
                });

        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let size = self.window.inner_size();

        if let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            self.surface.resize(width, height).map_err(|e| anyhow!("{e}"))?;
            let mut buffer = self.surface.buffer_mut().map_err(|e| anyhow!("{e}"))?;

            let mut frame = Frame {
                pixels: &mut buffer,
                width: width.get() as usize,
                height: height.get() as usize,
            };
            self.renderer.render(
                &mut frame,
                &primitives,
                &mut output.textures_delta,
                output.pixels_per_point,
            );

            if !self.shown {
                self.window.set_visible(true);
                self.window.focus_window();
                self.shown = true;
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
        }

        self.repaint_at = match repaint_delay {
            Duration::ZERO => {
                self.window.request_redraw();
                None
            }
            delay => Instant::now().checked_add(delay),
        };

        Ok(close)
    }
}
