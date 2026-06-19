#[cfg(feature = "servo-engine")]
use surfman::declare_surfman;

#[cfg(feature = "servo-engine")]
declare_surfman!();
#[cfg(not(feature = "servo-engine"))]
fn main() {
    println!("Ordo Servo Shell");
    println!(
        "status: integrated shell target; Servo engine feature is disabled for default builds"
    );
    println!("build Studio assets: cd ordo-studio && npm run build");
    println!("launch window: cargo run --manifest-path ordo-servo-shell/Cargo.toml --features servo-engine -- --target ordo-studio/dist/index.html");
}

#[cfg(feature = "servo-engine")]
fn main() -> anyhow::Result<()> {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(servo_engine::run)) {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown Servo renderer panic");
            anyhow::bail!(
                "Servo renderer failed before a frame could be read: {message}. On Windows this usually means the selected Servo graphics backend could not initialize."
            );
        }
    };
    std::panic::set_hook(previous_hook);
    result
}

#[cfg(feature = "servo-engine")]
mod servo_engine {
    use std::cell::{Cell, RefCell};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use anyhow::{bail, Context, Result};
    use clap::Parser;
    use dpi::PhysicalSize as DpiPhysicalSize;
    use euclid::{Box2D, Point2D, Scale};
    use image::RgbaImage;
    use servo::{
        InputEvent, LoadStatus, MouseButton, MouseButtonAction, MouseButtonEvent,
        MouseLeftViewportEvent, MouseMoveEvent, RenderingContext, Servo, ServoBuilder,
        SoftwareRenderingContext, WebView, WebViewBuilder, WebViewDelegate, WebViewPoint,
        WheelDelta, WheelEvent, WheelMode, WindowRenderingContext,
    };
    use winit::keyboard::Key;
    use winit::event::Modifiers;
    use tracing::warn;
    use url::Url;
    use webrender_api::units::DevicePoint;
    use winit::application::ApplicationHandler;
    use winit::dpi::PhysicalSize as WinitPhysicalSize;
    use winit::event::{
        ElementState, MouseButton as WinitMouseButton,
        MouseScrollDelta, WindowEvent,
    };
    use winit::event_loop::EventLoop;
    use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
    use winit::window::Window;

    #[derive(Parser, Debug, Clone)]
    #[command(author, version, about = "Launch Ordo Studio through Servo.")]
    struct Args {
        #[arg(long, default_value = "ordo-studio/dist/index.html")]
        target: String,

        #[arg(long, default_value_t = 1280)]
        width: u32,

        #[arg(long, default_value_t = 800)]
        height: u32,

        #[arg(long, default_value_t = 1.0)]
        dpr: f32,

        #[arg(
            long,
            help = "Run the offscreen PNG proof instead of opening a window."
        )]
        screenshot: bool,

        #[arg(long, default_value = "data/servo-render-proof.png")]
        out: String,

        #[arg(long, default_value_t = 60)]
        timeout_secs: u64,
    }

    pub fn run() -> Result<()> {
        let args = Args::parse();
        if args.screenshot {
            render_screenshot(args)
        } else {
            launch_window(args)
        }
    }

    fn launch_window(args: Args) -> Result<()> {
        let url = resolve_target(&args.target)?;
        let event_loop = EventLoop::with_user_event()
            .build()
            .context("creating Servo/winit event loop")?;
        let mut app = App::new(&event_loop, url, args.width, args.height);
        event_loop
            .run_app(&mut app)
            .context("running Servo window event loop")?;
        Ok(())
    }

    fn render_screenshot(args: Args) -> Result<()> {
        let url = resolve_target(&args.target)?;
        ensure_parent_dir(&args.out)?;

        let size = DpiPhysicalSize::new(args.width, args.height);
        let rendering_context: Rc<dyn RenderingContext> = Rc::new(
            SoftwareRenderingContext::new(size)
                .map_err(|err| anyhow::anyhow!("creating SoftwareRenderingContext: {err:?}"))?,
        );
        let _ = rendering_context.make_current();

        let servo: Servo = ServoBuilder::default().build();
        servo.setup_logging();

        let delegate = Rc::new(ProofDelegate::default());
        let webview: WebView = WebViewBuilder::new(&servo, rendering_context.clone())
            .url(url)
            .hidpi_scale_factor(Scale::new(args.dpr))
            .delegate(delegate.clone() as Rc<dyn WebViewDelegate>)
            .build();

        let deadline = Instant::now() + Duration::from_secs(args.timeout_secs);
        wait_until_loaded(&servo, &delegate, deadline, args.timeout_secs)?;
        nudge_post_load_frame(&servo, &webview, deadline)?;
        wait_for_post_load_frame(&servo, &delegate, deadline)?;

        let rect = Box2D::new(
            Point2D::new(0, 0),
            Point2D::new(args.width as i32, args.height as i32),
        );
        let image: RgbaImage = rendering_context
            .read_to_image(rect)
            .context("Servo read_to_image returned no pixels")?;
        image
            .save(&args.out)
            .with_context(|| format!("saving Servo render proof to {}", args.out))?;

        println!(
            "Ordo Servo Shell rendered {}x{} to {}",
            args.width, args.height, args.out
        );
        Ok(())
    }

    struct AppState {
        window: Window,
        servo: Servo,
        rendering_context: Rc<WindowRenderingContext>,
        webviews: RefCell<Vec<WebView>>,
        cursor_position: Cell<WebViewPoint>,
        modifiers: Cell<Modifiers>,
    }

    impl WebViewDelegate for AppState {
        fn notify_new_frame_ready(&self, _webview: WebView) {
            self.window.request_redraw();
        }

        fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
            if matches!(status, LoadStatus::Complete) {
                self.window.request_redraw();
            }
        }
    }

    enum App {
        Initial {
            waker: Waker,
            url: Url,
            width: u32,
            height: u32,
        },
        Running(Rc<AppState>),
    }

    impl App {
        fn new(event_loop: &EventLoop<WakerEvent>, url: Url, width: u32, height: u32) -> Self {
            Self::Initial {
                waker: Waker::new(event_loop),
                url,
                width,
                height,
            }
        }
    }

    impl ApplicationHandler<WakerEvent> for App {
        fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
            if let Self::Initial {
                waker,
                url,
                width,
                height,
            } = self
            {
                let display_handle = event_loop
                    .display_handle()
                    .expect("failed to get display handle");
                let window = event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title("Ordo")
                            .with_inner_size(WinitPhysicalSize::new(*width, *height)),
                    )
                    .expect("failed to create Ordo Servo window");
                let window_handle = window.window_handle().expect("failed to get window handle");

                let rendering_context = Rc::new(
                    WindowRenderingContext::new(display_handle, window_handle, window.inner_size())
                        .expect("could not create Servo WindowRenderingContext"),
                );
                let _ = rendering_context.make_current();

                let servo = ServoBuilder::default()
                    .event_loop_waker(Box::new(waker.clone()))
                    .build();
                servo.setup_logging();

                let cursor_position = Cell::new(DevicePoint::default().into());
                let modifiers = Cell::new(Modifiers::default());
                let app_state = Rc::new(AppState {
                    window,
                    servo,
                    rendering_context,
                    webviews: Default::default(),
                    cursor_position,
                    modifiers,
                });

                let webview =
                    WebViewBuilder::new(&app_state.servo, app_state.rendering_context.clone())
                        .url(url.clone())
                        .hidpi_scale_factor(Scale::new(app_state.window.scale_factor() as f32))
                        .delegate(app_state.clone() as Rc<dyn WebViewDelegate>)
                        .build();

                app_state.webviews.borrow_mut().push(webview);
                app_state.window.request_redraw();
                *self = Self::Running(app_state);
            }
        }

        fn user_event(
            &mut self,
            _event_loop: &winit::event_loop::ActiveEventLoop,
            _event: WakerEvent,
        ) {
            if let Self::Running(state) = self {
                state.servo.spin_event_loop();
            }
        }

        fn window_event(
            &mut self,
            event_loop: &winit::event_loop::ActiveEventLoop,
            _window_id: winit::window::WindowId,
            event: WindowEvent,
        ) {
            if let Self::Running(state) = self {
                state.servo.spin_event_loop();
            }

            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::RedrawRequested => {
                    if let Self::Running(state) = self {
                        if let Some(webview) = state.webviews.borrow().last() {
                            webview.paint();
                            state.rendering_context.present();
                        }
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if let Self::Running(state) = self {
                        let point: WebViewPoint =
                            DevicePoint::new(position.x as f32, position.y as f32).into();
                        state.cursor_position.set(point);
                        if let Some(webview) = state.webviews.borrow().last() {
                            webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(
                                point,
                            )));
                        }
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    if let Self::Running(state) = self {
                        if let Some(webview) = state.webviews.borrow().last() {
                            webview.notify_input_event(InputEvent::MouseLeftViewport(
                                MouseLeftViewportEvent::default(),
                            ));
                        }
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    if let Self::Running(app_state) = self {
                        if let Some(webview) = app_state.webviews.borrow().last() {
                            let action = match state {
                                ElementState::Pressed => MouseButtonAction::Down,
                                ElementState::Released => MouseButtonAction::Up,
                            };
                            webview.notify_input_event(InputEvent::MouseButton(
                                MouseButtonEvent::new(
                                    action,
                                    map_mouse_button(button),
                                    app_state.cursor_position.get(),
                                ),
                            ));
                        }
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    if let Self::Running(state) = self {
                        if let Some(webview) = state.webviews.borrow().last() {
                            let (delta_x, delta_y, mode) = match delta {
                                MouseScrollDelta::LineDelta(dx, dy) => {
                                    ((dx * 76.0) as f64, (dy * 76.0) as f64, WheelMode::DeltaLine)
                                }
                                MouseScrollDelta::PixelDelta(delta) => {
                                    (delta.x, delta.y, WheelMode::DeltaPixel)
                                }
                            };

                            webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                                WheelDelta {
                                    x: delta_x,
                                    y: delta_y,
                                    z: 0.0,
                                    mode,
                                },
                                state.cursor_position.get(),
                            )));
                        }
                    }
                }
                WindowEvent::ModifiersChanged(new_mods) => {
                    if let Self::Running(state) = self {
                        state.modifiers.set(new_mods);
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if let Self::Running(state) = self {
                    if event.state == ElementState::Pressed && !event.repeat {
                        let mods_state = state.modifiers.get().state();
                        if mods_state.alt_key() {
                                match event.logical_key {
                                    Key::Named(winit::keyboard::NamedKey::ArrowLeft) => {
                                        if let Some(wv) = state.webviews.borrow().last() {
                                            if wv.can_go_back() {
                                                let _ = wv.go_back(1);
                                            }
                                        }
                                    },
                                    Key::Named(winit::keyboard::NamedKey::ArrowRight) => {
                                        if let Some(wv) = state.webviews.borrow().last() {
                                            if wv.can_go_forward() {
                                                let _ = wv.go_forward(1);
                                            }
                                        }
                                    },
                                    _ => {},
                                }
                            }
                            if mods_state.control_key() {
                                if event.logical_key == Key::Character("r".into()) {
                                    if let Some(wv) = state.webviews.borrow().last() {
                                        wv.reload();
                                    }
                                }
                            }
                        }
                    }
                }
                WindowEvent::Resized(new_size) => {
                    if let Self::Running(state) = self {
                        if let Some(webview) = state.webviews.borrow().last() {
                            webview.resize(new_size);
                        }
                        state.window.request_redraw();
                    }
                }
                _ => (),
            }
        }
    }

    fn map_mouse_button(button: WinitMouseButton) -> MouseButton {
        match button {
            WinitMouseButton::Left => MouseButton::Left,
            WinitMouseButton::Middle => MouseButton::Middle,
            WinitMouseButton::Right => MouseButton::Right,
            WinitMouseButton::Back => MouseButton::Back,
            WinitMouseButton::Forward => MouseButton::Forward,
            WinitMouseButton::Other(value) => MouseButton::Other(value),
        }
    }

    #[derive(Clone)]
    struct Waker(winit::event_loop::EventLoopProxy<WakerEvent>);

    #[derive(Debug)]
    struct WakerEvent;

    impl Waker {
        fn new(event_loop: &EventLoop<WakerEvent>) -> Self {
            Self(event_loop.create_proxy())
        }
    }

    impl embedder_traits::EventLoopWaker for Waker {
        fn clone_box(&self) -> Box<dyn embedder_traits::EventLoopWaker> {
            Box::new(Self(self.0.clone()))
        }

        fn wake(&self) {
            if let Err(error) = self.0.send_event(WakerEvent) {
                warn!(?error, "failed to wake Servo event loop");
            }
        }
    }

    fn wait_until_loaded(
        servo: &Servo,
        delegate: &ProofDelegate,
        deadline: Instant,
        timeout_secs: u64,
    ) -> Result<()> {
        while !delegate.loaded.get() {
            if Instant::now() > deadline {
                bail!("timed out waiting for page load after {timeout_secs}s");
            }
            servo.spin_event_loop();
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    fn nudge_post_load_frame(servo: &Servo, webview: &WebView, deadline: Instant) -> Result<()> {
        let done = Rc::new(Cell::new(false));
        let done_for_callback = done.clone();
        webview.evaluate_javascript(
            "new Promise(r => requestAnimationFrame(() => { document.documentElement.getBoundingClientRect(); r(); }))",
            move |_result| done_for_callback.set(true),
        );

        while !done.get() {
            if Instant::now() > deadline {
                bail!("timed out waiting for Servo JavaScript frame nudge");
            }
            servo.spin_event_loop();
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    fn wait_for_post_load_frame(
        servo: &Servo,
        delegate: &ProofDelegate,
        deadline: Instant,
    ) -> Result<()> {
        let frames_at_load = delegate.frames.get();
        while delegate.frames.get() <= frames_at_load {
            if Instant::now() > deadline {
                bail!(
                    "timed out waiting for a post-load Servo frame; saw {} frames",
                    delegate.frames.get()
                );
            }
            servo.spin_event_loop();
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    fn resolve_target(target: &str) -> Result<Url> {
        if let Ok(url) = Url::parse(target) {
            if matches!(url.scheme(), "http" | "https" | "file" | "data" | "about") {
                return Ok(url);
            }
        }

        let path = Path::new(target);
        if !path.exists() {
            bail!(
                "{target:?} is not a supported URL or existing file. Build Studio first with `cd ordo-studio && npm run build`."
            );
        }

        let abs = path
            .canonicalize()
            .with_context(|| format!("canonicalizing {target}"))?;
        Url::from_file_path(&abs)
            .map_err(|_| anyhow::anyhow!("could not convert {:?} to a file:// URL", abs))
    }

    fn ensure_parent_dir(path: &str) -> Result<()> {
        if let Some(parent) = PathBuf::from(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating output directory {:?}", parent))?;
            }
        }
        Ok(())
    }

    #[derive(Default)]
    struct ProofDelegate {
        loaded: Cell<bool>,
        frames: Cell<u32>,
    }

    impl WebViewDelegate for ProofDelegate {
        fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
            if matches!(status, LoadStatus::Complete) {
                self.loaded.set(true);
            }
        }

        fn notify_new_frame_ready(&self, webview: WebView) {
            webview.paint();
            self.frames.set(self.frames.get() + 1);
        }
    }
}
