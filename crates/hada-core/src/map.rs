//! The map card: which part of the world shows its entities, and that part as one
//! image, stitched on the network thread from downloaded (and disk-cached) tiles.
//!
//! The tiles come from the OpenStreetMap tile server (its usage policy asks for an
//! identifying User-Agent, caching and the credit in [`ATTRIBUTION`]). OSM only
//! has a light style; the dark one is the same tiles with their colours turned
//! dark, like the usual dark-map filters.

use crate::artwork::Artwork;
use crate::ha::http;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Edge of one tile in pixels.
pub const TILE: f64 = 256.0;

/// A view only moves when its centre moves this many pixels, so small GPS
/// jitter does not download new tiles.
const SNAP: f64 = 64.0;

/// Cached tiles are downloaded again after this long.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

pub const ATTRIBUTION: &str = "© OpenStreetMap contributors";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapStyle {
    Dark,
    Light,
}

impl MapStyle {
    /// Shown where there is no tile (outside the world, or while downloading).
    fn background(self) -> image::Rgba<u8> {
        match self {
            MapStyle::Dark => image::Rgba([38, 38, 38, 255]),
            MapStyle::Light => image::Rgba([250, 250, 248, 255]),
        }
    }
}

/// A map image: `width` × `height` pixels whose top-left corner is at world
/// pixel (`left`, `top`) at `zoom` (Web Mercator, [`TILE`] pixels per tile).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapView {
    pub zoom: u8,
    pub left: i64,
    pub top: i64,
    pub width: u32,
    pub height: u32,
}

/// World pixel of a coordinate at `zoom`.
pub fn world(lat: f64, lon: f64, zoom: u8) -> (f64, f64) {
    let n = TILE * f64::from(1u32 << zoom);
    let lat = lat.clamp(-85.0511, 85.0511).to_radians();
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    (x, y)
}

/// The view of `width` × `height` pixels at `max_zoom`, or less if needed, that
/// shows every `(lat, lon)` point with some margin. `None` without points.
pub fn fit(points: &[(f64, f64)], max_zoom: u8, width: u32, height: u32) -> Option<MapView> {
    if points.is_empty() || width == 0 || height == 0 {
        return None;
    }
    let mut zoom = max_zoom.min(19);
    loop {
        let px: Vec<(f64, f64)> = points.iter().map(|&(la, lo)| world(la, lo, zoom)).collect();
        let span = |f: fn(&(f64, f64)) -> f64| {
            let lo = px.iter().map(f).fold(f64::INFINITY, f64::min);
            let hi = px.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
            (lo, hi)
        };
        let ((x0, x1), (y0, y1)) = (span(|p| p.0), span(|p| p.1));
        let fits = x1 - x0 <= f64::from(width) * 0.8 && y1 - y0 <= f64::from(height) * 0.8;
        if fits || zoom == 0 {
            let cx = ((x0 + x1) / 2.0 / SNAP).round() * SNAP;
            let cy = ((y0 + y1) / 2.0 / SNAP).round() * SNAP;
            return Some(MapView {
                zoom,
                left: (cx - f64::from(width) / 2.0).round() as i64,
                top: (cy - f64::from(height) / 2.0).round() as i64,
                width,
                height,
            });
        }
        zoom -= 1;
    }
}

impl MapView {
    /// Where `(lat, lon)` falls on the image, as fractions of its width and height
    /// (outside 0..1 when it is off the image).
    pub fn project(&self, lat: f64, lon: f64) -> (f64, f64) {
        let (x, y) = world(lat, lon, self.zoom);
        (
            (x - self.left as f64) / f64::from(self.width),
            (y - self.top as f64) / f64::from(self.height),
        )
    }

    /// The tiles under the view, as (x, y) tile indexes; x may wrap around the world.
    fn tiles(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        let t = TILE as i64;
        let n = 1i64 << self.zoom;
        let (tx0, ty0) = (self.left.div_euclid(t), self.top.div_euclid(t));
        let tx1 = (self.left + i64::from(self.width) - 1).div_euclid(t);
        let ty1 = (self.top + i64::from(self.height) - 1).div_euclid(t);
        (ty0.max(0)..=ty1.min(n - 1)).flat_map(move |ty| (tx0..=tx1).map(move |tx| (tx, ty)))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("{0}")]
    Http(#[from] http::HttpError),
    #[error("HTTP {0} for {1}")]
    Status(u16, String),
    #[error("cannot decode tile {0}: {1}")]
    Decode(String, String),
}

/// Download (or read from `cache`) the tiles under `view` and stitch them.
pub async fn render(
    view: &MapView,
    style: MapStyle,
    cache: Option<&Path>,
) -> Result<Artwork, MapError> {
    let mut canvas = image::RgbaImage::from_pixel(view.width, view.height, style.background());
    let n = 1i64 << view.zoom;
    let t = TILE as i64;
    for (tx, ty) in view.tiles() {
        let tile = load(view.zoom, tx.rem_euclid(n), ty, cache).await?;
        image::imageops::overlay(&mut canvas, &tile, tx * t - view.left, ty * t - view.top);
    }
    if style == MapStyle::Dark {
        canvas.pixels_mut().for_each(darken);
    }
    Ok(Artwork {
        width: view.width,
        height: view.height,
        rgba: canvas.into_raw(),
    })
}

/// Invert, then turn the hues back half a circle: light map → dark map with water
/// still blue and parks still green (CSS `invert(1) hue-rotate(180deg)`), dimmed a bit.
fn darken(p: &mut image::Rgba<u8>) {
    let [r, g, b] = [p[0], p[1], p[2]].map(|c| f32::from(255 - c));
    let rotate = |m: [f32; 3]| ((m[0] * r + m[1] * g + m[2] * b) * 0.85).clamp(0.0, 255.0) as u8;
    p[0] = rotate([-0.574, 1.43, 0.144]);
    p[1] = rotate([0.426, 0.43, 0.144]);
    p[2] = rotate([0.426, 1.43, -0.856]);
}

fn cache_file(cache: &Path, z: u8, x: i64, y: i64) -> PathBuf {
    cache
        .join("osm")
        .join(z.to_string())
        .join(x.to_string())
        .join(format!("{y}.png"))
}

async fn load(z: u8, x: i64, y: i64, cache: Option<&Path>) -> Result<image::RgbaImage, MapError> {
    let file = cache.map(|c| cache_file(c, z, x, y));
    let cached = file.as_ref().and_then(|f| {
        let age = std::fs::metadata(f).ok()?.modified().ok()?.elapsed().ok()?;
        (age < MAX_AGE).then(|| std::fs::read(f).ok()).flatten()
    });
    let bytes = match cached {
        Some(b) => b,
        None => {
            let url = format!("https://tile.openstreetmap.org/{z}/{x}/{y}.png");
            let resp = http::get(&url).await?;
            if resp.status != 200 {
                return Err(MapError::Status(resp.status, url));
            }
            if let Some(f) = &file {
                if let Some(dir) = f.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(f, &resp.bytes) {
                    tracing::warn!("cannot cache map tile {}: {e}", f.display());
                }
            }
            resp.bytes
        }
    };
    let img = image::load_from_memory(&bytes)
        .map_err(|e| MapError::Decode(format!("{z}/{x}/{y}"), e.to_string()))?
        .to_rgba8();
    let side = TILE as u32;
    Ok(if img.width() == side {
        img
    } else {
        image::imageops::resize(&img, side, side, image::imageops::FilterType::Triangle)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_known_points() {
        // Null Island is the centre of the world at every zoom.
        let (x, y) = world(0.0, 0.0, 3);
        assert_eq!((x, y), (TILE * 4.0, TILE * 4.0));
        // Greenwich, zoom 0: x in the middle, y in the upper half.
        let (x, y) = world(51.4779, 0.0, 0);
        assert!((x - TILE / 2.0).abs() < 1e-9);
        assert!(y < TILE / 2.0 && y > TILE * 0.3);
    }

    #[test]
    fn fits_points_and_projects_them_inside() {
        let home = (45.4642, 9.19);
        let v = fit(&[home], 14, 800, 500).unwrap();
        assert_eq!(v.zoom, 14);
        let (fx, fy) = v.project(home.0, home.1);
        // Snapping keeps the point near the centre.
        assert!((fx - 0.5).abs() < SNAP / 800.0 && (fy - 0.5).abs() < SNAP / 500.0);

        // Two cities far apart force a smaller zoom; both end up on the image.
        let rome = (41.9028, 12.4964);
        let v = fit(&[home, rome], 14, 800, 500).unwrap();
        assert!(v.zoom < 8);
        for (la, lo) in [home, rome] {
            let (fx, fy) = v.project(la, lo);
            assert!(
                (0.0..=1.0).contains(&fx) && (0.0..=1.0).contains(&fy),
                "{fx} {fy}"
            );
        }
        assert_eq!(fit(&[], 14, 800, 500), None);
    }

    #[test]
    fn lists_the_tiles_under_a_view() {
        let v = MapView {
            zoom: 2,
            left: 300,
            top: -100,
            width: 800,
            height: 400,
        };
        let tiles: Vec<_> = v.tiles().collect();
        // x 300..1100 covers tiles 1..=4 (4 wraps to 0 when fetched); y -100..300
        // covers rows 0 and 1 (row -1 is off the world).
        assert_eq!(
            tiles,
            vec![
                (1, 0),
                (2, 0),
                (3, 0),
                (4, 0),
                (1, 1),
                (2, 1),
                (3, 1),
                (4, 1)
            ]
        );
    }
}
