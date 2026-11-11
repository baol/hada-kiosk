//! User actions and their mapping to Home Assistant service calls.

use crate::domain::domain_of;
use crate::model::ActionSpec;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Toggle,
    TurnOn,
    TurnOff,
    /// Light brightness in percent (0–100).
    SetBrightnessPct(u8),
    /// Fan speed percentage.
    SetPercentage(u8),
    OpenCover,
    CloseCover,
    StopCover,
    SetCoverPosition(u8),
    SetCoverTilt(u8),
    /// `button.press`.
    Press,
    /// `script.turn_on` / `scene.turn_on`.
    Run,
    /// `update.install`.
    Install,
    /// `number.set_value` / `input_number.set_value`.
    SetValue(f64),
    /// `climate.set_temperature`.
    SetTemperature(f64),
    /// `media_player.media_play_pause`.
    MediaPlayPause,
    /// `media_player.media_previous_track`.
    MediaPrevious,
    /// `media_player.media_next_track`.
    MediaNext,
    /// `media_player.volume_set`, 0.0–1.0.
    VolumeSet(f64),
    /// `media_player.volume_mute`.
    VolumeMute(bool),
    /// Explicit `perform-action` from the dashboard config.
    Perform {
        domain: String,
        service: String,
        data: Value,
        target: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityAction {
    pub entity_id: Arc<str>,
    pub action: Action,
}

/// A `call_service` websocket command payload (without `id`/`type`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServiceCall {
    pub domain: String,
    pub service: String,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub service_data: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
}

impl ServiceCall {
    pub fn new(domain: &str, service: &str, entity_id: &str) -> Self {
        ServiceCall {
            domain: domain.into(),
            service: service.into(),
            service_data: Map::new(),
            target: Some(json!({ "entity_id": entity_id })),
        }
    }

    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.service_data.insert(key.into(), value);
        self
    }

    pub fn for_entities(domain: &str, service: &str, ids: &[Arc<str>]) -> Self {
        ServiceCall {
            domain: domain.into(),
            service: service.into(),
            service_data: Map::new(),
            target: Some(
                json!({ "entity_id": ids.iter().map(|s| s.as_ref()).collect::<Vec<_>>() }),
            ),
        }
    }
}

impl EntityAction {
    pub fn new(entity_id: &str, action: Action) -> Self {
        EntityAction {
            entity_id: entity_id.into(),
            action,
        }
    }

    pub fn from_spec(entity_id: &str, spec: &ActionSpec) -> Option<Self> {
        match spec {
            ActionSpec::Toggle => Some(Self::new(entity_id, Action::Toggle)),
            ActionSpec::PerformAction {
                domain,
                service,
                data,
                target,
            } => Some(Self::new(
                entity_id,
                Action::Perform {
                    domain: domain.clone(),
                    service: service.clone(),
                    data: data.clone(),
                    target: target.clone(),
                },
            )),
            _ => None,
        }
    }

    /// Map to a service call. `None` when the action makes no sense for the domain.
    pub fn to_service_call(&self) -> Option<ServiceCall> {
        let id = &*self.entity_id;
        let domain = domain_of(id);
        let call = match &self.action {
            Action::Toggle => match domain {
                "script" | "scene" => ServiceCall::new(domain, "turn_on", id),
                "button" => ServiceCall::new("button", "press", id),
                "cover" => ServiceCall::new("cover", "toggle", id),
                "lock" => return None,
                "switch" | "light" | "fan" | "input_boolean" | "automation" | "media_player"
                | "humidifier" | "siren" | "remote" | "climate" | "vacuum" => {
                    ServiceCall::new(domain, "toggle", id)
                }
                _ => ServiceCall::new("homeassistant", "toggle", id),
            },
            Action::TurnOn => ServiceCall::new(service_domain(domain), "turn_on", id),
            Action::TurnOff => ServiceCall::new(service_domain(domain), "turn_off", id),
            Action::SetBrightnessPct(p) => {
                if *p == 0 {
                    ServiceCall::new("light", "turn_off", id)
                } else {
                    ServiceCall::new("light", "turn_on", id)
                        .with("brightness_pct", json!(p.min(&100)))
                }
            }
            Action::SetPercentage(p) => {
                if *p == 0 {
                    ServiceCall::new("fan", "turn_off", id)
                } else {
                    ServiceCall::new("fan", "set_percentage", id)
                        .with("percentage", json!(p.min(&100)))
                }
            }
            Action::OpenCover => ServiceCall::new("cover", "open_cover", id),
            Action::CloseCover => ServiceCall::new("cover", "close_cover", id),
            Action::StopCover => ServiceCall::new("cover", "stop_cover", id),
            Action::SetCoverPosition(p) => ServiceCall::new("cover", "set_cover_position", id)
                .with("position", json!(p.min(&100))),
            Action::SetCoverTilt(p) => ServiceCall::new("cover", "set_cover_tilt_position", id)
                .with("tilt_position", json!(p.min(&100))),
            Action::Press => ServiceCall::new("button", "press", id),
            Action::Run => ServiceCall::new(
                if domain == "scene" { "scene" } else { "script" },
                "turn_on",
                id,
            ),
            Action::Install => ServiceCall::new("update", "install", id),
            Action::SetValue(v) => {
                ServiceCall::new(service_domain(domain), "set_value", id).with("value", json!(v))
            }
            Action::SetTemperature(t) => {
                ServiceCall::new("climate", "set_temperature", id).with("temperature", json!(t))
            }
            Action::MediaPlayPause => ServiceCall::new("media_player", "media_play_pause", id),
            Action::MediaPrevious => ServiceCall::new("media_player", "media_previous_track", id),
            Action::MediaNext => ServiceCall::new("media_player", "media_next_track", id),
            Action::VolumeSet(v) => ServiceCall::new("media_player", "volume_set", id).with(
                "volume_level",
                json!((v.clamp(0.0, 1.0) * 100.0).round() / 100.0),
            ),
            Action::VolumeMute(m) => ServiceCall::new("media_player", "volume_mute", id)
                .with("is_volume_muted", json!(m)),
            Action::Perform {
                domain,
                service,
                data,
                target,
            } => {
                let service_data = match data {
                    Value::Object(m) => m.clone(),
                    _ => Map::new(),
                };
                let target = match target {
                    Value::Null => Some(json!({ "entity_id": id })),
                    other => Some(other.clone()),
                };
                ServiceCall {
                    domain: domain.clone(),
                    service: service.clone(),
                    service_data,
                    target,
                }
            }
        };
        Some(call)
    }
}

fn service_domain(domain: &str) -> &str {
    match domain {
        "number" | "input_number" | "switch" | "light" | "fan" | "input_boolean" | "automation"
        | "media_player" | "humidifier" | "siren" | "remote" | "climate" | "vacuum" | "script"
        | "scene" => domain,
        _ => "homeassistant",
    }
}

/// The header toggle of an `entities` card: turn every toggleable row on or off.
pub fn header_toggle(ids: &[Arc<str>], on: bool) -> ServiceCall {
    ServiceCall::for_entities(
        "homeassistant",
        if on { "turn_on" } else { "turn_off" },
        ids,
    )
}

#[cfg(test)]
mod media_tests {
    use super::*;

    #[test]
    fn media_services() {
        let call = |a| {
            EntityAction::new("media_player.m", a)
                .to_service_call()
                .unwrap()
        };
        assert_eq!(call(Action::MediaPlayPause).service, "media_play_pause");
        assert_eq!(call(Action::MediaPrevious).service, "media_previous_track");
        assert_eq!(call(Action::MediaNext).service, "media_next_track");
        let v = call(Action::VolumeSet(0.456));
        assert_eq!(
            (v.domain.as_str(), v.service.as_str()),
            ("media_player", "volume_set")
        );
        assert_eq!(v.service_data["volume_level"], json!(0.46));
        assert_eq!(
            call(Action::VolumeSet(3.0)).service_data["volume_level"],
            json!(1.0)
        );
        let m = call(Action::VolumeMute(true));
        assert_eq!(m.service, "volume_mute");
        assert_eq!(m.service_data["is_volume_muted"], json!(true));
        assert_eq!(m.target, Some(json!({"entity_id": "media_player.m"})));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, a: Action) -> ServiceCall {
        EntityAction::new(id, a).to_service_call().unwrap()
    }

    #[test]
    fn toggle_mapping() {
        let c = call("switch.garden_lights", Action::Toggle);
        assert_eq!(
            (c.domain.as_str(), c.service.as_str()),
            ("switch", "toggle")
        );
        assert_eq!(c.target, Some(json!({"entity_id": "switch.garden_lights"})));
        let c = call("script.play_radio", Action::Toggle);
        assert_eq!(
            (c.domain.as_str(), c.service.as_str()),
            ("script", "turn_on")
        );
        let c = call("button.router_reboot", Action::Toggle);
        assert_eq!((c.domain.as_str(), c.service.as_str()), ("button", "press"));
        let c = call("update.nas_os", Action::Install);
        assert_eq!(
            (c.domain.as_str(), c.service.as_str()),
            ("update", "install")
        );
        let c = call("lock.x", Action::TurnOn);
        assert_eq!(c.domain, "homeassistant");
    }

    #[test]
    fn sliders() {
        let c = call("light.l", Action::SetBrightnessPct(40));
        assert_eq!(c.service, "turn_on");
        assert_eq!(c.service_data.get("brightness_pct"), Some(&json!(40)));
        let c = call("light.l", Action::SetBrightnessPct(0));
        assert_eq!(c.service, "turn_off");
        let c = call("cover.kids_room_blind", Action::SetCoverPosition(130));
        assert_eq!(c.service, "set_cover_position");
        assert_eq!(c.service_data.get("position"), Some(&json!(100)));
        let c = call("fan.air_purifier", Action::SetPercentage(66));
        assert_eq!(c.service, "set_percentage");
        let c = call("number.n", Action::SetValue(3.5));
        assert_eq!(
            (c.domain.as_str(), c.service.as_str()),
            ("number", "set_value")
        );
    }

    #[test]
    fn header_and_perform() {
        let ids: Vec<Arc<str>> = vec!["switch.a".into(), "switch.b".into()];
        let c = header_toggle(&ids, true);
        assert_eq!(
            (c.domain.as_str(), c.service.as_str()),
            ("homeassistant", "turn_on")
        );
        assert_eq!(
            c.target,
            Some(json!({"entity_id": ["switch.a", "switch.b"]}))
        );
        let spec = ActionSpec::PerformAction {
            domain: "light".into(),
            service: "turn_on".into(),
            data: json!({"brightness_pct": 10}),
            target: Value::Null,
        };
        let c = EntityAction::from_spec("light.l", &spec)
            .unwrap()
            .to_service_call()
            .unwrap();
        assert_eq!(c.service_data.get("brightness_pct"), Some(&json!(10)));
        assert_eq!(c.target, Some(json!({"entity_id": "light.l"})));
        let s = serde_json::to_value(&c).unwrap();
        assert_eq!(s["domain"], "light");
        assert!(s.get("service_data").is_some());
    }
}
