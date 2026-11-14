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

// Limit format negotiation to artwork this build can decode.
const POSTER_ACCEPT: &str = "image/webp,image/jpeg,image/png";
const POSTER_RETENTION_DAYS: u64 = 14;
const POSTER_RETENTION_SECS: u64 = POSTER_RETENTION_DAYS * 24 * 60 * 60;

pub fn cache_dir() -> PathBuf {
    CACHE_DIR_ONCE
        .get_or_init(|| {
            let mut path = preferred_cache_dir(load_config().cache_dir);

            if let Err(e) = fs::create_dir_all(&path) {
                warn!("failed to create cache dir {}: {e}", path.display());
                // Fall back to local folder if creation failed
                path = preferred_cache_dir(None);
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
            if let Err(err) = prune_poster_cache_in_dir(&path, path == cache_dir()) {
                warn!("poster cache prune failed: {err}");
            }
        }
    });

    dir.clone()
}

fn prune_poster_cache_if_needed() -> std::io::Result<usize> {
    let dir = poster_cache_dir();
    prune_poster_cache_in_dir(&dir, dir == cache_dir())
}

pub(crate) fn clear_poster_cache() -> std::io::Result<usize> {
    clear_poster_files(&poster_cache_dir(), &super::prefs::hotset_manifest_path())
}

fn clear_poster_files(poster_dir: &Path, hotset: &Path) -> std::io::Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(poster_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if !matches!(
            ext.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "rgba"
        ) {
            continue;
        }
        // If posters/ could not be created, cached posters live alongside the user's files.
        if hotset.parent() == Some(poster_dir) && !has_poster_key(&path) {
            continue;
        }
        fs::remove_file(path)?;
        removed += 1;
    }
    match fs::remove_file(hotset) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(removed)
}

fn has_poster_key(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    let stem = stem.strip_prefix("rgba_").unwrap_or(stem);
    let digest = stem.strip_suffix("__s").unwrap_or(stem);
    digest.len() == 32 && digest.bytes().all(|b| b.is_ascii_hexdigit())
}

fn prune_poster_cache_in_dir(dir: &Path, shared_dir: bool) -> std::io::Result<usize> {
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
        if shared_dir && !has_poster_key(&path) {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "rgba") {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let broken_rgba = ext == "rgba"
            && fs::File::open(&path)
                .and_then(|mut file| rgba_dimensions(&mut file))
                .is_err_and(|err| {
                    matches!(
                        err.kind(),
                        std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
                    )
                });
        if modified < cutoff || broken_rgba || metadata.len() == 0 {
            fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn url_to_cache_key(url: &str) -> String {
    format!("{:x}", md5::compute(url.as_bytes()))
}

fn rgba_byte_count(width: u32, height: u32) -> Option<u64> {
    if width == 0 || height == 0 {
        return None;
    }
    (u64::from(width) * u64::from(height)).checked_mul(4)
}

fn rgba_dimensions(file: &mut fs::File) -> std::io::Result<(u32, u32)> {
    let mut width = [0; 4];
    let mut height = [0; 4];
    file.read_exact(&mut width)?;
    file.read_exact(&mut height)?;
    let (w, h) = (u32::from_le_bytes(width), u32::from_le_bytes(height));
    let payload_len = file.metadata()?.len().checked_sub(8);
    if payload_len.is_none() || rgba_byte_count(w, h) != payload_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "RGBA dimensions do not match the file length",
        ));
    }
    Ok((w, h))
}

/// Return (width, height, RGBA8 bytes) from either an image file (.png/.jpg/.jpeg/.webp)
/// or a raw rgba file we now write as: 8-byte header (u32 LE width, u32 LE height) + bytes.
pub fn load_rgba_raw_or_image(path: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let p = Path::new(path);
    if let Some(ext) = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
    {
        if ext == "rgba" {
            let mut f = fs::File::open(p).map_err(|e| format!("open rgba: {e}"))?;
            let (w, h) = rgba_dimensions(&mut f).map_err(|e| format!("invalid rgba: {e}"))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)
                .map_err(|e| format!("read body: {e}"))?;
            if Some(buf.len() as u64) != rgba_byte_count(w, h) {
                return Err("RGBA payload length changed while reading".into());
            }
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
    let rgba = img.into_rgba8().into_raw();
    Ok((w, h, rgba))
}

pub fn find_any_by_key(key: &str) -> Option<PathBuf> {
    let poster_dir = poster_cache_dir();
    poster_names(key)
        .map(|name| poster_dir.join(name))
        .find(|path| path.is_file())
}

fn poster_names(key: &str) -> impl Iterator<Item = String> + '_ {
    ["png", "jpg", "jpeg", "webp", "rgba"]
        .into_iter()
        .map(move |ext| format!("{key}.{ext}"))
        .chain(std::iter::once_with(move || format!("rgba_{key}.rgba")))
}

pub(super) struct PosterFiles {
    dir: PathBuf,
    names: std::collections::HashSet<String>,
}

impl PosterFiles {
    // One listing replaces up to six filesystem probes per missing poster.
    pub(super) fn read(dir: PathBuf) -> std::io::Result<Self> {
        let mut names = std::collections::HashSet::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            #[cfg(windows)]
            let name = name.to_ascii_lowercase();
            if has_poster_key(Path::new(&name)) {
                let kind = entry.file_type()?;
                if kind.is_file() || (kind.is_symlink() && entry.path().is_file()) {
                    names.insert(name);
                }
            }
        }
        Ok(Self { dir, names })
    }

    pub(super) fn find(&self, key: &str) -> Option<PathBuf> {
        poster_names(key)
            .find(|name| self.names.contains(name))
            .map(|name| self.dir.join(name))
    }
}

/// Download, normalize to PNG and store in cache. Returns the stored path.
pub fn download_and_store(url: &str, key: &str) -> Result<PathBuf, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, POSTER_ACCEPT)
        .send()
        .map_err(|e| format!("GET {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    let body = resp.bytes().map_err(|e| format!("read body: {e}"))?;

    let img = image::load_from_memory(&body).map_err(|e| format!("decode poster: {e}"))?;
    let mut png_bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ImageFormat::Png)
        .map_err(|e| format!("encode png: {e}"))?;
    let out = poster_cache_dir().join(format!("{key}.png"));
    fs::write(&out, png_bytes).map_err(|e| format!("write {}: {e}", out.display()))?;
    Ok(out)
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
        .header(reqwest::header::ACCEPT, POSTER_ACCEPT)
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

    let shared_dir = dir == cache_dir();
    let mut removed = 0usize;
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }

        if shared_dir && !has_poster_key(&path) {
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
        .header(reqwest::header::ACCEPT, POSTER_ACCEPT)
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
        .header(reqwest::header::ACCEPT, POSTER_ACCEPT)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| format!("download bytes: {e}"))?;

    let img = image::load_from_memory(&bytes).map_err(|e| format!("decode poster: {e}"))?;

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

    Ok(dest)
}

fn preferred_cache_dir(configured: Option<PathBuf>) -> PathBuf {
    configured.unwrap_or_else(|| resolve_relative_path(".pex_cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_artwork_codecs_decode_posters_and_window_icon() {
        let temp = tempfile::tempdir().unwrap();
        let rgb = image::RgbImage::from_pixel(2, 3, image::Rgb([30, 60, 90]));
        for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::WebP] {
            let path = temp
                .path()
                .join(format!("art.{}", format.extensions_str()[0]));
            rgb.save_with_format(&path, format).unwrap();
            let (width, height, rgba) = load_rgba_raw_or_image(path.to_str().unwrap()).unwrap();
            assert_eq!((width, height, rgba.len()), (2, 3, 24), "{format:?}");
        }
        let ico = image::load_from_memory(include_bytes!("../assets/PEX.ico")).unwrap();
        assert!(ico.width() > 0 && ico.height() > 0);
    }

    #[test]
    fn indexed_poster_lookup_preserves_format_preference_and_legacy_names() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let key = "00001111222233334444555566667777__s";
        let names: Vec<_> = poster_names(key).collect();
        for name in &names {
            fs::write(dir.join(name), b"fixture").unwrap();
        }
        for name in &names {
            let files = PosterFiles::read(dir.to_owned()).unwrap();
            assert_eq!(files.find(key), Some(dir.join(name)));
            fs::remove_file(dir.join(name)).unwrap();
        }
        fs::create_dir(dir.join(&names[1])).unwrap();
        fs::write(dir.join(format!("{key}.jpg.part")), b"incomplete").unwrap();
        assert!(PosterFiles::read(dir.to_owned())
            .unwrap()
            .find(key)
            .is_none());
    }

    #[cfg(windows)]
    #[test]
    fn cache_directory_preserves_unc_share_and_verbatim_prefix() {
        for share in [
            r"\\media-server\cache\pex",
            r"\\?\UNC\media-server\cache\pex",
        ] {
            let path = preferred_cache_dir(Some(PathBuf::from(share)));
            assert!(path.is_absolute());
            assert_eq!(path.as_os_str(), std::ffi::OsStr::new(share));
        }
    }

    #[test]
    fn rebuild_clears_posters_and_hotset_but_preserves_other_cache_data() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let posters = root.join("posters");
        fs::create_dir(&posters).unwrap();
        let poster = posters.join("movie.jpg");
        let hotset = root.join("hotset.txt");
        let owned = root.join("owned_all.txt");
        let unrelated = root.join("photo.png");
        for file in [&poster, &hotset, &owned, &unrelated] {
            fs::write(file, b"fixture").unwrap();
        }
        assert_eq!(clear_poster_files(&posters, &hotset).unwrap(), 1);
        assert!(!poster.exists() && !hotset.exists());
        assert!(owned.exists() && unrelated.exists());

        let fallback_poster = root.join("00001111222233334444555566667777__s.jpg");
        fs::write(&fallback_poster, b"fixture").unwrap();
        assert_eq!(clear_poster_files(root, &hotset).unwrap(), 1);
        assert!(!fallback_poster.exists());
        assert!(owned.exists() && unrelated.exists());
    }

    #[test]
    fn prune_discards_unusable_raw_artwork_and_keeps_valid_rgba() {
        let tmp = tempfile::tempdir().unwrap();
        let valid = tmp.path().join("ok.rgba");
        let zero = tmp.path().join("http-error.rgba");
        let short = tmp.path().join("short.rgba");
        let mut payload = [2u32.to_le_bytes(), 3u32.to_le_bytes()].concat();
        payload.extend([0; 24]);
        fs::write(&valid, &payload).unwrap();
        fs::write(&short, &payload[..16]).unwrap();
        fs::write(&zero, [0; 20]).unwrap();
        assert!(load_rgba_raw_or_image(short.to_str().unwrap()).is_err());
        assert!(load_rgba_raw_or_image(zero.to_str().unwrap()).is_err());
        assert_eq!(prune_poster_cache_in_dir(tmp.path(), false).unwrap(), 2);
        assert!(!zero.exists() && !short.exists());
        let (w, h, pixels) = load_rgba_raw_or_image(valid.to_str().unwrap()).unwrap();
        assert_eq!((w, h, pixels.len()), (2, 3, 24));
    }
}
