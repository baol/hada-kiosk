//! Media players for the `media-control` card: which controls a player offers,
//! where its artwork is, and how far the track has played.

use crate::state::EntityState;
use crate::timefmt::parse_rfc3339;

/// `MediaPlayerEntityFeature` bits (Home Assistant `media_player/const.py`).
const PAUSE: u32 = 1;
const VOLUME_SET: u32 = 4;
const VOLUME_MUTE: u32 = 8;
const PREVIOUS_TRACK: u32 = 16;
const NEXT_TRACK: u32 = 32;
const TURN_ON: u32 = 128;
const TURN_OFF: u32 = 256;
const PLAY: u32 = 16384;

/// The controls a player supports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MediaFeatures {
    pub play_pause: bool,
    pub previous: bool,
    pub next: bool,
    pub power: bool,
    pub volume: bool,
    pub mute: bool,
}

impl MediaFeatures {
    pub fn from_flags(flags: u32) -> Self {
        let has = |f: u32| flags & f != 0;
        MediaFeatures {
            play_pause: has(PAUSE) || has(PLAY),
            previous: has(PREVIOUS_TRACK),
            next: has(NEXT_TRACK),
            power: has(TURN_ON) || has(TURN_OFF),
            volume: has(VOLUME_SET),
            mute: has(VOLUME_MUTE),
        }
    }

    pub fn of(st: &EntityState) -> Self {
        Self::from_flags(st.attr_u32("supported_features").unwrap_or(0))
    }
}

/// Position and duration in seconds. While playing, the position moves on from
/// `media_position_updated_at` (as the Home Assistant frontend does).
pub fn progress(st: &EntityState, now: f64) -> Option<(f64, f64)> {
    let duration = st.attr_f64("media_duration").filter(|d| *d > 0.0)?;
    let mut position = st.attr_f64("media_position").unwrap_or(0.0);
    if st.state == "playing" {
        if let Some(at) = st
            .attr_str("media_position_updated_at")
            .and_then(parse_rfc3339)
        {
            position += (now - at).max(0.0);
        }
    }
    Some((position.clamp(0.0, duration), duration))
}

/// "3:07", "1:02:05".
pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0).floor() as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Where to download the artwork: Home Assistant's proxy (`entity_picture_local`)
/// when there is one, else `entity_picture`. Relative paths are joined to `base`.
pub fn artwork_url(base: &str, st: &EntityState) -> Option<String> {
    if matches!(st.state.as_str(), "off" | "unavailable" | "unknown") {
        return None;
    }
    let pic = st
        .attr_str("entity_picture_local")
        .or_else(|| st.attr_str("entity_picture"))
        .filter(|p| !p.is_empty())?;
    if pic.starts_with("http://") || pic.starts_with("https://") {
        Some(pic.to_string())
    } else if pic.starts_with('/') {
        Some(format!("{}{pic}", base.trim_end_matches('/')))
    } else {
        None
    }
}

/// "Artist · Album" (either may be missing).
pub fn subtitle(st: &EntityState) -> String {
    [st.attr_str("media_artist"), st.attr_str("media_album_name")]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn features() {
        let f = MediaFeatures::from_flags(1 | 4 | 8 | 16 | 32 | 128 | 256 | 16384);
        assert!(f.play_pause && f.previous && f.next && f.power && f.volume && f.mute);
        let tv = MediaFeatures::from_flags(128 | 256);
        assert_eq!(
            tv,
            MediaFeatures {
                power: true,
                ..Default::default()
            }
        );
        assert!(MediaFeatures::from_flags(16384).play_pause);
    }

    #[test]
    fn progress_moves_while_playing() {
        let st = EntityState::new("playing")
            .with_attr("media_duration", json!(200))
            .with_attr("media_position", json!(60))
            .with_attr(
                "media_position_updated_at",
                json!("1970-01-01T00:01:40+00:00"),
            );
        assert_eq!(progress(&st, 110.0), Some((70.0, 200.0)));
        assert_eq!(progress(&st, 10_000.0), Some((200.0, 200.0)), "clamped");
        let paused = EntityState {
            state: "paused".into(),
            ..st.clone()
        };
        assert_eq!(progress(&paused, 110.0), Some((60.0, 200.0)));
        assert_eq!(progress(&EntityState::new("playing"), 0.0), None);
        assert_eq!(fmt_time(187.9), "3:07");
        assert_eq!(fmt_time(3725.0), "1:02:05");
    }

    #[test]
    fn artwork_urls() {
        let base = "http://ha.local:8123/";
        let proxy = "/api/media_player_proxy/media_player.m?token=t&cache=c";
        let st = EntityState::new("playing")
            .with_attr("entity_picture", json!("https://i.example.com/cover.jpg"))
            .with_attr("entity_picture_local", json!(proxy));
        assert_eq!(
            artwork_url(base, &st).as_deref(),
            Some("http://ha.local:8123/api/media_player_proxy/media_player.m?token=t&cache=c")
        );
        let remote = EntityState::new("paused")
            .with_attr("entity_picture", json!("https://i.example.com/cover.jpg"));
        assert_eq!(
            artwork_url(base, &remote).as_deref(),
            Some("https://i.example.com/cover.jpg")
        );
        let off = EntityState {
            state: "off".into(),
            ..st.clone()
        };
        assert_eq!(artwork_url(base, &off), None);
        assert_eq!(artwork_url(base, &EntityState::new("idle")), None);
        let full = EntityState::new("playing")
            .with_attr("media_artist", json!("Artist"))
            .with_attr("media_album_name", json!("Album"));
        assert_eq!(subtitle(&full), "Artist · Album");
        assert_eq!(subtitle(&EntityState::new("playing")), "");
    }
}
