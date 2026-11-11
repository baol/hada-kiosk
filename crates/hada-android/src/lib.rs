//! hada as an Android app. Android starts `android_main`; everything else
//! (dashboard, preferences page, Home Assistant session) is shared with the
//! Linux build through the `hada` library.
//!
//! On other targets this crate is empty, so `cargo build --workspace` keeps working.

#[cfg(target_os = "android")]
mod android {
    use hada::prefs::{Platform, Rotation};
    use slint::android::android_activity::{AndroidApp, WindowManagerFlags};
    use slint::ComponentHandle;

    #[unsafe(no_mangle)]
    fn android_main(app: AndroidApp) {
        logcat::init();
        tracing::info!("hada {}", hada::VERSION);
        if let Err(e) = run(app) {
            tracing::error!("hada stopped: {e:#}");
        }
        // Android may call android_main again in this same process when the activity
        // comes back, but Slint's platform can only be set up once: start afresh.
        tracing::info!("activity closed; exiting");
        std::process::exit(0);
    }

    fn run(app: AndroidApp) -> anyhow_lite::Result<()> {
        slint::android::init(app.clone()).map_err(|e| format!("Slint: {e}"))?;
        // A wall panel: keep the screen on, no status bar.
        app.set_window_flags(
            WindowManagerFlags::KEEP_SCREEN_ON | WindowManagerFlags::FULLSCREEN,
            WindowManagerFlags::empty(),
        );
        let data_dir = app.internal_data_path().ok_or("no app data folder")?;
        let language = hada_language(app.config().language());
        tracing::info!("data in {}, language {language}", data_dir.display());

        let ui = hada::AppWindow::new().map_err(|e| format!("UI: {e}"))?;
        let rotate_app = app.clone();
        hada::prefs::run_with_prefs(
            &ui,
            Platform {
                data_dir,
                language,
                apply_rotation: Box::new(move |r| set_orientation(&rotate_app, r)),
            },
        );
        ui.run().map_err(|e| format!("event loop: {e}"))?;
        Ok(())
    }

    /// `it`, `de`, … from the device locale; English when unknown.
    fn hada_language(lang: Option<String>) -> String {
        hada_core::translations::normalise_language(lang.as_deref().unwrap_or("en"))
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
        // Safety: android-activity documents these pointers as the process's JavaVM and
        // a global reference to the activity, valid while the app runs.
        let vm = jni::JavaVM::singleton()
            .unwrap_or_else(|_| unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as *mut _) });
        let result = vm.attach_current_thread(|env| -> Result<(), jni::errors::Error> {
            let activity = unsafe {
                jni::objects::JObject::from_raw(env, app.activity_as_ptr() as jni::sys::jobject)
            };
            env.call_method(
                &activity,
                jni::jni_str!("setRequestedOrientation"),
                jni::jni_sig!("(I)V"),
                &[jni::JValue::Int(value)],
            )?;
            Ok(())
        });
        match result {
            Ok(()) => tracing::info!("orientation {r:?}"),
            Err(e) => tracing::warn!("cannot set orientation {r:?}: {e}"),
        }
    }

    /// Error type without an extra dependency: any message.
    mod anyhow_lite {
        pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
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
