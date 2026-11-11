//! hada as a library: the Slint UI, the bridge between Home Assistant and the UI
//! models, and the session start-up. Used by the Linux binary (`main.rs`) and by
//! the Android app (`crates/hada-android`).

pub mod app;
pub mod build_ui;
#[cfg(feature = "screenshot")]
pub mod headless;
pub mod mdi;
pub mod prefs;

slint::include_modules!();

use anyhow::Context;
use hada_core::ha::{self, DashboardSource, HaCommand, HaConfig, HaEvent};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc::UnboundedSender;

/// Crate version plus the git commit it was built from.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("HADA_GIT_HASH"), ")");

/// The current session; events of a stopped one are dropped.
static SESSION: AtomicU64 = AtomicU64::new(0);

type Observer = Rc<dyn Fn(&HaEvent)>;

thread_local! {
    /// Sees every event of the current session before the UI does (UI thread only).
    static OBSERVER: RefCell<Option<Observer>> = const { RefCell::new(None) };
}

/// Watch the session's events (connection state, sign-in failures) on the UI thread.
pub fn set_event_observer(f: impl Fn(&HaEvent) + 'static) {
    OBSERVER.with(|o| *o.borrow_mut() = Some(Rc::new(f)));
}

/// A connection to Home Assistant feeding the UI.
pub struct Session {
    cmd_tx: UnboundedSender<HaCommand>,
}

impl Session {
    /// Close the connection; its pending events are ignored.
    pub fn stop(&self) {
        SESSION.fetch_add(1, Ordering::SeqCst);
        let _ = self.cmd_tx.send(HaCommand::Shutdown);
    }
}

/// Install the UI state on `ui` and connect to Home Assistant on a network thread.
/// Calling it again (new settings) replaces the previous session.
pub fn start_session(
    ui: &AppWindow,
    cfg: HaConfig,
    view: Option<String>,
    cache: Option<PathBuf>,
) -> anyhow::Result<Session> {
    let generation = SESSION.fetch_add(1, Ordering::SeqCst) + 1;
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<HaCommand>();
    let app = app::App::install(ui, Some(cmd_tx.clone()), view, cache, &cfg.language);
    // A local dashboard is shown right away, even before Home Assistant answers.
    if let DashboardSource::Local(d) = &cfg.source {
        app.borrow_mut().set_dashboard(d.clone());
    }
    // Network thread: tokio runtime driving the websocket session; events hop to the UI thread.
    std::thread::Builder::new()
        .name("ha-ws".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(ha::run(cfg, cmd_rx, move |ev: HaEvent| {
                let _ = slint::invoke_from_event_loop(move || {
                    if SESSION.load(Ordering::SeqCst) == generation {
                        if let Some(f) = OBSERVER.with(|o| o.borrow().clone()) {
                            f(&ev);
                        }
                        app::with_app(|app| app.handle(ev));
                    }
                });
            }));
        })
        .context("spawning network thread")?;
    Ok(Session { cmd_tx })
}
