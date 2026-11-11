//! A small Slint platform for Android 5.0+: Slint's software renderer (the same
//! one the Raspberry Pi kiosk uses) draws into the activity's window buffer.
//! Touch input only; the preferences are a separate Android screen.
//!
//! Slint's own Android backend draws with Skia, which needs Android 8.0.

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{EventLoopProxy, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{EventLoopError, LogicalPosition, PhysicalSize, PlatformError};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use android_activity::input::{InputEvent, KeyAction, Keycode, MotionAction};
use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent};
use ndk::hardware_buffer_format::HardwareBufferFormat;
use ndk::native_window::NativeWindow;

type Job = Box<dyn FnOnce() + Send>;

/// The activity this process currently runs (android_main can start more than once).
static APP: Mutex<Option<AndroidApp>> = Mutex::new(None);
static INSTALLED: OnceLock<()> = OnceLock::new();

thread_local! {
    static ON_RESUME: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Called whenever the dashboard comes back to the front (e.g. after the preferences).
pub fn set_on_resume(f: impl Fn() + 'static) {
    ON_RESUME.with(|r| *r.borrow_mut() = Some(Rc::new(f)));
}

/// Make Slint use this platform for `app`. Safe to call on every `android_main`.
pub fn install(app: AndroidApp) -> Result<(), PlatformError> {
    *APP.lock().unwrap() = Some(app);
    if INSTALLED.set(()).is_ok() {
        slint::platform::set_platform(Box::new(AndroidSoftware::default()))
            .map_err(|e| PlatformError::Other(e.to_string()))?;
    }
    Ok(())
}

fn current_app() -> Option<AndroidApp> {
    APP.lock().unwrap().clone()
}

#[derive(Default)]
struct Shared {
    jobs: VecDeque<Job>,
    quit: bool,
}

#[derive(Default)]
struct AndroidSoftware {
    window: RefCell<Option<Rc<MinimalSoftwareWindow>>>,
    shared: Arc<Mutex<Shared>>,
}

struct Proxy {
    shared: Arc<Mutex<Shared>>,
}

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), EventLoopError> {
        self.shared.lock().unwrap().quit = true;
        wake();
        Ok(())
    }

    fn invoke_from_event_loop(&self, job: Job) -> Result<(), EventLoopError> {
        self.shared.lock().unwrap().jobs.push_back(job);
        wake();
        Ok(())
    }
}

fn wake() {
    if let Some(app) = current_app() {
        app.create_waker().wake();
    }
}

impl Platform for AndroidSoftware {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        // Every buffer from the window may hold an older frame: repaint all of it.
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        *self.window.borrow_mut() = Some(w.clone());
        Ok(w)
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy {
            shared: self.shared.clone(),
        }))
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let app = current_app().ok_or_else(|| PlatformError::Other("no Android app".into()))?;
        self.shared.lock().unwrap().quit = false;
        let mut surface: Option<NativeWindow> = None;
        let pressed = Cell::new(false);
        let mut destroyed = false;
        let mut timeout: Option<Duration> = Some(Duration::ZERO);
        loop {
            let window = self.window.borrow().clone();
            let mut resized = false;
            let mut resumed = false;
            app.poll_events(timeout, |event| match event {
                PollEvent::Main(MainEvent::InitWindow { .. }) => {
                    surface = app.native_window();
                    resized = true;
                }
                PollEvent::Main(MainEvent::TerminateWindow { .. }) => surface = None,
                PollEvent::Main(
                    MainEvent::WindowResized { .. }
                    | MainEvent::ConfigChanged { .. }
                    | MainEvent::ContentRectChanged { .. }
                    | MainEvent::RedrawNeeded { .. },
                ) => resized = true,
                PollEvent::Main(MainEvent::Resume { .. }) => resumed = true,
                PollEvent::Main(MainEvent::Destroy) => destroyed = true,
                PollEvent::Main(MainEvent::InputAvailable) => {
                    if let Some(w) = &window {
                        handle_input(&app, w, &pressed);
                    }
                }
                _ => {}
            });
            // Input can also be waiting without an InputAvailable event.
            if let Some(w) = &window {
                handle_input(&app, w, &pressed);
            }
            if destroyed {
                tracing::info!("activity destroyed");
                return Ok(());
            }
            if resized {
                if let (Some(s), Some(w)) = (&surface, &window) {
                    configure(&app, s, w);
                }
            }
            if resumed {
                if let Some(f) = ON_RESUME.with(|r| r.borrow().clone()) {
                    f();
                }
            }
            let jobs: Vec<Job> = std::mem::take(&mut self.shared.lock().unwrap().jobs).into();
            for job in jobs {
                job();
            }
            if self.shared.lock().unwrap().quit {
                return Ok(());
            }
            slint::platform::update_timers_and_animations();
            let window = self.window.borrow().clone();
            if let (Some(s), Some(w)) = (&surface, &window) {
                draw(s, w);
            }
            timeout = match &window {
                Some(w) if w.has_active_animations() => Some(Duration::from_millis(16)),
                _ => slint::platform::duration_until_next_timer_update(),
            };
        }
    }
}

/// New window or new size/density: tell Slint and use 32-bit RGBA buffers.
fn configure(app: &AndroidApp, surface: &NativeWindow, window: &MinimalSoftwareWindow) {
    let (w, h) = (surface.width(), surface.height());
    if w <= 0 || h <= 0 {
        return;
    }
    if let Err(e) = surface.set_buffers_geometry(0, 0, Some(HardwareBufferFormat::R8G8B8A8_UNORM)) {
        tracing::warn!("cannot select RGBA window buffers: {e}");
    }
    let dpi = app.config().density().unwrap_or(160).max(80);
    let scale = dpi as f32 / 160.0;
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale,
    });
    window.set_size(PhysicalSize::new(w as u32, h as u32));
    window.request_redraw();
    tracing::info!("window {w}x{h} px, {dpi} dpi (scale {scale:.2})");
}

fn draw(surface: &NativeWindow, window: &MinimalSoftwareWindow) {
    window.draw_if_needed(|renderer| {
        let mut buf = match surface.lock(None) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("cannot lock the window buffer: {e}");
                return;
            }
        };
        if buf.format() != HardwareBufferFormat::R8G8B8A8_UNORM
            && buf.format() != HardwareBufferFormat::R8G8B8X8_UNORM
        {
            tracing::warn!("unexpected window format {:?}", buf.format());
            return;
        }
        let (stride, height) = (buf.stride(), buf.height());
        // Safety: the locked buffer holds stride * height 4-byte pixels (format checked
        // above) and stays valid until `buf` is dropped, which posts it.
        let pixels = unsafe {
            std::slice::from_raw_parts_mut(
                buf.bits() as *mut PremultipliedRgbaColor,
                stride * height,
            )
        };
        renderer.render(pixels, stride);
        static FIRST: std::sync::Once = std::sync::Once::new();
        FIRST.call_once(|| tracing::info!("first frame drawn ({}x{height} px)", buf.width()));
    });
}

fn handle_input(app: &AndroidApp, window: &MinimalSoftwareWindow, pressed: &Cell<bool>) {
    let Ok(mut iter) = app.input_events_iter() else {
        return;
    };
    let scale = window.window().scale_factor().max(0.1);
    loop {
        let more = iter.next(|event| match event {
            InputEvent::MotionEvent(m) => {
                // The first finger drives Slint's single pointer.
                let p = m.pointer_at_index(0);
                let position = LogicalPosition::new(p.x() / scale, p.y() / scale);
                match m.action() {
                    MotionAction::Down => {
                        pressed.set(true);
                        window.dispatch_event(WindowEvent::PointerMoved { position });
                        window.dispatch_event(WindowEvent::PointerPressed {
                            position,
                            button: PointerEventButton::Left,
                        });
                    }
                    MotionAction::Move if pressed.get() => {
                        window.dispatch_event(WindowEvent::PointerMoved { position });
                    }
                    MotionAction::Up => {
                        pressed.set(false);
                        window.dispatch_event(WindowEvent::PointerReleased {
                            position,
                            button: PointerEventButton::Left,
                        });
                        window.dispatch_event(WindowEvent::PointerExited);
                    }
                    MotionAction::Cancel => {
                        pressed.set(false);
                        window.dispatch_event(WindowEvent::PointerExited);
                    }
                    _ => {}
                }
                InputStatus::Handled
            }
            // Back on a wall panel would close the dashboard: ignore it.
            InputEvent::KeyEvent(k) if k.key_code() == Keycode::Back => {
                if k.action() == KeyAction::Up {
                    tracing::info!("Back ignored on the dashboard");
                }
                InputStatus::Handled
            }
            _ => InputStatus::Unhandled,
        });
        if !more {
            break;
        }
    }
}
