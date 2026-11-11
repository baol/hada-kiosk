//! hada as an Android app (Android 5.0+). Android starts `android_main`; the
//! dashboard, the Home Assistant session and the preferences logic are shared with
//! the Linux build through the `hada` library. The dashboard is drawn by Slint's
//! software renderer (see `platform.rs`); the preferences are a plain Android
//! screen (`android/java/.../PrefsActivity.java`).
//!
//! On other targets this crate is empty, so `cargo build --workspace` keeps working.

#[cfg(target_os = "android")]
mod platform;

#[cfg(target_os = "android")]
mod android {
    use crate::platform;
    use android_activity::{AndroidApp, WindowManagerFlags};
    use hada::prefs::{PrefsHandle, PrefsInput, PrefsResult, PrefsView, Rotation};
    use jni::objects::{JClassLoader, JObject, JString};
    use jni::refs::LoaderContext;
    use jni::{jni_sig, jni_str, Env, JValue, JavaVM};
    use slint::ComponentHandle;
    use std::cell::RefCell;

    const PREFS_CLASS: &str = "io.github.baol.hada.PrefsActivity";

    type BoxError = Box<dyn std::error::Error>;

    #[unsafe(no_mangle)]
    fn android_main(app: AndroidApp) {
        logcat::init();
        tracing::info!("hada {}", hada::VERSION);
        if let Err(e) = run(app) {
            tracing::error!("hada stopped: {e}");
        }
    }

    fn run(app: AndroidApp) -> Result<(), BoxError> {
        platform::install(app.clone())?;
        // A wall panel: keep the screen on, no status bar.
        app.set_window_flags(
            WindowManagerFlags::KEEP_SCREEN_ON | WindowManagerFlags::FULLSCREEN,
            WindowManagerFlags::empty(),
        );
        let data_dir = app.internal_data_path().ok_or("no app data folder")?;
        let language = hada_core::translations::normalise_language(
            app.config().language().as_deref().unwrap_or("en"),
        );
        tracing::info!("data in {}, language {language}", data_dir.display());

        let ui = hada::AppWindow::new()?;
        let (rot_app, prefs_app) = (app.clone(), app.clone());
        let handle = hada::prefs::run_with_prefs(
            &ui,
            hada::prefs::Platform {
                data_dir,
                language,
                apply_rotation: Box::new(move |r| set_orientation(&rot_app, r)),
                open_prefs: Box::new(move |view| open_prefs(&prefs_app, view)),
            },
        );
        // Back from the preferences screen: pick up what was saved.
        let handle: RefCell<PrefsHandle> = RefCell::new(handle);
        let result_app = app.clone();
        platform::set_on_resume(move || {
            if let Some(result) = take_result(&result_app) {
                handle.borrow().deliver(result);
            }
        });
        ui.run()?;
        Ok(())
    }

    fn with_activity<T>(
        app: &AndroidApp,
        f: impl FnOnce(&mut Env, &JObject) -> jni::errors::Result<T>,
    ) -> jni::errors::Result<T> {
        // Safety: android-activity documents these pointers as the process's JavaVM and
        // a global reference to the activity, valid while the app runs.
        let vm = JavaVM::singleton()
            .unwrap_or_else(|_| unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) });
        vm.attach_current_thread(|env| {
            let activity =
                unsafe { JObject::from_raw(env, app.activity_as_ptr() as jni::sys::jobject) };
            f(env, &activity)
        })
    }

    /// `Activity.setRequestedOrientation` with `ActivityInfo.SCREEN_ORIENTATION_*`.
    fn set_orientation(app: &AndroidApp, r: Rotation) {
        let value: i32 = match r {
            Rotation::Auto => -1,            // UNSPECIFIED: follows the system setting
            Rotation::Landscape => 0,        // LANDSCAPE
            Rotation::Portrait => 1,         // PORTRAIT
            Rotation::LandscapeReverse => 8, // REVERSE_LANDSCAPE
            Rotation::PortraitReverse => 9,  // REVERSE_PORTRAIT
        };
        let result = with_activity(app, |env, activity| {
            env.call_method(
                activity,
                jni_str!("setRequestedOrientation"),
                jni_sig!("(I)V"),
                &[JValue::Int(value)],
            )?;
            Ok(())
        });
        match result {
            Ok(()) => tracing::info!("orientation {r:?}"),
            Err(e) => tracing::warn!("cannot set orientation {r:?}: {e}"),
        }
    }

    /// The app's `PrefsActivity` class. The activity object is Android's own
    /// `NativeActivity`, whose class loader cannot see app classes, so ask the activity
    /// (a Context) for the app's class loader.
    fn prefs_class<'local>(
        env: &mut Env<'local>,
        activity: &JObject,
    ) -> jni::errors::Result<jni::objects::JClass<'local>> {
        let loader = env
            .call_method(
                activity,
                jni_str!("getClassLoader"),
                jni_sig!("()Ljava/lang/ClassLoader;"),
                &[],
            )?
            .l()?;
        let loader = env.cast_local::<JClassLoader>(loader)?;
        LoaderContext::Loader(&loader).load_class(
            env,
            jni_str!("io.github.baol.hada.PrefsActivity"),
            true,
        )
    }

    /// `PrefsActivity.open(activity, url, user, token, rotation, error, signedIn)`.
    fn open_prefs(app: &AndroidApp, view: &PrefsView) {
        let p = &view.prefs;
        let result = with_activity(app, |env, activity| {
            let class = prefs_class(env, activity)?;
            let url = env.new_string(&p.url)?;
            let user = env.new_string(&p.username)?;
            let token = env.new_string(&p.token)?;
            let error = env.new_string(&view.error)?;
            env.call_static_method(
                &class,
                jni_str!("open"),
                jni_sig!("(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;ILjava/lang/String;I)V"),
                &[
                    JValue::Object(activity),
                    JValue::Object(&url),
                    JValue::Object(&user),
                    JValue::Object(&token),
                    JValue::Int(p.rotation.index()),
                    JValue::Object(&error),
                    JValue::Int(i32::from(view.signed_in)),
                ],
            )?;
            Ok(())
        });
        match result {
            Ok(()) => tracing::info!(
                "preferences opened{}",
                if view.error.is_empty() {
                    String::new()
                } else {
                    format!(": {}", view.error)
                }
            ),
            Err(e) => tracing::error!("cannot open {PREFS_CLASS}: {e}"),
        }
    }

    /// `PrefsActivity.takeResult()`: what the screen returned, once.
    fn take_result(app: &AndroidApp) -> Option<PrefsResult> {
        let text = with_activity(app, |env, activity| {
            let class = prefs_class(env, activity)?;
            let obj = env
                .call_static_method(
                    &class,
                    jni_str!("takeResult"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            if obj.is_null() {
                return Ok(None);
            }
            let s = env.cast_local::<JString>(obj)?;
            Ok(Some(s.try_to_string(env)?))
        });
        match text {
            Ok(t) => parse_result(&t?),
            Err(e) => {
                tracing::error!("cannot read the preferences result: {e}");
                None
            }
        }
    }

    fn parse_result(text: &str) -> Option<PrefsResult> {
        let mut parts = text.split('\u{1f}');
        match parts.next()? {
            "saved" => {
                let mut next = || parts.next().unwrap_or("").to_string();
                let (url, username, password, token) = (next(), next(), next(), next());
                let rotation = Rotation::from_index(next().parse().unwrap_or(0));
                tracing::info!("preferences saved");
                Some(PrefsResult::Saved(PrefsInput {
                    url,
                    username,
                    password,
                    token,
                    rotation,
                }))
            }
            "signout" => Some(PrefsResult::SignOut),
            "cancel" => Some(PrefsResult::Cancelled),
            other => {
                tracing::warn!("unknown preferences result {other:?}");
                None
            }
        }
    }

    /// `tracing` output to logcat (tag "hada"); stdout and stderr go nowhere on Android.
    mod logcat {
        use std::ffi::{c_char, c_int, CString};
        use std::io::Write;

        #[link(name = "log")]
        extern "C" {
            fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
        }

        const ANDROID_LOG_INFO: c_int = 4;

        /// Collects one formatted event and writes it as one log line when dropped.
        pub struct Line(Vec<u8>);

        impl Write for Line {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl Drop for Line {
            fn drop(&mut self) {
                let text = String::from_utf8_lossy(&self.0);
                let text = text.trim_end();
                if text.is_empty() {
                    return;
                }
                if let Ok(c) = CString::new(text.replace('\0', " ")) {
                    // Safety: both strings are valid, NUL-terminated C strings.
                    unsafe { __android_log_write(ANDROID_LOG_INFO, c"hada".as_ptr(), c.as_ptr()) };
                }
            }
        }

        pub fn init() {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
                .with_ansi(false)
                .without_time()
                .with_target(false)
                .with_writer(|| Line(Vec::new()))
                .try_init();
            std::panic::set_hook(Box::new(|info| {
                tracing::error!("panic: {info}");
            }));
        }
    }
}
