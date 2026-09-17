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

pub trait App {
    fn ui(&mut self, ui: &mut egui::Ui);
}

thread_local! {
    /// winit allows one event loop per process, so it is created once and reused for every window.
    static EVENT_LOOP: RefCell<Option<EventLoop<()>>> = const { RefCell::new(None) };
}

/// Opens a window and runs `app` in it until the window closes.
///
/// `attributes` describes the window once the event loop is running (monitors are only known
/// then); `create` builds the app with the window's egui context.
pub fn run<A: App>(
    attributes: impl FnOnce(&ActiveEventLoop) -> WindowAttributes,
    create: impl FnOnce(&egui::Context) -> A,
) -> Result<()> {
    let mut event_loop = match EVENT_LOOP.take() {
        Some(event_loop) => event_loop,
        None => EventLoop::new().context("creating the window event loop")?,
    };

    let mut runner = Runner {
        attributes: Some(attributes),
        create: Some(create),
        ctx: egui::Context::default(),
        window: None,
        error: None,
    };
    let result = event_loop.run_app_on_demand(&mut runner);

    // Destroy the window before the next one can be opened.
    runner.window = None;
    EVENT_LOOP.set(Some(event_loop));

    result.context("window event loop failed")?;
    runner.error.map_or(Ok(()), Err)
}

/// Window attributes that center a window of `size` (in points) on the primary monitor.
pub fn centered(event_loop: &ActiveEventLoop, size: [f32; 2]) -> WindowAttributes {
    let attributes = Window::default_attributes().with_inner_size(LogicalSize::new(size[0], size[1]));

    let Some(monitor) = event_loop.primary_monitor() else {
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
    ctx: egui::Context,
    window: Option<AppWindow<A>>,
    error: Option<anyhow::Error>,
}

struct AppWindow<A> {
    app: A,
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    input: egui_winit::State,
    viewport: ViewportInfo,
    renderer: Renderer,
    repaint_at: Option<Instant>,
    shown: bool,
}

impl<A, Attributes, Create> ApplicationHandler for Runner<A, Attributes, Create>
where
    A: App,
    Attributes: FnOnce(&ActiveEventLoop) -> WindowAttributes,
    Create: FnOnce(&egui::Context) -> A,
{
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(attributes), Some(create)) = (self.attributes.take(), self.create.take()) else {
            return;
        };

        let result = AppWindow::open(event_loop, &self.ctx, attributes(event_loop), create)
            .and_then(|mut window| window.redraw(&self.ctx).map(|close| (window, close)));

        match result {
            Ok((window, false)) => self.window = Some(window),
            Ok((_, true)) => event_loop.exit(),
            Err(e) => self.fail(event_loop, e),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(window) = &mut self.window else {
            return;
        };

        let redrawn = match event {
            WindowEvent::CloseRequested => Ok(true),
            WindowEvent::RedrawRequested => window.redraw(&self.ctx),
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
        let Some(window) = &mut self.window else {
            return;
        };

        match window.repaint_at {
            Some(at) if at <= Instant::now() => {
                window.repaint_at = None;
                window.window.request_redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

impl<A, Attributes, Create> Runner<A, Attributes, Create> {
    fn close(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(window) = self.window.take() {
            window.window.set_visible(false);
        }

        event_loop.exit();
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        self.close(event_loop);
    }
}

impl<A: App> AppWindow<A> {
    fn open(
        event_loop: &ActiveEventLoop,
        ctx: &egui::Context,
        attributes: WindowAttributes,
        create: impl FnOnce(&egui::Context) -> A,
    ) -> Result<Self> {
        // Hidden until the first frame is drawn, so it never shows up blank.
        let window = event_loop
            .create_window(attributes.with_visible(false))
            .context("creating a window")?;
        let window = Rc::new(window);
        platform::disable_window_animations(&window);

        let context = softbuffer::Context::new(Rc::clone(&window)).map_err(|e| anyhow!("{e}"))?;
        let surface = softbuffer::Surface::new(&context, Rc::clone(&window)).map_err(|e| anyhow!("{e}"))?;

        let input = egui_winit::State::new(
            ctx.clone(),
            ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );

        Ok(Self {
            app: create(ctx),
            window,
            surface,
            input,
            viewport: ViewportInfo::default(),
            renderer: Renderer::default(),
            repaint_at: None,
            shown: false,
        })
    }

    /// Runs the app for one frame and presents it. Returns whether the window should close.
    fn redraw(&mut self, ctx: &egui::Context) -> Result<bool> {
        egui_winit::update_viewport_info(&mut self.viewport, ctx, &self.window, !self.shown);
        let mut input = self.input.take_egui_input(&self.window);
        input.viewports.insert(ViewportId::ROOT, self.viewport.clone());

        let mut output = ctx.run_ui(input, |ui| self.app.ui(ui));
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
