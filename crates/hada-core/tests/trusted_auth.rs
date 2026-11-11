//! The trusted-networks and username/password login flows against a minimal fake of Home Assistant's
//! `/auth/login_flow` and `/auth/token` endpoints (raw HTTP/1.1 over TCP).

use hada_core::ha::auth::{Auth, AuthState};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Default)]
struct Log {
    requests: Vec<(String, String)>, // (path, body)
}

async fn fake_ha(listener: TcpListener, log: Arc<Mutex<Log>>, users: usize) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        // Read until the full body is in (Content-Length).
        loop {
            let n = stream.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if buf.len() >= pos + 4 + len {
                    break;
                }
            }
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let path = head.split_whitespace().nth(1).unwrap().to_string();
        log.lock()
            .unwrap()
            .requests
            .push((path.clone(), body.to_string()));
        let (status, reply) = match path.as_str() {
            "/auth/login_flow" if body.contains("\"homeassistant\"") => {
                let v: Value = serde_json::from_str(body).unwrap();
                assert_eq!(v["handler"], json!(["homeassistant", null]));
                (
                    200,
                    json!({"type": "form", "flow_id": "fp", "step_id": "init", "errors": {},
                        "data_schema": [{"name": "username", "type": "string"}, {"name": "password", "type": "string"}]}),
                )
            }
            "/auth/login_flow/fp" => {
                let v: Value = serde_json::from_str(body).unwrap();
                if v["username"] == "kiosk" && v["password"] == "s3cret" {
                    (
                        200,
                        json!({"type": "create_entry", "flow_id": "fp", "result": "CODE-PW", "version": 1}),
                    )
                } else {
                    (
                        200,
                        json!({"type": "form", "flow_id": "fp", "step_id": "init", "errors": {"base": "invalid_auth"}}),
                    )
                }
            }
            "/auth/login_flow" => {
                let v: Value = serde_json::from_str(body).unwrap();
                assert_eq!(v["handler"], json!(["trusted_networks", null]));
                assert_eq!(v["client_id"], v["redirect_uri"]);
                if users == 1 {
                    (
                        200,
                        json!({"type": "create_entry", "flow_id": "f1", "result": "CODE-ONE", "version": 1}),
                    )
                } else {
                    (
                        200,
                        json!({"type": "form", "flow_id": "f2", "step_id": "init",
                        "data_schema": [{"name": "user", "type": "select", "options": [["u-alex", "Alex"], ["u-sam", "Sam"]]}]}),
                    )
                }
            }
            "/auth/login_flow/f2" => {
                let v: Value = serde_json::from_str(body).unwrap();
                assert_eq!(v["user"], "u-sam");
                (
                    200,
                    json!({"type": "create_entry", "flow_id": "f2", "result": "CODE-TWO", "version": 1}),
                )
            }
            "/auth/token" => {
                let form: std::collections::HashMap<_, _> = body
                    .split('&')
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| {
                        (
                            k.to_string(),
                            v.replace('+', " ").replace("%3A", ":").replace("%2F", "/"),
                        )
                    })
                    .collect();
                match form.get("grant_type").map(String::as_str) {
                    Some("authorization_code") => {
                        assert!(form["code"].starts_with("CODE-"));
                        (
                            200,
                            json!({"access_token": format!("ACCESS-{}", form["code"]), "refresh_token": "REFRESH-1", "expires_in": 1800, "token_type": "Bearer"}),
                        )
                    }
                    Some("refresh_token") => {
                        if form["refresh_token"] == "REFRESH-1" {
                            (
                                200,
                                json!({"access_token": "ACCESS-REFRESHED", "expires_in": 1800, "token_type": "Bearer"}),
                            )
                        } else {
                            (400, json!({"error": "invalid_grant"}))
                        }
                    }
                    _ => (400, json!({"error": "unsupported_grant_type"})),
                }
            }
            _ => (404, json!({"message": "not found"})),
        };
        let body = reply.to_string();
        let resp = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).await.unwrap();
        stream.shutdown().await.ok();
    }
}

#[tokio::test]
async fn single_user_bypass_then_refresh() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Log::default()));
    tokio::spawn(fake_ha(listener, log.clone(), 1));

    let dir = std::env::temp_dir().join(format!("hada-auth-test-{}", std::process::id()));
    let store = dir.join("auth.json");
    let _ = std::fs::remove_file(&store);

    let mut auth = AuthState::new(
        Auth::TrustedNetwork { user: None },
        &base,
        Some(store.clone()),
    );
    assert!(auth.is_trusted_network());
    let t = auth.access_token(&base).await.unwrap();
    assert_eq!(t, "ACCESS-CODE-ONE");
    // Cached while valid.
    assert_eq!(auth.access_token(&base).await.unwrap(), "ACCESS-CODE-ONE");
    assert!(store.exists(), "refresh token persisted");

    // A new process: the saved refresh token is used, no login flow.
    let mut again = AuthState::new(
        Auth::TrustedNetwork { user: None },
        &base,
        Some(store.clone()),
    );
    assert_eq!(again.access_token(&base).await.unwrap(), "ACCESS-REFRESHED");
    let paths: Vec<String> = log
        .lock()
        .unwrap()
        .requests
        .iter()
        .map(|(p, _)| p.clone())
        .collect();
    assert_eq!(
        paths,
        vec!["/auth/login_flow", "/auth/token", "/auth/token"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn picks_requested_user_among_several() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Log::default()));
    tokio::spawn(fake_ha(listener, log.clone(), 2));
    let mut auth = AuthState::new(
        Auth::TrustedNetwork {
            user: Some("sam".into()),
        },
        &base,
        None,
    );
    assert_eq!(auth.access_token(&base).await.unwrap(), "ACCESS-CODE-TWO");
    let paths: Vec<String> = log
        .lock()
        .unwrap()
        .requests
        .iter()
        .map(|(p, _)| p.clone())
        .collect();
    assert_eq!(
        paths,
        vec!["/auth/login_flow", "/auth/login_flow/f2", "/auth/token"]
    );
}

#[tokio::test]
async fn long_lived_token_needs_no_network() {
    let mut auth = AuthState::new(Auth::Token("LLAT".into()), "http://127.0.0.1:9", None);
    assert_eq!(
        auth.access_token("http://127.0.0.1:9").await.unwrap(),
        "LLAT"
    );
}

#[tokio::test]
async fn username_and_password() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Log::default()));
    tokio::spawn(fake_ha(listener, log.clone(), 1));
    let dir = std::env::temp_dir().join(format!("hada-pw-test-{}", std::process::id()));
    let store = dir.join("auth.json");
    let _ = std::fs::remove_file(&store);

    let mut wrong = AuthState::new(
        Auth::Password {
            username: "kiosk".into(),
            password: "nope".into(),
        },
        &base,
        Some(store.clone()),
    );
    let err = wrong.access_token(&base).await.unwrap_err().to_string();
    assert!(err.contains("wrong username or password"), "{err}");
    assert!(!store.exists());

    let mut auth = AuthState::new(
        Auth::Password {
            username: "kiosk".into(),
            password: "s3cret".into(),
        },
        &base,
        Some(store.clone()),
    );
    assert!(!auth.is_trusted_network());
    assert_eq!(auth.access_token(&base).await.unwrap(), "ACCESS-CODE-PW");
    let saved = std::fs::read_to_string(&store).unwrap();
    assert!(saved.contains("REFRESH-1"));
    assert!(!saved.contains("s3cret"), "the password is never stored");
    let _ = std::fs::remove_dir_all(&dir);
}
