//! Entity state store fed by the `subscribe_entities` compressed state format.
//!
//! Compressed state (from `homeassistant.core.State.as_compressed_state`):
//! `{"s": state, "a": attributes, "c": context, "lc": last_changed, "lu"?: last_updated}`.
//! Diff event (`_state_diff_event`): `{"a": {id: compressed}, "c": {id: {"+": {...}, "-": {"a": [keys]}}}, "r": [ids]}`.

use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Attributes the UI reads. Everything else is dropped to keep memory small.
pub const KEPT_ATTRIBUTES: &[&str] = &[
    "friendly_name",
    "icon",
    "unit_of_measurement",
    "device_class",
    "state_class",
    "brightness",
    "color_mode",
    "current_position",
    "current_tilt_position",
    "percentage",
    "percentage_step",
    "supported_features",
    "min",
    "max",
    "step",
    "mode",
    "temperature",
    "current_temperature",
    "target_temp_high",
    "target_temp_low",
    "hvac_modes",
    "hvac_action",
    "media_title",
    "media_artist",
    "media_album_name",
    "media_duration",
    "media_position",
    "media_position_updated_at",
    "entity_picture_local",
    "media_content_type",
    "volume_level",
    "is_volume_muted",
    "source",
    "source_list",
    "entity_picture",
    "installed_version",
    "latest_version",
    "in_progress",
    "update_percentage",
    "release_summary",
    "battery_level",
    "options",
    "assumed_state",
    "restored",
    // weather
    "temperature_unit",
    "apparent_temperature",
    "humidity",
    "pressure",
    "pressure_unit",
    "wind_speed",
    "wind_speed_unit",
    "wind_bearing",
    "wind_gust_speed",
    "precipitation_unit",
    "visibility",
    "visibility_unit",
    "dew_point",
    "cloud_coverage",
    "uv_index",
];

#[derive(Debug, Clone, PartialEq)]
pub struct EntityState {
    pub state: String,
    pub attributes: Map<String, Value>,
    /// Seconds since the Unix epoch.
    pub last_changed: f64,
    pub last_updated: f64,
}

impl EntityState {
    pub fn new(state: impl Into<String>) -> Self {
        EntityState {
            state: state.into(),
            attributes: Map::new(),
            last_changed: 0.0,
            last_updated: 0.0,
        }
    }

    pub fn with_attr(mut self, key: &str, value: Value) -> Self {
        self.attributes.insert(key.to_string(), value);
        self
    }

    pub fn attr_str(&self, key: &str) -> Option<&str> {
        self.attributes.get(key).and_then(Value::as_str)
    }

    pub fn attr_f64(&self, key: &str) -> Option<f64> {
        match self.attributes.get(key)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn attr_bool(&self, key: &str) -> Option<bool> {
        self.attributes.get(key).and_then(Value::as_bool)
    }

    pub fn attr_u32(&self, key: &str) -> Option<u32> {
        self.attr_f64(key)
            .map(|v| v.round().clamp(0.0, u32::MAX as f64) as u32)
    }

    pub fn friendly_name(&self) -> Option<&str> {
        self.attr_str("friendly_name")
    }

    pub fn unit(&self) -> Option<&str> {
        self.attr_str("unit_of_measurement")
    }

    pub fn device_class(&self) -> Option<&str> {
        self.attr_str("device_class")
    }

    pub fn numeric_state(&self) -> Option<f64> {
        self.state.trim().parse().ok()
    }

    pub fn is_unavailable(&self) -> bool {
        matches!(self.state.as_str(), "unavailable" | "unknown")
    }

    pub fn supports(&self, flag: u32) -> bool {
        self.attr_u32("supported_features")
            .is_some_and(|f| f & flag != 0)
    }
}

/// Wire format of one compressed state.
#[derive(Debug, Clone, Deserialize)]
pub struct CompressedState {
    pub s: String,
    #[serde(default)]
    pub a: Map<String, Value>,
    #[serde(default)]
    pub lc: f64,
    #[serde(default)]
    pub lu: Option<f64>,
}

impl From<CompressedState> for EntityState {
    fn from(c: CompressedState) -> Self {
        EntityState {
            state: c.s,
            attributes: prune_attributes(c.a),
            last_changed: c.lc,
            last_updated: c.lu.unwrap_or(c.lc),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StateDiff {
    #[serde(rename = "+", default)]
    pub add: Option<DiffAdd>,
    #[serde(rename = "-", default)]
    pub remove: Option<DiffRemove>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DiffAdd {
    pub s: Option<String>,
    #[serde(default)]
    pub a: Option<Map<String, Value>>,
    pub lc: Option<f64>,
    pub lu: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DiffRemove {
    #[serde(default)]
    pub a: Vec<String>,
}

/// One `subscribe_entities` event payload.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EntitiesEvent {
    #[serde(default)]
    pub a: HashMap<String, CompressedState>,
    #[serde(default)]
    pub c: HashMap<String, StateDiff>,
    #[serde(default)]
    pub r: Vec<String>,
}

pub fn prune_attributes(mut attrs: Map<String, Value>) -> Map<String, Value> {
    attrs.retain(|k, _| KEPT_ATTRIBUTES.contains(&k.as_str()));
    attrs
}

/// All known entity states keyed by entity id.
#[derive(Debug, Default, Clone)]
pub struct StateStore {
    map: HashMap<Arc<str>, EntityState>,
}

impl StateStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<&EntityState> {
        self.map.get(id)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn insert(&mut self, id: Arc<str>, state: EntityState) {
        self.map.insert(id, state);
    }

    pub fn remove(&mut self, id: &str) -> Option<EntityState> {
        self.map.remove(id)
    }

    /// Apply one event. Returns the ids whose state changed (added, changed or removed).
    pub fn apply(&mut self, ev: EntitiesEvent) -> Vec<Arc<str>> {
        let mut changed: Vec<Arc<str>> = Vec::with_capacity(ev.a.len() + ev.c.len() + ev.r.len());
        for (id, cs) in ev.a {
            let key = self.key_for(&id);
            self.map.insert(key.clone(), cs.into());
            changed.push(key);
        }
        for (id, diff) in ev.c {
            let Some((key, st)) = self
                .map
                .get_key_value(id.as_str())
                .map(|(k, _)| k.clone())
                .and_then(|k| {
                    let st = self.map.get_mut(&*k)?;
                    Some((k, st))
                })
            else {
                // A change for an entity we never saw: ignore (HA always sends an add first).
                continue;
            };
            if let Some(add) = diff.add {
                if let Some(s) = add.s {
                    st.state = s;
                }
                if let Some(a) = add.a {
                    for (k, v) in prune_attributes(a) {
                        st.attributes.insert(k, v);
                    }
                }
                if let Some(lc) = add.lc {
                    st.last_changed = lc;
                    st.last_updated = lc;
                }
                if let Some(lu) = add.lu {
                    st.last_updated = lu;
                }
            }
            if let Some(rm) = diff.remove {
                for k in rm.a {
                    st.attributes.remove(&k);
                }
            }
            changed.push(key);
        }
        for id in ev.r {
            if let Some((key, _)) = self.map.remove_entry(id.as_str()) {
                changed.push(key);
            }
        }
        changed
    }

    fn key_for(&self, id: &str) -> Arc<str> {
        self.map
            .get_key_value(id)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| Arc::from(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn add_change_remove() {
        let mut store = StateStore::new();
        let add: EntitiesEvent = serde_json::from_value(json!({
            "a": {
                "switch.a": {"s": "off", "a": {"friendly_name": "A", "junk": 1}, "c": "ctx", "lc": 10.0},
                "sensor.t": {"s": "21.5", "a": {"unit_of_measurement": "°C"}, "c": {"id": "x"}, "lc": 10.0, "lu": 12.5}
            }
        }))
        .unwrap();
        let changed = store.apply(add);
        assert_eq!(changed.len(), 2);
        let a = store.get("switch.a").unwrap();
        assert_eq!(a.state, "off");
        assert_eq!(a.friendly_name(), Some("A"));
        assert!(
            !a.attributes.contains_key("junk"),
            "unknown attributes are pruned"
        );
        assert_eq!(a.last_updated, 10.0, "lu defaults to lc");
        assert_eq!(store.get("sensor.t").unwrap().last_updated, 12.5);

        let change: EntitiesEvent = serde_json::from_value(json!({
            "c": {"switch.a": {"+": {"s": "on", "lc": 20.0, "a": {"icon": "mdi:x"}}, "-": {"a": ["friendly_name"]}}}
        }))
        .unwrap();
        let changed = store.apply(change);
        assert_eq!(&*changed[0], "switch.a");
        let a = store.get("switch.a").unwrap();
        assert_eq!(a.state, "on");
        assert_eq!(a.last_changed, 20.0);
        assert_eq!(a.attr_str("icon"), Some("mdi:x"));
        assert!(a.friendly_name().is_none());

        let remove: EntitiesEvent =
            serde_json::from_value(json!({"r": ["sensor.t", "sensor.never"]})).unwrap();
        let changed = store.apply(remove);
        assert_eq!(changed.len(), 1);
        assert!(store.get("sensor.t").is_none());
    }

    #[test]
    fn helpers() {
        let s = EntityState::new("72")
            .with_attr("brightness", json!(128))
            .with_attr("supported_features", json!(15))
            .with_attr("min", json!("5"));
        assert_eq!(s.numeric_state(), Some(72.0));
        assert_eq!(s.attr_u32("brightness"), Some(128));
        assert_eq!(s.attr_f64("min"), Some(5.0));
        assert!(s.supports(4));
        assert!(!s.supports(16));
        assert!(EntityState::new("unavailable").is_unavailable());
    }
}
