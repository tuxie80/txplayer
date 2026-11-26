#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analyzer;
mod app;
mod audio;
mod cue;
mod dsd;
mod dsp;
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
use app::{
    COMPACT_COLLAPSED_HEIGHT, COMPACT_EXPANDED_HEIGHT, COMPACT_MIN_HEIGHT, COMPACT_MIN_WIDTH,
    COMPACT_WIDTH, FULL_HEIGHT, FULL_MIN_HEIGHT, FULL_MIN_WIDTH, FULL_WIDTH, TxPlayerApp,
    startup_window_layout,
};
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
    let (compact_mode, compact_playlist, show_vis) = startup_window_layout();
    let (init_size, min_size) = if compact_mode {
        let h = if compact_playlist {
            COMPACT_EXPANDED_HEIGHT
        } else {
            COMPACT_COLLAPSED_HEIGHT
        } + if show_vis { 62.0 } else { 0.0 };
        ([COMPACT_WIDTH, h], [COMPACT_MIN_WIDTH, COMPACT_MIN_HEIGHT])
    } else {
        ([FULL_WIDTH, FULL_HEIGHT], [FULL_MIN_WIDTH, FULL_MIN_HEIGHT])
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("TxPlayer")
        .with_inner_size(init_size)
        .with_min_inner_size(min_size)
        .with_app_id("com.txplayer.player");

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
