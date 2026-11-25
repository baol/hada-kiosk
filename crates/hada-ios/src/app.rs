//! The iOS app. The dashboard, the Home Assistant session and the preferences
//! logic are shared with the Linux and Android builds through the `hada` library;
//! Slint's winit backend draws with Skia on Metal. This module adds what iOS needs
//! around it: a scene delegate, the preferences form (a UIKit alert) and rotation.

use crate::BUNDLE_ID;
use block2::RcBlock;
use hada::prefs::{PrefsHandle, PrefsInput, PrefsResult, PrefsView, Rotation};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, Sel};
use objc2::{define_class, sel, ClassType, MainThreadOnly, Message};
use objc2_foundation::{NSLocale, NSString};
use objc2_ui_kit::{
    UIAlertAction, UIAlertActionStyle, UIAlertController, UIAlertControllerStyle, UIApplication,
    UIKeyboardType, UIResponder, UIScene, UISceneConnectionOptions, UISceneDelegate,
    UISceneSession, UITextAutocapitalizationType, UITextAutocorrectionType, UITextField, UIView,
    UIViewController, UIWindow, UIWindowScene, UIWindowSceneDelegate,
};
use slint::winit_030::winit::platform::ios::{ValidOrientations, WindowExtIOS};
use slint::winit_030::WinitWindowAccessor;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::time::Duration;

thread_local! {
    static UI: RefCell<Option<slint::Weak<hada::AppWindow>>> = const { RefCell::new(None) };
    static HANDLE: RefCell<Option<PrefsHandle>> = const { RefCell::new(None) };
    /// The dashboard's window, once it is in the scene.
    static WINDOW: RefCell<Option<Retained<UIWindow>>> = const { RefCell::new(None) };
}

type BoxError = Box<dyn std::error::Error>;

pub fn main() {
    init_logging();
    tracing::info!("hada {} ({BUNDLE_ID})", hada::VERSION);
    // UIKit looks the scene delegate up by the name in Info.plist.
    let _ = SceneDelegate::class();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let data_dir = home.join("Library/Application Support/hada");
    let preferred = NSLocale::preferredLanguages()
        .firstObject()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "en".into());
    // "it-IT" → "it_IT", the form normalise_language expects.
    let language = hada_core::translations::normalise_language(&preferred.replace('-', "_"));
    tracing::info!("data in {}, language {language}", data_dir.display());
    if let Err(e) = run(data_dir, language) {
        tracing::error!("hada stopped: {e}");
    }
}

fn run(data_dir: PathBuf, language: String) -> Result<(), BoxError> {
    std::fs::create_dir_all(&data_dir)?;
    let ui = hada::AppWindow::new()?;
    UI.with(|u| *u.borrow_mut() = Some(ui.as_weak()));
    let handle = hada::prefs::run_with_prefs(
        &ui,
        hada::prefs::Platform {
            data_dir,
            language,
            apply_rotation: Box::new(apply_rotation),
            open_prefs: Box::new(open_prefs),
        },
    );
    HANDLE.with(|h| *h.borrow_mut() = Some(handle));
    ui.run()?;
    Ok(())
}

define_class!(
    // Apps built with the iOS 27 SDK must use scenes, but winit creates its
    // UIWindow the pre-scene way, which iOS no longer shows. When the scene
    // connects, the dashboard's window moves into it.
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "HadaSceneDelegate"]
    struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(
            &self,
            scene: &UIScene,
            _session: &UISceneSession,
            _options: &UISceneConnectionOptions,
        ) {
            // A wall panel: keep the screen on.
            UIApplication::sharedApplication(self.mtm()).setIdleTimerDisabled(true);
            match scene.downcast_ref::<UIWindowScene>() {
                Some(scene) => attach(scene.retain(), 100),
                None => tracing::error!("not a window scene"),
            }
        }
    }

    unsafe impl UIWindowSceneDelegate for SceneDelegate {}
);

/// Move the dashboard's window into `scene`. The window is created as Slint's
/// event loop starts, possibly just after the scene connects: retry for a while.
fn attach(scene: Retained<UIWindowScene>, tries: u32) {
    if let Some(window) = dashboard_window() {
        window.setWindowScene(Some(&scene));
        window.makeKeyAndVisible();
        WINDOW.with(|w| *w.borrow_mut() = Some(window));
        with_ui(|ui| {
            ui.window()
                .with_winit_window(|w| w.set_prefers_home_indicator_hidden(true));
        });
        tracing::info!("window in scene");
    } else if tries > 0 {
        slint::Timer::single_shot(Duration::from_millis(50), move || attach(scene, tries - 1));
    } else {
        tracing::error!("the dashboard window never appeared");
    }
}

fn with_ui(f: impl FnOnce(&hada::AppWindow)) {
    if let Some(ui) = UI.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())) {
        f(&ui);
    }
}

/// The UIWindow winit created for the dashboard, if any yet.
fn dashboard_window() -> Option<Retained<UIWindow>> {
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let ui = UI.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade()))?;
    let view = ui
        .window()
        .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
            RawWindowHandle::UiKit(h) => Some(h.ui_view),
            _ => None,
        })
        .flatten()?;
    // Safety: winit's UIView, alive as long as its window, which `ui` keeps.
    let view: &UIView = unsafe { view.cast::<UIView>().as_ref() };
    view.window()
}

/// iOS cannot pick one of the two landscape (or portrait) sides; the reverse
/// rotations allow both.
fn apply_rotation(r: Rotation) {
    let valid = match r {
        Rotation::Auto => ValidOrientations::LandscapeAndPortrait,
        Rotation::Landscape | Rotation::LandscapeReverse => ValidOrientations::Landscape,
        Rotation::Portrait | Rotation::PortraitReverse => ValidOrientations::Portrait,
    };
    // The window may not exist yet on the first call: wait for the event loop.
    slint::Timer::single_shot(Duration::ZERO, move || {
        with_ui(|ui| {
            ui.window()
                .with_winit_window(|w| w.set_valid_orientations(valid));
            tracing::info!("orientation {r:?}");
        });
    });
}

/// What the preferences form shows and returns.
#[derive(Clone)]
struct Form {
    url: String,
    user: String,
    password: String,
    token: String,
    rotation: Rotation,
    error: String,
    signed_in: bool,
}

fn open_prefs(view: &PrefsView) {
    let p = &view.prefs;
    let form = Form {
        url: p.url.clone(),
        user: p.username.clone(),
        password: String::new(),
        token: p.token.clone(),
        rotation: match p.rotation {
            Rotation::PortraitReverse => Rotation::Portrait,
            Rotation::LandscapeReverse => Rotation::Landscape,
            r => r,
        },
        error: view.error.clone(),
        signed_in: view.signed_in,
    };
    // Let the current event finish first.
    slint::Timer::single_shot(Duration::ZERO, move || present(form, 100));
    if view.error.is_empty() {
        tracing::info!("preferences opened");
    } else {
        tracing::info!("preferences opened: {}", view.error);
    }
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// The view controller everything else is presented on.
fn top_view_controller() -> Option<Retained<UIViewController>> {
    let window = WINDOW.with(|w| w.borrow().clone())?;
    let mut top = window.rootViewController()?;
    while let Some(presented) = top.presentedViewController() {
        top = presented;
    }
    Some(top)
}

/// Show the form as an alert with text fields; the rotation is a button that
/// cycles and reopens the form with what was typed.
fn present(form: Form, tries: u32) {
    let Some(host) = top_view_controller() else {
        // The window is not in its scene yet (first start).
        if tries > 0 {
            slint::Timer::single_shot(Duration::from_millis(100), move || present(form, tries - 1));
        }
        return;
    };
    let mtm = host.mtm();
    let mut message = String::new();
    if !form.error.is_empty() {
        message.push_str(&form.error);
        message.push_str("\n\n");
    }
    message.push_str(
        "A dashboard address such as …:8123/lovelace/kiosk also opens that view. \
         Leave the password empty to sign in through trusted networks; \
         it is used once and never stored.",
    );
    let alert = UIAlertController::alertControllerWithTitle_message_preferredStyle(
        Some(&ns("Connect to Home Assistant")),
        Some(&ns(&message)),
        UIAlertControllerStyle::Alert,
        mtm,
    );
    add_field(
        &alert,
        "http://192.168.1.10:8123",
        &form.url,
        false,
        UIKeyboardType::URL,
    );
    add_field(
        &alert,
        "Username",
        &form.user,
        false,
        UIKeyboardType::Default,
    );
    add_field(
        &alert,
        "Password (optional)",
        &form.password,
        true,
        UIKeyboardType::Default,
    );
    add_field(
        &alert,
        "Access token (optional)",
        &form.token,
        true,
        UIKeyboardType::Default,
    );

    add_action(
        &alert,
        "Save & connect",
        UIAlertActionStyle::Default,
        &form,
        |f| {
            deliver(PrefsResult::Saved(PrefsInput {
                url: f.url,
                username: f.user,
                password: f.password,
                token: f.token,
                rotation: f.rotation,
            }))
        },
    );
    let (label, next) = match form.rotation {
        Rotation::Portrait => ("Portrait", Rotation::Landscape),
        Rotation::Landscape => ("Landscape", Rotation::Auto),
        _ => ("Auto", Rotation::Portrait),
    };
    add_action(
        &alert,
        &format!("Rotation: {label}"),
        UIAlertActionStyle::Default,
        &form,
        move |f| {
            let f = Form {
                rotation: next,
                ..f
            };
            slint::Timer::single_shot(Duration::ZERO, move || present(f, 100));
        },
    );
    if form.signed_in {
        add_action(
            &alert,
            "Sign out",
            UIAlertActionStyle::Destructive,
            &form,
            |_| deliver(PrefsResult::SignOut),
        );
    }
    add_action(&alert, "Cancel", UIAlertActionStyle::Cancel, &form, |_| {
        deliver(PrefsResult::Cancelled)
    });
    host.presentViewController_animated_completion(&alert, true, None);
}

fn add_field(
    alert: &UIAlertController,
    placeholder: &str,
    text: &str,
    secure: bool,
    keyboard: UIKeyboardType,
) {
    let (placeholder, text) = (placeholder.to_string(), text.to_string());
    let configure = RcBlock::new(move |field: NonNull<UITextField>| {
        // Safety: UIKit passes the new, live text field.
        let field = unsafe { field.as_ref() };
        field.setPlaceholder(Some(&ns(&placeholder)));
        field.setText(Some(&ns(&text)));
        // On devices the alert's text field forwards the text input traits instead
        // of implementing them, which objc2's debug method check rejects.
        // Safety: UITextField accepts these messages with these argument types.
        unsafe {
            send(field, sel!(setSecureTextEntry:), Bool::new(secure));
            send(field, sel!(setKeyboardType:), keyboard.0);
            send(
                field,
                sel!(setAutocapitalizationType:),
                UITextAutocapitalizationType::None.0,
            );
            send(
                field,
                sel!(setAutocorrectionType:),
                UITextAutocorrectionType::No.0,
            );
        }
    });
    alert.addTextFieldWithConfigurationHandler(Some(&configure));
}

/// `[obj sel:arg]` through `objc_msgSend` directly, without objc2's check that
/// the class itself implements `sel` (it fails for forwarded methods).
///
/// # Safety
/// `obj` responds to `sel`, which takes one argument of type `A` and returns nothing.
unsafe fn send<A: objc2::encode::Encode>(obj: &UITextField, sel: Sel, arg: A) {
    let f: unsafe extern "C-unwind" fn(*const UITextField, Sel, A) =
        unsafe { std::mem::transmute(objc2::ffi::objc_msgSend as unsafe extern "C-unwind" fn()) };
    unsafe { f(obj, sel, arg) }
}

/// A button that reads the fields into a copy of `form` and hands it to `then`.
/// UIKit calls it after the alert has closed.
fn add_action(
    alert: &Retained<UIAlertController>,
    title: &str,
    style: UIAlertActionStyle,
    form: &Form,
    then: impl Fn(Form) + 'static,
) {
    let weak = Weak::from_retained(alert);
    let base = form.clone();
    let handler = RcBlock::new(move |_: NonNull<UIAlertAction>| {
        let mut f = base.clone();
        if let Some(fields) = weak.load().and_then(|a| a.textFields()) {
            let text = |i: usize| {
                fields
                    .iter()
                    .nth(i)
                    .and_then(|t| t.text())
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            };
            (f.url, f.user, f.password, f.token) = (text(0), text(1), text(2), text(3));
        }
        then(f);
    });
    let action = UIAlertAction::actionWithTitle_style_handler(
        Some(&ns(title)),
        style,
        Some(&handler),
        alert.mtm(),
    );
    alert.addAction(&action);
}

/// Hand the form's result to the shared preferences logic, from the event loop:
/// it may reopen the form at once.
fn deliver(result: PrefsResult) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(h) = HANDLE.with(|h| h.borrow().clone()) {
            h.deliver(result);
        }
    });
}

/// `tracing` output to stderr, which `cargo run` shows.
fn init_logging() {
    let _ = tracing_subscriber::fmt()
        // winit warns on every frame about iOS-specific event ordering.
        .with_env_filter(tracing_subscriber::EnvFilter::new("info,winit=error"))
        .with_ansi(false)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
    std::panic::set_hook(Box::new(|info| {
        tracing::error!("panic: {info}");
    }));
}
