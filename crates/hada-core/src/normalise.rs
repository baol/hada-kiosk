//! Convert a [`LovelaceConfig`] into the renderer-facing [`Dashboard`] tree.

use crate::domain;
use crate::lovelace::*;
use crate::model::*;

/// Section grids in Home Assistant are 12 columns wide.
pub const SECTION_COLUMNS: u32 = 12;

pub fn normalise(cfg: &LovelaceConfig) -> Dashboard {
    let views = cfg
        .views
        .iter()
        .enumerate()
        .map(|(i, v)| normalise_view(v, i))
        .collect();
    Dashboard {
        title: cfg.title.clone(),
        views,
    }
}

fn normalise_view(v: &ViewConfig, index: usize) -> View {
    let title = v
        .title
        .clone()
        .unwrap_or_else(|| format!("View {}", index + 1));
    let path = v.path.clone().unwrap_or_else(|| index.to_string());
    let badges = v.badges.iter().filter_map(badge).collect();
    let layout = if v.view_type.as_deref() == Some("sections") {
        Layout::Sections {
            max_columns: v.max_columns.unwrap_or(4).max(1),
            sections: v
                .sections
                .iter()
                .map(|s| Section {
                    title: s.title.clone(),
                    column_span: s.column_span.unwrap_or(1).max(1),
                    cards: s.cards.iter().map(card).collect(),
                })
                .collect(),
        }
    } else {
        Layout::Masonry(v.cards.iter().map(card).collect())
    };
    View {
        title,
        path,
        icon: v.icon.clone(),
        subview: v.subview,
        badges,
        layout,
    }
}

fn badge(b: &BadgeConfig) -> Option<Badge> {
    match b {
        BadgeConfig::Id(id) => Some(Badge {
            entity: EntityRef::new(id),
            show_name: false,
            show_state: true,
            show_icon: true,
        }),
        BadgeConfig::Object(o) => {
            let id = o.entity.as_deref()?;
            Some(Badge {
                entity: EntityRef {
                    entity_id: id.into(),
                    name: o.name.clone(),
                    icon: o.icon.clone(),
                },
                show_name: o.show_name.unwrap_or(false),
                show_state: o.show_state.unwrap_or(true),
                show_icon: o.show_icon.unwrap_or(true),
            })
        }
    }
}

fn entity_ref(id: &str, name: &Option<String>, icon: &Option<String>) -> EntityRef {
    EntityRef {
        entity_id: id.into(),
        name: name.clone(),
        icon: icon.clone(),
    }
}

fn grid_opts(c: &CardConfig, default_columns: u32, default_rows: Option<u32>) -> GridOpts {
    let mut g = GridOpts {
        columns: default_columns,
        rows: default_rows,
        auto_columns: true,
    };
    if let Some(o) = &c.grid_options {
        match &o.columns {
            Some(GridSize::Cells(n)) => {
                g.columns = (*n).clamp(1, SECTION_COLUMNS);
                g.auto_columns = false;
            }
            Some(GridSize::Word(w)) if w == "full" => {
                g.columns = SECTION_COLUMNS;
                g.auto_columns = false;
            }
            _ => {}
        }
        match &o.rows {
            Some(GridSize::Cells(n)) => g.rows = Some((*n).max(1)),
            Some(GridSize::Word(w)) if w == "auto" => g.rows = None,
            _ => {}
        }
    }
    g
}

pub fn card(c: &CardConfig) -> CardNode {
    let raw_type = c.card_type.clone();
    let (kind, grid) = match c.card_type.as_str() {
        "tile" => {
            let id = c.entity.clone().unwrap_or_default();
            let features: Vec<Feature> = c.features.iter().map(feature).collect();
            let tap_toggles = match &c.icon_tap_action {
                Some(a) => matches!(action(a), Some(ActionSpec::Toggle)),
                None => domain::is_toggleable(domain::domain_of(&id)),
            };
            let rows = 1 + features
                .iter()
                .filter(|f| !matches!(f, Feature::Unknown(_)))
                .count() as u32;
            (
                CardKind::Tile {
                    entity: entity_ref(&id, &c.name, &c.icon),
                    features,
                    hide_state: c.hide_state.unwrap_or(false),
                    vertical: c.vertical.unwrap_or(false),
                    tap_toggles,
                },
                grid_opts(c, 6, Some(rows)),
            )
        }
        "entities" => {
            let rows: Vec<Row> = c.entities.iter().filter_map(row).collect();
            let has_toggleable = rows.iter().any(|r| match r {
                Row::Entity { entity, .. } => domain::is_toggleable(entity.domain()),
                _ => false,
            });
            let state_color =
                c.state_color.unwrap_or(false) || matches!(c.color.as_deref(), Some("state"));
            (
                CardKind::Entities {
                    rows,
                    header_toggle: c.show_header_toggle.unwrap_or(true) && has_toggleable,
                    state_color,
                },
                grid_opts(c, SECTION_COLUMNS, None),
            )
        }
        "glance" => {
            let items = c
                .entities
                .iter()
                .filter_map(|e| match e {
                    EntityRowConfig::Id(id) => Some(EntityRef::new(id)),
                    EntityRowConfig::Row(r) => r
                        .entity
                        .as_deref()
                        .map(|id| entity_ref(id, &r.name, &r.icon)),
                })
                .collect();
            (
                CardKind::Glance {
                    items,
                    show_name: c.show_name.unwrap_or(true),
                    show_icon: c.show_icon.unwrap_or(true),
                    show_state: c.show_state.unwrap_or(true),
                    columns: c.columns,
                },
                grid_opts(c, SECTION_COLUMNS, None),
            )
        }
        "sensor" => {
            let id = c.entity.clone().unwrap_or_default();
            (
                CardKind::Sensor {
                    entity: entity_ref(&id, &c.name, &c.icon),
                    unit: c.unit.clone(),
                },
                grid_opts(c, 6, Some(2)),
            )
        }
        "gauge" => {
            let id = c.entity.clone().unwrap_or_default();
            (
                CardKind::Gauge {
                    entity: entity_ref(&id, &c.name, &c.icon),
                    min: c.min.unwrap_or(0.0),
                    max: c.max.unwrap_or(100.0),
                    unit: c.unit.clone(),
                    needle: c.needle.unwrap_or(false),
                    severity: c.severity,
                    segments: c.segments.clone(),
                },
                grid_opts(c, 6, Some(4)),
            )
        }
        "grid" => (
            CardKind::Grid {
                columns: c.columns.unwrap_or(3).max(1),
                square: c.square.unwrap_or(true),
                cards: c.cards.iter().map(card).collect(),
            },
            grid_opts(c, SECTION_COLUMNS, None),
        ),
        "markdown" => (
            CardKind::Markdown {
                lines: markdown_lines(c.content.as_deref().unwrap_or("")),
            },
            grid_opts(c, SECTION_COLUMNS, None),
        ),
        "calendar" => {
            let entities = c
                .entities
                .iter()
                .filter_map(|e| match e {
                    EntityRowConfig::Id(id) => Some(EntityRef::new(id)),
                    EntityRowConfig::Row(r) => r
                        .entity
                        .as_deref()
                        .map(|id| entity_ref(id, &r.name, &r.icon)),
                })
                .collect();
            let days = match c.initial_view.as_deref() {
                Some("dayGridDay") => 1,
                Some("dayGridMonth") => 31,
                _ => 7,
            };
            (
                CardKind::Calendar { entities, days },
                grid_opts(c, SECTION_COLUMNS, None),
            )
        }
        "weather-forecast" => {
            let id = c.entity.clone().unwrap_or_default();
            let forecast_type = c.forecast_type.as_deref().and_then(|t| {
                let parsed = ForecastType::parse(t);
                if parsed.is_none() {
                    tracing::warn!("unknown forecast_type {t:?}; using the first supported one");
                }
                parsed
            });
            (
                CardKind::Weather {
                    entity: entity_ref(&id, &c.name, &c.icon),
                    show_current: c.show_current.unwrap_or(true),
                    show_forecast: c.show_forecast.unwrap_or(true),
                    forecast_type,
                    display: WeatherDisplay {
                        secondary_attribute: c.secondary_info_attribute.clone(),
                        round_temperature: c.round_temperature.unwrap_or(false),
                    },
                },
                grid_opts(c, SECTION_COLUMNS, None),
            )
        }
        "button" => {
            let entity = c
                .entity
                .as_deref()
                .map(|id| entity_ref(id, &c.name, &c.icon));
            (
                CardKind::Button {
                    entity,
                    name: c.name.clone(),
                    icon: c.icon.clone(),
                    action: c.tap_action.as_ref().and_then(action),
                },
                grid_opts(c, 6, Some(2)),
            )
        }
        _ => {
            let mut entities: Vec<EntityRef> = c
                .entities
                .iter()
                .filter_map(|e| match e {
                    EntityRowConfig::Id(id) => Some(EntityRef::new(id)),
                    EntityRowConfig::Row(r) => r
                        .entity
                        .as_deref()
                        .map(|id| entity_ref(id, &r.name, &r.icon)),
                })
                .collect();
            if let Some(id) = &c.entity {
                entities.insert(0, entity_ref(id, &c.name, &c.icon));
            }
            (
                CardKind::Unsupported { entities },
                grid_opts(c, SECTION_COLUMNS, Some(2)),
            )
        }
    };
    CardNode {
        kind,
        title: c.title.clone(),
        grid,
        raw_type,
    }
}

fn row(e: &EntityRowConfig) -> Option<Row> {
    match e {
        EntityRowConfig::Id(id) => Some(Row::Entity {
            entity: EntityRef::new(id),
            action_name: None,
            secondary_info: None,
            tap_action: None,
        }),
        EntityRowConfig::Row(r) => row_object(r),
    }
}

fn row_object(r: &EntityRowObject) -> Option<Row> {
    match r.row_type.as_deref() {
        Some("section") => Some(Row::Section {
            label: r.label.clone(),
        }),
        Some("divider") => Some(Row::Divider),
        Some("button") => Some(Row::Button {
            name: r.name.clone(),
            icon: r.icon.clone(),
            action_name: r.action_name.clone(),
            action: r.tap_action.as_ref().and_then(action),
        }),
        Some("attribute") => Some(Row::Attribute {
            entity: entity_ref(r.entity.as_deref()?, &r.name, &r.icon),
            attribute: r.attribute.clone()?,
            prefix: r.prefix.clone(),
            suffix: r.suffix.clone(),
        }),
        Some("weblink") => Some(Row::Weblink {
            name: r.name.clone(),
            url: r.url.clone()?,
        }),
        Some("buttons") => Some(Row::Buttons(
            r.entities
                .iter()
                .filter_map(|e| match e {
                    EntityRowConfig::Id(id) => Some(EntityRef::new(id)),
                    EntityRowConfig::Row(o) => o
                        .entity
                        .as_deref()
                        .map(|id| entity_ref(id, &o.name, &o.icon)),
                })
                .collect(),
        )),
        // We cannot evaluate conditions without more context; show the inner row.
        Some("conditional") => r.row.as_deref().and_then(row_object),
        Some("cast") | Some("custom") => None,
        Some(t) if t.starts_with("custom:") => None,
        _ => {
            let id = r.entity.as_deref()?;
            let secondary_info = match &r.secondary_info {
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                _ => None,
            };
            Some(Row::Entity {
                entity: entity_ref(id, &r.name, &r.icon),
                action_name: r.action_name.clone(),
                secondary_info,
                tap_action: r.tap_action.as_ref().and_then(action),
            })
        }
    }
}

pub fn feature(f: &FeatureConfig) -> Feature {
    match f.feature_type.as_str() {
        "toggle" => Feature::Toggle,
        "cover-open-close" => Feature::CoverOpenClose,
        "cover-position" => Feature::CoverPosition,
        "cover-tilt" => Feature::CoverTilt,
        "cover-tilt-position" => Feature::CoverTiltPosition,
        "light-brightness" => Feature::LightBrightness,
        "fan-speed" => Feature::FanSpeed,
        "numeric-input" => Feature::NumericInput,
        "target-temperature" => Feature::TargetTemperature,
        "update-actions" => Feature::UpdateActions,
        "button" => Feature::Button {
            action_name: f.action_name.clone(),
        },
        other => Feature::Unknown(other.to_string()),
    }
}

pub fn action(a: &ActionConfig) -> Option<ActionSpec> {
    match a.action.as_deref()? {
        "more-info" => Some(ActionSpec::MoreInfo),
        "toggle" => Some(ActionSpec::Toggle),
        "none" => Some(ActionSpec::None),
        "assist" => Some(ActionSpec::Assist),
        "navigate" => a.navigation_path.clone().map(ActionSpec::Navigate),
        "url" => a.url_path.clone().map(ActionSpec::Url),
        "perform-action" | "call-service" => {
            let svc = a.perform_action.as_deref().or(a.service.as_deref())?;
            let (domain, service) = svc.split_once('.')?;
            Some(ActionSpec::PerformAction {
                domain: domain.to_string(),
                service: service.to_string(),
                data: a
                    .data
                    .clone()
                    .or_else(|| a.service_data.clone())
                    .unwrap_or(serde_json::Value::Null),
                target: a.target.clone().unwrap_or(serde_json::Value::Null),
            })
        }
        _ => None,
    }
}

/// Split markdown content into lines, extracting `[text](url)` links.
pub fn markdown_lines(content: &str) -> Vec<MarkdownLine> {
    let mut out = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(link) = parse_link(line) {
            out.push(link);
        } else {
            out.push(MarkdownLine {
                text: strip_md(line),
                url: None,
            });
        }
    }
    out
}

fn parse_link(line: &str) -> Option<MarkdownLine> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find("](")?;
    let text = &rest[..close];
    let after = &rest[close + 2..];
    let end = after.find(')')?;
    let url = &after[..end];
    let trailing = after[end + 1..].trim();
    let text = if trailing.is_empty() {
        text.to_string()
    } else {
        format!("{text} {trailing}")
    };
    Some(MarkdownLine {
        text,
        url: Some(url.to_string()),
    })
}

fn strip_md(line: &str) -> String {
    line.trim_start_matches('#')
        .trim()
        .replace("**", "")
        .replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/dashboard.yaml");

    fn dash() -> Dashboard {
        normalise(&LovelaceConfig::from_yaml_str(FIXTURE).unwrap())
    }

    #[test]
    fn views_and_layouts() {
        let d = dash();
        assert_eq!(d.views.len(), 4);
        assert_eq!(d.views[0].title, "Home");
        assert_eq!(d.views[0].path, "0");
        assert!(matches!(d.views[0].layout, Layout::Masonry(_)));
        let kiosk = &d.views[3];
        assert_eq!(kiosk.path, "kiosk");
        assert_eq!(kiosk.badges.len(), 1);
        match &kiosk.layout {
            Layout::Sections {
                max_columns,
                sections,
            } => {
                assert_eq!(*max_columns, 2);
                assert_eq!(sections.len(), 3);
                assert_eq!(sections[2].column_span, 2);
                let cal = &sections[2].cards[0];
                assert_eq!(cal.raw_type, "calendar");
                assert_eq!(
                    cal.grid,
                    GridOpts {
                        columns: 12,
                        rows: None,
                        auto_columns: false
                    }
                );
                assert!(matches!(cal.kind, CardKind::Calendar { days: 7, .. }));
                assert_eq!(sections[0].cards.len(), 6);
                assert_eq!(
                    sections[0].cards[0].grid,
                    GridOpts {
                        columns: 6,
                        rows: Some(2),
                        auto_columns: true
                    }
                );
            }
            _ => panic!("Kiosk should be a sections view"),
        }
    }

    #[test]
    fn card_kinds_resolve() {
        let d = dash();
        let Layout::Masonry(cards) = &d.views[0].layout else {
            panic!()
        };
        let kinds: Vec<&str> = cards.iter().map(|c| c.raw_type.as_str()).collect();
        assert_eq!(kinds[0], "map");
        let unsupported: Vec<&str> = cards
            .iter()
            .filter(|c| matches!(c.kind, CardKind::Unsupported { .. }))
            .map(|c| c.raw_type.as_str())
            .collect();
        assert_eq!(unsupported, vec!["map"]);
        let weather = cards
            .iter()
            .find(|c| c.raw_type == "weather-forecast")
            .unwrap();
        match &weather.kind {
            CardKind::Weather {
                entity,
                show_current,
                show_forecast,
                forecast_type,
                display,
            } => {
                assert_eq!(&*entity.entity_id, "weather.home");
                assert!(*show_current && *show_forecast);
                assert_eq!(*forecast_type, None);
                assert_eq!(display, &WeatherDisplay::default());
            }
            other => panic!("expected a weather card, got {other:?}"),
        }
        // grid of two media-control cards: grid is supported, children are not.
        let grid = cards.iter().find(|c| c.raw_type == "grid").unwrap();
        match &grid.kind {
            CardKind::Grid {
                columns,
                square,
                cards,
            } => {
                assert_eq!(*columns, 1);
                assert!(!square);
                assert_eq!(cards.len(), 2);
                assert!(matches!(cards[0].kind, CardKind::Unsupported { .. }));
            }
            _ => panic!(),
        }
        // Tile with cover features.
        let blind = cards.iter().find(|c| c.raw_type == "tile" && matches!(&c.kind, CardKind::Tile{entity,..} if &*entity.entity_id == "cover.kids_room_blind")).unwrap();
        match &blind.kind {
            CardKind::Tile {
                features,
                tap_toggles,
                ..
            } => {
                assert_eq!(features, &[Feature::CoverOpenClose, Feature::CoverPosition]);
                assert!(!tap_toggles, "covers are not toggleable by tap");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn entities_rows_and_header_toggle() {
        let d = dash();
        let Layout::Masonry(cards) = &d.views[0].layout else {
            panic!()
        };
        let luci = cards
            .iter()
            .find(|c| c.title.as_deref() == Some("Living room lights"))
            .unwrap();
        match &luci.kind {
            CardKind::Entities {
                rows,
                header_toggle,
                state_color,
            } => {
                assert_eq!(rows.len(), 4);
                assert!(*header_toggle);
                assert!(*state_color);
            }
            _ => panic!(),
        }
        let devices = cards
            .iter()
            .find(|c| c.title.as_deref() == Some("Devices"))
            .unwrap();
        match &devices.kind {
            CardKind::Entities {
                header_toggle,
                state_color,
                ..
            } => {
                // script + media_players: media_player is toggleable, so header toggle shows.
                assert!(*header_toggle);
                assert!(!*state_color);
            }
            _ => panic!(),
        }
        // Devices view: bare string + named rows, printer card has no toggleable rows.
        let Layout::Masonry(cards) = &d.views[2].layout else {
            panic!()
        };
        match &cards[0].kind {
            CardKind::Entities {
                rows,
                header_toggle,
                ..
            } => {
                assert!(!*header_toggle);
                match &rows[1] {
                    Row::Entity { entity, .. } => {
                        assert_eq!(entity.name.as_deref(), Some("Black toner"))
                    }
                    _ => panic!(),
                }
            }
            _ => panic!(),
        }
    }

    #[test]
    fn markdown_links() {
        let d = dash();
        let Layout::Masonry(cards) = &d.views[2].layout else {
            panic!()
        };
        let links = cards.iter().find(|c| c.raw_type == "markdown").unwrap();
        match &links.kind {
            CardKind::Markdown { lines } => {
                assert_eq!(lines.len(), 7);
                assert_eq!(lines[0].text, "NAS");
                assert_eq!(lines[0].url.as_deref(), Some("http://nas.local:5000"));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn entity_id_set() {
        let d = dash();
        let ids = d.entity_ids();
        assert!(ids.contains("switch.garden_lights"));
        assert!(ids.contains("sensor.living_room_temperature"));
        assert!(ids.contains("calendar.family"));
        assert!(ids.contains("person.alex"));
        assert_eq!(ids.len(), 63, "{ids:?}");
        let cals = d.calendar_entities();
        assert_eq!(cals.get("calendar.family"), Some(&7));
        assert_eq!(cals.len(), 1);
        let weather = d.weather_entities();
        assert_eq!(weather.len(), 1);
        assert_eq!(weather.get("weather.home"), Some(&None));
        assert_eq!(d.view_index("kiosk"), Some(3));
        assert_eq!(d.view_index("home"), Some(0));
        assert_eq!(d.view_index("2"), Some(2));
    }
}
