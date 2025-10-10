// src/main.rs
use std::env;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

fn pick_renderer() -> eframe::Renderer {
    match env::var("PEX_RENDERER").as_deref() {
        Ok("glow") => eframe::Renderer::Glow,
        Ok("wgpu") => eframe::Renderer::Wgpu,
        _ => {
            // Default: Windows = WGPU (DX12), Others = Glow (GL)
            #[cfg(target_os = "windows")]
            { eframe::Renderer::Wgpu }
            #[cfg(not(target_os = "windows"))]
            { eframe::Renderer::Glow }
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

    let options = eframe::NativeOptions {
        renderer: pick_renderer(),
        multisampling: 0,
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
