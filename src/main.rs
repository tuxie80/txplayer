#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod abx;
mod analyzer;
mod app;
mod archive;
mod audio;
mod cue;
mod dsd;
mod dsp;
mod exclusive;
mod icons;
mod ipc;
mod library;
mod lyrics;
mod media_controls;
mod metadata;
mod modals;
mod platform;
mod playlist_io;
mod plugins;
mod remote;
mod tagger;
mod theme;
mod tray;
mod visualizer;
mod watcher;

use std::sync::Arc;

use anyhow::Result;
use app::{TxPlayerApp, startup_window_layout};
use eframe::egui;

pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/AppIcon.png");
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

fn load_app_icon() -> Option<Arc<egui::IconData>> {
    let image = image::load_from_memory(APP_ICON_PNG).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Some(Arc::new(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let ephemeral = args
        .iter()
        .any(|a| a == "--screenshot" || a == "--demo-piano" || a == "--demo-spectrogram");
    let launch_paths = ipc::extract_cli_launch_paths(&args);

    let single_instance = if !ephemeral {
        let port_file = ipc::instance_port_file();
        if ipc::try_forward_to_running_instance(&port_file, &launch_paths) {
            return Ok(());
        }
        ipc::SingleInstanceServer::start(port_file)
    } else {
        None
    };

    let layout = startup_window_layout();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("TxPlayer")
        .with_inner_size(layout.init_size)
        .with_min_inner_size(layout.min_size)
        .with_app_id("com.txplayer.player");
    if let Some(pos) = layout.init_pos {
        viewport = viewport.with_position(pos);
    }

    #[cfg(target_os = "macos")]
    {
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_titlebar_buttons_shown(false);
    }

    #[cfg(not(target_os = "macos"))]
    {
        viewport = viewport.with_decorations(false);
    }

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let mut single_instance_opt = single_instance;
    let mut launch_paths_opt = Some(launch_paths);

    eframe::run_native(
        "TxPlayer",
        options,
        Box::new(move |context| {
            Ok(Box::new(TxPlayerApp::new(
                context,
                single_instance_opt.take(),
                launch_paths_opt.take().unwrap_or_default(),
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}
