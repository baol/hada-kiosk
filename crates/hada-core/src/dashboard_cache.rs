//! The last dashboard from Home Assistant, kept on disk in a compact binary form
//! close to [`Dashboard`], so the app can show it at startup without fetching,
//! parsing and normalising the JSON config.
//!
//! File: `b"HADA"`, format version (u16 LE), model hash (u64 LE), then the
//! [postcard](https://postcard.jamesmunns.com/) encoding of [`Stored`]. A different
//! format version or model hash (the model's source changed) means the file is
//! ignored, never mis-decoded.

use crate::model::Dashboard;
use serde::{Deserialize, Serialize};
use std::path::Path;

const MAGIC: &[u8; 4] = b"HADA";
/// Bump when the file layout changes (the model itself is covered by the hash).
pub const FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 4 + 2 + 8;

fn model_hash() -> u64 {
    env!("HADA_MODEL_HASH").parse().expect("set by build.rs")
}

/// FNV-1a 64 of a dashboard config as received, to notice changes without parsing.
pub fn source_hash(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Which dashboard a cache belongs to: Home Assistant address + `url_path`.
pub fn cache_key(base_url: &str, url_path: Option<&str>) -> String {
    format!(
        "{}|{}",
        base_url.trim_end_matches('/'),
        url_path.unwrap_or("")
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub key: String,
    /// [`source_hash`] of the config text the dashboard was made from.
    pub source_hash: u64,
    pub dashboard: Dashboard,
}

pub fn encode(stored: &Stored) -> Vec<u8> {
    let mut out = Vec::with_capacity(4096);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&model_hash().to_le_bytes());
    out.extend(postcard::to_stdvec(stored).expect("the model always serialises"));
    out
}

/// `None` (with the reason logged) unless the bytes are a cache of this build's
/// format and model for `key`.
pub fn decode(bytes: &[u8], key: &str) -> Option<Stored> {
    let why = |reason: &str| {
        tracing::info!("dashboard cache not used: {reason}");
        None
    };
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
        return why("not a hada cache");
    }
    if u16::from_le_bytes([bytes[4], bytes[5]]) != FORMAT_VERSION {
        return why("other format version");
    }
    if u64::from_le_bytes(bytes[6..14].try_into().ok()?) != model_hash() {
        return why("written by a build with another dashboard model");
    }
    match postcard::from_bytes::<Stored>(&bytes[HEADER_LEN..]) {
        Ok(s) if s.key == key => Some(s),
        Ok(_) => why("for another Home Assistant or dashboard"),
        Err(e) => why(&format!("cannot decode ({e})")),
    }
}

pub fn load(path: &Path, key: &str) -> Option<Stored> {
    let bytes = std::fs::read(path).ok()?;
    decode(&bytes, key)
}

/// Written to a temporary file first, so a crash never leaves half a cache.
pub fn save(path: &Path, stored: &Stored) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, encode(stored))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lovelace::LovelaceConfig;
    use crate::model::{ActionSpec, CardKind, Layout};
    use crate::normalise::normalise;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/dashboard.yaml");

    fn demo() -> Dashboard {
        normalise(&LovelaceConfig::from_yaml_str(FIXTURE).unwrap())
    }

    /// The demo dashboard plus the card kinds it does not use.
    fn everything() -> Dashboard {
        use crate::lovelace::{Segment, Severity};
        use crate::model::{CardNode, EntityRef, GridOpts};
        let mut d = demo();
        let grid = GridOpts {
            columns: 6,
            rows: Some(4),
            auto_columns: false,
        };
        let Layout::Masonry(cards) = &mut d.views[0].layout else {
            panic!()
        };
        cards.push(CardNode {
            kind: CardKind::Gauge {
                entity: EntityRef::new("sensor.cpu"),
                min: 0.0,
                max: 100.0,
                unit: Some("%".into()),
                needle: true,
                severity: Some(Severity {
                    green: 0.0,
                    yellow: 60.0,
                    red: 85.0,
                }),
                segments: vec![Segment {
                    from: 0.0,
                    color: "#00ff00".into(),
                    label: Some("ok".into()),
                }],
            },
            title: Some("CPU".into()),
            grid,
            raw_type: "gauge".into(),
        });
        cards.push(CardNode {
            kind: CardKind::Button {
                entity: Some(EntityRef::new("script.night")),
                name: None,
                icon: Some("mdi:weather-night".into()),
                action: Some(ActionSpec::Navigate("/lovelace/kiosk".into())),
            },
            title: None,
            grid,
            raw_type: "button".into(),
        });
        d
    }

    #[test]
    fn round_trip_of_every_card_kind() {
        let d = everything();
        let all = format!("{d:?}");
        for kind in [
            "Tile",
            "Entities",
            "Glance",
            "Sensor",
            "Gauge",
            "Grid",
            "Markdown",
            "Button",
            "Calendar",
            "Weather",
            "Media",
            "Unsupported",
        ] {
            assert!(
                all.contains(&format!("{kind} {{")),
                "test dashboard lacks {kind}"
            );
        }
        let key = cache_key("http://ha.local:8123/", None);
        let s = Stored {
            key: key.clone(),
            source_hash: 42,
            dashboard: d.clone(),
        };
        let bytes = encode(&s);
        assert_eq!(&bytes[..4], b"HADA");
        assert_eq!(decode(&bytes, &key), Some(s));
        // Compact: far smaller than the YAML it came from.
        assert!(
            bytes.len() < FIXTURE.len(),
            "{} >= {}",
            bytes.len(),
            FIXTURE.len()
        );
    }

    #[test]
    fn json_action_data_survives() {
        let mut d = demo();
        let Layout::Masonry(cards) = &mut d.views[0].layout else {
            panic!()
        };
        cards[0].kind = CardKind::Button {
            entity: None,
            name: Some("Scene".into()),
            icon: None,
            action: Some(ActionSpec::PerformAction {
                domain: "scene".into(),
                service: "turn_on".into(),
                data: serde_json::json!({"transition": 2.5, "x": [1, "a", null]}),
                target: serde_json::json!({"entity_id": "scene.evening"}),
            }),
        };
        let s = Stored {
            key: "k".into(),
            source_hash: 1,
            dashboard: d,
        };
        assert_eq!(decode(&encode(&s), "k"), Some(s));
    }

    #[test]
    fn refuses_foreign_or_broken_files() {
        let s = Stored {
            key: "k".into(),
            source_hash: 1,
            dashboard: demo(),
        };
        let good = encode(&s);
        assert!(decode(&good, "other").is_none(), "other key");
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(decode(&bad, "k").is_none(), "magic");
        let mut bad = good.clone();
        bad[4] ^= 0xff;
        assert!(decode(&bad, "k").is_none(), "format version");
        let mut bad = good.clone();
        bad[6] ^= 0xff;
        assert!(decode(&bad, "k").is_none(), "model hash");
        assert!(decode(&good[..good.len() / 2], "k").is_none(), "truncated");
        assert!(decode(b"", "k").is_none());
    }

    #[test]
    fn saves_atomically_and_loads() {
        let dir = std::env::temp_dir().join(format!("hada-dash-{}", std::process::id()));
        let path = dir.join("dashboard.bin");
        let s = Stored {
            key: "k".into(),
            source_hash: 7,
            dashboard: demo(),
        };
        save(&path, &s).unwrap();
        assert!(!path.with_extension("tmp").exists());
        assert_eq!(load(&path, "k"), Some(s));
        assert_eq!(load(&dir.join("missing.bin"), "k"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_hash_is_stable() {
        assert_eq!(source_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(source_hash("a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(
            source_hash("{\"views\":[]}"),
            source_hash("{\"views\":[1]}")
        );
    }
}
