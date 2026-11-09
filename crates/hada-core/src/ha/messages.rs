//! Wire types of the Home Assistant websocket API
//! (`homeassistant/components/websocket_api/{auth,messages}.py`).

use serde::Deserialize;
use serde_json::{json, Map, Value};

/// Any message from the server. `type` decides which fields are present.
#[derive(Debug, Clone, Deserialize)]
pub struct Incoming {
    pub id: Option<u64>,
    #[serde(rename = "type")]
    pub kind: String,
    pub success: Option<bool>,
    pub result: Option<Value>,
    pub error: Option<WsError>,
    pub event: Option<Value>,
    /// `auth_invalid` reason.
    pub message: Option<String>,
    pub ha_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct WsError {
    pub code: String,
    pub message: String,
}

pub fn auth_message(token: &str) -> String {
    json!({ "type": "auth", "access_token": token }).to_string()
}

/// `{"id": id, "type": kind, ...fields}`.
pub fn command(id: u64, kind: &str, fields: Value) -> String {
    let mut m = match fields {
        Value::Object(m) => m,
        Value::Null => Map::new(),
        other => {
            let mut m = Map::new();
            m.insert("data".into(), other);
            m
        }
    };
    m.insert("id".into(), json!(id));
    m.insert("type".into(), json!(kind));
    Value::Object(m).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_parses() {
        let s = command(7, "subscribe_entities", json!({"entity_ids": ["switch.a"]}));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["type"], "subscribe_entities");
        assert_eq!(v["entity_ids"][0], "switch.a");

        let inc: Incoming = serde_json::from_str(r#"{"id":1,"type":"result","success":false,"error":{"code":"not_found","message":"nope"}}"#).unwrap();
        assert_eq!(inc.kind, "result");
        assert_eq!(inc.error.unwrap().code, "not_found");
        let inc: Incoming =
            serde_json::from_str(r#"{"type":"auth_required","ha_version":"2026.2.3"}"#).unwrap();
        assert_eq!(inc.id, None);
        assert_eq!(inc.ha_version.as_deref(), Some("2026.2.3"));
    }
}
