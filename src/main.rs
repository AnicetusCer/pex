#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
// src/main.rs
use eframe::egui::{IconData, ViewportBuilder};

#[cfg(not(target_os = "windows"))]
use eframe::egui::Vec2;
use std::env;
use std::sync::Arc;
use tracing::error;

#[cfg(target_os = "linux")]
use tracing::info;
use tracing_subscriber::EnvFilter;

fn pick_renderer() -> eframe::Renderer {
    match env::var("PEX_RENDERER").as_deref() {
        Ok("glow") => eframe::Renderer::Glow,
        Ok("wgpu") => eframe::Renderer::Wgpu,
        _ => {
            // Default: Windows = WGPU (DX12), Others = Glow (GL)
            #[cfg(target_os = "windows")]
            {
                eframe::Renderer::Wgpu
            }
            #[cfg(not(target_os = "windows"))]
            {
                eframe::Renderer::Glow
            }
        }
    }
}

fn main() -> eframe::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    #[cfg(target_os = "linux")]
    {
        info!("WSL_DISTRO_NAME={:?}", env::var_os("WSL_DISTRO_NAME"));
        info!("XDG_SESSION_TYPE={:?}", env::var_os("XDG_SESSION_TYPE"));
        info!("WAYLAND_DISPLAY={:?}", env::var_os("WAYLAND_DISPLAY"));
        info!("DISPLAY={:?}", env::var_os("DISPLAY"));
        info!("WINIT_UNIX_BACKEND={:?}", env::var_os("WINIT_UNIX_BACKEND"));
    }

    let mut viewport = ViewportBuilder::default().with_maximized(true);

    #[cfg(not(target_os = "windows"))]
    {
        // Keep a sensible restore size for platforms where this does not break maximization.
        viewport = viewport.with_inner_size(Vec2::new(1600.0, 900.0));
    }
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        renderer: pick_renderer(),
        multisampling: 0,
        viewport,
        ..Default::default()
    };

    match eframe::run_native(
        "Plex EPG Explorer",
        options,
        Box::new(|_cc| Ok(Box::new(pex::app::PexApp::default()))),
    ) {
        Ok(_) => Ok(()),
        Err(e) => {
            error!("eframe failed to start: {e:?}");
            error!("Hint: on WSL use X/Wayland; on Windows try PEX_RENDERER=wgpu or glow.");
            Err(e)
        }
    }
}

fn load_app_icon() -> Option<IconData> {
    const ICON_BYTES: &[u8] = include_bytes!("assets/PEX.ico");
    let dyn_image = image::load_from_memory(ICON_BYTES).ok()?;
    let rgba = dyn_image.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    Some(IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}
