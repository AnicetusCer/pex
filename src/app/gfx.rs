// src/app/gfx.rs
use eframe::egui::{self as eg, ColorImage, TextureHandle};

/// Upload an RGBA image to a GPU texture. (UI thread only)
pub fn upload_rgba(
    ctx: &eg::Context,
    w: u32,
    h: u32,
    bytes: &[u8],
    name: &str,
) -> TextureHandle {
    let img = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], bytes);
    ctx.load_texture(name.to_string(), img, eg::TextureOptions::LINEAR)
}

/// Load a texture from a cached file path; validates portrait-ish aspect.
/// (UI thread only)
pub fn load_texture_from_path(
    ctx: &eg::Context,
    path_str: &str,
    cache_name: &str,
) -> Result<TextureHandle, String> {
    let (w, h, bytes) = crate::app::cache::load_rgba_raw_or_image(path_str)?;
    // Portrait sanity check ~2:3
    let ar = (w as f32) / (h as f32);
    if !(0.55..=0.80).contains(&ar) {
        return Err(format!("non-poster aspect {w}x{h} ar={ar:.2}"));
    }
    Ok(upload_rgba(ctx, w, h, &bytes, cache_name))
}
