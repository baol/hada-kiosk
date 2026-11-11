//! Album art for the media card: download, decode and shrink it on the network
//! thread, so the UI only receives a small RGBA image.

use crate::ha::http;

/// Artwork is shown at most this many pixels wide or high.
pub const MAX_SIDE: u32 = 384;

/// Decoded RGBA8 pixels, `width * height * 4` bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct Artwork {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Artwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Artwork({}x{})", self.width, self.height)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArtworkError {
    #[error("{0}")]
    Http(#[from] http::HttpError),
    #[error("HTTP {0}")]
    Status(u16),
    #[error("cannot decode image: {0}")]
    Decode(String),
}

/// Download and decode the image at `url`.
pub async fn fetch(url: &str) -> Result<Artwork, ArtworkError> {
    let resp = http::get(url).await?;
    if resp.status != 200 {
        return Err(ArtworkError::Status(resp.status));
    }
    decode(&resp.bytes)
}

/// Decode JPEG/PNG/WebP and shrink it to fit `MAX_SIDE`.
pub fn decode(bytes: &[u8]) -> Result<Artwork, ArtworkError> {
    let img = image::load_from_memory(bytes).map_err(|e| ArtworkError::Decode(e.to_string()))?;
    let img = if img.width() > MAX_SIDE || img.height() > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Ok(Artwork {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, 128, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn decodes_and_shrinks() {
        let a = decode(&png(1000, 500)).unwrap();
        assert_eq!((a.width, a.height), (384, 192));
        assert_eq!(a.rgba.len(), 384 * 192 * 4);
        let small = decode(&png(100, 100)).unwrap();
        assert_eq!((small.width, small.height), (100, 100));
        assert_eq!(&small.rgba[..4], &[0, 0, 128, 255]);
        assert!(decode(b"not an image").is_err());
    }
}
