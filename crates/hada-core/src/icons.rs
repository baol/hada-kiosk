//! Icon resolution: config override → entity `icon` attribute → Home Assistant
//! `frontend/get_icons` (`entity_component` category) resources → built-in fallback.
//!
//! The resources have the shape of each integration's `icons.json`:
//! `{domain: {"_" | device_class: {"default": "mdi:x", "state": {state: icon}, "range": {"0": icon, "10": icon}}}}`.

use crate::domain::domain_of;
use crate::state::EntityState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const FALLBACK_ICON: &str = "mdi:help-circle-outline";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct IconEntry {
    pub default: Option<String>,
    #[serde(default)]
    pub state: HashMap<String, String>,
    /// Keys are numeric thresholds as strings ("0", "10", …).
    #[serde(default)]
    pub range: HashMap<String, String>,
}

/// `{"resources": {domain: {key: entry}}}` as returned by `frontend/get_icons`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct IconResources {
    #[serde(default)]
    pub resources: HashMap<String, HashMap<String, IconEntry>>,
}

impl IconResources {
    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

#[derive(Debug, Clone, Default)]
pub struct IconResolver {
    resources: IconResources,
}

impl IconResolver {
    pub fn new(resources: IconResources) -> Self {
        IconResolver { resources }
    }

    pub fn set_resources(&mut self, resources: IconResources) {
        self.resources = resources;
    }

    pub fn has_resources(&self) -> bool {
        !self.resources.is_empty()
    }

    /// Resolve the `mdi:` icon name to show for an entity.
    pub fn resolve(
        &self,
        entity_id: &str,
        st: Option<&EntityState>,
        config_icon: Option<&str>,
    ) -> String {
        if let Some(i) = config_icon.filter(|s| !s.is_empty()) {
            return i.to_string();
        }
        if let Some(i) = st
            .and_then(|s| s.attr_str("icon"))
            .filter(|s| !s.is_empty())
        {
            return i.to_string();
        }
        let domain = domain_of(entity_id);
        let state = st.map(|s| s.state.as_str()).unwrap_or("");
        let device_class = st.and_then(|s| s.device_class());
        if let Some(domain_icons) = self.resources.resources.get(domain) {
            let entry = device_class
                .and_then(|dc| domain_icons.get(dc))
                .or_else(|| domain_icons.get("_"));
            if let Some(e) = entry {
                if let Some(i) = e.state.get(state) {
                    return i.clone();
                }
                if !e.range.is_empty() {
                    if let Some(v) = st.and_then(|s| s.numeric_state()) {
                        if let Some(i) = range_icon(&e.range, v) {
                            return i.clone();
                        }
                    }
                }
                if let Some(d) = &e.default {
                    return d.clone();
                }
            }
        }
        builtin_icon(domain, device_class, state).to_string()
    }
}

fn range_icon(range: &HashMap<String, String>, value: f64) -> Option<&String> {
    range
        .iter()
        .filter_map(|(k, v)| k.parse::<f64>().ok().map(|t| (t, v)))
        .filter(|(t, _)| *t <= value)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, v)| v)
}

/// Small built-in table used before the HA icon resources arrive (or offline).
pub fn builtin_icon(domain: &str, device_class: Option<&str>, state: &str) -> &'static str {
    let on = matches!(state, "on" | "open" | "playing" | "home");
    match (domain, device_class.unwrap_or("")) {
        ("switch", "outlet") => {
            if state == "off" {
                "mdi:power-plug-off"
            } else {
                "mdi:power-plug"
            }
        }
        ("switch", _) => {
            if state == "off" {
                "mdi:toggle-switch-variant-off"
            } else {
                "mdi:toggle-switch-variant"
            }
        }
        ("light", _) => {
            if state == "off" {
                "mdi:lightbulb-off"
            } else {
                "mdi:lightbulb"
            }
        }
        ("fan", _) => {
            if state == "off" {
                "mdi:fan-off"
            } else {
                "mdi:fan"
            }
        }
        ("cover", "shutter") => {
            if state == "closed" {
                "mdi:window-shutter"
            } else {
                "mdi:window-shutter-open"
            }
        }
        ("cover", "blind") => {
            if state == "closed" {
                "mdi:blinds-horizontal-closed"
            } else {
                "mdi:blinds-horizontal"
            }
        }
        ("cover", "curtain") => {
            if state == "closed" {
                "mdi:curtains-closed"
            } else {
                "mdi:curtains"
            }
        }
        ("cover", "garage") => {
            if state == "closed" {
                "mdi:garage"
            } else {
                "mdi:garage-open"
            }
        }
        ("cover", _) => {
            if state == "closed" {
                "mdi:window-closed"
            } else {
                "mdi:window-open"
            }
        }
        ("sensor", "temperature") => "mdi:thermometer",
        ("sensor", "humidity") => "mdi:water-percent",
        ("sensor", "battery") => "mdi:battery",
        ("sensor", "timestamp") => "mdi:clock",
        ("sensor", "data_rate") => "mdi:transmission-tower",
        ("sensor", "data_size") => "mdi:database",
        ("sensor", "pm25") | ("sensor", "aqi") => "mdi:air-filter",
        ("sensor", "power") => "mdi:flash",
        ("sensor", "energy") => "mdi:lightning-bolt",
        ("sensor", _) => "mdi:eye",
        ("binary_sensor", "connectivity") => {
            if on {
                "mdi:check-network-outline"
            } else {
                "mdi:close-network-outline"
            }
        }
        ("binary_sensor", "problem" | "safety") => {
            if on {
                "mdi:alert-circle"
            } else {
                "mdi:check-circle"
            }
        }
        ("binary_sensor", "motion") => {
            if on {
                "mdi:motion-sensor"
            } else {
                "mdi:motion-sensor-off"
            }
        }
        ("binary_sensor", "door") => {
            if on {
                "mdi:door-open"
            } else {
                "mdi:door-closed"
            }
        }
        ("binary_sensor", _) => {
            if on {
                "mdi:checkbox-marked-circle"
            } else {
                "mdi:radiobox-blank"
            }
        }
        ("media_player", "tv") => {
            if state == "off" {
                "mdi:television-off"
            } else {
                "mdi:television"
            }
        }
        ("media_player", "speaker") => {
            if state == "off" {
                "mdi:speaker-off"
            } else {
                "mdi:speaker"
            }
        }
        ("media_player", "receiver") => {
            if state == "off" {
                "mdi:audio-video-off"
            } else {
                "mdi:audio-video"
            }
        }
        ("media_player", _) => {
            if state == "off" {
                "mdi:cast-off"
            } else {
                "mdi:cast"
            }
        }
        ("button", "restart") => "mdi:restart",
        ("button", "update") => "mdi:package-up",
        ("button", _) => "mdi:button-pointer",
        ("script", _) => {
            if state == "on" {
                "mdi:script-text-play"
            } else {
                "mdi:script-text"
            }
        }
        ("scene", _) => "mdi:palette",
        ("update", _) => {
            if state == "off" {
                "mdi:package"
            } else {
                "mdi:package-up"
            }
        }
        ("person", _) => {
            if state == "not_home" {
                "mdi:account-arrow-right"
            } else {
                "mdi:account"
            }
        }
        ("device_tracker", _) => "mdi:account",
        ("weather", _) => "mdi:weather-partly-cloudy",
        ("climate", _) => "mdi:thermostat",
        ("input_boolean", _) => {
            if state == "off" {
                "mdi:close-circle-outline"
            } else {
                "mdi:check-circle-outline"
            }
        }
        ("calendar", _) => "mdi:calendar",
        ("automation", _) => "mdi:robot",
        ("number" | "input_number", _) => "mdi:ray-vertex",
        ("lock", _) => {
            if state == "unlocked" {
                "mdi:lock-open"
            } else {
                "mdi:lock"
            }
        }
        ("camera", _) => "mdi:video",
        ("vacuum", _) => "mdi:robot-vacuum",
        ("humidifier", _) => "mdi:air-humidifier",
        _ => FALLBACK_ICON,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/icons.json");

    #[test]
    fn resolves_from_ha_resources() {
        let r = IconResolver::new(IconResources::from_json_str(FIXTURE).unwrap());
        assert_eq!(
            r.resolve("switch.a", Some(&EntityState::new("off")), None),
            "mdi:toggle-switch-variant-off"
        );
        assert_eq!(
            r.resolve("switch.a", Some(&EntityState::new("on")), None),
            "mdi:toggle-switch-variant"
        );
        let shutter = EntityState::new("closed").with_attr("device_class", json!("shutter"));
        assert_eq!(
            r.resolve("cover.a", Some(&shutter), None),
            "mdi:window-shutter"
        );
        let bat = EntityState::new("57").with_attr("device_class", json!("battery"));
        assert_eq!(r.resolve("sensor.b", Some(&bat), None), "mdi:battery-50");
        let bat_low = EntityState::new("3").with_attr("device_class", json!("battery"));
        assert_eq!(
            r.resolve("sensor.b", Some(&bat_low), None),
            "mdi:battery-alert"
        );
        let conn = EntityState::new("on").with_attr("device_class", json!("connectivity"));
        assert_eq!(
            r.resolve("binary_sensor.c", Some(&conn), None),
            "mdi:check-network-outline"
        );
        assert_eq!(
            r.resolve("update.u", Some(&EntityState::new("off")), None),
            "mdi:package"
        );
        assert_eq!(
            r.resolve("person.p", Some(&EntityState::new("not_home")), None),
            "mdi:account-arrow-right"
        );
        // Overrides win.
        let custom = EntityState::new("on").with_attr("icon", json!("mdi:lamp"));
        assert_eq!(r.resolve("switch.a", Some(&custom), None), "mdi:lamp");
        assert_eq!(
            r.resolve("switch.a", Some(&custom), Some("mdi:pine-tree")),
            "mdi:pine-tree"
        );
        // Unknown state falls back to the default of the entry.
        assert_eq!(
            r.resolve("sensor.x", Some(&EntityState::new("12")), None),
            "mdi:eye"
        );
    }

    #[test]
    fn builtin_fallback_without_resources() {
        let r = IconResolver::default();
        assert_eq!(
            r.resolve("light.l", Some(&EntityState::new("off")), None),
            "mdi:lightbulb-off"
        );
        assert_eq!(r.resolve("whatever.x", None, None), FALLBACK_ICON);
    }
}
