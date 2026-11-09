//! Per-domain presentation and control rules (what the HA frontend does in
//! `common/entity/*` and the tile/entities card code), kept as pure functions.

use crate::model::Feature;
use crate::state::EntityState;
use crate::timefmt;
use crate::translations::Translations;

/// `"switch.a"` → `"switch"`.
pub fn domain_of(entity_id: &str) -> &str {
    entity_id
        .split_once('.')
        .map(|(d, _)| d)
        .unwrap_or(entity_id)
}

/// Domains that respond to `<domain>.toggle` and are shown with a switch.
pub fn is_toggleable(domain: &str) -> bool {
    matches!(
        domain,
        "switch"
            | "light"
            | "fan"
            | "input_boolean"
            | "automation"
            | "humidifier"
            | "siren"
            | "remote"
            | "media_player"
    )
}

/// Whether the entity should be drawn in the "active" colour.
pub fn is_active(domain: &str, state: &str) -> bool {
    if matches!(
        state,
        "unavailable"
            | "unknown"
            | ""
            | "off"
            | "closed"
            | "idle"
            | "standby"
            | "not_home"
            | "locked"
            | "disarmed"
    ) {
        return false;
    }
    match domain {
        "cover" => matches!(state, "open" | "opening" | "closing"),
        "lock" => state == "unlocked",
        "alarm_control_panel" => state != "disarmed",
        "climate" => state != "off",
        "media_player" => matches!(state, "on" | "playing" | "paused" | "buffering"),
        "person" | "device_tracker" => state == "home",
        "binary_sensor" | "switch" | "light" | "fan" | "input_boolean" | "automation"
        | "script" | "update" | "humidifier" | "siren" | "remote" | "timer" | "vacuum" => {
            matches!(state, "on" | "active" | "cleaning" | "returning")
        }
        "sensor" | "number" | "input_number" | "button" | "scene" => false,
        _ => state == "on",
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn pretty_word(state: &str) -> String {
    capitalise(&state.replace('_', " "))
}

/// Format a numeric string: trims float noise, keeps at most 2 decimals unless
/// the source already had fewer.
pub fn format_number(raw: &str) -> String {
    let Ok(v) = raw.trim().parse::<f64>() else {
        return raw.to_string();
    };
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let decimals = raw
        .split('.')
        .nth(1)
        .map(|d| d.trim_end_matches('0').len())
        .unwrap_or(0);
    let d = decimals.min(2);
    format!("{v:.d$}")
}

fn with_unit(value: String, unit: Option<&str>) -> String {
    match unit {
        None | Some("") => value,
        Some("%") => format!("{value}%"),
        Some(u) => format!("{value} {u}"),
    }
}

fn binary_sensor_text(device_class: Option<&str>, on: bool) -> &'static str {
    match (device_class.unwrap_or(""), on) {
        ("connectivity", true) => "Connected",
        ("connectivity", false) => "Disconnected",
        ("door" | "window" | "opening" | "garage_door", true) => "Open",
        ("door" | "window" | "opening" | "garage_door", false) => "Closed",
        (
            "motion" | "occupancy" | "presence" | "sound" | "vibration" | "smoke" | "gas"
            | "carbon_monoxide" | "light",
            true,
        ) => "Detected",
        (
            "motion" | "occupancy" | "presence" | "sound" | "vibration" | "smoke" | "gas"
            | "carbon_monoxide" | "light",
            false,
        ) => "Clear",
        ("problem" | "safety" | "tamper", true) => "Problem",
        ("problem" | "safety" | "tamper", false) => "OK",
        ("battery", true) => "Low",
        ("battery", false) => "Normal",
        ("battery_charging", true) => "Charging",
        ("battery_charging", false) => "Not charging",
        ("plug" | "power", true) => "Plugged in",
        ("plug" | "power", false) => "Unplugged",
        ("lock", true) => "Unlocked",
        ("lock", false) => "Locked",
        ("moisture", true) => "Wet",
        ("moisture", false) => "Dry",
        ("running", true) => "Running",
        ("running", false) => "Not running",
        ("update", true) => "Update available",
        ("update", false) => "Up-to-date",
        ("cold", true) => "Cold",
        ("heat", true) => "Hot",
        ("cold" | "heat", false) => "Normal",
        ("moving", true) => "Moving",
        ("moving", false) => "Not moving",
        (_, true) => "On",
        (_, false) => "Off",
    }
}

/// Human readable state text for an entity (English).
pub fn format_state(entity_id: &str, st: &EntityState) -> String {
    format_state_tr(entity_id, st, timefmt::now_unix(), None)
}

pub fn format_state_at(entity_id: &str, st: &EntityState, now: f64) -> String {
    format_state_tr(entity_id, st, now, None)
}

/// State text using Home Assistant's translations when available.
pub fn format_state_tr(
    entity_id: &str,
    st: &EntityState,
    now: f64,
    tr: Option<&Translations>,
) -> String {
    let domain = domain_of(entity_id);
    let state = st.state.as_str();
    let dc = st.device_class();
    // Translated word for a plain state, falling back to a prettified English one.
    let word = |s: &str| -> String {
        tr.and_then(|t| t.state(domain, dc, s))
            .map(str::to_string)
            .unwrap_or_else(|| pretty_word(s))
    };
    match state {
        "unavailable" => return word_or(tr, domain, dc, state, "Unavailable"),
        "unknown" => return word_or(tr, domain, dc, state, "Unknown"),
        _ => {}
    }
    match domain {
        "sensor" | "number" | "input_number" => {
            if st.device_class() == Some("timestamp") {
                return timefmt::parse_rfc3339(state)
                    .map(|ts| timefmt::relative(ts, now))
                    .unwrap_or_else(|| state.to_string());
            }
            if st.device_class() == Some("enum") {
                return word(state);
            }
            if state.parse::<f64>().is_ok() {
                with_unit(format_number(state), st.unit())
            } else {
                word(state)
            }
        }
        "binary_sensor" => tr
            .and_then(|t| t.state(domain, dc, state))
            .map(str::to_string)
            .unwrap_or_else(|| binary_sensor_text(dc, state == "on").to_string()),
        "cover" => match st.attr_u32("current_position") {
            Some(p) if state == "open" && p < 100 => format!("{} · {p}%", word(state)),
            _ => word(state),
        },
        "light" => match (state, st.attr_u32("brightness")) {
            ("on", Some(b)) => format!("{}%", (f64::from(b) * 100.0 / 255.0).round() as u32),
            _ => word(state),
        },
        "fan" => match (state, st.attr_u32("percentage")) {
            ("on", Some(p)) => format!("{p}%"),
            _ => word(state),
        },
        "climate" => {
            let mut s = word(state);
            if let Some(t) = st.attr_f64("temperature") {
                s = format!(
                    "{s} · {}",
                    with_unit(format_number(&t.to_string()), Some("°"))
                );
            }
            s
        }
        "media_player" => match state {
            "playing" | "paused" => {
                let title = st.attr_str("media_title").unwrap_or("");
                if title.is_empty() {
                    word(state)
                } else {
                    format!("{} · {title}", word(state))
                }
            }
            _ => word(state),
        },
        "update" => match state {
            "on" => match (st.attr_bool("in_progress"), st.attr_str("latest_version")) {
                (Some(true), _) => "Installing…".into(),
                (_, Some(v)) => {
                    format!("{}: {v}", word_or(tr, domain, dc, "on", "Update available"))
                }
                _ => word_or(tr, domain, dc, "on", "Update available"),
            },
            "off" => word_or(tr, domain, dc, "off", "Up-to-date"),
            _ => word(state),
        },
        "person" | "device_tracker" => match state {
            "home" => word_or(tr, domain, dc, state, "Home"),
            "not_home" => word_or(tr, domain, dc, state, "Away"),
            other => word(other),
        },
        "button" | "scene" | "script" => {
            if domain == "script" && state == "on" {
                word_or(tr, domain, dc, state, "Running")
            } else if let Some(ts) = timefmt::parse_rfc3339(state) {
                timefmt::relative(ts, now)
            } else {
                word(state)
            }
        }
        _ => word(state),
    }
}

fn word_or(
    tr: Option<&Translations>,
    domain: &str,
    dc: Option<&str>,
    state: &str,
    fallback: &str,
) -> String {
    tr.and_then(|t| t.state(domain, dc, state))
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

/// Interactive control a row/tile offers.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    None,
    Toggle,
    Slider(SliderSpec),
    OpenCloseStop,
    Press,
    Run,
    Install,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SliderSpec {
    pub kind: SliderKind,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub value: f64,
    pub unit: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliderKind {
    BrightnessPct,
    FanPercentage,
    CoverPosition,
    CoverTilt,
    Number,
    Temperature,
}

/// The one control shown at the right of an `entities` row / glance tap.
pub fn primary_control(entity_id: &str, st: Option<&EntityState>) -> Control {
    let domain = domain_of(entity_id);
    match domain {
        d if is_toggleable(d) => Control::Toggle,
        "cover" => Control::OpenCloseStop,
        "button" => Control::Press,
        "script" | "scene" => Control::Run,
        "update" => {
            if st.is_some_and(|s| s.state == "on") {
                Control::Install
            } else {
                Control::None
            }
        }
        "number" | "input_number" => st
            .map(|s| Control::Slider(number_slider(s)))
            .unwrap_or(Control::None),
        _ => Control::None,
    }
}

fn number_slider(st: &EntityState) -> SliderSpec {
    SliderSpec {
        kind: SliderKind::Number,
        min: st.attr_f64("min").unwrap_or(0.0),
        max: st.attr_f64("max").unwrap_or(100.0),
        step: st.attr_f64("step").unwrap_or(1.0),
        value: st.numeric_state().unwrap_or(0.0),
        unit: "",
    }
}

/// Controls for a tile card's feature list.
pub fn feature_control(entity_id: &str, feature: &Feature, st: Option<&EntityState>) -> Control {
    let domain = domain_of(entity_id);
    match feature {
        Feature::Toggle => Control::Toggle,
        Feature::CoverOpenClose | Feature::CoverTilt => Control::OpenCloseStop,
        Feature::CoverPosition => Control::Slider(SliderSpec {
            kind: SliderKind::CoverPosition,
            min: 0.0,
            max: 100.0,
            step: 1.0,
            value: st
                .and_then(|s| s.attr_f64("current_position"))
                .unwrap_or(0.0),
            unit: "%",
        }),
        Feature::CoverTiltPosition => Control::Slider(SliderSpec {
            kind: SliderKind::CoverTilt,
            min: 0.0,
            max: 100.0,
            step: 1.0,
            value: st
                .and_then(|s| s.attr_f64("current_tilt_position"))
                .unwrap_or(0.0),
            unit: "%",
        }),
        Feature::LightBrightness => Control::Slider(SliderSpec {
            kind: SliderKind::BrightnessPct,
            min: 0.0,
            max: 100.0,
            step: 1.0,
            value: st
                .filter(|s| s.state == "on")
                .and_then(|s| s.attr_f64("brightness"))
                .map(|b| (b * 100.0 / 255.0).round())
                .unwrap_or(0.0),
            unit: "%",
        }),
        Feature::FanSpeed => Control::Slider(SliderSpec {
            kind: SliderKind::FanPercentage,
            min: 0.0,
            max: 100.0,
            step: st
                .and_then(|s| s.attr_f64("percentage_step"))
                .unwrap_or(1.0)
                .max(1.0),
            value: st
                .filter(|s| s.state == "on")
                .and_then(|s| s.attr_f64("percentage"))
                .unwrap_or(0.0),
            unit: "%",
        }),
        Feature::NumericInput => st
            .map(|s| Control::Slider(number_slider(s)))
            .unwrap_or(Control::None),
        Feature::TargetTemperature => Control::Slider(SliderSpec {
            kind: SliderKind::Temperature,
            min: st.and_then(|s| s.attr_f64("min_temp")).unwrap_or(7.0),
            max: st.and_then(|s| s.attr_f64("max_temp")).unwrap_or(35.0),
            step: st
                .and_then(|s| s.attr_f64("target_temp_step"))
                .unwrap_or(0.5),
            value: st.and_then(|s| s.attr_f64("temperature")).unwrap_or(20.0),
            unit: "°",
        }),
        Feature::UpdateActions => {
            if st.is_some_and(|s| s.state == "on") {
                Control::Install
            } else {
                Control::None
            }
        }
        Feature::Button { .. } => match domain {
            "button" => Control::Press,
            _ => Control::Run,
        },
        Feature::Unknown(_) => Control::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn domains_and_activity() {
        assert_eq!(domain_of("switch.table_lamp"), "switch");
        assert!(is_toggleable("switch"));
        assert!(!is_toggleable("sensor"));
        assert!(is_active("switch", "on"));
        assert!(!is_active("switch", "off"));
        assert!(is_active("cover", "open"));
        assert!(!is_active("cover", "closed"));
        assert!(is_active("media_player", "playing"));
        assert!(!is_active("media_player", "standby"));
        assert!(is_active("person", "home"));
        assert!(!is_active("person", "not_home"));
        assert!(!is_active("sensor", "21.5"));
        assert!(!is_active("switch", "unavailable"));
    }

    #[test]
    fn formatting() {
        let t = EntityState::new("21.333").with_attr("unit_of_measurement", json!("°C"));
        assert_eq!(format_state_at("sensor.t", &t, 0.0), "21.33 °C");
        let pct = EntityState::new("85").with_attr("unit_of_measurement", json!("%"));
        assert_eq!(format_state_at("sensor.b", &pct, 0.0), "85%");
        let raw = EntityState::new("12.50").with_attr("unit_of_measurement", json!("MB/s"));
        assert_eq!(format_state_at("sensor.x", &raw, 0.0), "12.5 MB/s");
        let up = EntityState::new("1970-01-01T00:00:00+00:00")
            .with_attr("device_class", json!("timestamp"));
        assert_eq!(format_state_at("sensor.uptime", &up, 90_000.0), "1 d 1 h");
        let conn = EntityState::new("on").with_attr("device_class", json!("connectivity"));
        assert_eq!(format_state_at("binary_sensor.c", &conn, 0.0), "Connected");
        let cov = EntityState::new("open").with_attr("current_position", json!(70));
        assert_eq!(format_state_at("cover.c", &cov, 0.0), "Open · 70%");
        let light = EntityState::new("on").with_attr("brightness", json!(128));
        assert_eq!(format_state_at("light.l", &light, 0.0), "50%");
        let upd = EntityState::new("on").with_attr("latest_version", json!("8.03"));
        assert_eq!(
            format_state_at("update.u", &upd, 0.0),
            "Update available: 8.03"
        );
        assert_eq!(
            format_state_at("update.u", &EntityState::new("off"), 0.0),
            "Up-to-date"
        );
        assert_eq!(
            format_state_at("person.p", &EntityState::new("not_home"), 0.0),
            "Away"
        );
        assert_eq!(
            format_state_at("switch.s", &EntityState::new("unavailable"), 0.0),
            "Unavailable"
        );
        let mp = EntityState::new("playing").with_attr("media_title", json!("Rai Radio 2"));
        assert_eq!(
            format_state_at("media_player.m", &mp, 0.0),
            "Playing · Rai Radio 2"
        );
        assert_eq!(format_number("1234"), "1234");
        assert_eq!(format_number("0.1000"), "0.1");
    }

    #[test]
    fn translated_states() {
        let tr = Translations::from_json_str(include_str!(
            "../../../tests/fixtures/translations_it.json"
        ))
        .unwrap();
        let tr = Some(&tr);
        assert_eq!(
            format_state_tr("switch.a", &EntityState::new("off"), 0.0, tr),
            "Spento"
        );
        let conn = EntityState::new("off").with_attr("device_class", json!("connectivity"));
        assert_eq!(
            format_state_tr("binary_sensor.c", &conn, 0.0, tr),
            "Disconnesso"
        );
        let cov = EntityState::new("open").with_attr("current_position", json!(70));
        assert_eq!(format_state_tr("cover.c", &cov, 0.0, tr), "Aperto · 70%");
        assert_eq!(
            format_state_tr("person.p", &EntityState::new("not_home"), 0.0, tr),
            "Fuori casa"
        );
        assert_eq!(
            format_state_tr("update.u", &EntityState::new("off"), 0.0, tr),
            "Aggiornato"
        );
        // Numbers and units are untouched.
        let t = EntityState::new("21.4").with_attr("unit_of_measurement", json!("°C"));
        assert_eq!(format_state_tr("sensor.t", &t, 0.0, tr), "21.4 °C");
    }

    #[test]
    fn controls() {
        assert_eq!(primary_control("switch.a", None), Control::Toggle);
        assert_eq!(primary_control("script.a", None), Control::Run);
        assert_eq!(primary_control("button.a", None), Control::Press);
        assert_eq!(primary_control("cover.a", None), Control::OpenCloseStop);
        assert_eq!(primary_control("sensor.a", None), Control::None);
        assert_eq!(
            primary_control("update.a", Some(&EntityState::new("on"))),
            Control::Install
        );
        assert_eq!(
            primary_control("update.a", Some(&EntityState::new("off"))),
            Control::None
        );
        let cov = EntityState::new("open").with_attr("current_position", json!(40));
        match feature_control("cover.a", &Feature::CoverPosition, Some(&cov)) {
            Control::Slider(s) => {
                assert_eq!(s.kind, SliderKind::CoverPosition);
                assert_eq!(s.value, 40.0);
            }
            other => panic!("{other:?}"),
        }
        let light = EntityState::new("on").with_attr("brightness", json!(255));
        match feature_control("light.a", &Feature::LightBrightness, Some(&light)) {
            Control::Slider(s) => assert_eq!(s.value, 100.0),
            other => panic!("{other:?}"),
        }
    }
}
