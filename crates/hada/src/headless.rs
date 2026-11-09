//! Render the UI to a PNG without any display: a minimal `slint::platform::Platform`
//! backed by the software renderer. Used for `--screenshot` in CI and containers.

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::PhysicalSize;
use slint::Rgb8Pixel;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

thread_local! {
    static WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

struct HeadlessPlatform {
    size: PhysicalSize,
}

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        // Like the KMS backend: the window has its final size from the start and
        // never resizes, so layout code cannot rely on a later resize event.
        w.set_size(self.size);
        WINDOW.with(|s| *s.borrow_mut() = Some(w.clone()));
        Ok(w)
    }
}

/// Must be called before any Slint component is created.
pub fn install(width: u32, height: u32) -> Result<(), slint::platform::SetPlatformError> {
    slint::platform::set_platform(Box::new(HeadlessPlatform {
        size: PhysicalSize::new(width, height),
    }))
}

/// Render the current scene at `width`×`height` and write it as PNG.
pub fn render_png(width: u32, height: u32, out: &Path) -> anyhow::Result<()> {
    let window = WINDOW.with(|w| w.borrow().clone()).ok_or_else(|| {
        anyhow::anyhow!(
            "headless platform has no window; was install() called before creating the UI?"
        )
    })?;
    window.set_size(PhysicalSize::new(width, height));
    window.show()?;
    let mut pixels = vec![Rgb8Pixel::default(); (width * height) as usize];
    // A few passes so layout-driven callbacks (relayout) and animations settle.
    for _ in 0..4 {
        slint::platform::update_timers_and_animations();
        window.request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, width as usize);
        });
    }
    let mut raw = Vec::with_capacity(pixels.len() * 3);
    for p in &pixels {
        raw.extend_from_slice(&[p.r, p.g, p.b]);
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(out)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&raw)?;
    window.hide()?;
    Ok(())
}
