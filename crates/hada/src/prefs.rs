//! Preferences page of the Android app: what is saved, how it turns into a
//! Home Assistant session, and the page's callbacks. Platform-specific parts
//! (where files live, how rotation is applied) come in through [`Platform`].
//! The Linux binary does not use this module.

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

/// What the app needs from the device.
pub struct Platform {
    /// Private folder for the preferences, the refresh token and caches.
    pub data_dir: PathBuf,
    /// Language for state texts, e.g. `it`.
    pub language: String,
    /// Lock the screen orientation (or let it follow the sensor).
    pub apply_rotation: Box<dyn Fn(Rotation)>,
}

struct Controller {
    ui: slint::Weak<AppWindow>,
    platform: Platform,
    prefs: Option<Prefs>,
    session: Option<Session>,
    /// Waiting for the first answer after "Save & connect".
    pending: bool,
    connected_once: bool,
}

/// Wire the preferences page and start: connect with the saved preferences, or show
/// the page when there are none.
pub fn run_with_prefs(ui: &AppWindow, platform: Platform) {
    let prefs = Prefs::load(&platform.data_dir).filter(Prefs::is_complete);
    (platform.apply_rotation)(prefs.as_ref().map(|p| p.rotation).unwrap_or_default());
    ui.set_version(crate::VERSION.into());
    let ctl = Rc::new(RefCell::new(Controller {
        ui: ui.as_weak(),
        platform,
        prefs,
        session: None,
        pending: false,
        connected_once: false,
    }));

    let c = ctl.clone();
    ui.on_status_clicked(move || c.borrow_mut().open_page(""));
    let c = ctl.clone();
    ui.on_prefs_save(move || c.borrow_mut().save());
    let c = ctl.clone();
    ui.on_prefs_cancel(move || c.borrow_mut().cancel());
    let c = ctl.clone();
    ui.on_prefs_sign_out(move || c.borrow_mut().sign_out());
    let c = Rc::downgrade(&ctl);
    crate::set_event_observer(move |ev| {
        if let Some(c) = c.upgrade() {
            c.borrow_mut().observe(ev);
        }
    });

    let mut c = ctl.borrow_mut();
    if c.prefs.is_some() {
        c.connect("");
    } else {
        c.open_page("");
    }
}

impl Controller {
    fn open_page(&mut self, error: &str) {
        let Some(ui) = self.ui.upgrade() else { return };
        let p = self.prefs.clone().unwrap_or_default();
        ui.set_prefs_url(p.url.as_str().into());
        ui.set_prefs_username(p.username.as_str().into());
        ui.set_prefs_password("".into());
        ui.set_prefs_token(p.token.as_str().into());
        ui.set_prefs_rotation(p.rotation.index());
        ui.set_prefs_error(error.into());
        ui.set_prefs_busy(false);
        ui.set_prefs_signed_in(self.prefs.is_some());
        ui.set_show_prefs(true);
    }

    fn save(&mut self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let password = ui.get_prefs_password().to_string();
        let token = ui.get_prefs_token().to_string();
        let prefs = Prefs {
            url: normalise_url(&ui.get_prefs_url()),
            username: ui.get_prefs_username().trim().to_string(),
            method: method_for(&token, &password),
            token: token.trim().to_string(),
            rotation: Rotation::from_index(ui.get_prefs_rotation()),
        };
        if url::Url::parse(&prefs.url).is_err() || prefs.url.is_empty() {
            ui.set_prefs_error("That address is not valid, e.g. http://192.168.1.10:8123".into());
            return;
        }
        if prefs.method == Method::Password && prefs.username.is_empty() {
            ui.set_prefs_error("Enter the username that goes with the password.".into());
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
            ui.set_prefs_error(format!("Cannot save the settings: {e}").into());
            return;
        }
        (self.platform.apply_rotation)(prefs.rotation);
        self.prefs = Some(prefs);
        ui.set_prefs_password("".into());
        ui.set_prefs_error("".into());
        ui.set_prefs_busy(true);
        self.connect(&password);
        self.pending = true;
    }

    fn connect(&mut self, password: &str) {
        let (Some(ui), Some(prefs)) = (self.ui.upgrade(), self.prefs.clone()) else {
            return;
        };
        if let Some(old) = self.session.take() {
            old.stop();
        }
        self.connected_once = false;
        let dir = &self.platform.data_dir;
        let (cfg, view) = session_config(&prefs, password, dir, &self.platform.language);
        tracing::info!("connecting to {} ({:?})", cfg.base_url, prefs.method);
        match start_session(&ui, cfg, view, Some(dir.join("cache"))) {
            Ok(s) => self.session = Some(s),
            Err(e) => self.open_page(&format!("Cannot start: {e}")),
        }
    }

    fn cancel(&mut self) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_show_prefs(false);
        }
    }

    fn sign_out(&mut self) {
        if let Some(s) = self.session.take() {
            s.stop();
        }
        let dir = &self.platform.data_dir;
        let _ = std::fs::remove_file(dir.join(AUTH_FILE));
        if let Some(p) = &mut self.prefs {
            p.token.clear();
            p.method = Method::TrustedNetworks;
            let _ = p.save(dir);
        }
        let keep = self.prefs.take();
        self.open_page("Signed out.");
        // Keep the address and rotation in the fields for the next sign-in.
        if let (Some(ui), Some(p)) = (self.ui.upgrade(), keep) {
            ui.set_prefs_url(p.url.as_str().into());
            ui.set_prefs_username(p.username.as_str().into());
            ui.set_prefs_rotation(p.rotation.index());
            self.prefs = None;
        }
    }

    fn observe(&mut self, ev: &HaEvent) {
        let Some(ui) = self.ui.upgrade() else { return };
        match ev {
            HaEvent::Connected { .. } => {
                self.connected_once = true;
                if self.pending {
                    self.pending = false;
                    ui.set_prefs_busy(false);
                    ui.set_show_prefs(false);
                }
            }
            HaEvent::AuthFailed(msg) => {
                self.pending = false;
                if let Some(s) = self.session.take() {
                    s.stop(); // retrying a wrong password helps nobody
                }
                self.open_page(&format!("Sign-in failed: {msg}"));
            }
            HaEvent::Disconnected { reason, .. } if !self.connected_once => {
                // Never reached: show why, while the session keeps retrying.
                self.pending = false;
                if !ui.get_show_prefs() || ui.get_prefs_busy() {
                    self.open_page(&format!("Cannot reach Home Assistant: {reason}"));
                } else {
                    ui.set_prefs_error(format!("Cannot reach Home Assistant: {reason}").into());
                }
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
