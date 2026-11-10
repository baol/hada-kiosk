//! Weather entities and forecasts for the `weather-forecast` card.
//!
//! The entity state is the condition (`sunny`, `partlycloudy`, …); current values
//! are attributes. Forecasts are not attributes: they arrive through the
//! `weather/subscribe_forecast` websocket subscription, one list per forecast type.

use crate::state::EntityState;
use crate::translations::Translations;
use serde::Deserialize;

/// `forecast_type` of `weather/subscribe_forecast`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ForecastType {
    Daily,
    Hourly,
    TwiceDaily,
}

impl ForecastType {
    pub const ALL: [ForecastType; 3] = [
        ForecastType::Daily,
        ForecastType::Hourly,
        ForecastType::TwiceDaily,
    ];

    pub fn parse(s: &str) -> Option<ForecastType> {
        match s {
            "daily" => Some(ForecastType::Daily),
            "hourly" => Some(ForecastType::Hourly),
            "twice_daily" => Some(ForecastType::TwiceDaily),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ForecastType::Daily => "daily",
            ForecastType::Hourly => "hourly",
            ForecastType::TwiceDaily => "twice_daily",
        }
    }

    /// `WeatherEntityFeature` bit in `supported_features`.
    pub fn feature_flag(self) -> u32 {
        match self {
            ForecastType::Daily => 1,
            ForecastType::Hourly => 2,
            ForecastType::TwiceDaily => 4,
        }
    }

    /// Types to try, in order: the configured one, then daily, hourly, twice_daily
    /// (the order the Home Assistant frontend uses when none is configured).
    pub fn candidates(wanted: Option<ForecastType>) -> Vec<ForecastType> {
        let mut out: Vec<ForecastType> = wanted.into_iter().collect();
        out.extend(Self::ALL.iter().copied().filter(|t| Some(*t) != wanted));
        out
    }
}

/// One forecast entry, as sent by Home Assistant (converted units, non-native keys).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ForecastItem {
    /// ISO date-time, e.g. `2026-11-10T12:00:00+00:00`.
    pub datetime: String,
    pub condition: Option<String>,
    pub temperature: Option<f64>,
    pub templow: Option<f64>,
    pub precipitation: Option<f64>,
    pub precipitation_probability: Option<f64>,
    pub is_daytime: Option<bool>,
}

/// Parse the `forecast` field of a subscription event (`null` = no forecast).
pub fn parse_forecast(v: &serde_json::Value) -> Vec<ForecastItem> {
    let Some(list) = v.as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|e| serde_json::from_value::<ForecastItem>(e.clone()).ok())
        .filter(|e| !e.datetime.is_empty())
        .collect()
}

/// How a weather card shows the current conditions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WeatherDisplay {
    /// `secondary_info_attribute`: attribute shown under the temperature.
    pub secondary_attribute: Option<String>,
    pub round_temperature: bool,
}

/// "14.2 °C" (or "14 °C" when rounding). An empty unit prints the number only.
pub fn format_temperature(v: f64, unit: &str, round: bool) -> String {
    let n = if round {
        format!("{}", v.round() as i64)
    } else {
        let r = (v * 10.0).round() / 10.0;
        if r.fract() == 0.0 {
            format!("{}", r as i64)
        } else {
            format!("{r:.1}")
        }
    };
    if unit.is_empty() {
        n
    } else {
        format!("{n} {unit}")
    }
}

/// Short forecast temperature: "15°".
pub fn short_temperature(v: f64) -> String {
    format!("{}°", v.round() as i64)
}

/// Condition text: Home Assistant's translation, else the English name.
pub fn condition_text(condition: &str, tr: Option<&Translations>) -> String {
    if let Some(t) = tr.and_then(|t| t.state("weather", None, condition)) {
        return t.to_string();
    }
    match condition {
        "clear-night" => "Clear, night",
        "cloudy" => "Cloudy",
        "exceptional" => "Exceptional",
        "fog" => "Fog",
        "hail" => "Hail",
        "lightning" => "Lightning",
        "lightning-rainy" => "Lightning, rainy",
        "partlycloudy" => "Partly cloudy",
        "pouring" => "Pouring",
        "rainy" => "Rainy",
        "snowy" => "Snowy",
        "snowy-rainy" => "Snowy, rainy",
        "sunny" => "Sunny",
        "windy" => "Windy",
        "windy-variant" => "Windy",
        other => return other.to_string(),
    }
    .to_string()
}

/// Built-in condition icons, used before Home Assistant's icon table is loaded.
pub fn condition_icon(condition: &str) -> &'static str {
    match condition {
        "clear-night" => "mdi:weather-night",
        "cloudy" => "mdi:weather-cloudy",
        "exceptional" => "mdi:alert-circle-outline",
        "fog" => "mdi:weather-fog",
        "hail" => "mdi:weather-hail",
        "lightning" => "mdi:weather-lightning",
        "lightning-rainy" => "mdi:weather-lightning-rainy",
        "pouring" => "mdi:weather-pouring",
        "rainy" => "mdi:weather-rainy",
        "snowy" => "mdi:weather-snowy",
        "snowy-rainy" => "mdi:weather-snowy-rainy",
        "sunny" => "mdi:weather-sunny",
        "windy" => "mdi:weather-windy",
        "windy-variant" => "mdi:weather-windy-variant",
        _ => "mdi:weather-partly-cloudy",
    }
}

/// Current temperature of a weather entity, with its unit.
pub fn current_temperature(st: &EntityState, round: bool) -> Option<String> {
    let t = st.attr_f64("temperature")?;
    Some(format_temperature(
        t,
        st.attr_str("temperature_unit").unwrap_or(""),
        round,
    ))
}

/// Line under the temperature: the configured attribute, or humidity and wind.
pub fn secondary_line(st: &EntityState, attribute: Option<&str>) -> String {
    let number = |key: &str| -> Option<String> {
        let v = st.attr_f64(key)?;
        let r = (v * 10.0).round() / 10.0;
        Some(if r.fract() == 0.0 {
            format!("{}", r as i64)
        } else {
            format!("{r:.1}")
        })
    };
    let with_unit = |key: &str| -> Option<String> {
        let n = number(key)?;
        let unit = match key {
            "humidity" | "cloud_coverage" | "precipitation_probability" => Some("%"),
            "pressure" => st.attr_str("pressure_unit"),
            "wind_speed" | "wind_gust_speed" => st.attr_str("wind_speed_unit"),
            "visibility" => st.attr_str("visibility_unit"),
            "temperature" | "apparent_temperature" | "dew_point" => st.attr_str("temperature_unit"),
            "precipitation" => st.attr_str("precipitation_unit"),
            _ => None,
        };
        Some(match unit {
            Some("%") => format!("{n}%"),
            Some(u) if !u.is_empty() => format!("{n} {u}"),
            _ => n,
        })
    };
    if let Some(a) = attribute.filter(|a| !a.is_empty()) {
        return with_unit(a)
            .or_else(|| st.attr_str(a).map(str::to_string))
            .unwrap_or_default();
    }
    let mut parts = Vec::new();
    if let Some(h) = with_unit("humidity") {
        parts.push(h);
    }
    if let Some(w) = with_unit("wind_speed") {
        let dir = st
            .attr_f64("wind_bearing")
            .map(compass)
            .or_else(|| st.attr_str("wind_bearing").map(str::to_string));
        parts.push(match dir {
            Some(d) => format!("{w} {d}"),
            None => w,
        });
    }
    parts.join(" · ")
}

/// 16-point compass direction for a bearing in degrees.
pub fn compass(bearing: f64) -> String {
    const POINTS: [&str; 16] = [
        "N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW",
        "NW", "NNW",
    ];
    let i = ((bearing.rem_euclid(360.0) / 22.5).round() as usize) % 16;
    POINTS[i].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn candidate_order() {
        use ForecastType::*;
        assert_eq!(
            ForecastType::candidates(None),
            vec![Daily, Hourly, TwiceDaily]
        );
        assert_eq!(
            ForecastType::candidates(Some(Hourly)),
            vec![Hourly, Daily, TwiceDaily]
        );
        assert_eq!(ForecastType::parse("twice_daily"), Some(TwiceDaily));
        assert_eq!(ForecastType::parse("weekly"), None);
        assert_eq!(TwiceDaily.feature_flag(), 4);
    }

    #[test]
    fn parses_forecast_leniently() {
        let v = json!([
            {"datetime": "2026-11-10T12:00:00+00:00", "condition": "rainy",
             "temperature": 15.4, "templow": 9.0, "precipitation_probability": 80},
            {"datetime": "2026-11-11T12:00:00+00:00", "temperature": null},
            {"condition": "sunny"},
            "garbage"
        ]);
        let f = parse_forecast(&v);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].condition.as_deref(), Some("rainy"));
        assert_eq!(f[0].templow, Some(9.0));
        assert_eq!(f[0].precipitation_probability, Some(80.0));
        assert_eq!(f[1].temperature, None);
        assert!(parse_forecast(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn formats_values() {
        assert_eq!(format_temperature(14.24, "°C", false), "14.2 °C");
        assert_eq!(format_temperature(14.0, "°C", false), "14 °C");
        assert_eq!(format_temperature(14.6, "°F", true), "15 °F");
        assert_eq!(format_temperature(-0.4, "", true), "0");
        assert_eq!(short_temperature(8.5), "9°");
        assert_eq!(compass(0.0), "N");
        assert_eq!(compass(200.0), "SSW");
        assert_eq!(compass(359.0), "N");
    }

    #[test]
    fn current_and_secondary() {
        let st = EntityState::new("partlycloudy")
            .with_attr("temperature", json!(14.2))
            .with_attr("temperature_unit", json!("°C"))
            .with_attr("humidity", json!(71))
            .with_attr("wind_speed", json!(12.6))
            .with_attr("wind_speed_unit", json!("km/h"))
            .with_attr("wind_bearing", json!(225))
            .with_attr("pressure", json!(1013))
            .with_attr("pressure_unit", json!("hPa"));
        assert_eq!(current_temperature(&st, false).as_deref(), Some("14.2 °C"));
        assert_eq!(current_temperature(&st, true).as_deref(), Some("14 °C"));
        assert_eq!(secondary_line(&st, None), "71% · 12.6 km/h SW");
        assert_eq!(secondary_line(&st, Some("pressure")), "1013 hPa");
        assert_eq!(secondary_line(&st, Some("missing")), "");
        assert_eq!(condition_text("partlycloudy", None), "Partly cloudy");
        let mut tr = Translations::default();
        tr.resources.insert(
            "component.weather.entity_component._.state.partlycloudy".into(),
            "Parzialmente nuvoloso".into(),
        );
        assert_eq!(
            condition_text("partlycloudy", Some(&tr)),
            "Parzialmente nuvoloso"
        );
        assert_eq!(condition_icon("rainy"), "mdi:weather-rainy");
    }
}
