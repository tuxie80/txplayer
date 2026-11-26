#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analyzer;
mod app;
mod archive;
mod audio;
mod cue;
mod dsd;
mod dsp;
mod exclusive;
mod icons;
mod library;
mod lyrics;
mod media_controls;
mod metadata;
mod modals;
mod platform;
mod playlist_io;
mod plugins;
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

    eframe::run_native(
        "TxPlayer",
        options,
        Box::new(|context| Ok(Box::new(TxPlayerApp::new(context)))),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}
