//! Serde model of a Lovelace dashboard configuration.
//!
//! The same structs are used for the JSON returned by the `lovelace/config`
//! websocket command and for a YAML file exported from Home Assistant. Every
//! field is optional so unknown or partial configs still load; card specific
//! keys live in one flat [`CardConfig`] struct and are interpreted by
//! [`crate::normalise`] based on `type`.

use serde::Deserialize;
use serde_json::{Map, Value};
use std::fmt;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LovelaceConfig {
    pub title: Option<String>,
    pub views: Vec<ViewConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ViewConfig {
    pub title: Option<String>,
    pub path: Option<String>,
    pub icon: Option<String>,
    pub theme: Option<String>,
    #[serde(rename = "type")]
    pub view_type: Option<String>,
    pub subview: bool,
    pub badges: Vec<BadgeConfig>,
    pub cards: Vec<CardConfig>,
    pub sections: Vec<SectionConfig>,
    pub max_columns: Option<u32>,
    pub dense_section_placement: bool,
}

/// A badge is either a bare entity id or an object.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BadgeConfig {
    Id(String),
    Object(BadgeObject),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BadgeObject {
    #[serde(rename = "type")]
    pub badge_type: Option<String>,
    pub entity: Option<String>,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub show_name: Option<bool>,
    pub show_state: Option<bool>,
    pub show_icon: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SectionConfig {
    #[serde(rename = "type")]
    pub section_type: Option<String>,
    pub title: Option<String>,
    pub cards: Vec<CardConfig>,
    pub column_span: Option<u32>,
}

/// Flat union of the configuration keys of every card type we understand.
/// Unknown keys are kept in `extra` for diagnostics.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CardConfig {
    #[serde(rename = "type")]
    pub card_type: String,
    pub entity: Option<String>,
    pub entities: Vec<EntityRowConfig>,
    pub title: Option<String>,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub features: Vec<FeatureConfig>,
    pub features_position: Option<String>,
    pub cards: Vec<CardConfig>,
    pub columns: Option<u32>,
    pub square: Option<bool>,
    pub content: Option<String>,
    pub text_only: Option<bool>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub needle: Option<bool>,
    pub severity: Option<Severity>,
    pub segments: Vec<Segment>,
    pub unit: Option<String>,
    pub detail: Option<u32>,
    pub graph: Option<String>,
    pub hours_to_show: Option<u32>,
    pub show_name: Option<bool>,
    pub show_icon: Option<bool>,
    pub show_state: Option<bool>,
    pub hide_state: Option<bool>,
    pub vertical: Option<bool>,
    pub state_color: Option<bool>,
    pub color: Option<String>,
    pub show_header_toggle: Option<bool>,
    pub grid_options: Option<GridOptions>,
    pub initial_view: Option<String>,
    pub show_current: Option<bool>,
    pub show_forecast: Option<bool>,
    pub forecast_type: Option<String>,
    pub secondary_info_attribute: Option<String>,
    pub round_temperature: Option<bool>,
    pub tap_action: Option<ActionConfig>,
    pub icon_tap_action: Option<ActionConfig>,
    pub hold_action: Option<ActionConfig>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// An entry of an `entities` list: a bare entity id or a row object.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum EntityRowConfig {
    Id(String),
    Row(Box<EntityRowObject>),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EntityRowObject {
    #[serde(rename = "type")]
    pub row_type: Option<String>,
    pub entity: Option<String>,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub action_name: Option<String>,
    pub secondary_info: Option<Value>,
    pub label: Option<String>,
    pub attribute: Option<String>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub url: Option<String>,
    pub entities: Vec<EntityRowConfig>,
    pub row: Option<Box<EntityRowObject>>,
    pub show_name: Option<bool>,
    pub show_icon: Option<bool>,
    pub tap_action: Option<ActionConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FeatureConfig {
    #[serde(rename = "type")]
    pub feature_type: String,
    pub action_name: Option<String>,
    pub style: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GridOptions {
    pub columns: Option<GridSize>,
    pub rows: Option<GridSize>,
}

/// `grid_options.columns` / `rows`: an integer or the words `full` / `auto`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum GridSize {
    Cells(u32),
    Word(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Severity {
    pub green: f64,
    pub yellow: f64,
    pub red: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Segment {
    pub from: f64,
    pub color: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ActionConfig {
    pub action: Option<String>,
    pub perform_action: Option<String>,
    /// Legacy spelling of `perform_action`.
    pub service: Option<String>,
    pub target: Option<Value>,
    pub data: Option<Value>,
    /// Legacy spelling of `data`.
    pub service_data: Option<Value>,
    pub navigation_path: Option<String>,
    pub url_path: Option<String>,
    pub entity: Option<String>,
}

#[derive(Debug)]
pub enum LoadError {
    Yaml(serde_yaml_ng::Error),
    Json(serde_json::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Yaml(e) => write!(f, "invalid dashboard YAML: {e}"),
            LoadError::Json(e) => write!(f, "invalid dashboard JSON: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl LovelaceConfig {
    pub fn from_yaml_str(s: &str) -> Result<Self, LoadError> {
        serde_yaml_ng::from_str(s).map_err(LoadError::Yaml)
    }

    pub fn from_json_value(v: Value) -> Result<Self, LoadError> {
        serde_json::from_value(v).map_err(LoadError::Json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/dashboard.yaml");

    #[test]
    fn parses_fixture() {
        let cfg = LovelaceConfig::from_yaml_str(FIXTURE).unwrap();
        assert_eq!(cfg.title.as_deref(), Some("Demo"));
        assert_eq!(cfg.views.len(), 4);
        let kiosk = &cfg.views[3];
        assert_eq!(kiosk.view_type.as_deref(), Some("sections"));
        assert_eq!(kiosk.sections.len(), 3);
        assert_eq!(kiosk.sections[2].column_span, Some(2));
        let cal = &kiosk.sections[2].cards[0];
        assert_eq!(cal.card_type, "calendar");
        assert_eq!(
            cal.grid_options.as_ref().unwrap().columns,
            Some(GridSize::Word("full".into()))
        );
        // Bare-string entity row in the Devices view.
        let devices = &cfg.views[2];
        match &devices.cards[0].entities[0] {
            EntityRowConfig::Id(id) => assert_eq!(id, "sensor.printer"),
            other => panic!("expected bare id, got {other:?}"),
        }
        match &devices.cards[0].entities[1] {
            EntityRowConfig::Row(r) => {
                assert_eq!(r.name.as_deref(), Some("Black toner"))
            }
            other => panic!("expected row object, got {other:?}"),
        }
    }

    #[test]
    fn parses_json_value_too() {
        let v: Value = serde_json::json!({
            "views": [{"title": "T", "cards": [{"type": "tile", "entity": "switch.a",
                        "features": [{"type": "toggle"}], "unknown_key": 1}]}]
        });
        let cfg = LovelaceConfig::from_json_value(v).unwrap();
        let card = &cfg.views[0].cards[0];
        assert_eq!(card.features[0].feature_type, "toggle");
        assert!(card.extra.contains_key("unknown_key"));
    }
}
