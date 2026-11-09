//! Access tokens for the websocket: either a long-lived token given by the
//! user, or tokens obtained through Home Assistant's login flow with the
//! `trusted_networks` auth provider (`POST /auth/login_flow`, `POST /auth/token`),
//! kept alive with the refresh token like the frontend does.

use crate::ha::http::{self, form_encode, HttpError};
use crate::timefmt::now_unix;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

/// Refresh the access token this many seconds before it expires.
const EXPIRY_MARGIN: f64 = 120.0;

#[derive(Debug, Clone)]
pub enum Auth {
    /// A long-lived access token.
    Token(String),
    /// Log in through the trusted-networks provider; `user` selects the Home
    /// Assistant user (name or id) when the provider offers several.
    TrustedNetwork { user: Option<String> },
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("{0}")]
    Http(#[from] HttpError),
    #[error("login flow failed: {0}")]
    Flow(String),
    #[error("token request failed ({status}): {body}")]
    Token { status: u16, body: String },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    client_id: String,
    refresh_token: String,
}

/// Token state for one Home Assistant instance.
#[derive(Debug)]
pub struct AuthState {
    auth: Auth,
    client_id: String,
    access_token: Option<String>,
    expires_at: f64,
    refresh_token: Option<String>,
    store_path: Option<PathBuf>,
}

impl AuthState {
    pub fn new(auth: Auth, base_url: &str, store_path: Option<PathBuf>) -> Self {
        let client_id = client_id_for(base_url);
        let mut s = AuthState {
            auth,
            client_id,
            access_token: None,
            expires_at: 0.0,
            refresh_token: None,
            store_path,
        };
        if let Some(p) = &s.store_path {
            if let Ok(text) = std::fs::read_to_string(p) {
                if let Ok(st) = serde_json::from_str::<Stored>(&text) {
                    if st.client_id == s.client_id && !st.refresh_token.is_empty() {
                        tracing::info!("using saved refresh token from {}", p.display());
                        s.refresh_token = Some(st.refresh_token);
                    }
                }
            }
        }
        s
    }

    pub fn is_trusted_network(&self) -> bool {
        matches!(self.auth, Auth::TrustedNetwork { .. })
    }

    /// A token good for connecting now.
    pub async fn access_token(&mut self, base_url: &str) -> Result<String, AuthError> {
        if let Auth::Token(t) = &self.auth {
            return Ok(t.clone());
        }
        if let Some(t) = &self.access_token {
            if self.expires_at - EXPIRY_MARGIN > now_unix() {
                return Ok(t.clone());
            }
        }
        if let Some(rt) = self.refresh_token.clone() {
            match self
                .token_request(
                    base_url,
                    &[
                        ("grant_type", "refresh_token"),
                        ("refresh_token", &rt),
                        ("client_id", &self.client_id.clone()),
                    ],
                )
                .await
            {
                Ok(t) => return Ok(t),
                Err(e) => {
                    tracing::warn!("refresh token rejected ({e}); logging in again");
                    self.refresh_token = None;
                    self.persist();
                }
            }
        }
        let code = self.login_flow(base_url).await?;
        self.token_request(
            base_url,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("client_id", &self.client_id.clone()),
            ],
        )
        .await
    }

    /// Called when the websocket rejected the token.
    pub fn invalidate(&mut self) {
        self.access_token = None;
        self.expires_at = 0.0;
    }

    async fn login_flow(&self, base_url: &str) -> Result<String, AuthError> {
        let Auth::TrustedNetwork { user } = &self.auth else {
            unreachable!("token auth has no login flow")
        };
        let body = json!({
            "client_id": self.client_id,
            "handler": ["trusted_networks", null],
            "redirect_uri": self.client_id,
        });
        let resp = http::post(
            base_url,
            "/auth/login_flow",
            "application/json",
            &body.to_string(),
        )
        .await?;
        let mut result = resp
            .json()
            .ok_or_else(|| AuthError::Flow(format!("HTTP {}: {}", resp.status, resp.body)))?;
        if resp.status >= 400 {
            return Err(AuthError::Flow(format!(
                "HTTP {}: {}",
                resp.status, resp.body
            )));
        }
        if result["type"] == "form" {
            // Several users are allowed from this network: choose one.
            let flow_id = result["flow_id"]
                .as_str()
                .ok_or_else(|| AuthError::Flow("missing flow_id".into()))?
                .to_string();
            let options = user_options(&result);
            if options.is_empty() {
                return Err(AuthError::Flow(format!(
                    "trusted_networks offered no users: {}",
                    result
                )));
            }
            let chosen =
                match user {
                    Some(wanted) => options
                        .iter()
                        .find(|(id, name)| id == wanted || name.eq_ignore_ascii_case(wanted))
                        .ok_or_else(|| {
                            AuthError::Flow(format!(
                                "user {wanted:?} not available; choices: {}",
                                options
                                    .iter()
                                    .map(|(_, n)| n.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ))
                        })?,
                    None => {
                        if options.len() > 1 {
                            tracing::warn!(
                            "several users allowed, picking {:?}; use --user to choose among: {}",
                            options[0].1,
                            options.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ")
                        );
                        }
                        &options[0]
                    }
                };
            tracing::info!("logging in as {}", chosen.1);
            let body = json!({ "client_id": self.client_id, "user": chosen.0 });
            let resp = http::post(
                base_url,
                &format!("/auth/login_flow/{flow_id}"),
                "application/json",
                &body.to_string(),
            )
            .await?;
            result = resp
                .json()
                .ok_or_else(|| AuthError::Flow(format!("HTTP {}: {}", resp.status, resp.body)))?;
        }
        match result["type"].as_str() {
            Some("create_entry") => result["result"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| AuthError::Flow("missing authorization code".into())),
            Some("abort") => Err(AuthError::Flow(format!(
                "aborted: {}",
                result["reason"].as_str().unwrap_or("unknown reason")
            ))),
            Some("form") => Err(AuthError::Flow(format!(
                "unexpected form: {}",
                result["errors"]
            ))),
            _ => Err(AuthError::Flow(format!("unexpected response: {result}"))),
        }
    }

    async fn token_request(
        &mut self,
        base_url: &str,
        fields: &[(&str, &str)],
    ) -> Result<String, AuthError> {
        let resp = http::post(
            base_url,
            "/auth/token",
            "application/x-www-form-urlencoded",
            &form_encode(fields),
        )
        .await?;
        let v = resp.json().unwrap_or(Value::Null);
        let token = match v["access_token"].as_str() {
            Some(t) if resp.status < 400 => t.to_string(),
            _ => {
                return Err(AuthError::Token {
                    status: resp.status,
                    body: resp.body,
                })
            }
        };
        self.access_token = Some(token.clone());
        self.expires_at = now_unix() + v["expires_in"].as_f64().unwrap_or(1800.0);
        if let Some(rt) = v["refresh_token"].as_str() {
            self.refresh_token = Some(rt.to_string());
            self.persist();
        }
        Ok(token)
    }

    fn persist(&self) {
        let Some(p) = &self.store_path else { return };
        match &self.refresh_token {
            Some(rt) => {
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let st = Stored {
                    client_id: self.client_id.clone(),
                    refresh_token: rt.clone(),
                };
                if let Err(e) = std::fs::write(p, serde_json::to_string(&st).unwrap_or_default()) {
                    tracing::warn!("cannot save refresh token to {}: {e}", p.display());
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
                }
            }
            None => {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

/// IndieAuth-style client id: the Home Assistant URL with a trailing slash,
/// exactly what the frontend uses, so it passes `verify_client_id`.
pub fn client_id_for(base_url: &str) -> String {
    match url::Url::parse(base_url.trim()) {
        Ok(mut u) => {
            let scheme = match u.scheme() {
                "ws" => "http",
                "wss" => "https",
                s => s,
            }
            .to_string();
            let _ = u.set_scheme(&scheme);
            u.set_path("/");
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => format!("{}/", base_url.trim_end_matches('/')),
    }
}

/// `(user_id, name)` pairs from the serialised `vol.In` schema of the form step.
fn user_options(result: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(fields) = result["data_schema"].as_array() {
        for f in fields {
            if f["name"] != "user" {
                continue;
            }
            match &f["options"] {
                Value::Array(list) => {
                    for o in list {
                        match o {
                            Value::Array(pair) if pair.len() == 2 => out.push((
                                pair[0].as_str().unwrap_or_default().to_string(),
                                pair[1].as_str().unwrap_or_default().to_string(),
                            )),
                            Value::String(s) => out.push((s.clone(), s.clone())),
                            _ => {}
                        }
                    }
                }
                Value::Object(map) => {
                    for (k, v) in map {
                        out.push((k.clone(), v.as_str().unwrap_or(k).to_string()));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids() {
        assert_eq!(
            client_id_for("http://localhost:8123"),
            "http://localhost:8123/"
        );
        assert_eq!(
            client_id_for("https://ha.example.com/"),
            "https://ha.example.com/"
        );
        assert_eq!(
            client_id_for("ws://10.0.0.5:8123/api/websocket"),
            "http://10.0.0.5:8123/"
        );
    }

    #[test]
    fn parses_user_options() {
        let v = json!({"type": "form", "data_schema": [{"name": "user", "type": "select", "options": [["abc", "Alex"], ["def", "Sam"]]}]});
        assert_eq!(
            user_options(&v),
            vec![
                ("abc".to_string(), "Alex".to_string()),
                ("def".to_string(), "Sam".to_string())
            ]
        );
        let v = json!({"type": "form", "data_schema": [{"name": "user", "type": "select", "options": {"abc": "Alex"}}]});
        assert_eq!(
            user_options(&v),
            vec![("abc".to_string(), "Alex".to_string())]
        );
    }
}
