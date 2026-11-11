//! hada — native fullscreen Home Assistant dashboard (Linux desktop and kiosk).

#[cfg(feature = "screenshot")]
use hada::{app, headless};
use hada::{AppWindow, VERSION};

use anyhow::{bail, Context};
use clap::Parser;
#[cfg(feature = "screenshot")]
use hada_core::ha::HaEvent;
use hada_core::ha::{self, Auth, DashboardSource, HaConfig};
use hada_core::lovelace::LovelaceConfig;
use hada_core::model::Dashboard;
use hada_core::normalise;
use serde::Deserialize;
use slint::ComponentHandle;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "hada",
    version = VERSION,
    about = "Native fullscreen Home Assistant dashboard"
)]
struct Cli {
    /// Home Assistant URL (default http://localhost:8123). A dashboard URL such as
    /// http://host:8123/lovelace/kiosk also selects the dashboard and the view.
    #[arg(long, env = "HADA_URL")]
    url: Option<String>,
    /// Long-lived access token. Without a token the trusted-networks login flow is used.
    #[arg(long, env = "HADA_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// Read the token from this file instead
    #[arg(long, env = "HADA_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    /// Home Assistant user (name or id) to log in as with trusted networks, when several are allowed
    #[arg(long, env = "HADA_USER")]
    user: Option<String>,
    /// Language for state texts (default from LANG, e.g. `it`)
    #[arg(long, env = "HADA_LANG")]
    lang: Option<String>,
    /// Dashboard: a Lovelace url_path (e.g. `dashboard-kiosk`) or a local .yaml file
    #[arg(long, env = "HADA_DASHBOARD")]
    dashboard: Option<String>,
    /// View to open first (path, title or index)
    #[arg(long, env = "HADA_VIEW")]
    view: Option<String>,
    /// Fullscreen window (the linuxkms backend is always fullscreen)
    #[arg(long, env = "HADA_FULLSCREEN")]
    fullscreen: bool,
    /// UI scale factor (sets SLINT_SCALE_FACTOR)
    #[arg(long)]
    scale: Option<f32>,
    /// Config file (default: $XDG_CONFIG_HOME/hada/config.toml)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Render one frame to this PNG and exit (no display needed; requires the `screenshot` feature)
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// Entity states for --screenshot: JSON in the `subscribe_entities` event format
    #[arg(long)]
    states: Option<PathBuf>,
    /// Icon resources for --screenshot (output of `frontend/get_icons`)
    #[arg(long)]
    icons: Option<PathBuf>,
    /// Calendar events for --screenshot: JSON `{ "calendar.x": [ {start, end, summary, location?} ] }`
    #[arg(long)]
    events: Option<PathBuf>,
    /// State translations for --screenshot (output of `frontend/get_translations`)
    #[arg(long)]
    translations: Option<PathBuf>,
    /// Weather forecasts for --screenshot: JSON `{ "weather.x": {"type": "daily", "forecast": [...]} }`
    #[arg(long)]
    forecast: Option<PathBuf>,
    /// Screenshot size, e.g. 1024x600
    #[arg(long, default_value = "1024x600")]
    size: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FileConfig {
    url: Option<String>,
    token: Option<String>,
    token_file: Option<PathBuf>,
    user: Option<String>,
    lang: Option<String>,
    dashboard: Option<String>,
    view: Option<String>,
    fullscreen: Option<bool>,
    scale: Option<f32>,
}

fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "hada").map(|d| d.config_dir().to_path_buf())
}

fn cache_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "hada").map(|d| d.cache_dir().to_path_buf())
}

fn load_file_config(path: Option<&Path>) -> anyhow::Result<FileConfig> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => match config_dir() {
            Some(d) => d.join("config.toml"),
            None => return Ok(FileConfig::default()),
        },
    };
    if !path.exists() {
        return Ok(FileConfig::default());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn load_local_dashboard(path: &Path) -> anyhow::Result<Dashboard> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let cfg = LovelaceConfig::from_yaml_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(normalise::normalise(&cfg))
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    tracing::info!("hada {VERSION}");
    let file = load_file_config(cli.config.as_deref())?;

    // The URL may be a full dashboard address: http://host:8123/<url_path>/<view>.
    let (url, url_dashboard, url_view) = ha::ws::split_dashboard_url(
        &cli.url
            .or(file.url)
            .unwrap_or_else(|| "http://localhost:8123".to_string()),
    );
    let language = hada_core::translations::normalise_language(
        &cli.lang
            .or(file.lang)
            .or_else(|| std::env::var("LC_ALL").ok().filter(|s| !s.is_empty()))
            .or_else(|| std::env::var("LC_MESSAGES").ok().filter(|s| !s.is_empty()))
            .or_else(|| std::env::var("LANG").ok())
            .unwrap_or_default(),
    );
    let user = cli.user.or(file.user);
    let token = match (cli.token, cli.token_file.or(file.token_file), file.token) {
        (Some(t), _, _) => Some(t),
        (None, Some(p), _) => Some(
            std::fs::read_to_string(&p)
                .with_context(|| format!("reading token file {}", p.display()))?
                .trim()
                .to_string(),
        ),
        (None, None, t) => t,
    };
    let dashboard_arg = cli.dashboard.or(file.dashboard).or(url_dashboard);
    let view = cli.view.or(file.view).or(url_view);
    let fullscreen = cli.fullscreen || file.fullscreen.unwrap_or(false);
    if let Some(s) = cli.scale.or(file.scale) {
        // Safe: nothing else is running yet.
        std::env::set_var("SLINT_SCALE_FACTOR", s.to_string());
    }
    if fullscreen {
        std::env::set_var("SLINT_FULLSCREEN", "1");
    }

    // Local YAML file or remote url_path?
    let source = match &dashboard_arg {
        Some(d) if (d.ends_with(".yaml") || d.ends_with(".yml")) && Path::new(d).exists() => {
            tracing::info!("using local dashboard {d}");
            DashboardSource::Local(load_local_dashboard(Path::new(d))?)
        }
        Some(d) if d.ends_with(".yaml") || d.ends_with(".yml") => {
            bail!("dashboard file {d} not found")
        }
        Some(d) => DashboardSource::Remote {
            url_path: Some(d.clone()),
        },
        None => DashboardSource::Remote { url_path: None },
    };

    if let Some(out) = cli.screenshot {
        let fixtures = Fixtures {
            states: cli.states.as_deref(),
            icons: cli.icons.as_deref(),
            events: cli.events.as_deref(),
            translations: cli.translations.as_deref(),
            forecast: cli.forecast.as_deref(),
        };
        return screenshot(out, &cli.size, source, fixtures, view, &language);
    }

    let auth = match token {
        Some(t) => Auth::Token(t),
        None => {
            tracing::info!("no token given: using the trusted_networks login flow at {url}");
            Auth::TrustedNetwork { user }
        }
    };

    let ui = AppWindow::new()?;
    let cache = cache_dir();
    let cfg = HaConfig {
        base_url: url,
        auth,
        auth_store: cache.as_ref().map(|d| d.join("auth.json")),
        language,
        source,
    };
    let session = hada::start_session(&ui, cfg, view, cache)?;
    ui.run()?;
    session.stop();
    Ok(())
}

/// Files standing in for Home Assistant in --screenshot mode.
#[cfg_attr(not(feature = "screenshot"), allow(dead_code))]
struct Fixtures<'a> {
    states: Option<&'a Path>,
    icons: Option<&'a Path>,
    events: Option<&'a Path>,
    translations: Option<&'a Path>,
    forecast: Option<&'a Path>,
}

#[cfg(feature = "screenshot")]
fn screenshot(
    out: PathBuf,
    size: &str,
    source: DashboardSource,
    fixtures: Fixtures<'_>,
    view: Option<String>,
    language: &str,
) -> anyhow::Result<()> {
    let Fixtures {
        states,
        icons,
        events,
        translations,
        forecast,
    } = fixtures;
    let (w, h) = size
        .split_once('x')
        .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
        .context("--size must look like 1024x600")?;
    let DashboardSource::Local(dashboard) = source else {
        bail!("--screenshot needs a local --dashboard file");
    };
    headless::install(w, h).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let ui = AppWindow::new()?;
    let app = app::App::install(&ui, None, view, None, language);
    {
        let mut a = app.borrow_mut();
        if let Some(p) = translations {
            let text = std::fs::read_to_string(p)?;
            a.translations = Some(hada_core::translations::Translations::from_json_str(&text)?);
        }
        if let Some(p) = icons {
            let text = std::fs::read_to_string(p)?;
            a.icons
                .set_resources(hada_core::icons::IconResources::from_json_str(&text)?);
        }
        // Same order as a live session: dashboard first, then the state snapshot
        // arrives as an event and must reach every row through the update path.
        a.set_dashboard(dashboard);
        a.handle(HaEvent::Connected {
            ha_version: "fixture".into(),
        });
        if let Some(p) = states {
            let text = std::fs::read_to_string(p)?;
            let ev: hada_core::state::EntitiesEvent = serde_json::from_str(&text)?;
            a.handle(HaEvent::Entities(ev));
        }
        if let Some(p) = events {
            let text = std::fs::read_to_string(p)?;
            let all: std::collections::HashMap<String, Vec<hada_core::ha::ws::CalendarEvent>> =
                serde_json::from_str(&text)?;
            for (entity_id, events) in all {
                a.handle(HaEvent::Calendar { entity_id, events });
            }
        }
        if let Some(p) = forecast {
            let text = std::fs::read_to_string(p)?;
            let all: std::collections::HashMap<String, serde_json::Value> =
                serde_json::from_str(&text)?;
            for (entity_id, v) in all {
                let forecast_type = v["type"]
                    .as_str()
                    .and_then(hada_core::weather::ForecastType::parse)
                    .context("forecast fixture: \"type\" must be daily, hourly or twice_daily")?;
                let items = hada_core::weather::parse_forecast(&v["forecast"]);
                a.handle(HaEvent::Forecast {
                    entity_id,
                    forecast_type,
                    items,
                });
            }
        }
    }
    ui.set_status_text("Demo".into());
    headless::render_png(w, h, &out)?;
    tracing::info!("wrote {}", out.display());
    Ok(())
}

#[cfg(not(feature = "screenshot"))]
fn screenshot(
    _: PathBuf,
    _: &str,
    _: DashboardSource,
    _: Fixtures<'_>,
    _: Option<String>,
    _: &str,
) -> anyhow::Result<()> {
    bail!("this binary was built without the `screenshot` feature")
}
