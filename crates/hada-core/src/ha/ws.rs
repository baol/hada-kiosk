//! Home Assistant websocket session driver.
//!
//! [`run`] keeps a connection alive for the lifetime of the app: it
//! authenticates, loads the dashboard (unless a local one was given), fetches
//! the icon resources, subscribes to the referenced entities and then forwards
//! state events to the UI while executing service calls coming back from it.
//! On any failure it reconnects with exponential backoff.

use crate::actions::ServiceCall;
use crate::ha::auth::{Auth, AuthState};
use crate::ha::messages::{auth_message, command, Incoming, WsError};
use crate::icons::IconResources;
use crate::lovelace::LovelaceConfig;
use crate::model::Dashboard;
use crate::normalise;
use crate::state::EntitiesEvent;
use crate::translations::Translations;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{interval, timeout, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PING_INTERVAL: Duration = Duration::from_secs(30);
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const AUTH_FAIL_BACKOFF: Duration = Duration::from_secs(60);
const CALENDAR_REFRESH: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
pub struct HaConfig {
    /// `http://host:8123` or `https://…` (also accepts `ws://`/`wss://`).
    pub base_url: String,
    pub auth: Auth,
    /// Where to persist the refresh token obtained through the login flow.
    pub auth_store: Option<std::path::PathBuf>,
    /// Language for `frontend/get_translations`, e.g. `it`.
    pub language: String,
    pub source: DashboardSource,
}

#[derive(Debug, Clone)]
pub enum DashboardSource {
    /// Fetch with `lovelace/config`; `None` = the default dashboard.
    Remote { url_path: Option<String> },
    /// Already loaded from a local YAML file.
    Local(Dashboard),
}

#[derive(Debug, Clone)]
pub enum HaEvent {
    Connecting {
        attempt: u32,
    },
    Connected {
        ha_version: String,
    },
    /// The dashboard to render (sent once per successful connection).
    Dashboard(Dashboard),
    Icons(IconResources),
    /// State translations for the configured language.
    Translations(Translations),
    Entities(EntitiesEvent),
    /// Upcoming events of one calendar entity (`calendar.get_events`).
    Calendar {
        entity_id: String,
        events: Vec<CalendarEvent>,
    },
    CallFailed {
        call: ServiceCall,
        error: String,
    },
    Disconnected {
        reason: String,
        retry_in: Duration,
    },
    /// Token rejected; retrying slowly.
    AuthFailed(String),
}

/// One event as returned by the `calendar.get_events` service response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct CalendarEvent {
    /// `YYYY-MM-DD` for all-day events, otherwise an ISO date-time with offset.
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}

#[derive(Debug, Clone)]
pub enum HaCommand {
    CallService(ServiceCall),
    /// Drop the connection and start over (re-fetches the dashboard).
    Reload,
    Shutdown,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("invalid Home Assistant URL: {0}")]
    Url(String),
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("connection timed out")]
    Timeout,
    #[error("authentication rejected: {0}")]
    Auth(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("websocket error: {0}")]
    Ws(String),
    #[error("{0}")]
    Server(WsError),
    #[error("dashboard config invalid: {0}")]
    Dashboard(String),
    #[error("connection closed")]
    Closed,
}

enum SessionEnd {
    Shutdown,
    Reconnect,
}

/// Turn the configured base URL into the websocket endpoint.
pub fn websocket_url(base: &str) -> Result<String, SessionError> {
    let mut u = url::Url::parse(base.trim()).map_err(|e| SessionError::Url(e.to_string()))?;
    let scheme = match u.scheme() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        other => return Err(SessionError::Url(format!("unsupported scheme {other}"))),
    };
    u.set_scheme(scheme)
        .map_err(|_| SessionError::Url("cannot set scheme".into()))?;
    let path = u.path().trim_end_matches('/');
    let new_path = if path.ends_with("/api/websocket") {
        path.to_string()
    } else {
        format!("{path}/api/websocket")
    };
    u.set_path(&new_path);
    u.set_query(None);
    u.set_fragment(None);
    Ok(u.to_string())
}

/// Split a Home Assistant address into (base URL, dashboard url_path, view path).
///
/// `http://host:8123` → base only; `http://host:8123/lovelace/kiosk` → default
/// dashboard (`lovelace`) and view `kiosk`; `http://host:8123/dashboard-x/1` →
/// dashboard `dashboard-x`, view `1`. `/api/websocket` is ignored.
pub fn split_dashboard_url(input: &str) -> (String, Option<String>, Option<String>) {
    let Ok(mut u) = url::Url::parse(input.trim()) else {
        return (input.trim().to_string(), None, None);
    };
    let segments: Vec<String> = u
        .path_segments()
        .map(|p| p.filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    u.set_path("/");
    u.set_query(None);
    u.set_fragment(None);
    let base = u.to_string().trim_end_matches('/').to_string();
    if segments.first().map(String::as_str) == Some("api") || segments.is_empty() {
        return (base, None, None);
    }
    let dashboard = match segments[0].as_str() {
        "lovelace" => None,
        other => Some(other.to_string()),
    };
    let view = segments.get(1).cloned();
    (base, dashboard, view)
}

type Tx = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
type Rx = SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

struct Conn {
    tx: Tx,
    rx: Rx,
    next_id: u64,
    /// Events received while waiting for a result; replayed afterwards.
    stray_events: Vec<Incoming>,
}

impl Conn {
    async fn send(&mut self, kind: &str, fields: Value) -> Result<u64, SessionError> {
        let id = self.next_id;
        self.next_id += 1;
        self.tx
            .send(Message::text(command(id, kind, fields)))
            .await
            .map_err(|e| SessionError::Ws(e.to_string()))?;
        Ok(id)
    }

    async fn next_message(&mut self) -> Result<Incoming, SessionError> {
        loop {
            match self.rx.next().await {
                None => return Err(SessionError::Closed),
                Some(Err(e)) => return Err(SessionError::Ws(e.to_string())),
                Some(Ok(Message::Text(t))) => {
                    return serde_json::from_str::<Incoming>(t.as_str())
                        .map_err(|e| SessionError::Protocol(format!("bad JSON from server: {e}")));
                }
                Some(Ok(Message::Close(_))) => return Err(SessionError::Closed),
                // Binary frames are not used by HA; ping/pong are handled by tungstenite.
                Some(Ok(_)) => continue,
            }
        }
    }

    /// Send a command and wait for its `result`. Events seen meanwhile are kept.
    async fn request(&mut self, kind: &str, fields: Value) -> Result<Value, SessionError> {
        let id = self.send(kind, fields).await?;
        let fut = async {
            loop {
                let msg = self.next_message().await?;
                match msg.kind.as_str() {
                    "result" if msg.id == Some(id) => {
                        return if msg.success.unwrap_or(false) {
                            Ok(msg.result.unwrap_or(Value::Null))
                        } else {
                            Err(SessionError::Server(msg.error.unwrap_or(WsError {
                                code: "unknown_error".into(),
                                message: "request failed".into(),
                            })))
                        };
                    }
                    "event" => self.stray_events.push(msg),
                    _ => {}
                }
            }
        };
        timeout(REQUEST_TIMEOUT, fut)
            .await
            .map_err(|_| SessionError::Timeout)?
    }
}

/// Drive the connection forever (until [`HaCommand::Shutdown`]).
pub async fn run<F>(cfg: HaConfig, mut cmds: mpsc::UnboundedReceiver<HaCommand>, emit: F)
where
    F: Fn(HaEvent) + Send + 'static,
{
    let mut backoff = MIN_BACKOFF;
    let mut attempt: u32 = 0;
    let mut auth = AuthState::new(cfg.auth.clone(), &cfg.base_url, cfg.auth_store.clone());
    loop {
        attempt += 1;
        emit(HaEvent::Connecting { attempt });
        let outcome = session(&cfg, &mut auth, &mut cmds, &emit).await;
        let wait = match outcome {
            Ok(SessionEnd::Shutdown) => return,
            Ok(SessionEnd::Reconnect) => {
                backoff = MIN_BACKOFF;
                MIN_BACKOFF
            }
            Err(SessionError::Auth(msg)) => {
                tracing::error!("authentication failed: {msg}");
                emit(HaEvent::AuthFailed(msg));
                // With the trusted-networks flow a fresh token is fetched on the next
                // attempt, so retry soon; a rejected long-lived token will not fix itself.
                if auth.is_trusted_network() {
                    auth.invalidate();
                    Duration::from_secs(5)
                } else {
                    AUTH_FAIL_BACKOFF
                }
            }
            Err(e) => {
                tracing::warn!("connection lost: {e}; retrying in {backoff:?}");
                emit(HaEvent::Disconnected {
                    reason: e.to_string(),
                    retry_in: backoff,
                });
                let w = backoff;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                w
            }
        };
        // Wait, but keep draining commands so a Shutdown is honoured promptly.
        let sleep = tokio::time::sleep(wait);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                cmd = cmds.recv() => match cmd {
                    None | Some(HaCommand::Shutdown) => return,
                    Some(HaCommand::Reload) => break,
                    Some(HaCommand::CallService(call)) => emit(HaEvent::CallFailed {
                        call,
                        error: "not connected".into(),
                    }),
                },
            }
        }
    }
}

async fn session<F>(
    cfg: &HaConfig,
    auth: &mut AuthState,
    cmds: &mut mpsc::UnboundedReceiver<HaCommand>,
    emit: &F,
) -> Result<SessionEnd, SessionError>
where
    F: Fn(HaEvent),
{
    let url = websocket_url(&cfg.base_url)?;
    let token = auth
        .access_token(&cfg.base_url)
        .await
        .map_err(|e| SessionError::Auth(e.to_string()))?;
    tracing::info!("connecting to {url}");
    let (stream, _) = timeout(CONNECT_TIMEOUT, connect_async(url))
        .await
        .map_err(|_| SessionError::Timeout)?
        .map_err(|e| SessionError::Connect(e.to_string()))?;
    let (tx, rx) = stream.split();
    let mut conn = Conn {
        tx,
        rx,
        next_id: 1,
        stray_events: Vec::new(),
    };

    // --- auth -------------------------------------------------------------
    let first = timeout(CONNECT_TIMEOUT, conn.next_message())
        .await
        .map_err(|_| SessionError::Timeout)??;
    if first.kind != "auth_required" {
        return Err(SessionError::Protocol(format!(
            "expected auth_required, got {}",
            first.kind
        )));
    }
    conn.tx
        .send(Message::text(auth_message(&token)))
        .await
        .map_err(|e| SessionError::Ws(e.to_string()))?;
    let resp = timeout(CONNECT_TIMEOUT, conn.next_message())
        .await
        .map_err(|_| SessionError::Timeout)??;
    match resp.kind.as_str() {
        "auth_ok" => {}
        "auth_invalid" => {
            return Err(SessionError::Auth(
                resp.message.unwrap_or_else(|| "invalid token".into()),
            ))
        }
        other => {
            return Err(SessionError::Protocol(format!(
                "expected auth_ok, got {other}"
            )))
        }
    }
    let ha_version = resp
        .ha_version
        .clone()
        .or(first.ha_version)
        .unwrap_or_default();
    tracing::info!("authenticated with Home Assistant {ha_version}");
    emit(HaEvent::Connected { ha_version });

    // --- dashboard --------------------------------------------------------
    let dashboard = match &cfg.source {
        DashboardSource::Local(d) => d.clone(),
        DashboardSource::Remote { url_path } => {
            let mut fields = json!({ "force": false });
            if let Some(p) = url_path {
                fields["url_path"] = json!(p);
            }
            let raw = conn.request("lovelace/config", fields).await?;
            let cfg = LovelaceConfig::from_json_value(raw)
                .map_err(|e| SessionError::Dashboard(e.to_string()))?;
            normalise::normalise(&cfg)
        }
    };
    let ids: Vec<String> = dashboard
        .entity_ids()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let calendars: Vec<(String, u32)> = dashboard
        .calendar_entities()
        .iter()
        .map(|(k, v)| (k.to_string(), *v))
        .collect();
    emit(HaEvent::Dashboard(dashboard));

    // --- icons (best effort) ---------------------------------------------
    match conn
        .request(
            "frontend/get_icons",
            json!({ "category": "entity_component" }),
        )
        .await
    {
        Ok(v) => match serde_json::from_value::<IconResources>(v) {
            Ok(r) => emit(HaEvent::Icons(r)),
            Err(e) => tracing::warn!("cannot parse icon resources: {e}"),
        },
        Err(e) => tracing::warn!("frontend/get_icons failed: {e}"),
    }

    // --- translations (best effort) --------------------------------------
    match conn
        .request(
            "frontend/get_translations",
            json!({ "language": cfg.language, "category": "entity_component" }),
        )
        .await
    {
        Ok(v) => match serde_json::from_value::<Translations>(v) {
            Ok(t) => emit(HaEvent::Translations(t)),
            Err(e) => tracing::warn!("cannot parse translations: {e}"),
        },
        Err(e) => tracing::warn!("frontend/get_translations failed: {e}"),
    }

    // --- subscribe ----------------------------------------------------------
    let sub_id = conn.next_id;
    conn.request("subscribe_entities", json!({ "entity_ids": ids }))
        .await?;
    for ev in std::mem::take(&mut conn.stray_events) {
        handle_event(&ev, sub_id, emit);
    }

    // --- main loop ----------------------------------------------------------
    let mut ping = interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ping.tick().await; // first tick fires immediately; skip it
    let mut awaiting_pong: Option<u64> = None;
    let mut pending_calls: HashMap<u64, ServiceCall> = HashMap::new();
    let mut pending_calendars: HashMap<u64, String> = HashMap::new();
    let mut calendar_tick = interval(CALENDAR_REFRESH);
    calendar_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            msg = conn.next_message() => {
                let msg = msg?;
                match msg.kind.as_str() {
                    "event" => handle_event(&msg, sub_id, emit),
                    "pong" => awaiting_pong = None,
                    "result" => {
                        if let Some(entity_id) = msg.id.and_then(|id| pending_calendars.remove(&id)) {
                            handle_calendar_result(entity_id, &msg, emit);
                        } else if let Some(call) = msg.id.and_then(|id| pending_calls.remove(&id)) {
                            if !msg.success.unwrap_or(false) {
                                let error = msg.error.map(|e| e.to_string()).unwrap_or_else(|| "unknown error".into());
                                tracing::warn!("service call {}.{} failed: {error}", call.domain, call.service);
                                emit(HaEvent::CallFailed { call, error });
                            }
                        }
                    }
                    other => tracing::debug!("ignoring message type {other}"),
                }
            }
            cmd = cmds.recv() => match cmd {
                None | Some(HaCommand::Shutdown) => {
                    let _ = conn.tx.send(Message::Close(None)).await;
                    return Ok(SessionEnd::Shutdown);
                }
                Some(HaCommand::Reload) => {
                    let _ = conn.tx.send(Message::Close(None)).await;
                    return Ok(SessionEnd::Reconnect);
                }
                Some(HaCommand::CallService(call)) => {
                    let fields = serde_json::to_value(&call).unwrap_or(Value::Null);
                    let id = conn.send("call_service", fields).await?;
                    pending_calls.insert(id, call);
                }
            },
            _ = ping.tick() => {
                if awaiting_pong.is_some() {
                    return Err(SessionError::Protocol("ping timeout".into()));
                }
                awaiting_pong = Some(conn.send("ping", Value::Null).await?);
            }
            // Fires immediately on the first tick, then every CALENDAR_REFRESH.
            _ = calendar_tick.tick() => {
                for (entity_id, days) in &calendars {
                    let fields = json!({
                        "domain": "calendar",
                        "service": "get_events",
                        "service_data": { "duration": { "days": days } },
                        "target": { "entity_id": entity_id },
                        "return_response": true,
                    });
                    let id = conn.send("call_service", fields).await?;
                    pending_calendars.insert(id, entity_id.clone());
                }
            }
        }
    }
}

fn handle_calendar_result<F: Fn(HaEvent)>(entity_id: String, msg: &Incoming, emit: &F) {
    if !msg.success.unwrap_or(false) {
        tracing::warn!(
            "calendar.get_events for {entity_id} failed: {:?}",
            msg.error
        );
        return;
    }
    // {"response": {"calendar.x": {"events": [...]}}}
    let events = msg
        .result
        .as_ref()
        .and_then(|r| r.get("response"))
        .and_then(|r| r.get(&entity_id))
        .and_then(|r| r.get("events"))
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    match serde_json::from_value::<Vec<CalendarEvent>>(events) {
        Ok(mut events) => {
            events.sort_by(|a, b| a.start.cmp(&b.start));
            emit(HaEvent::Calendar { entity_id, events });
        }
        Err(e) => tracing::warn!("cannot parse calendar events for {entity_id}: {e}"),
    }
}

fn handle_event<F: Fn(HaEvent)>(msg: &Incoming, sub_id: u64, emit: &F) {
    if msg.id != Some(sub_id) {
        return;
    }
    let Some(ev) = &msg.event else { return };
    match serde_json::from_value::<EntitiesEvent>(ev.clone()) {
        Ok(e) => emit(HaEvent::Entities(e)),
        Err(e) => tracing::warn!("cannot parse entities event: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_mapping() {
        assert_eq!(
            websocket_url("http://ha.local:8123").unwrap(),
            "ws://ha.local:8123/api/websocket"
        );
        assert_eq!(
            websocket_url("https://ha.example.com/").unwrap(),
            "wss://ha.example.com/api/websocket"
        );
        assert_eq!(
            websocket_url("ws://1.2.3.4:8123/api/websocket").unwrap(),
            "ws://1.2.3.4:8123/api/websocket"
        );
        assert_eq!(
            websocket_url("https://host/prefix").unwrap(),
            "wss://host/prefix/api/websocket"
        );
        assert!(websocket_url("ftp://x").is_err());
        assert!(websocket_url("not a url").is_err());
    }

    #[test]
    fn dashboard_urls() {
        assert_eq!(
            split_dashboard_url("http://localhost:8123"),
            ("http://localhost:8123".into(), None, None)
        );
        assert_eq!(
            split_dashboard_url("http://localhost:8123/lovelace/kiosk"),
            ("http://localhost:8123".into(), None, Some("kiosk".into()))
        );
        assert_eq!(
            split_dashboard_url("https://ha.example.com/dashboard-kiosk/0?edit=1"),
            (
                "https://ha.example.com".into(),
                Some("dashboard-kiosk".into()),
                Some("0".into())
            )
        );
        assert_eq!(
            split_dashboard_url("ws://1.2.3.4:8123/api/websocket"),
            ("ws://1.2.3.4:8123".into(), None, None)
        );
        assert_eq!(
            split_dashboard_url("http://localhost:8123/lovelace"),
            ("http://localhost:8123".into(), None, None)
        );
    }
}
