//! State translations fetched from Home Assistant
//! (`frontend/get_translations`, category `entity_component`).
//!
//! Keys are flattened like the frontend expects them, e.g.
//! `component.switch.entity_component._.state.on` → "Acceso",
//! `component.binary_sensor.entity_component.connectivity.state.off` → "Disconnesso".

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Translations {
    #[serde(default)]
    pub resources: HashMap<String, String>,
}

impl Translations {
    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// Translated state text, trying the device class first and then the generic `_` entry.
    pub fn state(&self, domain: &str, device_class: Option<&str>, state: &str) -> Option<&str> {
        if let Some(dc) = device_class.filter(|d| !d.is_empty()) {
            let key = format!("component.{domain}.entity_component.{dc}.state.{state}");
            if let Some(t) = self.resources.get(&key) {
                return Some(t);
            }
        }
        let key = format!("component.{domain}.entity_component._.state.{state}");
        self.resources.get(&key).map(String::as_str)
    }
}

/// Language code to request: `it_IT.UTF-8` → `it`, `pt-BR` → `pt-BR`, empty → `en`.
pub fn normalise_language(raw: &str) -> String {
    let s = raw.trim();
    let s = s.split('.').next().unwrap_or(s);
    let s = s.split('@').next().unwrap_or(s);
    if s.is_empty() || s == "C" || s == "POSIX" {
        return "en".into();
    }
    match s.split_once('_') {
        Some((lang, region)) if lang.len() == 2 && region.len() == 2 => {
            // Home Assistant only has regional variants for a few languages.
            match (lang, region) {
                ("pt", "BR") => "pt-BR".into(),
                ("zh", "TW") => "zh-Hant".into(),
                ("zh", _) => "zh-Hans".into(),
                ("en", "GB") => "en-GB".into(),
                _ => lang.to_lowercase(),
            }
        }
        _ => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/translations_it.json");

    #[test]
    fn lookups() {
        let t = Translations::from_json_str(FIXTURE).unwrap();
        assert_eq!(t.state("switch", None, "off"), Some("Spento"));
        assert_eq!(t.state("switch", Some("outlet"), "on"), Some("Acceso"));
        assert_eq!(
            t.state("binary_sensor", Some("connectivity"), "on"),
            Some("Connesso")
        );
        assert_eq!(
            t.state("binary_sensor", Some("nonexistent"), "on"),
            Some("Acceso")
        );
        assert_eq!(t.state("cover", Some("shutter"), "open"), Some("Aperto"));
        assert_eq!(t.state("person", None, "not_home"), Some("Fuori casa"));
        assert_eq!(t.state("switch", None, "weird"), None);
        assert!(Translations::default().is_empty());
    }

    #[test]
    fn language_codes() {
        assert_eq!(normalise_language("it_IT.UTF-8"), "it");
        assert_eq!(normalise_language("de"), "de");
        assert_eq!(normalise_language("pt_BR.UTF-8"), "pt-BR");
        assert_eq!(normalise_language("C"), "en");
        assert_eq!(normalise_language(""), "en");
    }
}
