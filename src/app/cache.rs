use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use image::{GenericImageView, ImageFormat};
use reqwest::blocking::Client;
use tracing::warn;

use crate::config::{load_config, resolve_relative_path};

// Chosen once on first call
use std::sync::{Once, OnceLock};
static CACHE_DIR_ONCE: OnceLock<PathBuf> = OnceLock::new();
static POSTER_DIR_ONCE: OnceLock<PathBuf> = OnceLock::new();
static CHANNEL_ICON_DIR_ONCE: OnceLock<PathBuf> = OnceLock::new();
static POSTER_PRUNE_ONCE: Once = Once::new();

const POSTER_RETENTION_DAYS: u64 = 14;
const POSTER_RETENTION_SECS: u64 = POSTER_RETENTION_DAYS * 24 * 60 * 60;

pub fn cache_dir() -> PathBuf {
    CACHE_DIR_ONCE
        .get_or_init(|| {
            let cfg = load_config();
            let mut path = normalize_dir(
                cfg.cache_dir
                    .clone()
                    .unwrap_or_else(|| resolve_relative_path(".pex_cache")),
            );

            if let Err(e) = fs::create_dir_all(&path) {
                warn!("failed to create cache dir {}: {e}", path.display());
                // Fall back to local folder if creation failed
                path = normalize_dir(resolve_relative_path(".pex_cache"));
                let _ = fs::create_dir_all(&path);
            }
            path
        })
        .clone()
}

pub fn poster_cache_dir() -> PathBuf {
    let dir = POSTER_DIR_ONCE.get_or_init(|| {
        let mut path = cache_dir().join("posters");
        if let Err(e) = fs::create_dir_all(&path) {
            warn!("failed to create poster cache dir {}: {e}", path.display());
            path = cache_dir();
        }
        path
    });

    POSTER_PRUNE_ONCE.call_once({
        let path = dir.clone();
        move || {
            if let Err(err) = prune_poster_cache_in_dir(&path) {
                warn!("poster cache prune failed: {err}");
            }
        }
    });

    dir.clone()
}

fn prune_poster_cache_if_needed() -> std::io::Result<usize> {
    let dir = poster_cache_dir();
    prune_poster_cache_in_dir(&dir)
}

fn prune_poster_cache_in_dir(dir: &Path) -> std::io::Result<usize> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(POSTER_RETENTION_SECS))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0usize;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "rgba") {
                continue;
            }
        } else {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if modified < cutoff {
            let _ = fs::remove_file(&path);
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn url_to_cache_key(url: &str) -> String {
    format!("{:x}", md5::compute(url.as_bytes()))
}

/// Return (width, height, RGBA8 bytes) from either an image file (.png/.jpg/.jpeg/.webp)
/// or a raw rgba file we now write as: 8-byte header (u32 LE width, u32 LE height) + bytes.
pub fn load_rgba_raw_or_image(path: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err("not found".into());
    }
    if let Some(ext) = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
    {
        if ext == "rgba" {
            let mut f = fs::File::open(p).map_err(|e| format!("open rgba: {e}"))?;
            let mut header = [0u8; 8];
            f.read_exact(&mut header)
                .map_err(|e| format!("read header: {e}"))?;
            let w = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
            let h = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)
                .map_err(|e| format!("read body: {e}"))?;
            return Ok((w, h, buf));
        }
    }
    // Fallback: decode via image crate
    let img = image::ImageReader::open(p)
        .map_err(|e| format!("open image {}: {e}", p.display()))?
        .with_guessed_format()
        .map_err(|e| format!("guess format {}: {e}", p.display()))?
        .decode()
        .map_err(|e| format!("decode {}: {e}", p.display()))?;
    let (w, h) = img.dimensions();
    let rgba = img.to_rgba8().to_vec();
    Ok((w, h, rgba))
}

pub fn find_any_by_key(key: &str) -> Option<PathBuf> {
    let poster_dir = poster_cache_dir();
    let candidates = [
        format!("{}.png", key),
        format!("{}.jpg", key),
        format!("{}.jpeg", key),
        format!("{}.webp", key),
        format!("{}.rgba", key),
        format!("rgba_{}.rgba", key),
    ];
    for c in candidates {
        let p = poster_dir.join(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Download, normalize to PNG or RGBA and store in cache. Returns the stored path.
pub fn download_and_store(url: &str, key: &str) -> Result<PathBuf, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("GET {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    let body = resp
        .bytes()
        .map_err(|e| format!("read body: {e}"))?
        .to_vec();

    // Try decode with image crate
    match image::load_from_memory(&body) {
        Ok(img) => {
            let out = poster_cache_dir().join(format!("{key}.png"));
            let mut f =
                fs::File::create(&out).map_err(|e| format!("create {}: {e}", out.display()))?;
            let mut png_bytes: Vec<u8> = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ImageFormat::Png)
                .map_err(|e| format!("encode png: {e}"))?;
            f.write_all(&png_bytes)
                .map_err(|e| format!("write {}: {e}", out.display()))?;
            let _ = prune_poster_cache_if_needed();
            Ok(out)
        }
        Err(e) => {
            warn!("image decode failed for {url}: {e}; storing raw");
            // Store as rgba with w/h header if we really fail (rare)
            let out = poster_cache_dir().join(format!("{key}.rgba"));
            let mut f =
                fs::File::create(&out).map_err(|e| format!("create {}: {e}", out.display()))?;
            // We don't know w/h here; write zeros so loader will reject gracefully
            f.write_all(&0u32.to_le_bytes())
                .map_err(|e| format!("write hdr: {e}"))?;
            f.write_all(&0u32.to_le_bytes())
                .map_err(|e| format!("write hdr: {e}"))?;
            f.write_all(&body)
                .map_err(|e| format!("write {}: {e}", out.display()))?;
            let _ = prune_poster_cache_if_needed();
            Ok(out)
        }
    }
}
/// Download an image, resize to `max_width` (keeping aspect), and store as JPEG with `quality`.
///
/// Returns the on-disk path. Falls back to `download_and_store` if decode/resize fails.
/// This writes `<poster_cache_dir>/<key>.jpg`.
pub fn download_and_store_resized(
    url: &str,
    key: &str,
    max_width: u32,
    quality: u8,
) -> Result<std::path::PathBuf, String> {
    use image::{imageops::FilterType, DynamicImage};
    use reqwest::blocking::Client;
    use std::{fs, io::Write};

    let dest = poster_cache_dir().join(format!("{key}.jpg"));

    // If already present, return immediately.
    if dest.exists() {
        return Ok(dest);
    }

    // Download bytes
    let client = Client::builder()
        .user_agent("pex_new/resize-prefetch")
        .build()
        .map_err(|e| format!("reqwest client build: {e}"))?;

    let bytes = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("download bytes: {e}"))?;

    // Try to decode the image
    let img = match image::load_from_memory(&bytes) {
        Ok(img) => img,
        Err(_) => {
            // Fallback to original path via existing helper
            return download_and_store(url, key);
        }
    };

    // Resize if needed, keep aspect
    let (w, h) = img.dimensions();
    let out: DynamicImage = if w > max_width {
        let new_h = ((h as f32) * (max_width as f32 / w as f32))
            .round()
            .max(1.0) as u32;
        img.resize_exact(max_width, new_h, FilterType::CatmullRom)
    } else {
        img
    };

    // Encode JPEG with requested quality
    let mut jpeg_bytes: Vec<u8> = Vec::new();
    {
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, quality);
        encoder
            .encode_image(&out)
            .map_err(|e| format!("jpeg encode: {e}"))?;
    }

    // Ensure cache dir exists and write atomically-ish
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = dest.with_extension("jpg.part");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
        f.write_all(&jpeg_bytes)
            .map_err(|e| format!("write: {e}"))?;
    }
    fs::rename(&tmp, &dest).map_err(|e| format!("rename: {e}"))?;

    let _ = prune_poster_cache_if_needed();
    Ok(dest)
}

pub fn prune_poster_cache_now() -> std::io::Result<usize> {
    prune_poster_cache_if_needed()
}

pub fn refresh_poster_cache_light() -> std::io::Result<usize> {
    let dir = poster_cache_dir();
    if !dir.exists() {
        return Ok(0);
    }

    let mut removed = 0usize;
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }

        let metadata = entry.metadata()?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());

        let should_remove = ext.as_ref().is_none_or(|ext| {
            if ext == "part" {
                true
            } else if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                metadata.len() == 0
            } else if ext == "rgba" {
                metadata.len() <= 8
            } else {
                true
            }
        });

        if should_remove {
            fs::remove_file(&path)?;
            removed += 1;
        }
    }

    let aged_removed = prune_poster_cache_if_needed()?;

    Ok(removed + aged_removed)
}

fn channel_icon_dir() -> PathBuf {
    CHANNEL_ICON_DIR_ONCE
        .get_or_init(|| {
            let mut path = cache_dir().join("channel_icons");
            if let Err(e) = fs::create_dir_all(&path) {
                warn!("failed to create channel icon dir {}: {e}", path.display());
                path = cache_dir();
            }
            path
        })
        .clone()
}

pub fn channel_icon_path(url: &str) -> PathBuf {
    channel_icon_dir().join(format!("{}.png", url_to_cache_key(url)))
}

pub fn ensure_channel_icon(url: &str) -> Result<PathBuf, String> {
    if url.trim().is_empty() {
        return Err("empty url".into());
    }
    let dest = channel_icon_path(url);
    if dest.exists() {
        return Ok(dest);
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let bytes = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("download icon: {e}"))?;

    let img = image::load_from_memory(&bytes).map_err(|e| format!("decode icon: {e}"))?;
    let resized = if img.width() > 256 || img.height() > 256 {
        img.resize(256, 256, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let mut png_bytes: Vec<u8> = Vec::new();
    resized
        .write_to(&mut std::io::Cursor::new(&mut png_bytes), ImageFormat::Png)
        .map_err(|e| format!("encode icon png: {e}"))?;

    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = dest.with_extension("png.part");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| format!("create icon tmp: {e}"))?;
        f.write_all(&png_bytes)
            .map_err(|e| format!("write icon: {e}"))?;
    }
    fs::rename(&tmp, &dest).map_err(|e| format!("finalize icon: {e}"))?;

    Ok(dest)
}

/// Same as `download_and_store_resized` but reuses a provided reqwest Client
/// for connection pooling (faster parallel downloads).
pub fn download_and_store_resized_with_client(
    client: &reqwest::blocking::Client,
    url: &str,
    key: &str,
    max_width: u32,
    quality: u8,
) -> Result<std::path::PathBuf, String> {
    use image::{imageops::FilterType, DynamicImage};
    use std::{fs, io::Write};

    let dest = poster_cache_dir().join(format!("{key}.jpg"));

    // If already present, return immediately.
    if dest.exists() {
        return Ok(dest);
    }

    // Download bytes using shared client
    let bytes = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("download bytes: {e}"))?;

    // Try to decode the image
    let img = match image::load_from_memory(&bytes) {
        Ok(img) => img,
        Err(_) => {
            // Fallback to original path via existing helper
            return download_and_store(url, key);
        }
    };

    // Resize if needed, keep aspect
    let (w, h) = img.dimensions();
    let out: DynamicImage = if w > max_width {
        let new_h = ((h as f32) * (max_width as f32 / w as f32))
            .round()
            .max(1.0) as u32;
        img.resize_exact(max_width, new_h, FilterType::CatmullRom)
    } else {
        img
    };

    // Encode JPEG with requested quality
    let mut jpeg_bytes: Vec<u8> = Vec::new();
    {
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, quality);
        encoder
            .encode_image(&out)
            .map_err(|e| format!("jpeg encode: {e}"))?;
    }

    // Ensure cache dir exists and write atomically-ish
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = dest.with_extension("jpg.part");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
        f.write_all(&jpeg_bytes)
            .map_err(|e| format!("write: {e}"))?;
    }
    fs::rename(&tmp, &dest).map_err(|e| format!("rename: {e}"))?;

    let _ = prune_poster_cache_if_needed();
    Ok(dest)
}

fn normalize_dir(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    let fixed = s.replace("\\\\", "\\").replace('/', "\\");
    PathBuf::from(fixed)
}
