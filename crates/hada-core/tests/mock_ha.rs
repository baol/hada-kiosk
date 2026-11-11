//! Integration test: a mock Home Assistant websocket server speaking the real
//! protocol (auth handshake, `lovelace/config`, `frontend/get_icons`,
//! `subscribe_entities` with the compressed state format, `call_service`,
//! `weather/subscribe_forecast`).

use futures_util::{SinkExt, StreamExt};
use hada_core::actions::{Action, EntityAction};
use hada_core::ha::{run, Auth, DashboardSource, HaCommand, HaConfig, HaEvent};
use hada_core::state::StateStore;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const DASHBOARD_YAML: &str = include_str!("../../../tests/fixtures/dashboard.yaml");
const ICONS_JSON: &str = include_str!("../../../tests/fixtures/icons.json");

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::text(v.to_string())).await.unwrap();
}

/// Runs one mock HA connection; returns the recorded `call_service` messages.
async fn mock_server(listener: TcpListener, token: &'static str) -> Vec<Value> {
    let (stream, _) = listener.accept().await.unwrap();
    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
    send(
        &mut ws,
        json!({"type": "auth_required", "ha_version": "2026.2.3"}),
    )
    .await;
    let auth: Value =
        serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(auth["type"], "auth");
    if auth["access_token"] != token {
        send(
            &mut ws,
            json!({"type": "auth_invalid", "message": "Invalid access token or password"}),
        )
        .await;
        return vec![];
    }
    send(
        &mut ws,
        json!({"type": "auth_ok", "ha_version": "2026.2.3"}),
    )
    .await;

    let dashboard_json: Value = serde_yaml_ng::from_str(DASHBOARD_YAML).unwrap();
    let icons: Value = serde_json::from_str(ICONS_JSON).unwrap();
    let mut calls = Vec::new();
    while let Some(Ok(msg)) = ws.next().await {
        let Message::Text(t) = msg else { continue };
        let m: Value = serde_json::from_str(t.as_str()).unwrap();
        let id = m["id"].as_u64().unwrap();
        match m["type"].as_str().unwrap() {
            "lovelace/config" => {
                assert!(
                    m.get("url_path").is_none(),
                    "default dashboard has no url_path"
                );
                send(
                    &mut ws,
                    json!({"id": id, "type": "result", "success": true, "result": dashboard_json}),
                )
                .await;
            }
            "frontend/get_translations" => {
                assert_eq!(m["language"], "it");
                assert_eq!(m["category"], "entity_component");
                send(&mut ws, json!({"id": id, "type": "result", "success": true, "result": {"resources": {"component.switch.entity_component._.state.off": "Spento"}}})).await;
            }
            "frontend/get_icons" => {
                assert_eq!(m["category"], "entity_component");
                send(
                    &mut ws,
                    json!({"id": id, "type": "result", "success": true, "result": icons}),
                )
                .await;
            }
            "subscribe_entities" => {
                let ids = m["entity_ids"].as_array().unwrap();
                assert_eq!(
                    ids.len(),
                    63,
                    "subscribes exactly to the dashboard's entities"
                );
                assert!(ids.contains(&json!("switch.table_lamp")));
                send(
                    &mut ws,
                    json!({"id": id, "type": "result", "success": true, "result": null}),
                )
                .await;
                send(
                    &mut ws,
                    json!({"id": id, "type": "event", "event": {"a": {
                        "switch.table_lamp": {"s": "off", "a": {"friendly_name": "Table lamp"}, "c": "01ABC", "lc": 1700000000.0},
                        "sensor.living_room_temperature": {"s": "21.4", "a": {"unit_of_measurement": "°C", "device_class": "temperature"}, "c": "01ABD", "lc": 1700000000.0, "lu": 1700000100.0}
                    }}}),
                )
                .await;
                // A later state change in the compressed diff format.
                send(
                    &mut ws,
                    json!({"id": id, "type": "event", "event": {"c": {
                        "switch.table_lamp": {"+": {"s": "on", "lc": 1700000200.0}}
                    }}}),
                )
                .await;
            }
            "call_service" if m["domain"] == "calendar" => {
                assert_eq!(m["service"], "get_events");
                assert_eq!(m["return_response"], true);
                assert_eq!(m["service_data"]["duration"]["days"], 7);
                assert_eq!(m["target"]["entity_id"], "calendar.family");
                send(&mut ws, json!({"id": id, "type": "result", "success": true, "result": {"context": {"id": "c"}, "response": {
                    "calendar.family": {"events": [
                        {"start": "2025-11-11T18:30:00+01:00", "end": "2025-11-11T19:30:00+01:00", "summary": "Dentist", "location": "Main Street 1"},
                        {"start": "2025-11-10", "end": "2025-11-11", "summary": "Public holiday"}
                    ]}}}})).await;
            }
            "call_service" => {
                calls.push(m.clone());
                if calls.len() == 2 {
                    // Second call: pretend the service does not exist.
                    send(&mut ws, json!({"id": id, "type": "result", "success": false, "error": {"code": "not_found", "message": "Service x.y not found."}})).await;
                } else {
                    send(&mut ws, json!({"id": id, "type": "result", "success": true, "result": {"context": {"id": "x"}}})).await;
                }
            }
            // Like a weather entity with only an hourly forecast (supported_features = 2).
            "weather/subscribe_forecast" => {
                assert_eq!(m["entity_id"], "weather.home");
                if m["forecast_type"] != "hourly" {
                    send(&mut ws, json!({"id": id, "type": "result", "success": false, "error": {
                        "code": "forecast_not_supported",
                        "message": format!("The weather entity does not support forecast type: {}", m["forecast_type"].as_str().unwrap())}})).await;
                } else {
                    send(
                        &mut ws,
                        json!({"id": id, "type": "result", "success": true, "result": null}),
                    )
                    .await;
                    send(&mut ws, json!({"id": id, "type": "event", "event": {"type": "hourly", "forecast": [
                        {"datetime": "2025-11-10T13:00:00+00:00", "condition": "rainy", "temperature": 12.5, "precipitation_probability": 60},
                        {"datetime": "2025-11-10T14:00:00+00:00", "condition": "cloudy", "temperature": 12.0}
                    ]}})).await;
                }
            }
            "ping" => send(&mut ws, json!({"id": id, "type": "pong"})).await,
            other => panic!("unexpected command {other}"),
        }
    }
    calls
}

async fn recv_until<F: FnMut(&HaEvent) -> bool>(
    rx: &mut mpsc::UnboundedReceiver<HaEvent>,
    mut pred: F,
) -> HaEvent {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for event")
            .expect("event channel closed");
        if pred(&ev) {
            return ev;
        }
    }
}

#[tokio::test]
async fn full_session_against_mock_ha() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(mock_server(listener, "secret-token"));

    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<HaEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<HaCommand>();
    let cfg = HaConfig {
        base_url: format!("http://{addr}"),
        auth: Auth::Token("secret-token".into()),
        auth_store: None,
        language: "it".into(),
        source: DashboardSource::Remote { url_path: None },
    };
    let client = tokio::spawn(run(cfg, cmd_rx, move |e| {
        let _ = ev_tx.send(e);
    }));

    let connected = recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Connected { .. })).await;
    match connected {
        HaEvent::Connected { ha_version } => assert_eq!(ha_version, "2026.2.3"),
        _ => unreachable!(),
    }
    let dashboard = match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Dashboard(_))).await {
        HaEvent::Dashboard(d) => d,
        _ => unreachable!(),
    };
    assert_eq!(dashboard.views.len(), 4);
    match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Icons(_))).await {
        HaEvent::Icons(r) => assert!(r.resources.contains_key("switch")),
        _ => unreachable!(),
    }
    match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Translations(_))).await {
        HaEvent::Translations(t) => assert_eq!(t.state("switch", None, "off"), Some("Spento")),
        _ => unreachable!(),
    }

    let mut store = StateStore::new();
    let first = match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Entities(_))).await {
        HaEvent::Entities(e) => e,
        _ => unreachable!(),
    };
    store.apply(first);
    assert_eq!(store.get("switch.table_lamp").unwrap().state, "off");
    assert_eq!(
        store
            .get("sensor.living_room_temperature")
            .unwrap()
            .last_updated,
        1_700_000_100.0
    );
    let second = match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Entities(_))).await {
        HaEvent::Entities(e) => e,
        _ => unreachable!(),
    };
    let changed = store.apply(second);
    assert_eq!(&*changed[0], "switch.table_lamp");
    assert_eq!(store.get("switch.table_lamp").unwrap().state, "on");

    // The calendar fetch is issued right after the subscription.
    match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Calendar { .. })).await {
        HaEvent::Calendar { entity_id, events } => {
            assert_eq!(entity_id, "calendar.family");
            assert_eq!(events.len(), 2);
            assert_eq!(
                events[0].summary, "Public holiday",
                "events are sorted by start"
            );
            assert_eq!(events[1].location.as_deref(), Some("Main Street 1"));
        }
        _ => unreachable!(),
    }

    // Daily is refused, so the client falls back to the hourly forecast.
    match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Forecast { .. })).await {
        HaEvent::Forecast {
            entity_id,
            forecast_type,
            items,
        } => {
            assert_eq!(entity_id, "weather.home");
            assert_eq!(forecast_type, hada_core::weather::ForecastType::Hourly);
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].condition.as_deref(), Some("rainy"));
            assert_eq!(items[0].precipitation_probability, Some(60.0));
        }
        _ => unreachable!(),
    }

    // Album art: downloaded beside the websocket, decoded, sent back as an event.
    let art_server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let art_url = format!(
        "http://{}/api/media_player_proxy/media_player.av_receiver?token=t&cache=1",
        art_server.local_addr().unwrap()
    );
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut s, _) = art_server.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let n = s.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n])
            .starts_with("GET /api/media_player_proxy/media_player.av_receiver?token=t&cache=1 "));
        let img = image::RgbaImage::from_pixel(600, 600, image::Rgba([200, 30, 30, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
            png.len()
        );
        s.write_all(head.as_bytes()).await.unwrap();
        s.write_all(&png).await.unwrap();
    });
    cmd_tx
        .send(HaCommand::FetchArtwork {
            entity_id: "media_player.av_receiver".into(),
            url: art_url.clone(),
        })
        .await_free();
    match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Artwork { .. })).await {
        HaEvent::Artwork {
            entity_id,
            url,
            image,
        } => {
            assert_eq!(entity_id, "media_player.av_receiver");
            assert_eq!(url, art_url);
            let a = image.expect("decoded artwork");
            assert_eq!((a.width, a.height), (384, 384), "shrunk to MAX_SIDE");
            assert_eq!(&a.rgba[..4], &[200, 30, 30, 255]);
        }
        _ => unreachable!(),
    }

    // UI → HA: a toggle becomes switch.toggle targeted at the entity.
    let call = EntityAction::new("switch.table_lamp", Action::Toggle)
        .to_service_call()
        .unwrap();
    cmd_tx.send(HaCommand::CallService(call)).await_free();
    // A failing call is reported back.
    let bad = EntityAction::new("cover.kids_room_blind", Action::SetCoverPosition(50))
        .to_service_call()
        .unwrap();
    cmd_tx.send(HaCommand::CallService(bad)).await_free();
    match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::CallFailed { .. })).await {
        HaEvent::CallFailed { call, error } => {
            assert_eq!(call.service, "set_cover_position");
            assert!(error.contains("not_found"), "{error}");
        }
        _ => unreachable!(),
    }

    cmd_tx.send(HaCommand::Shutdown).await_free();
    tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .unwrap()
        .unwrap();
    let calls = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["domain"], "switch");
    assert_eq!(calls[0]["service"], "toggle");
    assert_eq!(calls[0]["target"]["entity_id"], "switch.table_lamp");
    assert_eq!(calls[1]["service_data"]["position"], 50);
}

#[tokio::test]
async fn invalid_token_is_reported() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(mock_server(listener, "right-token"));
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<HaEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<HaCommand>();
    let cfg = HaConfig {
        base_url: format!("http://{addr}"),
        auth: Auth::Token("wrong".into()),
        auth_store: None,
        language: "it".into(),
        source: DashboardSource::Remote { url_path: None },
    };
    let client = tokio::spawn(run(cfg, cmd_rx, move |e| {
        let _ = ev_tx.send(e);
    }));
    match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::AuthFailed(_))).await {
        HaEvent::AuthFailed(msg) => assert!(msg.contains("Invalid access token")),
        _ => unreachable!(),
    }
    cmd_tx.send(HaCommand::Shutdown).await_free();
    tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn unreachable_server_reports_disconnect_with_backoff() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<HaEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<HaCommand>();
    let cfg = HaConfig {
        base_url: format!("http://{addr}"),
        auth: Auth::Token("t".into()),
        auth_store: None,
        language: "it".into(),
        source: DashboardSource::Remote { url_path: None },
    };
    let client = tokio::spawn(run(cfg, cmd_rx, move |e| {
        let _ = ev_tx.send(e);
    }));
    match recv_until(&mut ev_rx, |e| matches!(e, HaEvent::Disconnected { .. })).await {
        HaEvent::Disconnected { retry_in, .. } => assert_eq!(retry_in, Duration::from_secs(1)),
        _ => unreachable!(),
    }
    cmd_tx.send(HaCommand::Shutdown).await_free();
    tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .unwrap()
        .unwrap();
}

/// Tiny helper so `send(...)` results read naturally in the test body.
trait AwaitFree {
    fn await_free(self);
}
impl<T, E: std::fmt::Debug> AwaitFree for Result<T, E> {
    fn await_free(self) {
        self.expect("command channel closed");
    }
}
