//! Normalised dashboard tree consumed by the UI layer.
//!
//! Produced from [`crate::lovelace::LovelaceConfig`] by [`crate::normalise`].
//! Everything the renderer needs is resolved here: shorthand strings are
//! expanded, grid options are concrete, and each leaf knows which entity ids
//! it displays.

use crate::lovelace::{Segment, Severity};
pub use crate::weather::{ForecastType, WeatherDisplay};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct Dashboard {
    pub title: Option<String>,
    pub views: Vec<View>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub title: String,
    pub path: String,
    pub icon: Option<String>,
    pub subview: bool,
    pub badges: Vec<Badge>,
    pub layout: Layout,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    /// Classic masonry view: cards flow into N columns.
    Masonry(Vec<CardNode>),
    /// `type: sections` view.
    Sections {
        max_columns: u32,
        sections: Vec<Section>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: Option<String>,
    pub column_span: u32,
    pub cards: Vec<CardNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Badge {
    pub entity: EntityRef,
    pub show_name: bool,
    pub show_state: bool,
    pub show_icon: bool,
}

/// Reference to an entity with optional display overrides from the config.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntityRef {
    pub entity_id: Arc<str>,
    pub name: Option<String>,
    pub icon: Option<String>,
}

impl EntityRef {
    pub fn new(id: &str) -> Self {
        EntityRef {
            entity_id: Arc::from(id),
            name: None,
            icon: None,
        }
    }
    pub fn domain(&self) -> &str {
        crate::domain::domain_of(&self.entity_id)
    }
}

/// Concrete grid placement inside a sections-view grid (12 columns wide).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridOpts {
    /// 1..=12 columns.
    pub columns: u32,
    /// `None` = auto height; `Some(n)` = n rows of 56 px.
    pub rows: Option<u32>,
    /// `columns` is the card type's default (no `grid_options.columns`), so the
    /// renderer may pick a density that fits the screen.
    pub auto_columns: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CardNode {
    pub kind: CardKind,
    pub title: Option<String>,
    pub grid: GridOpts,
    /// Original `type` string, for diagnostics and placeholders.
    pub raw_type: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CardKind {
    Tile {
        entity: EntityRef,
        features: Vec<Feature>,
        hide_state: bool,
        vertical: bool,
        /// Tap on the tile body toggles the entity (HA default `icon_tap_action: toggle`).
        tap_toggles: bool,
    },
    Entities {
        rows: Vec<Row>,
        /// Show the header toggle (only meaningful when toggleable rows exist).
        header_toggle: bool,
        /// Colour icons by state (`state_color` / `color: state`).
        state_color: bool,
    },
    Glance {
        items: Vec<EntityRef>,
        show_name: bool,
        show_icon: bool,
        show_state: bool,
        columns: Option<u32>,
    },
    Sensor {
        entity: EntityRef,
        unit: Option<String>,
    },
    Gauge {
        entity: EntityRef,
        min: f64,
        max: f64,
        unit: Option<String>,
        needle: bool,
        severity: Option<Severity>,
        segments: Vec<Segment>,
    },
    Grid {
        columns: u32,
        square: bool,
        cards: Vec<CardNode>,
    },
    Markdown {
        lines: Vec<MarkdownLine>,
    },
    Button {
        entity: Option<EntityRef>,
        name: Option<String>,
        icon: Option<String>,
        action: Option<ActionSpec>,
    },
    /// `calendar` card: upcoming events of one or more calendar entities.
    Calendar {
        entities: Vec<EntityRef>,
        /// Days to list (7 for `listWeek`, 1 for `dayGridDay`, 31 for `dayGridMonth`).
        days: u32,
    },
    /// `weather-forecast` card: current conditions and a forecast row.
    Weather {
        entity: EntityRef,
        show_current: bool,
        show_forecast: bool,
        /// Configured `forecast_type`; `None` = the first one the entity supports.
        forecast_type: Option<ForecastType>,
        display: WeatherDisplay,
    },
    /// A card type we do not render natively (map, media-control, …).
    /// Entities are still listed so the UI can show their state.
    Unsupported {
        entities: Vec<EntityRef>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownLine {
    pub text: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Entity {
        entity: EntityRef,
        /// Label of the run/press button for scripts and scenes (default "Run").
        action_name: Option<String>,
        secondary_info: Option<String>,
        tap_action: Option<ActionSpec>,
    },
    Section {
        label: Option<String>,
    },
    Divider,
    Button {
        name: Option<String>,
        icon: Option<String>,
        action_name: Option<String>,
        action: Option<ActionSpec>,
    },
    Attribute {
        entity: EntityRef,
        attribute: String,
        prefix: Option<String>,
        suffix: Option<String>,
    },
    Weblink {
        name: Option<String>,
        url: String,
    },
    Buttons(Vec<EntityRef>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feature {
    Toggle,
    CoverOpenClose,
    CoverPosition,
    CoverTilt,
    CoverTiltPosition,
    LightBrightness,
    FanSpeed,
    NumericInput,
    TargetTemperature,
    UpdateActions,
    Button { action_name: Option<String> },
    Unknown(String),
}

/// A resolved `tap_action`-style action.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionSpec {
    MoreInfo,
    Toggle,
    PerformAction {
        domain: String,
        service: String,
        data: serde_json::Value,
        target: serde_json::Value,
    },
    Navigate(String),
    Url(String),
    Assist,
    None,
}

impl Dashboard {
    /// Every entity id referenced anywhere in the dashboard, sorted and unique.
    pub fn entity_ids(&self) -> BTreeSet<Arc<str>> {
        let mut out = BTreeSet::new();
        for v in &self.views {
            for b in &v.badges {
                out.insert(b.entity.entity_id.clone());
            }
            match &v.layout {
                Layout::Masonry(cards) => cards.iter().for_each(|c| c.collect_entities(&mut out)),
                Layout::Sections { sections, .. } => sections
                    .iter()
                    .flat_map(|s| s.cards.iter())
                    .for_each(|c| c.collect_entities(&mut out)),
            }
        }
        out
    }

    /// Find a view by `path` or (case-insensitive) title; falls back to index.
    pub fn view_index(&self, key: &str) -> Option<usize> {
        if let Ok(i) = key.parse::<usize>() {
            if i < self.views.len() {
                return Some(i);
            }
        }
        self.views
            .iter()
            .position(|v| v.path == key || v.title.eq_ignore_ascii_case(key))
    }
}

impl CardNode {
    pub fn collect_entities(&self, out: &mut BTreeSet<Arc<str>>) {
        match &self.kind {
            CardKind::Tile { entity, .. }
            | CardKind::Sensor { entity, .. }
            | CardKind::Gauge { entity, .. }
            | CardKind::Weather { entity, .. } => {
                out.insert(entity.entity_id.clone());
            }
            CardKind::Entities { rows, .. } => {
                for r in rows {
                    match r {
                        Row::Entity { entity, .. } | Row::Attribute { entity, .. } => {
                            out.insert(entity.entity_id.clone());
                        }
                        Row::Buttons(list) => list.iter().for_each(|e| {
                            out.insert(e.entity_id.clone());
                        }),
                        _ => {}
                    }
                }
            }
            CardKind::Glance { items, .. }
            | CardKind::Unsupported { entities: items }
            | CardKind::Calendar {
                entities: items, ..
            } => items.iter().for_each(|e| {
                out.insert(e.entity_id.clone());
            }),
            CardKind::Grid { cards, .. } => cards.iter().for_each(|c| c.collect_entities(out)),
            CardKind::Button {
                entity: Some(e), ..
            } => {
                out.insert(e.entity_id.clone());
            }
            CardKind::Button { entity: None, .. } | CardKind::Markdown { .. } => {}
        }
    }

    /// Rough height estimate in "row units" used for masonry column balancing.
    pub fn estimated_rows(&self) -> u32 {
        let header = u32::from(self.title.is_some());
        match &self.kind {
            CardKind::Tile { features, .. } => 1 + features.len() as u32,
            CardKind::Entities { rows, .. } => header + rows.len() as u32,
            CardKind::Glance { items, columns, .. } => {
                let cols = columns.unwrap_or(5).max(1);
                header + items.len().div_ceil(cols as usize) as u32 * 2
            }
            CardKind::Sensor { .. } => 2,
            CardKind::Gauge { .. } => 3,
            CardKind::Grid { columns, cards, .. } => {
                let cols = (*columns).max(1) as usize;
                let mut rows_total = header;
                for chunk in cards.chunks(cols) {
                    rows_total += chunk.iter().map(|c| c.estimated_rows()).max().unwrap_or(1);
                }
                rows_total
            }
            CardKind::Markdown { lines } => header + lines.len() as u32,
            CardKind::Button { .. } => 2,
            CardKind::Calendar { .. } => 8,
            CardKind::Weather {
                show_current,
                show_forecast,
                ..
            } => 2 * u32::from(*show_current) + 2 * u32::from(*show_forecast),
            CardKind::Unsupported { .. } => 2,
        }
    }

    fn collect_weather(
        &self,
        out: &mut std::collections::BTreeMap<Arc<str>, Option<ForecastType>>,
    ) {
        match &self.kind {
            CardKind::Weather {
                entity,
                show_forecast: true,
                forecast_type,
                ..
            } => {
                // One subscription per entity: the first card that names a type wins.
                let e = out.entry(entity.entity_id.clone()).or_insert(None);
                if e.is_none() {
                    *e = *forecast_type;
                }
            }
            CardKind::Grid { cards, .. } => cards.iter().for_each(|c| c.collect_weather(out)),
            _ => {}
        }
    }

    fn collect_calendars(&self, out: &mut std::collections::BTreeMap<Arc<str>, u32>) {
        match &self.kind {
            CardKind::Calendar { entities, days } => {
                for e in entities {
                    let d = out.entry(e.entity_id.clone()).or_insert(0);
                    *d = (*d).max(*days);
                }
            }
            CardKind::Grid { cards, .. } => cards.iter().for_each(|c| c.collect_calendars(out)),
            _ => {}
        }
    }
}

impl Dashboard {
    /// Calendar entities shown by calendar cards, with the number of days to fetch.
    pub fn calendar_entities(&self) -> std::collections::BTreeMap<Arc<str>, u32> {
        let mut out = std::collections::BTreeMap::new();
        for v in &self.views {
            match &v.layout {
                Layout::Masonry(cards) => cards.iter().for_each(|c| c.collect_calendars(&mut out)),
                Layout::Sections { sections, .. } => sections
                    .iter()
                    .flat_map(|s| s.cards.iter())
                    .for_each(|c| c.collect_calendars(&mut out)),
            }
        }
        out
    }

    /// Weather entities whose forecast is shown, with the configured forecast type.
    pub fn weather_entities(&self) -> std::collections::BTreeMap<Arc<str>, Option<ForecastType>> {
        let mut out = std::collections::BTreeMap::new();
        for v in &self.views {
            match &v.layout {
                Layout::Masonry(cards) => cards.iter().for_each(|c| c.collect_weather(&mut out)),
                Layout::Sections { sections, .. } => sections
                    .iter()
                    .flat_map(|s| s.cards.iter())
                    .for_each(|c| c.collect_weather(&mut out)),
            }
        }
        out
    }
}
