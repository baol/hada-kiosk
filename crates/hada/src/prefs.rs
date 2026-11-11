//! Preferences of the Android app: what is saved, how it turns into a Home
//! Assistant session, and what happens when the preferences screen closes. The
//! screen itself and other device parts (where files live, rotation) come in
//! through [`Platform`]. The Linux binary does not use this module.

use crate::{start_session, AppWindow, Session};
use hada_core::ha::ws::split_dashboard_url;
use hada_core::ha::{Auth, DashboardSource, HaConfig, HaEvent};
use serde::{Deserialize, Serialize};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const PREFS_FILE: &str = "settings.toml";
const AUTH_FILE: &str = "auth.json";

/// How to sign in. The password itself is never saved: after the first login the
/// refresh token in `auth.json` is used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    TrustedNetworks,
    Password,
    Token,
}

/// Screen orientation; the numbers match the page's `rotation` property.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    #[default]
    Auto,
    Portrait,
    Landscape,
    PortraitReverse,
    LandscapeReverse,
}

impl Rotation {
    pub fn from_index(i: i32) -> Rotation {
        match i {
            1 => Rotation::Portrait,
            2 => Rotation::Landscape,
            3 => Rotation::PortraitReverse,
            4 => Rotation::LandscapeReverse,
            _ => Rotation::Auto,
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Rotation::Auto => 0,
            Rotation::Portrait => 1,
            Rotation::Landscape => 2,
            Rotation::PortraitReverse => 3,
            Rotation::LandscapeReverse => 4,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Home Assistant address, possibly a dashboard URL (`…/lovelace/kiosk`).
    pub url: String,
    pub username: String,
    pub method: Method,
    /// Long-lived token, only with [`Method::Token`].
    pub token: String,
    pub rotation: Rotation,
}

impl Prefs {
    pub fn load(dir: &Path) -> Option<Prefs> {
        let text = std::fs::read_to_string(dir.join(PREFS_FILE)).ok()?;
        toml::from_str(&text)
            .map_err(|e| tracing::warn!("ignoring unreadable {PREFS_FILE}: {e}"))
            .ok()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        std::fs::write(dir.join(PREFS_FILE), text)
    }

    /// Complete enough to connect without asking.
    pub fn is_complete(&self) -> bool {
        !self.url.trim().is_empty() && (self.method != Method::Token || !self.token.is_empty())
    }

    /// The sign-in for these preferences. `password` is what was just typed (empty
    /// on later starts, when the stored refresh token is used).
    pub fn auth(&self, password: &str) -> Auth {
        let user = Some(self.username.trim().to_string()).filter(|u| !u.is_empty());
        match self.method {
            Method::Token => Auth::Token(self.token.trim().to_string()),
            Method::Password => Auth::Password {
                username: user.unwrap_or_default(),
                password: password.to_string(),
            },
            Method::TrustedNetworks => Auth::TrustedNetwork { user },
        }
    }
}

/// "192.168.1.10:8123" → "http://192.168.1.10:8123"; surrounding spaces removed.
pub fn normalise_url(input: &str) -> String {
    let t = input.trim().trim_end_matches('/');
    if t.contains("://") || t.is_empty() {
        t.to_string()
    } else {
        format!("http://{t}")
    }
}

/// What the page's fields mean: the method follows from which fields are filled.
pub fn method_for(token: &str, password: &str) -> Method {
    if !token.trim().is_empty() {
        Method::Token
    } else if !password.is_empty() {
        Method::Password
    } else {
        Method::TrustedNetworks
    }
}

/// Session settings for these preferences.
pub fn session_config(
    prefs: &Prefs,
    password: &str,
    dir: &Path,
    language: &str,
) -> (HaConfig, Option<String>) {
    let (base, dashboard, view) = split_dashboard_url(&normalise_url(&prefs.url));
    let cfg = HaConfig {
        base_url: base,
        auth: prefs.auth(password),
        auth_store: Some(dir.join(AUTH_FILE)),
        language: language.to_string(),
        source: DashboardSource::Remote {
            url_path: dashboard,
        },
    };
    (cfg, view)
}

/// What the preferences screen shows when it opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrefsView {
    pub prefs: Prefs,
    /// Why it opened, e.g. "Sign-in failed: wrong username or password".
    pub error: String,
    /// Signed in before: offer "Sign out".
    pub signed_in: bool,
}

/// What the user entered on the preferences screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrefsInput {
    pub url: String,
    pub username: String,
    /// Used for this sign-in only, never saved.
    pub password: String,
    pub token: String,
    pub rotation: Rotation,
}

/// How the preferences screen was closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefsResult {
    Saved(PrefsInput),
    SignOut,
    /// Closed without saving.
    Cancelled,
}

/// What the app needs from the device.
pub struct Platform {
    /// Private folder for the preferences, the refresh token and caches.
    pub data_dir: PathBuf,
    /// Language for state texts, e.g. `it`.
    pub language: String,
    /// Lock the screen orientation (or let it follow the sensor).
    pub apply_rotation: Box<dyn Fn(Rotation)>,
    /// Show the preferences screen; its result comes back through
    /// [`PrefsHandle::deliver`].
    pub open_prefs: Box<dyn Fn(&PrefsView)>,
}

/// Hands the preferences screen's result back to the app.
#[derive(Clone)]
pub struct PrefsHandle(Rc<RefCell<Controller>>);

impl PrefsHandle {
    pub fn deliver(&self, result: PrefsResult) {
        let mut c = self.0.borrow_mut();
        match result {
            PrefsResult::Saved(input) => c.save(input),
            PrefsResult::SignOut => c.sign_out(),
            PrefsResult::Cancelled => c.show_setup_hint(),
        }
    }
}

struct Controller {
    ui: slint::Weak<AppWindow>,
    platform: Platform,
    prefs: Option<Prefs>,
    session: Option<Session>,
    /// Just saved: report the first failure on the preferences screen.
    pending: bool,
}

/// Start with the saved preferences, or open the preferences screen when there are
/// none. Tapping the status chip opens it later.
pub fn run_with_prefs(ui: &AppWindow, platform: Platform) -> PrefsHandle {
    let prefs = Prefs::load(&platform.data_dir).filter(Prefs::is_complete);
    (platform.apply_rotation)(prefs.as_ref().map(|p| p.rotation).unwrap_or_default());
    let ctl = Rc::new(RefCell::new(Controller {
        ui: ui.as_weak(),
        platform,
        prefs,
        session: None,
        pending: false,
    }));
    let c = ctl.clone();
    ui.on_status_clicked(move || c.borrow().open(""));
    let c = Rc::downgrade(&ctl);
    crate::set_event_observer(move |ev| {
        if let Some(c) = c.upgrade() {
            c.borrow_mut().observe(ev);
        }
    });
    {
        let mut c = ctl.borrow_mut();
        if c.prefs.is_some() {
            c.connect("");
        } else {
            c.show_setup_hint();
            c.open("");
        }
    }
    PrefsHandle(ctl)
}

impl Controller {
    fn open(&self, error: &str) {
        let view = PrefsView {
            prefs: self.prefs.clone().unwrap_or_default(),
            error: error.to_string(),
            signed_in: self.prefs.is_some(),
        };
        (self.platform.open_prefs)(&view);
    }

    /// Nothing to show yet: the status chip says how to start.
    fn show_setup_hint(&self) {
        if self.prefs.is_none() {
            if let Some(ui) = self.ui.upgrade() {
                ui.set_status_text("Tap to set up".into());
                ui.set_connected(false);
            }
        }
    }

    fn save(&mut self, input: PrefsInput) {
        let prefs = Prefs {
            url: normalise_url(&input.url),
            username: input.username.trim().to_string(),
            method: method_for(&input.token, &input.password),
            token: input.token.trim().to_string(),
            rotation: input.rotation,
        };
        if url::Url::parse(&prefs.url).is_err() || prefs.url.is_empty() {
            self.open_with(
                &prefs,
                "That address is not valid, e.g. http://192.168.1.10:8123",
            );
            return;
        }
        if prefs.method == Method::Password && prefs.username.is_empty() {
            self.open_with(&prefs, "Enter the username that goes with the password.");
            return;
        }
        // A new server or new credentials: forget the old refresh token.
        let changed = self.prefs.as_ref().is_none_or(|old| {
            old.url != prefs.url || old.username != prefs.username || old.method != prefs.method
        });
        if changed || prefs.method == Method::Password {
            let _ = std::fs::remove_file(self.platform.data_dir.join(AUTH_FILE));
        }
        if let Err(e) = prefs.save(&self.platform.data_dir) {
            self.open_with(&prefs, &format!("Cannot save the settings: {e}"));
            return;
        }
        (self.platform.apply_rotation)(prefs.rotation);
        self.prefs = Some(prefs);
        self.connect(&input.password);
        self.pending = true;
    }

    /// Reopen with what was typed (not saved) and an error.
    fn open_with(&self, prefs: &Prefs, error: &str) {
        (self.platform.open_prefs)(&PrefsView {
            prefs: prefs.clone(),
            error: error.to_string(),
            signed_in: self.prefs.is_some(),
        });
    }

    fn connect(&mut self, password: &str) {
        let (Some(ui), Some(prefs)) = (self.ui.upgrade(), self.prefs.clone()) else {
            return;
        };
        if let Some(old) = self.session.take() {
            old.stop();
        }
        let dir = &self.platform.data_dir;
        let (cfg, view) = session_config(&prefs, password, dir, &self.platform.language);
        tracing::info!("connecting to {} ({:?})", cfg.base_url, prefs.method);
        match start_session(&ui, cfg, view, Some(dir.join("cache"))) {
            Ok(s) => self.session = Some(s),
            Err(e) => self.open(&format!("Cannot start: {e}")),
        }
    }

    fn sign_out(&mut self) {
        if let Some(s) = self.session.take() {
            s.stop();
        }
        let dir = &self.platform.data_dir;
        let _ = std::fs::remove_file(dir.join(AUTH_FILE));
        let mut kept = self.prefs.take().unwrap_or_default();
        kept.token.clear();
        kept.method = Method::TrustedNetworks;
        let _ = std::fs::remove_file(dir.join(PREFS_FILE));
        self.show_setup_hint();
        // Keep the address, username and rotation in the form for the next sign-in.
        self.open_with(&kept, "Signed out.");
    }

    fn observe(&mut self, ev: &HaEvent) {
        match ev {
            HaEvent::Connected { .. } => self.pending = false,
            HaEvent::AuthFailed(msg) => {
                self.pending = false;
                if let Some(s) = self.session.take() {
                    s.stop(); // retrying a wrong password helps nobody
                }
                self.open(&format!("Sign-in failed: {msg}"));
            }
            // Right after "Save & connect" the address is probably wrong. At other
            // times (e.g. Wi-Fi not up yet after a reboot) keep retrying quietly.
            HaEvent::Disconnected { reason, .. } if self.pending => {
                self.pending = false;
                if let Some(s) = self.session.take() {
                    s.stop();
                }
                self.open(&format!("Cannot reach Home Assistant: {reason}"));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_methods() {
        assert_eq!(
            normalise_url(" 192.168.1.10:8123/ "),
            "http://192.168.1.10:8123"
        );
        assert_eq!(
            normalise_url("https://ha.example.com"),
            "https://ha.example.com"
        );
        assert_eq!(method_for("", ""), Method::TrustedNetworks);
        assert_eq!(method_for("", "pw"), Method::Password);
        assert_eq!(method_for("tok", "pw"), Method::Token);
        assert_eq!(Rotation::from_index(4), Rotation::LandscapeReverse);
        assert_eq!(Rotation::from_index(9), Rotation::Auto);
        assert_eq!(Rotation::LandscapeReverse.index(), 4);
    }

    #[test]
    fn saves_without_password_and_builds_session() {
        let dir = std::env::temp_dir().join(format!("hada-prefs-{}", std::process::id()));
        let p = Prefs {
            url: "http://10.0.0.5:8123/lovelace/kiosk".into(),
            username: "kiosk".into(),
            method: Method::Password,
            token: String::new(),
            rotation: Rotation::Landscape,
        };
        p.save(&dir).unwrap();
        let text = std::fs::read_to_string(dir.join(PREFS_FILE)).unwrap();
        assert!(!text.contains("password ="), "{text}");
        assert_eq!(Prefs::load(&dir), Some(p.clone()));

        let (cfg, view) = session_config(&p, "s3cret", &dir, "it");
        assert_eq!(cfg.base_url, "http://10.0.0.5:8123");
        assert_eq!(view.as_deref(), Some("kiosk"));
        assert!(
            matches!(cfg.auth, Auth::Password { ref username, ref password }
            if username == "kiosk" && password == "s3cret")
        );
        assert_eq!(cfg.auth_store, Some(dir.join(AUTH_FILE)));
        let trusted = Prefs {
            method: Method::TrustedNetworks,
            username: String::new(),
            ..p
        };
        assert!(matches!(
            trusted.auth(""),
            Auth::TrustedNetwork { user: None }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
