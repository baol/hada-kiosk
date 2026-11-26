//! Integration test: the dashboard cache and `lovelace_updated`, against a mock Home
//! Assistant that serves several connections and lets the test edit its dashboard.

use futures_util::{SinkExt, StreamExt};
use hada_core::dashboard_cache::{self, cache_key};
use hada_core::ha::{run, Auth, DashboardSource, HaCommand, HaConfig, HaEvent};
use hada_core::Dashboard;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

/// What the mock serves and what it was asked.
struct Mock {
    /// The `lovelace/config` result.
    config: Mutex<Value>,
    /// Every command received, with the number of the connection it came on.
    log: Mutex<Vec<(usize, Value)>>,
    /// `url_path` values of `lovelace_updated` events to fire on the open connection.
    edits: tokio::sync::Mutex<mpsc::UnboundedReceiver<Option<String>>>,
}

impl Mock {
    fn commands(&self, conn: usize, kind: &str) -> Vec<Value> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, m)| *c == conn && m["type"] == kind)
            .map(|(_, m)| m.clone())
            .collect()
    }
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::text(v.to_string())).await.unwrap();
}

async fn serve(listener: TcpListener, mock: Arc<Mock>) {
    let mut conn = 0;
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        connection(&mut ws, &mock, conn).await;
        conn += 1;
    }
}

async fn connection(ws: &mut Ws, mock: &Mock, conn: usize) {
    send(
        ws,
        json!({"type": "auth_required", "ha_version": "2026.2.3"}),
    )
    .await;
    let _auth = ws.next().await;
    send(ws, json!({"type": "auth_ok", "ha_version": "2026.2.3"})).await;
    let mut edits = mock.edits.lock().await;
    let mut lovelace_sub: Option<u64> = None;
    loop {
        tokio::select! {
            msg = ws.next() => {
                let Some(Ok(msg)) = msg else { return };
                let Message::Text(t) = msg else { continue };
                let m: Value = serde_json::from_str(t.as_str()).unwrap();
                mock.log.lock().unwrap().push((conn, m.clone()));
                let id = m["id"].as_u64().unwrap();
                let ok = |result: Value| json!({"id": id, "type": "result", "success": true, "result": result});
                match m["type"].as_str().unwrap() {
                    "lovelace/config" => {
                        let config = mock.config.lock().unwrap().clone();
                        send(ws, ok(config)).await;
                    }
                    "frontend/get_icons" | "frontend/get_translations" => {
                        send(ws, ok(json!({"resources": {}}))).await;
                    }
                    "subscribe_entities" => {
                        send(ws, ok(Value::Null)).await;
                        send(ws, json!({"id": id, "type": "event", "event": {"a": {
                            "switch.a": {"s": "on", "a": {}, "c": "01ABC", "lc": 1700000000.0}
                        }}})).await;
                    }
                    "subscribe_events" => {
                        lovelace_sub = Some(id);
                        send(ws, ok(Value::Null)).await;
                    }
                    "ping" => send(ws, json!({"id": id, "type": "pong"})).await,
                    other => panic!("unexpected command {other}"),
                }
            }
            Some(url_path) = edits.recv() => {
                let id = lovelace_sub.expect("edit fired before subscribe_events");
                // As fired by homeassistant/components/lovelace/dashboard.py.
                send(ws, json!({"id": id, "type": "event", "event": {
                    "event_type": "lovelace_updated",
                    "data": {"url_path": url_path},
                    "origin": "LOCAL",
                    "time_fired": "2026-11-26T10:00:00.000000+00:00",
                    "context": {"id": "01ABD", "parent_id": null, "user_id": null}
                }})).await;
            }
        }
    }
}

fn dashboard_json(entities: &[&str]) -> Value {
    let cards: Vec<Value> = entities
        .iter()
        .map(|e| json!({"type": "tile", "entity": e}))
        .collect();
    json!({"views": [{"title": "Home", "cards": cards}]})
}

async fn next_event(rx: &mut mpsc::UnboundedReceiver<HaEvent>) -> HaEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for event")
        .expect("event channel closed")
}

/// Events up to and including the first `Entities`, i.e. the end of a connect.
async fn until_entities(rx: &mut mpsc::UnboundedReceiver<HaEvent>) -> Vec<HaEvent> {
    let mut seen = Vec::new();
    loop {
        let ev = next_event(rx).await;
        let done = matches!(ev, HaEvent::Entities(_));
        seen.push(ev);
        if done {
            return seen;
        }
    }
}

async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..500 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

fn dashboards(events: &[HaEvent]) -> Vec<&Dashboard> {
    events
        .iter()
        .filter_map(|e| match e {
            HaEvent::Dashboard(d) => Some(d),
            _ => None,
        })
        .collect()
}

fn subscribed(mock: &Mock, conn: usize) -> Vec<String> {
    let subs = mock.commands(conn, "subscribe_entities");
    assert_eq!(subs.len(), 1, "one subscribe_entities per connection");
    let mut ids: Vec<String> = subs[0]["entity_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

fn config(base_url: &str, cache: &Path, cached: Option<dashboard_cache::Stored>) -> HaConfig {
    HaConfig {
        base_url: base_url.to_string(),
        auth: Auth::Token("token".into()),
        auth_store: None,
        language: "en".into(),
        source: DashboardSource::Remote { url_path: None },
        cached,
        dashboard_cache: Some(cache.to_path_buf()),
    }
}

#[tokio::test]
async fn cache_update_check_and_lovelace_updated() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (edit_tx, edit_rx) = mpsc::unbounded_channel();
    let mock = Arc::new(Mock {
        config: Mutex::new(dashboard_json(&["switch.a"])),
        log: Mutex::new(Vec::new()),
        edits: tokio::sync::Mutex::new(edit_rx),
    });
    tokio::spawn(serve(listener, mock.clone()));
    let dir = std::env::temp_dir().join(format!("hada-updates-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cache = dir.join("dashboard.bin");
    let _ = std::fs::remove_file(&cache);
    let key = cache_key(&base_url, None);

    // 1. No cache: the dashboard is fetched, shown and written to the cache.
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<HaEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<HaCommand>();
    let client = tokio::spawn(run(config(&base_url, &cache, None), cmd_rx, move |e| {
        let _ = ev_tx.send(e);
    }));
    let events = until_entities(&mut ev_rx).await;
    let first = dashboards(&events);
    assert_eq!(first.len(), 1);
    let first = first[0].clone();
    assert_eq!(subscribed(&mock, 0), ["switch.a"]);
    wait_for("subscribe_events", || {
        !mock.commands(0, "subscribe_events").is_empty()
    })
    .await;
    assert_eq!(
        mock.commands(0, "subscribe_events")[0]["event_type"],
        "lovelace_updated"
    );
    let stored = dashboard_cache::load(&cache, &key).expect("cache written");
    assert_eq!(stored.dashboard, first);
    assert!(dashboard_cache::load(&cache, &cache_key("http://other:8123", None)).is_none());
    cmd_tx.send(HaCommand::Shutdown).unwrap();
    client.await.unwrap();

    // 2. Start from the cache: the check on connect finds the same config, so no
    //    Dashboard event, but the same entities are subscribed.
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<HaEvent>();
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<HaCommand>();
    let client = tokio::spawn(run(
        config(&base_url, &cache, Some(stored)),
        cmd_rx,
        move |e| {
            let _ = ev_tx.send(e);
        },
    ));
    let events = until_entities(&mut ev_rx).await;
    assert!(dashboards(&events).is_empty(), "unchanged: nothing rebuilt");
    assert_eq!(mock.commands(1, "lovelace/config").len(), 1);
    assert_eq!(subscribed(&mock, 1), ["switch.a"]);
    wait_for("subscribe_events", || {
        !mock.commands(1, "subscribe_events").is_empty()
    })
    .await;

    // 3. An edit of another dashboard is ignored.
    edit_tx.send(Some("other".into())).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mock.commands(1, "lovelace/config").len(), 1);

    // 4. An edit of ours that changed nothing: fetched, compared, nothing else.
    edit_tx.send(None).unwrap();
    wait_for("refetch", || mock.commands(1, "lovelace/config").len() == 2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut quiet = Vec::new();
    while let Ok(ev) = ev_rx.try_recv() {
        quiet.push(ev);
    }
    assert!(
        !quiet
            .iter()
            .any(|e| matches!(e, HaEvent::Dashboard(_) | HaEvent::Connecting { .. })),
        "unchanged edit: no rebuild, no reconnect: {quiet:?}"
    );

    // 5. A real edit: new dashboard shown and cached, then a reconnect subscribes to
    //    the new entities without fetching-and-rebuilding twice.
    *mock.config.lock().unwrap() = dashboard_json(&["switch.a", "light.b"]);
    edit_tx.send(None).unwrap();
    let second = loop {
        if let HaEvent::Dashboard(d) = next_event(&mut ev_rx).await {
            break d;
        }
    };
    assert_ne!(second, first);
    loop {
        if let HaEvent::Connecting { attempt } = next_event(&mut ev_rx).await {
            assert_eq!(attempt, 2);
            break;
        }
    }
    let events = until_entities(&mut ev_rx).await;
    assert!(
        dashboards(&events).is_empty(),
        "already shown before reconnect"
    );
    assert_eq!(subscribed(&mock, 2), ["light.b", "switch.a"]);
    assert_eq!(
        dashboard_cache::load(&cache, &key).unwrap().dashboard,
        second
    );
    cmd_tx.send(HaCommand::Shutdown).unwrap();
    client.await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
