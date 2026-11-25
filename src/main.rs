mod app;
mod audio;
mod dsp;
mod icons;
mod library;
mod metadata;
mod modals;
mod plugins;
mod tagger;
mod theme;
mod visualizer;

use std::sync::Arc;

use anyhow::Result;
use app::TxPlayerApp;
use eframe::egui;

pub const APP_ICON_PNG: &[u8] = include_bytes!("../assets/AppIcon.png");

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
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("TxPlayer")
        .with_inner_size([1240.0, 790.0])
        .with_min_inner_size([920.0, 560.0])
        .with_app_id("com.txplayer.player")
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_titlebar_buttons_shown(false);

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
