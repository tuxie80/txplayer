use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, CursorIcon, FontId, Frame, Layout, Margin, Rect,
    RichText, ScrollArea, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2, pos2, vec2,
};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::audio::{
    AudioEvent, AudioHandle, VisualTap, compute_waveform, list_output_devices,
};
use crate::icons::{self, Icon};
use crate::library::{LibraryStore, ScanEvent, cover_directory, scan};
use crate::metadata::{
    QualityTier, SUPPORTED_EXTENSIONS, Track, format_duration, format_sample_rate,
};
use crate::modals::{
    self, CommandPaletteState, FolderModalAction, PaletteCommand, PaletteEntry, PillStyle,
    ProgressBarStyle, RowDensity, SettingsTab, SignalPathDisplay, TagEditorState, TagWorkerEvent,
    TrackInfoAction, pill_button,
};
use crate::platform::{FILE_MANAGER_NAME, MOD_SHORT, reveal_in_file_manager};
use crate::plugins::{EqPreset, PianoEngineId, PluginConfig, ToolHealth};
use crate::theme::{self, Palette, ThemeId};
use crate::visualizer::{VisualizerMode, VisualizerState};

const SIDEBAR_WIDTH: f32 = 228.0;
const TRANSPORT_HEIGHT: f32 = 104.0;
const WAVEFORM_BINS: usize = 160;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Library,
    Favorites,
    Queue,
    Playlist(u64),
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RepeatMode {
    Off,
    All,
    One,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Artist,
    Album,
    Title,
    Quality,
    Duration,
}

#[derive(Serialize, Deserialize)]
struct Preferences {
    volume: f32,
    shuffle: bool,
    repeat: RepeatMode,
    #[serde(default)]
    preferred_device: Option<String>,
    #[serde(default)]
    theme: ThemeId,
    #[serde(default)]
    progress_style: ProgressBarStyle,
    #[serde(default)]
    row_density: RowDensity,
    #[serde(default)]
    visualizer_mode: VisualizerMode,
    #[serde(default = "default_true")]
    show_concert_visualizer: bool,
    #[serde(default = "default_concert_height")]
    concert_visualizer_height: f32,
    #[serde(default = "default_true")]
    show_piano_keys: bool,
    #[serde(default = "default_sensitivity")]
    visualizer_sensitivity: f32,
    #[serde(default)]
    linear_volume: bool,
    #[serde(default)]
    acoustid_api_key: String,
    #[serde(default)]
    plugins: PluginConfig,
}

fn default_true() -> bool {
    true
}
fn default_concert_height() -> f32 {
    132.0
}
fn default_sensitivity() -> f32 {
    1.15
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            volume: 0.75,
            shuffle: false,
            repeat: RepeatMode::Off,
            preferred_device: None,
            theme: ThemeId::ObsidianStudio,
            progress_style: ProgressBarStyle::AcousticWaveform,
            row_density: RowDensity::Compact,
            visualizer_mode: VisualizerMode::PianoHarmonic,
            show_concert_visualizer: true,
            concert_visualizer_height: default_concert_height(),
            show_piano_keys: true,
            visualizer_sensitivity: default_sensitivity(),
            linear_volume: false,
            acoustid_api_key: String::new(),
            plugins: PluginConfig::default(),
        }
    }
}

impl Preferences {
    fn load() -> Self {
        std::fs::read(crate::library::data_directory().join("preferences.json"))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let directory = crate::library::data_directory();
        let _ = std::fs::create_dir_all(&directory);
        if let Ok(data) = serde_json::to_vec(self) {
            let _ = std::fs::write(directory.join("preferences.json"), data);
        }
    }
}

#[derive(Debug)]
enum DialogEvent {
    Files(Vec<PathBuf>),
    Folders(Vec<PathBuf>),
}

#[derive(Clone)]
struct StreamInfo {
    source_rate: u32,
    output_rate: u32,
    channels: u16,
    bit_depth: Option<u32>,
    device_name: String,
}

/// Deterministic column layout shared identically by table header AND every track row.
struct TrackColumns {
    checkbox: Rect,
    index: Rect,
    cover: Rect,
    title: Rect,
    artist: Rect,
    album: Rect,
    format: Rect,
    duration: Rect,
    favorite: Rect,
}

impl TrackColumns {
    fn compute(row_rect: Rect) -> Self {
        let top = row_rect.top();
        let bot = row_rect.bottom();
        let mut x = row_rect.left() + 6.0;

        let mut take_fixed = |w: f32| -> Rect {
            let r = Rect::from_min_max(pos2(x, top), pos2(x + w, bot));
            x += w;
            r
        };

        let checkbox = take_fixed(26.0);
        let index = take_fixed(28.0);
        let cover = take_fixed(36.0);
        x += 6.0;

        // Right-aligned fixed columns
        let right_edge = row_rect.right() - 6.0;
        let fav_w = 28.0;
        let dur_w = 54.0;
        let fmt_w = 136.0;
        let gap = 8.0;

        let favorite = Rect::from_min_max(pos2(right_edge - fav_w, top), pos2(right_edge, bot));
        let duration = Rect::from_min_max(
            pos2(favorite.left() - gap - dur_w, top),
            pos2(favorite.left() - gap, bot),
        );
        let format = Rect::from_min_max(
            pos2(duration.left() - gap - fmt_w, top),
            pos2(duration.left() - gap, bot),
        );

        // Flexible metadata area divided proportionally
        let flex_right = (format.left() - gap).max(x + 120.0);
        let flex_w = flex_right - x;
        let title_w = (flex_w * 0.40 - gap).max(60.0);
        let artist_w = (flex_w * 0.29 - gap).max(40.0);

        let title = Rect::from_min_max(pos2(x, top), pos2(x + title_w, bot));
        let artist = Rect::from_min_max(
            pos2(title.right() + gap, top),
            pos2(title.right() + gap + artist_w, bot),
        );
        let album = Rect::from_min_max(pos2(artist.right() + gap, top), pos2(flex_right, bot));

        Self {
            checkbox,
            index,
            cover,
            title,
            artist,
            album,
            format,
            duration,
            favorite,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct VisibleCacheKey {
    library_rev: u64,
    queue_rev: u64,
    active_lib_id: u64,
    view: View,
    folder_filter: Option<PathBuf>,
    sort: SortMode,
    sort_ascending: bool,
    search: String,
}

pub struct TxPlayerApp {
    audio: AudioHandle,
    visual_tap: Arc<VisualTap>,
    visualizer: VisualizerState,
    store: LibraryStore,
    plugins: PluginConfig,
    tool_health: ToolHealth,
    tool_health_rx: crossbeam_channel::Receiver<ToolHealth>,
    theme_id: ThemeId,
    palette: Palette,
    progress_style: ProgressBarStyle,
    row_density: RowDensity,
    show_concert_visualizer: bool,
    concert_visualizer_height: f32,
    linear_volume: bool,
    acoustid_api_key: String,

    view: View,
    folder_filter: Option<PathBuf>,
    sort: SortMode,
    sort_ascending: bool,
    search: String,
    focus_search: bool,

    // Cached filtered/sorted track table & sidebar stats (eliminates 60 Hz cloning/sorting!)
    library_rev: u64,
    queue_rev: u64,
    visible_cache_key: Option<VisibleCacheKey>,
    cached_visible: Vec<Track>,
    cached_total_duration: f64,
    cached_stats_rev: u64,
    cached_stats_lib_id: u64,
    cached_hires_count: usize,
    cached_folder_counts: HashMap<PathBuf, usize>,

    // Multi-track selection state
    selected_tracks: HashSet<u64>,
    selected_queue_indices: HashSet<usize>,
    selection_anchor: Option<usize>,

    // Playback state
    current_queue: Vec<u64>,
    current_index: Option<usize>,
    playing: bool,
    position: f64,
    duration: f64,
    show_remaining: bool,
    volume: f32,
    muted_volume: Option<f32>,
    shuffle: bool,
    repeat: RepeatMode,
    preferred_device: Option<String>,
    available_devices: Vec<String>,
    show_device_picker: bool,
    device_btn_rect: Option<Rect>,
    show_signal_path: bool,
    signal_btn_rect: Option<Rect>,
    always_on_top: bool,
    stream_info: Option<StreamInfo>,
    bitrate: Option<u32>,
    variable_bitrate: bool,

    // Acoustic waveform cache + async worker
    waveforms: HashMap<u64, Vec<f32>>,
    waveform_tx: crossbeam_channel::Sender<(u64, Vec<f32>)>,
    waveform_rx: crossbeam_channel::Receiver<(u64, Vec<f32>)>,
    waveform_inflight: Option<u64>,

    // Async album-keyed cover thumbnail cache (128x128 thumbnails, never blocks UI thread!)
    brand_icon: Option<TextureHandle>,
    artwork_by_path: HashMap<PathBuf, Option<TextureHandle>>,
    artwork_inflight: HashSet<PathBuf>,
    artwork_req_tx: crossbeam_channel::Sender<PathBuf>,
    artwork_res_rx: crossbeam_channel::Receiver<(PathBuf, Option<ColorImage>)>,

    scan_events: Option<crossbeam_channel::Receiver<ScanEvent>>,
    pending_scans: Vec<PathBuf>,
    scanning: bool,
    scan_progress: usize,
    scan_total: usize,
    scan_found: usize,

    // Modals & popups
    show_settings: bool,
    settings_tab: SettingsTab,
    show_folders: bool,
    show_libraries_modal: bool,
    new_library_name: String,
    show_new_playlist_modal: bool,
    new_playlist_name: String,
    pending_playlist_tracks: Vec<u64>,
    inspect_track_id: Option<u64>,
    tag_editor: TagEditorState,
    command_palette: CommandPaletteState,

    dialog_sender: crossbeam_channel::Sender<DialogEvent>,
    dialog_events: crossbeam_channel::Receiver<DialogEvent>,
    dialog_open: bool,
    status: Option<(String, Instant)>,
    last_frame: Instant,
    screenshot_out: Option<PathBuf>,
    screenshot_frames: u32,
    demo_piano_lock: bool,
    ephemeral_session: bool,
}

impl TxPlayerApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        let preferences = Preferences::load();
        let palette = theme::apply(&context.egui_ctx, preferences.theme);
        let audio = AudioHandle::spawn();
        let visual_tap = audio.visual_tap();
        audio.set_volume(preferences.volume);
        audio.set_linear_volume(preferences.linear_volume);
        audio.update_dsp(&preferences.plugins.dsp);
        if let Some(device) = &preferences.preferred_device {
            audio.select_device(Some(device.clone()));
        }
        let (dialog_sender, dialog_events) = crossbeam_channel::unbounded();
        let (waveform_tx, waveform_rx) = crossbeam_channel::unbounded();
        let (tool_health_tx, tool_health_rx) = crossbeam_channel::bounded(1);
        thread::spawn(move || {
            let _ = tool_health_tx.send(crate::tagger::detect_tool_health());
        });

        // Dedicated background artwork thumbnail decoder (128x128 fast resize, zero UI-thread stall)
        let (artwork_req_tx, artwork_req_rx) = crossbeam_channel::unbounded::<PathBuf>();
        let (artwork_res_tx, artwork_res_rx) =
            crossbeam_channel::unbounded::<(PathBuf, Option<ColorImage>)>();
        thread::Builder::new()
            .name("txplayer-art".into())
            .spawn(move || {
                while let Ok(path) = artwork_req_rx.recv() {
                    let img = std::fs::read(&path).ok().and_then(|bytes| {
                        let decoded = image::load_from_memory(&bytes).ok()?;
                        let thumb = if decoded.width() > 160 || decoded.height() > 160 {
                            decoded.thumbnail(160, 160)
                        } else {
                            decoded
                        };
                        let rgba = thumb.into_rgba8();
                        let dims = [rgba.width() as usize, rgba.height() as usize];
                        Some(ColorImage::from_rgba_unmultiplied(dims, rgba.as_raw()))
                    });
                    if artwork_res_tx.send((path, img)).is_err() {
                        break;
                    }
                }
            })
            .expect("artwork worker thread should start");

        let mut store = LibraryStore::load();
        store.active_mut().sort_default();

        let visualizer = VisualizerState::new(
            preferences.visualizer_mode,
            preferences.show_piano_keys,
            preferences.visualizer_sensitivity,
        );

        // Unified TxPlayer app icon texture (exact same assets/AppIcon.png as macOS Dock & .icns)
        let brand_icon = image::load_from_memory(crate::APP_ICON_PNG).ok().map(|img| {
            let thumb = img.thumbnail(128, 128).into_rgba8();
            let dims = [thumb.width() as usize, thumb.height() as usize];
            let color = ColorImage::from_rgba_unmultiplied(dims, thumb.as_raw());
            context.egui_ctx.load_texture(
                "txplayer-brand-icon",
                color,
                egui::TextureOptions::LINEAR,
            )
        });

        let mut app = Self {
            audio,
            visual_tap,
            visualizer,
            store,
            plugins: preferences.plugins,
            tool_health: ToolHealth::default(),
            tool_health_rx,
            theme_id: preferences.theme,
            palette,
            progress_style: preferences.progress_style,
            row_density: preferences.row_density,
            show_concert_visualizer: preferences.show_concert_visualizer,
            concert_visualizer_height: preferences.concert_visualizer_height,
            linear_volume: preferences.linear_volume,
            acoustid_api_key: preferences.acoustid_api_key,
            view: View::Library,
            folder_filter: None,
            sort: SortMode::Artist,
            sort_ascending: true,
            search: String::new(),
            focus_search: false,
            library_rev: 1,
            queue_rev: 1,
            visible_cache_key: None,
            cached_visible: Vec::new(),
            cached_total_duration: 0.0,
            cached_stats_rev: 0,
            cached_stats_lib_id: 0,
            cached_hires_count: 0,
            cached_folder_counts: HashMap::new(),
            selected_tracks: HashSet::new(),
            selected_queue_indices: HashSet::new(),
            selection_anchor: None,
            current_queue: Vec::new(),
            current_index: None,
            playing: false,
            position: 0.0,
            duration: 0.0,
            show_remaining: false,
            volume: preferences.volume,
            muted_volume: None,
            shuffle: preferences.shuffle,
            repeat: preferences.repeat,
            preferred_device: preferences.preferred_device,
            available_devices: list_output_devices(),
            show_device_picker: false,
            device_btn_rect: None,
            show_signal_path: false,
            signal_btn_rect: None,
            always_on_top: false,
            stream_info: None,
            bitrate: None,
            variable_bitrate: false,
            waveforms: HashMap::new(),
            waveform_tx,
            waveform_rx,
            waveform_inflight: None,
            brand_icon,
            artwork_by_path: HashMap::new(),
            artwork_inflight: HashSet::new(),
            artwork_req_tx,
            artwork_res_rx,
            scan_events: None,
            pending_scans: Vec::new(),
            scanning: false,
            scan_progress: 0,
            scan_total: 0,
            scan_found: 0,
            show_settings: false,
            settings_tab: SettingsTab::Appearance,
            show_folders: false,
            show_libraries_modal: false,
            new_library_name: String::new(),
            show_new_playlist_modal: false,
            new_playlist_name: String::new(),
            pending_playlist_tracks: Vec::new(),
            inspect_track_id: None,
            tag_editor: TagEditorState::new(),
            command_palette: CommandPaletteState::default(),
            dialog_sender,
            dialog_events,
            dialog_open: false,
            status: None,
            last_frame: Instant::now(),
            screenshot_out: None,
            screenshot_frames: 0,
            demo_piano_lock: false,
            ephemeral_session: false,
        };

        let arguments: Vec<String> = std::env::args().collect();
        if let Some(index) = arguments.iter().position(|a| a == "--theme")
            && let Some(val) = arguments.get(index + 1)
            && let Some(tid) = ThemeId::ALL.iter().copied().find(|t| {
                t.name()
                    .to_lowercase()
                    .replace(' ', "-")
                    == val.to_lowercase()
            })
        {
            app.theme_id = tid;
            app.palette = theme::apply(&context.egui_ctx, tid);
        }
        if let Some(index) = arguments.iter().position(|a| a == "--vis-mode")
            && let Some(val) = arguments.get(index + 1)
        {
            app.show_concert_visualizer = true;
            match val.to_lowercase().as_str() {
                "piano" => app.visualizer.mode = VisualizerMode::PianoHarmonic,
                "roll" => app.visualizer.mode = VisualizerMode::PianoRoll,
                "spectrum" => app.visualizer.mode = VisualizerMode::ConcertSpectrum,
                "scope" => app.visualizer.mode = VisualizerMode::StereoScope,
                "vu" => app.visualizer.mode = VisualizerMode::MasteringVu,
                _ => {}
            }
        }
        if let Some(index) = arguments.iter().position(|a| a == "--open-settings")
            && let Some(val) = arguments.get(index + 1)
        {
            app.show_settings = true;
            app.settings_tab = match val.to_lowercase().as_str() {
                "appearance" => SettingsTab::Appearance,
                "visualizer" => SettingsTab::Visualizer,
                "audio" | "dsp" => SettingsTab::Audio,
                "plugins" => SettingsTab::Plugins,
                "library" | "tags" => SettingsTab::LibraryTags,
                _ => SettingsTab::Plugins,
            };
        }
        if arguments.iter().any(|a| a == "--demo-piano") {
            app.show_concert_visualizer = true;
            app.visualizer.show_piano_keys = true;
            app.visualizer.seed_demo_piano_state();
            app.demo_piano_lock = true;
        }
        if let Some(index) = arguments.iter().position(|a| a == "--screenshot")
            && let Some(path) = arguments.get(index + 1)
        {
            app.screenshot_out = Some(PathBuf::from(path));
            app.ephemeral_session = true;
        }
        if let Some(index) = arguments.iter().position(|argument| argument == "--play")
            && let Some(path) = arguments.get(index + 1).map(PathBuf::from)
        {
            match crate::metadata::read_track(&path, &cover_directory()) {
                Ok(track) => {
                    app.current_queue = vec![track.id];
                    app.current_index = Some(0);
                    app.store.active_mut().upsert(track);
                    app.store.active_mut().sort_default();
                    app.start_current();
                }
                Err(error) => app.set_status(format!("Could not open {path:?}: {error:#}")),
            }
        }
        app
    }

    fn set_theme(&mut self, ctx: &egui::Context, theme_id: ThemeId) {
        self.theme_id = theme_id;
        self.palette = theme::apply(ctx, theme_id);
        self.set_status(format!("Theme switched to {}", theme_id.name()));
    }

    fn current_id(&self) -> Option<u64> {
        self.current_index
            .and_then(|index| self.current_queue.get(index).copied())
    }

    fn current_track(&self) -> Option<&Track> {
        let id = self.current_id()?;
        self.track(id)
    }

    fn track(&self, id: u64) -> Option<&Track> {
        self.store
            .active()
            .tracks
            .iter()
            .find(|track| track.id == id)
    }

    fn mark_library_dirty(&mut self) {
        self.library_rev = self.library_rev.wrapping_add(1);
    }

    fn mark_queue_dirty(&mut self) {
        self.queue_rev = self.queue_rev.wrapping_add(1);
    }

    fn ensure_sidebar_stats(&mut self) {
        let active_id = self.store.active_id;
        if self.cached_stats_rev == self.library_rev && self.cached_stats_lib_id == active_id {
            return;
        }
        let lib = self.store.active();
        self.cached_hires_count = lib.tracks.iter().filter(|t| t.is_hires()).count();
        self.cached_folder_counts.clear();
        for folder in &lib.folders {
            let count = lib.folder_track_count(folder);
            self.cached_folder_counts.insert(folder.clone(), count);
        }
        self.cached_stats_rev = self.library_rev;
        self.cached_stats_lib_id = active_id;
    }

    fn ensure_visible_cache(&mut self) {
        let key = VisibleCacheKey {
            library_rev: self.library_rev,
            queue_rev: if self.view == View::Queue {
                self.queue_rev
            } else {
                0
            },
            active_lib_id: self.store.active_id,
            view: self.view,
            folder_filter: self.folder_filter.clone(),
            sort: self.sort,
            sort_ascending: self.sort_ascending,
            search: self.search.clone(),
        };
        if self.visible_cache_key.as_ref() == Some(&key) {
            return;
        }
        let tracks = self.compute_visible_tracks();
        self.cached_total_duration = tracks.iter().filter_map(|t| t.duration).sum();
        self.cached_visible = tracks;
        self.visible_cache_key = Some(key);
    }

    fn visible_tracks(&mut self) -> Vec<Track> {
        self.ensure_visible_cache();
        self.cached_visible.clone()
    }

    fn compute_visible_tracks(&self) -> Vec<Track> {
        let lib = self.store.active();
        let query = self.search.trim().to_lowercase();
        let mut tracks: Vec<Track> = match self.view {
            View::Library => lib
                .tracks
                .iter()
                .filter(|track| {
                    self.folder_filter
                        .as_ref()
                        .is_none_or(|folder| track.path.starts_with(folder))
                })
                .filter(|track| query.is_empty() || track.search_text().contains(&query))
                .cloned()
                .collect(),
            View::Favorites => lib
                .tracks
                .iter()
                .filter(|track| lib.favorites.contains(&track.id))
                .filter(|track| query.is_empty() || track.search_text().contains(&query))
                .cloned()
                .collect(),
            View::Queue => self
                .current_queue
                .iter()
                .filter_map(|id| self.track(*id))
                .filter(|track| query.is_empty() || track.search_text().contains(&query))
                .cloned()
                .collect(),
            View::Playlist(pl_id) => lib
                .playlist(pl_id)
                .map(|pl| {
                    pl.track_ids
                        .iter()
                        .filter_map(|id| self.track(*id))
                        .filter(|track| query.is_empty() || track.search_text().contains(&query))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
        };

        if matches!(self.view, View::Library | View::Favorites) {
            tracks.sort_by(|a, b| {
                let ord = match self.sort {
                    SortMode::Artist => a
                        .artist
                        .to_lowercase()
                        .cmp(&b.artist.to_lowercase())
                        .then(a.album.to_lowercase().cmp(&b.album.to_lowercase()))
                        .then(a.disc_number.cmp(&b.disc_number))
                        .then(a.track_number.cmp(&b.track_number))
                        .then(a.title.to_lowercase().cmp(&b.title.to_lowercase())),
                    SortMode::Album => a
                        .album
                        .to_lowercase()
                        .cmp(&b.album.to_lowercase())
                        .then(a.disc_number.cmp(&b.disc_number))
                        .then(a.track_number.cmp(&b.track_number))
                        .then(a.title.to_lowercase().cmp(&b.title.to_lowercase())),
                    SortMode::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                    SortMode::Quality => {
                        let score = |t: &Track| -> (u8, u32, u32) {
                            let tier = match t.quality_tier() {
                                QualityTier::HiRes => 2,
                                QualityTier::Lossless => 1,
                                QualityTier::Standard => 0,
                            };
                            (tier, t.sample_rate.unwrap_or(0), t.bit_depth.unwrap_or(0))
                        };
                        score(b)
                            .cmp(&score(a))
                            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
                    }
                    SortMode::Duration => a
                        .duration
                        .partial_cmp(&b.duration)
                        .unwrap_or(std::cmp::Ordering::Equal),
                };
                if self.sort_ascending {
                    ord
                } else {
                    ord.reverse()
                }
            });
        }
        tracks
    }

    fn clear_selection(&mut self) {
        self.selected_tracks.clear();
        self.selected_queue_indices.clear();
        self.selection_anchor = None;
    }

    fn selected_count(&self) -> usize {
        if self.view == View::Queue {
            self.selected_queue_indices.len()
        } else {
            self.selected_tracks.len()
        }
    }

    fn selected_ordered_ids(&self, visible: &[Track]) -> Vec<u64> {
        if self.view == View::Queue {
            let mut idxs: Vec<usize> = self.selected_queue_indices.iter().copied().collect();
            idxs.sort_unstable();
            idxs.into_iter()
                .filter_map(|i| self.current_queue.get(i).copied())
                .collect()
        } else {
            visible
                .iter()
                .filter(|t| self.selected_tracks.contains(&t.id))
                .map(|t| t.id)
                .collect()
        }
    }

    fn select_row_click(
        &mut self,
        row_idx: usize,
        track_id: u64,
        visible: &[Track],
        cmd: bool,
        shift: bool,
    ) {
        if self.view == View::Queue {
            if shift && let Some(anchor) = self.selection_anchor {
                let (lo, hi) = (anchor.min(row_idx), anchor.max(row_idx));
                if !cmd {
                    self.selected_queue_indices.clear();
                }
                for i in lo..=hi {
                    self.selected_queue_indices.insert(i);
                }
            } else if cmd {
                if !self.selected_queue_indices.remove(&row_idx) {
                    self.selected_queue_indices.insert(row_idx);
                }
                self.selection_anchor = Some(row_idx);
            } else {
                self.selected_queue_indices.clear();
                self.selected_queue_indices.insert(row_idx);
                self.selection_anchor = Some(row_idx);
            }
        } else if shift && let Some(anchor) = self.selection_anchor {
            let (lo, hi) = (anchor.min(row_idx), anchor.max(row_idx));
            if !cmd {
                self.selected_tracks.clear();
            }
            for t in visible.iter().take(hi + 1).skip(lo) {
                self.selected_tracks.insert(t.id);
            }
        } else if cmd {
            if !self.selected_tracks.remove(&track_id) {
                self.selected_tracks.insert(track_id);
            }
            self.selection_anchor = Some(row_idx);
        } else {
            self.selected_tracks.clear();
            self.selected_tracks.insert(track_id);
            self.selection_anchor = Some(row_idx);
        }
    }

    fn toggle_row_checkbox(&mut self, row_idx: usize, track_id: u64) {
        if self.view == View::Queue {
            if !self.selected_queue_indices.remove(&row_idx) {
                self.selected_queue_indices.insert(row_idx);
            }
        } else if !self.selected_tracks.remove(&track_id) {
            self.selected_tracks.insert(track_id);
        }
        self.selection_anchor = Some(row_idx);
    }

    fn toggle_sort(&mut self, mode: SortMode) {
        if self.sort == mode {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort = mode;
            self.sort_ascending = true;
        }
    }

    fn play_from_list(&mut self, id: u64, tracks: &[Track]) {
        self.current_queue = tracks.iter().map(|track| track.id).collect();
        self.current_index = self.current_queue.iter().position(|item| *item == id);
        self.mark_queue_dirty();
        self.start_current();
    }

    fn play_selected(&mut self, visible: &[Track]) {
        let ids = self.selected_ordered_ids(visible);
        if ids.is_empty() {
            return;
        }
        if ids.len() == 1 {
            let id = ids[0];
            if self.current_id() == Some(id) {
                self.toggle_playback();
                return;
            }
            if self.view == View::Queue {
                if let Some(&qidx) = self.selected_queue_indices.iter().next() {
                    self.play_queue_at(qidx);
                }
            } else {
                self.play_from_list(id, visible);
            }
        } else {
            self.current_queue = ids;
            self.current_index = Some(0);
            self.mark_queue_dirty();
            self.start_current();
        }
    }

    fn play_queue_at(&mut self, index: usize) {
        if index < self.current_queue.len() {
            self.current_index = Some(index);
            self.start_current();
        }
    }

    fn play_next_ids(&mut self, ids: &[u64]) {
        if ids.is_empty() {
            return;
        }
        if self.current_queue.is_empty() {
            self.current_queue = ids.to_vec();
            self.current_index = Some(0);
            self.mark_queue_dirty();
            self.start_current();
            return;
        }
        let insert_at = self
            .current_index
            .map(|i| (i + 1).min(self.current_queue.len()))
            .unwrap_or(0);
        for (offset, &id) in ids.iter().enumerate() {
            self.current_queue.insert(insert_at + offset, id);
        }
        self.mark_queue_dirty();
        self.set_status(if ids.len() == 1 {
            let name = self.track(ids[0]).map(|t| t.title.as_str()).unwrap_or("Track");
            format!("Playing next: {name}")
        } else {
            format!("Queued {} tracks to play next", ids.len())
        });
    }

    fn enqueue_ids(&mut self, ids: &[u64]) {
        if ids.is_empty() {
            return;
        }
        let was_empty = self.current_queue.is_empty();
        self.current_queue.extend_from_slice(ids);
        self.mark_queue_dirty();
        if was_empty {
            self.current_index = Some(0);
            self.start_current();
        } else {
            self.set_status(if ids.len() == 1 {
                let name = self.track(ids[0]).map(|t| t.title.as_str()).unwrap_or("Track");
                format!("Added to queue: {name}")
            } else {
                format!("Added {} tracks to play queue", ids.len())
            });
        }
    }

    fn remove_tracks_from_library(&mut self, ids: &[u64]) {
        let removed = self.store.active_mut().remove_tracks(ids);
        if removed.is_empty() {
            return;
        }
        for track in &removed {
            self.waveforms.remove(&track.id);
            self.selected_tracks.remove(&track.id);
            if self.inspect_track_id == Some(track.id) {
                self.inspect_track_id = None;
            }
        }
        let removed_ids: Vec<u64> = removed.iter().map(|t| t.id).collect();
        self.purge_ids_from_queue(&removed_ids);
        self.mark_library_dirty();
        self.store.save();
        self.set_status(if removed.len() == 1 {
            format!("Removed \"{}\" from library", removed[0].title)
        } else {
            format!("Removed {} tracks from library", removed.len())
        });
    }

    fn remove_selected_in_current_view(&mut self, visible: &[Track]) {
        match self.view {
            View::Queue => {
                let mut indices: Vec<usize> = self.selected_queue_indices.iter().copied().collect();
                indices.sort_unstable();
                indices.reverse();
                let count = indices.len();
                for idx in indices {
                    self.remove_from_queue_at(idx, false);
                }
                self.selected_queue_indices.clear();
                if count > 0 {
                    self.set_status(format!(
                        "Removed {} {} from queue",
                        count,
                        if count == 1 { "track" } else { "tracks" }
                    ));
                }
            }
            View::Playlist(pl_id) => {
                let ids = self.selected_ordered_ids(visible);
                let removed = self
                    .store
                    .active_mut()
                    .remove_tracks_from_playlist(pl_id, &ids);
                if removed > 0 {
                    self.mark_library_dirty();
                    self.store.save();
                    self.selected_tracks.clear();
                    self.set_status(format!(
                        "Removed {} {} from playlist",
                        removed,
                        if removed == 1 { "track" } else { "tracks" }
                    ));
                }
            }
            View::Library | View::Favorites => {
                let ids = self.selected_ordered_ids(visible);
                if !ids.is_empty() {
                    self.remove_tracks_from_library(&ids);
                }
            }
        }
    }

    fn remove_folder_from_library(&mut self, folder: &Path) {
        let removed_ids = self.store.active_mut().remove_folder(folder);
        for id in &removed_ids {
            self.waveforms.remove(id);
            self.selected_tracks.remove(id);
            if self.inspect_track_id == Some(*id) {
                self.inspect_track_id = None;
            }
        }
        if self.folder_filter.as_deref() == Some(folder) {
            self.folder_filter = None;
        }
        self.purge_ids_from_queue(&removed_ids);
        self.mark_library_dirty();
        self.store.save();
        let name = folder
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("directory");
        self.set_status(format!(
            "Removed directory \"{name}\" ({} tracks)",
            removed_ids.len()
        ));
    }

    fn purge_ids_from_queue(&mut self, removed_ids: &[u64]) {
        if removed_ids.is_empty() || self.current_queue.is_empty() {
            return;
        }
        let was_current_removed = self
            .current_id()
            .is_some_and(|cid| removed_ids.contains(&cid));
        let old_current_idx = self.current_index;
        let mut new_queue = Vec::with_capacity(self.current_queue.len());
        let mut new_current_idx = None;
        for (idx, &qid) in self.current_queue.iter().enumerate() {
            if !removed_ids.contains(&qid) {
                if old_current_idx == Some(idx) {
                    new_current_idx = Some(new_queue.len());
                }
                new_queue.push(qid);
            }
        }
        self.current_queue = new_queue;
        self.selected_queue_indices.clear();
        self.mark_queue_dirty();

        if was_current_removed {
            if self.current_queue.is_empty() {
                self.stop_playback();
            } else {
                let next_idx = old_current_idx
                    .unwrap_or(0)
                    .min(self.current_queue.len() - 1);
                self.current_index = Some(next_idx);
                if self.playing {
                    self.start_current();
                } else {
                    self.stop_playback_keep_queue(Some(next_idx));
                }
            }
        } else {
            self.current_index = new_current_idx;
        }
    }

    fn remove_from_queue_at(&mut self, queue_index: usize, announce: bool) {
        if queue_index >= self.current_queue.len() {
            return;
        }
        let removed_id = self.current_queue.remove(queue_index);
        self.mark_queue_dirty();
        let title = self
            .track(removed_id)
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "Track".into());

        if let Some(cur) = self.current_index {
            if cur == queue_index {
                if self.current_queue.is_empty() {
                    self.stop_playback();
                } else {
                    let next_idx = queue_index.min(self.current_queue.len() - 1);
                    self.current_index = Some(next_idx);
                    if self.playing {
                        self.start_current();
                    } else {
                        self.stop_playback_keep_queue(Some(next_idx));
                    }
                }
            } else if cur > queue_index {
                self.current_index = Some(cur - 1);
            }
        }
        if announce {
            self.set_status(format!("Removed \"{title}\" from queue"));
        }
    }

    fn move_queue_item(&mut self, from: usize, to: usize) {
        if from >= self.current_queue.len() || to >= self.current_queue.len() || from == to {
            return;
        }
        self.current_queue.swap(from, to);
        self.mark_queue_dirty();
        if self.current_index == Some(from) {
            self.current_index = Some(to);
        } else if self.current_index == Some(to) {
            self.current_index = Some(from);
        }
        self.selected_queue_indices.clear();
        self.selected_queue_indices.insert(to);
    }

    fn clear_queue(&mut self) {
        self.current_queue.clear();
        self.mark_queue_dirty();
        self.stop_playback();
        self.selected_queue_indices.clear();
        self.set_status("Play queue cleared".into());
    }

    fn stop_playback(&mut self) {
        self.audio.stop();
        self.visualizer.reset();
        self.playing = false;
        self.current_index = None;
        self.position = 0.0;
        self.duration = 0.0;
        self.bitrate = None;
        self.variable_bitrate = false;
        self.stream_info = None;
    }

    fn stop_playback_keep_queue(&mut self, new_index: Option<usize>) {
        self.audio.stop();
        self.playing = false;
        self.current_index = new_index;
        self.position = 0.0;
        self.duration = self
            .current_track()
            .and_then(|t| t.duration)
            .unwrap_or(0.0);
        self.bitrate = None;
        self.variable_bitrate = false;
        self.stream_info = None;
    }

    fn request_waveform(&mut self, track: &Track) {
        if self.waveforms.contains_key(&track.id) || self.waveform_inflight == Some(track.id) {
            return;
        }
        self.waveform_inflight = Some(track.id);
        let id = track.id;
        let path = track.path.clone();
        let tx = self.waveform_tx.clone();
        thread::spawn(move || {
            if let Some(wf) = compute_waveform(&path, WAVEFORM_BINS) {
                let _ = tx.send((id, wf));
            }
        });
    }

    fn start_current(&mut self) {
        let Some(track) = self.current_track().cloned() else {
            self.playing = false;
            return;
        };
        if !track.path.exists() {
            self.set_status(format!("File not found: {}", track.path.display()));
            self.playing = false;
            return;
        }
        self.position = 0.0;
        self.duration = track.duration.unwrap_or(0.0);
        self.bitrate = None;
        self.variable_bitrate = false;
        self.stream_info = None;
        self.visualizer.reset();
        self.request_waveform(&track);
        self.audio.play(track.path);
        self.playing = true;
    }

    fn toggle_playback(&mut self) {
        if self.current_id().is_none() {
            let tracks = self.visible_tracks();
            let sel = self.selected_ordered_ids(&tracks);
            if let Some(&first_sel) = sel.first() {
                self.play_from_list(first_sel, &tracks);
            } else if let Some(track) = tracks.first() {
                self.play_from_list(track.id, &tracks);
            }
        } else if self.playing {
            self.audio.pause();
        } else {
            self.audio.resume();
        }
    }

    fn next(&mut self, automatic: bool) {
        if self.current_queue.is_empty() {
            return;
        }
        if automatic && self.repeat == RepeatMode::One {
            self.start_current();
            return;
        }
        let next = if self.shuffle && self.current_queue.len() > 1 {
            let current = self.current_index.unwrap_or(0);
            let offset = rand::thread_rng().gen_range(1..self.current_queue.len());
            Some((current + offset) % self.current_queue.len())
        } else {
            let current = self.current_index.unwrap_or(0);
            if current + 1 < self.current_queue.len() {
                Some(current + 1)
            } else if self.repeat == RepeatMode::All || !automatic {
                Some(0)
            } else {
                None
            }
        };
        if let Some(index) = next {
            self.current_index = Some(index);
            self.start_current();
        } else {
            self.playing = false;
            self.position = self.duration;
        }
    }

    fn previous(&mut self) {
        if self.position > 3.0 {
            self.audio.seek(0.0);
            self.position = 0.0;
            return;
        }
        if self.current_queue.is_empty() {
            return;
        }
        let current = self.current_index.unwrap_or(0);
        self.current_index = Some(if current == 0 {
            self.current_queue.len() - 1
        } else {
            current - 1
        });
        self.start_current();
    }

    fn start_scan(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.scanning {
            self.pending_scans.extend(paths);
            return;
        }
        let (sender, receiver) = crossbeam_channel::unbounded();
        let cover_directory = cover_directory();
        thread::spawn(move || scan(paths, sender, cover_directory));
        self.scan_events = Some(receiver);
        self.scanning = true;
        self.scan_progress = 0;
        self.scan_total = 0;
        self.scan_found = 0;
    }

    fn add_files(&mut self) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Add Audio Tracks")
            .add_filter("Audio Files", SUPPORTED_EXTENSIONS);
        let events = self.dialog_sender.clone();
        thread::spawn(move || {
            let paths = pollster::block_on(dialog.pick_files())
                .unwrap_or_default()
                .into_iter()
                .map(|handle| handle.path().to_path_buf())
                .collect();
            let _ = events.send(DialogEvent::Files(paths));
        });
    }

    fn add_folder(&mut self) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        let dialog = rfd::AsyncFileDialog::new().set_title("Add Music Directory");
        let events = self.dialog_sender.clone();
        thread::spawn(move || {
            let paths = pollster::block_on(dialog.pick_folders())
                .unwrap_or_default()
                .into_iter()
                .map(|handle| handle.path().to_path_buf())
                .collect();
            let _ = events.send(DialogEvent::Folders(paths));
        });
    }

    fn rescan(&mut self) {
        let folders = self.store.active().folders.clone();
        if folders.is_empty() {
            self.set_status("No directories configured in this library".into());
            return;
        }
        self.start_scan(folders);
    }

    fn process_background_events(&mut self, context: &egui::Context) {
        if let Ok(health) = self.tool_health_rx.try_recv() {
            self.tool_health = health;
        }

        // Upload decoded artwork thumbnails on UI thread (fast 160x160 GPU upload, zero disk I/O!)
        let mut uploaded = 0usize;
        while uploaded < 8
            && let Ok((path, maybe_img)) = self.artwork_res_rx.try_recv()
        {
            self.artwork_inflight.remove(&path);
            let tex = maybe_img.map(|img| {
                context.load_texture(
                    format!("cover-{}", path.display()),
                    img,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.artwork_by_path.insert(path, tex);
            uploaded += 1;
        }

        // Audio events
        while let Some(event) = self.audio.try_event() {
            match event {
                AudioEvent::Started {
                    source_sample_rate,
                    output_sample_rate,
                    channels,
                    bit_depth,
                    duration,
                    device_name,
                } => {
                    self.stream_info = Some(StreamInfo {
                        source_rate: source_sample_rate,
                        output_rate: output_sample_rate,
                        channels,
                        bit_depth,
                        device_name,
                    });
                    if let Some(duration) = duration {
                        self.duration = duration;
                        if let Some(cid) = self.current_id()
                            && let Some(track) = self
                                .store
                                .active_mut()
                                .tracks
                                .iter_mut()
                                .find(|t| t.id == cid)
                            && track.duration.is_none()
                        {
                            track.duration = Some(duration);
                        }
                    }
                }
                AudioEvent::DeviceRouted(name) => {
                    self.available_devices = list_output_devices();
                    self.set_status(format!("Audio output switched to {name}"));
                }
                AudioEvent::Playing(playing) => self.playing = playing,
                AudioEvent::Stopped => {
                    self.playing = false;
                    self.position = 0.0;
                }
                AudioEvent::Position(position) => self.position = position,
                AudioEvent::Bitrate { kbps, variable } => {
                    self.bitrate = Some(kbps);
                    self.variable_bitrate = variable;
                }
                AudioEvent::Ended => self.next(true),
                AudioEvent::Error(error) => self.set_status(error),
            }
        }

        // Waveform results
        while let Ok((id, wf)) = self.waveform_rx.try_recv() {
            if self.waveform_inflight == Some(id) {
                self.waveform_inflight = None;
            }
            self.waveforms.insert(id, wf);
        }

        // File picker dialogs
        while let Ok(event) = self.dialog_events.try_recv() {
            self.dialog_open = false;
            match event {
                DialogEvent::Files(paths) => {
                    if !paths.is_empty() {
                        self.start_scan(paths);
                    }
                }
                DialogEvent::Folders(paths) => {
                    if !paths.is_empty() {
                        for path in &paths {
                            self.store.active_mut().add_folder(path.clone());
                        }
                        self.mark_library_dirty();
                        self.store.save();
                        self.start_scan(paths);
                    }
                }
            }
        }

        // Library scan events
        let mut finished = false;
        let mut message = None;
        let mut scan_dirty = false;
        if let Some(events) = &self.scan_events {
            while let Ok(event) = events.try_recv() {
                match event {
                    ScanEvent::Track(track) => {
                        self.scan_found += 1;
                        self.store.active_mut().upsert(*track);
                        scan_dirty = true;
                    }
                    ScanEvent::Progress { scanned, total } => {
                        self.scan_progress = scanned;
                        self.scan_total = total;
                    }
                    ScanEvent::Finished { found, failed } => {
                        finished = true;
                        self.scanning = false;
                        self.store.active_mut().sort_default();
                        scan_dirty = true;
                        self.store.save();
                        message = Some(if found == 0 && failed == 0 {
                            "No supported audio files found".into()
                        } else if failed == 0 {
                            format!(
                                "Added or updated {found} {}",
                                if found == 1 { "track" } else { "tracks" }
                            )
                        } else {
                            format!("Imported {found} tracks · {failed} unreadable")
                        });
                    }
                }
            }
        }
        if scan_dirty {
            self.mark_library_dirty();
        }
        if let Some(msg) = message {
            self.set_status(msg);
        }
        if finished {
            self.scan_events = None;
            if !self.pending_scans.is_empty() {
                let paths = std::mem::take(&mut self.pending_scans);
                self.start_scan(paths);
            }
        }

        // Tag editor background worker events
        while let Ok(ev) = self.tag_editor.rx.try_recv() {
            match ev {
                TagWorkerEvent::HashesReady { track_id, hashes } => {
                    if self.tag_editor.primary_track.as_ref().map(|t| t.id) == Some(track_id) {
                        self.tag_editor.hashing = false;
                        match hashes {
                            Ok(h) => self.tag_editor.hashes = Some(h),
                            Err(e) => self.tag_editor.hash_error = Some(e),
                        }
                    }
                }
                TagWorkerEvent::LookupReady {
                    candidates,
                    note,
                    updated_hashes,
                } => {
                    self.tag_editor.searching = false;
                    if let Some(h) = updated_hashes {
                        self.tag_editor.hashes = Some(h);
                    }
                    self.tag_editor.candidates = candidates;
                    self.tag_editor.lookup_note = note;
                    if !self.tag_editor.candidates.is_empty() {
                        self.tag_editor.selected_candidate = Some(0);
                    }
                }
                TagWorkerEvent::CoverReady { track_ids, result } => {
                    self.tag_editor.fetching_cover = false;
                    match result {
                        Ok(cover_path) => {
                            self.artwork_by_path.remove(&cover_path);
                            self.artwork_inflight.remove(&cover_path);
                            for tid in &track_ids {
                                if let Some(track) = self
                                    .store
                                    .active_mut()
                                    .tracks
                                    .iter_mut()
                                    .find(|t| t.id == *tid)
                                {
                                    track.cover_path = Some(cover_path.clone());
                                }
                            }
                            self.mark_library_dirty();
                            self.store.save();
                            self.set_status(format!(
                                "Applied cover art to {} track(s)",
                                track_ids.len()
                            ));
                        }
                        Err(e) => self.set_status(format!("Cover fetch failed: {e}")),
                    }
                }
                TagWorkerEvent::SaveDone {
                    updated_tracks,
                    disk_written,
                    disk_errors,
                } => {
                    self.tag_editor.saving = false;
                    let total = updated_tracks.len();
                    for track in updated_tracks {
                        self.store.active_mut().upsert(track);
                    }
                    self.store.active_mut().sort_default();
                    self.mark_library_dirty();
                    self.store.save();
                    if disk_errors.is_empty() {
                        self.set_status(if disk_written > 0 {
                            format!("Saved tags to library and {disk_written} audio file(s)")
                        } else {
                            format!("Saved tags for {total} track(s) in library")
                        });
                        self.tag_editor.open = false;
                    } else {
                        self.set_status(format!(
                            "Updated library; file write note: {}",
                            disk_errors[0]
                        ));
                    }
                }
            }
        }
    }

    fn process_input(&mut self, context: &egui::Context) {
        let dropped = context.input(|input| input.raw.dropped_files.clone());
        let paths: Vec<PathBuf> = dropped
            .into_iter()
            .map(|file| file.path().to_path_buf())
            .collect();
        if !paths.is_empty() {
            let mut added_folder = false;
            for path in &paths {
                if path.is_dir() {
                    self.store.active_mut().add_folder(path.clone());
                    added_folder = true;
                }
            }
            if added_folder {
                self.store.save();
            }
            self.start_scan(paths);
        }

        // Escape ALWAYS dismisses active popovers/modals even if a child widget holds focus
        if context.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.command_palette.open {
                self.command_palette.open = false;
                return;
            } else if self.show_device_picker {
                self.show_device_picker = false;
                return;
            } else if self.show_signal_path {
                self.show_signal_path = false;
                return;
            } else if self.tag_editor.open {
                self.tag_editor.open = false;
                return;
            } else if self.show_settings {
                self.show_settings = false;
                return;
            } else if self.inspect_track_id.is_some() {
                self.inspect_track_id = None;
                return;
            } else if self.show_folders {
                self.show_folders = false;
                return;
            } else if self.show_libraries_modal {
                self.show_libraries_modal = false;
                return;
            } else if self.show_new_playlist_modal {
                self.show_new_playlist_modal = false;
                return;
            } else if !context.egui_wants_keyboard_input() {
                self.clear_selection();
                return;
            }
        }

        let cmd = context.input(|i| i.modifiers.command);
        if cmd && context.input(|i| i.key_pressed(egui::Key::K)) {
            if self.command_palette.open {
                self.command_palette.open = false;
            } else {
                self.command_palette.open_fresh();
            }
            return;
        }
        if cmd && context.input(|i| i.key_pressed(egui::Key::F)) {
            self.focus_search = true;
            return;
        }
        if cmd && context.input(|i| i.key_pressed(egui::Key::Comma)) {
            self.show_settings = !self.show_settings;
            return;
        }

        if self.command_palette.open || context.egui_wants_keyboard_input() {
            return;
        }

        if cmd && context.input(|i| i.key_pressed(egui::Key::T)) {
            let visible = self.visible_tracks();
            let sel = self.selected_ordered_ids(&visible);
            if !sel.is_empty() {
                self.open_tag_editor_for_ids(&sel);
            } else if let Some(cid) = self.current_id() {
                self.open_tag_editor_for_ids(&[cid]);
            }
            return;
        }
        if cmd && context.input(|i| i.key_pressed(egui::Key::I)) {
            let visible = self.visible_tracks();
            let sel = self.selected_ordered_ids(&visible);
            if let Some(&first) = sel.first() {
                self.inspect_track_id = Some(first);
            } else if let Some(cid) = self.current_id() {
                self.inspect_track_id = Some(cid);
            }
            return;
        }

        if cmd && context.input(|i| i.key_pressed(egui::Key::A)) {
            let visible = self.visible_tracks();
            if self.view == View::Queue {
                self.selected_queue_indices = (0..visible.len()).collect();
            } else {
                self.selected_tracks = visible.iter().map(|t| t.id).collect();
            }
            return;
        }

        if context.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle_playback();
        }
        if context.input(|i| i.key_pressed(egui::Key::M)) {
            self.toggle_mute();
        }
        if !cmd && context.input(|i| i.key_pressed(egui::Key::V)) {
            self.show_concert_visualizer = !self.show_concert_visualizer;
        }
        if !cmd && context.input(|i| i.key_pressed(egui::Key::E)) {
            self.plugins.dsp.enabled = !self.plugins.dsp.enabled;
            self.audio.update_dsp(&self.plugins.dsp);
            self.set_status(format!("Audio DSP: {}", self.plugins.dsp.summary()));
        }
        if !cmd {
            let num_keys = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
            ];
            let enabled = self.plugins.visualizers.enabled_modes();
            for (idx, key) in num_keys.iter().enumerate() {
                if context.input(|i| i.key_pressed(*key))
                    && let Some(&mode) = enabled.get(idx)
                {
                    self.visualizer.mode = mode;
                    self.show_concert_visualizer = true;
                }
            }
        }

        if context.input(|i| i.key_pressed(egui::Key::ArrowRight)) && self.current_id().is_some() {
            if cmd {
                self.next(false);
            } else {
                let target = (self.position + 10.0).min(self.duration.max(self.position + 10.0));
                self.audio.seek(target);
                self.position = target;
            }
        }
        if context.input(|i| i.key_pressed(egui::Key::ArrowLeft)) && self.current_id().is_some() {
            if cmd {
                self.previous();
            } else {
                let target = (self.position - 10.0).max(0.0);
                self.audio.seek(target);
                self.position = target;
            }
        }
        if context.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            if cmd {
                self.set_volume(self.volume + 0.05);
            } else {
                self.move_selection(-1);
            }
        }
        if context.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            if cmd {
                self.set_volume(self.volume - 0.05);
            } else {
                self.move_selection(1);
            }
        }
        if context.input(|i| i.key_pressed(egui::Key::Enter)) {
            let visible = self.visible_tracks();
            self.play_selected(&visible);
        }
        if context.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        {
            let visible = self.visible_tracks();
            self.remove_selected_in_current_view(&visible);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let tracks = self.visible_tracks();
        if tracks.is_empty() {
            return;
        }
        let cur = self.selection_anchor.unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, tracks.len() as isize - 1) as usize;
        self.selection_anchor = Some(next);
        if self.view == View::Queue {
            self.selected_queue_indices.clear();
            self.selected_queue_indices.insert(next);
        } else {
            self.selected_tracks.clear();
            self.selected_tracks.insert(tracks[next].id);
        }
    }

    fn set_volume(&mut self, value: f32) {
        self.volume = value.clamp(0.0, 1.0);
        if self.volume > 0.0 {
            self.muted_volume = None;
        }
        self.audio.set_volume(self.volume);
    }

    fn toggle_mute(&mut self) {
        if self.volume > 0.001 {
            self.muted_volume = Some(self.volume);
            self.volume = 0.0;
            self.audio.set_volume(0.0);
        } else {
            let restore = self.muted_volume.take().unwrap_or(0.75).max(0.1);
            self.volume = restore;
            self.audio.set_volume(restore);
        }
    }

    fn set_status(&mut self, message: String) {
        self.status = Some((message, Instant::now()));
    }

    fn open_tag_editor_for_ids(&mut self, ids: &[u64]) {
        let tracks: Vec<Track> = ids.iter().filter_map(|id| self.track(*id).cloned()).collect();
        if !tracks.is_empty() {
            self.tag_editor.open_for(&tracks);
        }
    }

    /// Custom theme-aligned macOS traffic-light window controls + draggable chrome strip.
    fn window_controls_strip(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let p = self.palette;
        let (strip_rect, strip_resp) =
            ui.allocate_exact_size(vec2(SIDEBAR_WIDTH, 32.0), Sense::click_and_drag());

        let is_focused = ctx.input(|i| i.focused);
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let is_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        let alt_held = ctx.input(|i| i.modifiers.alt || i.modifiers.shift);

        // 3 custom themed traffic dots positioned cleanly at top-left
        let dot_r = 6.0;
        let dot_y = strip_rect.top() + 17.0;
        let start_x = strip_rect.left() + 18.0;
        let spacing = 20.0;

        let close_rect = Rect::from_center_size(pos2(start_x, dot_y), Vec2::splat(16.0));
        let min_rect = Rect::from_center_size(pos2(start_x + spacing, dot_y), Vec2::splat(16.0));
        let zoom_rect =
            Rect::from_center_size(pos2(start_x + spacing * 2.0, dot_y), Vec2::splat(16.0));

        let close_resp = ui
            .interact(close_rect, ui.scope_id().with("win_btn_close"), Sense::click())
            .on_hover_text("Close Window");
        let min_resp = ui
            .interact(min_rect, ui.scope_id().with("win_btn_min"), Sense::click())
            .on_hover_text("Minimize Window");
        let zoom_resp = ui
            .interact(zoom_rect, ui.scope_id().with("win_btn_zoom"), Sense::click())
            .on_hover_text(if is_fullscreen {
                "Exit Fullscreen"
            } else {
                "Zoom Window · Option+Click for Fullscreen"
            });

        // Subtle Always-on-Top pin dot on top-right of sidebar chrome
        let pin_rect = Rect::from_center_size(pos2(strip_rect.right() - 18.0, dot_y), Vec2::splat(18.0));
        let pin_resp = ui
            .interact(pin_rect, ui.scope_id().with("win_btn_pin"), Sense::click())
            .on_hover_text(if self.always_on_top {
                "Always on Top: On (Click to disable)"
            } else {
                "Pin Window Always on Top"
            });

        let cluster_rect = Rect::from_min_max(close_rect.min, zoom_rect.max).expand(2.0);
        let cluster_hovered = ctx
            .pointer_hover_pos()
            .is_some_and(|pt| cluster_rect.contains(pt));

        let painter = ui.painter();
        let glyph_col = if p.is_light {
            Color32::from_rgb(24, 34, 38)
        } else {
            Color32::from_rgb(12, 16, 18)
        };

        let paint_dot = |center: egui::Pos2, base_col: Color32, hovered: bool, kind: u8| {
            let fill = if !is_focused && !cluster_hovered {
                p.surface_elevated
            } else if hovered {
                lerp_c(base_col, Color32::WHITE, 0.18)
            } else {
                base_col
            };
            let border = if !is_focused && !cluster_hovered {
                p.border_strong
            } else {
                Color32::from_rgba_unmultiplied(0, 0, 0, if p.is_light { 45 } else { 90 })
            };
            painter.circle_filled(center, dot_r, fill);
            painter.circle_stroke(center, dot_r, Stroke::new(0.8, border));

            if cluster_hovered {
                let st = Stroke::new(1.3, glyph_col);
                match kind {
                    // Close (×)
                    0 => {
                        let d = 2.6;
                        painter.line_segment(
                            [pos2(center.x - d, center.y - d), pos2(center.x + d, center.y + d)],
                            st,
                        );
                        painter.line_segment(
                            [pos2(center.x - d, center.y + d), pos2(center.x + d, center.y - d)],
                            st,
                        );
                    }
                    // Minimize (−)
                    1 => {
                        let d = 3.1;
                        painter.line_segment([pos2(center.x - d, center.y), pos2(center.x + d, center.y)], st);
                    }
                    // Zoom / Fullscreen (⤢)
                    _ => {
                        let d = 2.6;
                        painter.line_segment(
                            [pos2(center.x - d, center.y + d), pos2(center.x + d, center.y - d)],
                            st,
                        );
                        painter.line_segment(
                            [pos2(center.x + d - 2.2, center.y - d), pos2(center.x + d, center.y - d)],
                            st,
                        );
                        painter.line_segment(
                            [pos2(center.x + d, center.y - d), pos2(center.x + d, center.y - d + 2.2)],
                            st,
                        );
                        painter.line_segment(
                            [pos2(center.x - d, center.y + d), pos2(center.x - d + 2.2, center.y + d)],
                            st,
                        );
                        painter.line_segment(
                            [pos2(center.x - d, center.y + d), pos2(center.x - d, center.y + d - 2.2)],
                            st,
                        );
                    }
                }
            }
        };

        paint_dot(close_rect.center(), p.danger, close_resp.hovered(), 0);
        paint_dot(min_rect.center(), p.hires_gold, min_resp.hovered(), 1);
        paint_dot(zoom_rect.center(), p.accent, zoom_resp.hovered(), 2);

        // Subtle Always-on-Top indicator
        if self.always_on_top || pin_resp.hovered() {
            let bg = if self.always_on_top {
                p.accent_subtle
            } else {
                p.hover
            };
            painter.rect_filled(pin_rect, 4.0, bg);
        }
        let pin_c = pin_rect.center();
        let pin_col = if self.always_on_top {
            p.accent
        } else if pin_resp.hovered() {
            p.text_secondary
        } else {
            p.faint
        };
        painter.circle_filled(pos2(pin_c.x, pin_c.y - 1.8), 2.5, pin_col);
        painter.line_segment(
            [pos2(pin_c.x, pin_c.y + 0.5), pos2(pin_c.x, pin_c.y + 4.8)],
            Stroke::new(1.3, pin_col),
        );

        // Dispatch real window commands
        if close_resp.clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if min_resp.clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        } else if zoom_resp.clicked() {
            if is_fullscreen || alt_held {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
            }
        } else if pin_resp.clicked() {
            self.always_on_top = !self.always_on_top;
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if self.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));
            self.set_status(if self.always_on_top {
                "Window pinned Always on Top".into()
            } else {
                "Window unpinned".into()
            });
        } else if strip_resp.double_clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
        } else if strip_resp.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }

    fn sidebar(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        self.ensure_sidebar_stats();
        let p = self.palette;
        self.window_controls_strip(ctx, ui);
        ui.add_space(4.0);

        // Brand Header with Unified AppIcon
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let (mark_rect, _) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
            self.paint_brand_icon(ui.painter(), mark_rect);
            ui.add_space(5.0);
            ui.vertical(|ui| {
                ui.style_mut().spacing.item_spacing.y = 0.0;
                ui.horizontal(|ui| {
                    ui.style_mut().spacing.item_spacing.x = 0.0;
                    ui.label(RichText::new("Tx").size(16.5).strong().color(p.accent));
                    ui.label(RichText::new("Player").size(16.5).strong().color(p.text));
                });
                ui.label(
                    RichText::new("CONCERT HI-RES")
                        .size(8.5)
                        .strong()
                        .color(p.faint),
                );
            });
        });

        ui.add_space(12.0);
        // Library Switcher (System with Multiple Libraries)
        let active_lib_name = self.store.active().name.clone();
        let libraries_list: Vec<(u64, String, usize)> = self
            .store
            .libraries
            .iter()
            .map(|l| (l.id, l.name.clone(), l.tracks.len()))
            .collect();
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            egui::ComboBox::from_id_salt("active_library_switcher")
                .width(SIDEBAR_WIDTH - 48.0)
                .selected_text(RichText::new(&active_lib_name).size(11.5).strong())
                .show_ui(ui, |ui| {
                    for (id, name, count) in &libraries_list {
                        let is_cur = *id == self.store.active_id;
                        if ui
                            .selectable_label(is_cur, format!("{name}  ({count})"))
                            .clicked()
                            && !is_cur
                        {
                            self.store.active_id = *id;
                            self.view = View::Library;
                            self.folder_filter = None;
                            self.clear_selection();
                            self.store.save();
                        }
                    }
                    ui.separator();
                    if ui.button("+ Manage Libraries…").clicked() {
                        self.show_libraries_modal = true;
                    }
                });
            if icons::icon_button(
                ui,
                Vec2::splat(22.0),
                Icon::Plus,
                12.0,
                p.muted,
                p.accent,
                p.hover,
                "Create or manage libraries",
            )
            .clicked()
            {
                self.show_libraries_modal = true;
            }
        });

        ui.add_space(10.0);
        // Primary Add Actions
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            let half = (SIDEBAR_WIDTH - 30.0) * 0.5;
            if sidebar_compact_action(ui, half, "Tracks", Icon::TrackPlus, true, &p).clicked() {
                self.add_files();
            }
            if sidebar_compact_action(ui, half, "Folder", Icon::FolderPlus, false, &p).clicked() {
                self.add_folder();
            }
        });

        ui.add_space(12.0);
        // Scrollable Sidebar Body: Views + Playlists + Directories
        let footer_reserve = if self.scanning { 104.0 } else { 70.0 };
        ScrollArea::vertical()
            .id_salt("sidebar_scroll")
            .max_height((ui.available_height() - footer_reserve).max(120.0))
            .show(ui, |ui| {
                ui.style_mut().spacing.item_spacing.y = 2.0;

                // VIEWS
                let all_count = self.store.active().tracks.len();
                let fav_count = self.store.active().favorites.len();
                let queue_count = self.current_queue.len();

                self.nav_row(
                    ui,
                    "All Tracks",
                    Icon::Library,
                    all_count,
                    self.view == View::Library && self.folder_filter.is_none(),
                    View::Library,
                );
                self.nav_row(
                    ui,
                    "Favorites",
                    Icon::HeartFilled,
                    fav_count,
                    self.view == View::Favorites,
                    View::Favorites,
                );
                self.nav_row(
                    ui,
                    "Play Queue",
                    Icon::Queue,
                    queue_count,
                    self.view == View::Queue,
                    View::Queue,
                );

                // PLAYLISTS
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(RichText::new("PLAYLISTS").size(9.5).color(p.faint).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(10.0);
                        if icons::icon_button(
                            ui,
                            Vec2::splat(20.0),
                            Icon::Plus,
                            11.5,
                            p.muted,
                            p.accent,
                            p.hover,
                            "New playlist",
                        )
                        .clicked()
                        {
                            self.new_playlist_name.clear();
                            self.pending_playlist_tracks.clear();
                            self.show_new_playlist_modal = true;
                        }
                    });
                });

                let playlists: Vec<(u64, String, usize)> = self
                    .store
                    .active()
                    .playlists
                    .iter()
                    .map(|pl| (pl.id, pl.name.clone(), pl.track_ids.len()))
                    .collect();
                let mut playlist_to_delete: Option<u64> = None;
                if playlists.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new("No playlists yet")
                                .size(10.5)
                                .color(p.faint),
                        );
                    });
                } else {
                    for (pl_id, name, count) in playlists {
                        let selected = self.view == View::Playlist(pl_id);
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            let (rect, resp) = ui.allocate_exact_size(
                                vec2(SIDEBAR_WIDTH - 20.0, 26.0),
                                Sense::click(),
                            );
                            let hovered = resp.hovered();
                            if selected {
                                ui.painter().rect_filled(rect, 6.0, p.selected);
                                let bar = Rect::from_min_max(
                                    pos2(rect.left() + 2.0, rect.top() + 6.0),
                                    pos2(rect.left() + 5.0, rect.bottom() - 6.0),
                                );
                                ui.painter().rect_filled(bar, 1.5, p.accent);
                            } else if hovered {
                                ui.painter().rect_filled(rect, 6.0, p.hover);
                            }
                            let icon_rect = Rect::from_center_size(
                                pos2(rect.left() + 15.0, rect.center().y),
                                Vec2::splat(13.0),
                            );
                            icons::paint_icon(
                                ui.painter(),
                                icon_rect,
                                Icon::Playlist,
                                if selected { p.accent } else { p.muted },
                            );

                            let del_rect = Rect::from_center_size(
                                pos2(rect.right() - 13.0, rect.center().y),
                                Vec2::splat(18.0),
                            );
                            let over_del = ui
                                .ctx()
                                .pointer_hover_pos()
                                .is_some_and(|pt| del_rect.contains(pt));

                            let text_rect = Rect::from_min_max(
                                pos2(rect.left() + 28.0, rect.top()),
                                pos2(rect.right() - 28.0, rect.bottom()),
                            );
                            ui.painter().with_clip_rect(text_rect).text(
                                pos2(text_rect.left(), rect.center().y),
                                Align2::LEFT_CENTER,
                                &name,
                                FontId::proportional(11.5),
                                if selected { p.text } else { p.text_secondary },
                            );

                            if hovered {
                                if over_del {
                                    ui.painter().rect_filled(del_rect, 4.0, p.danger_bg);
                                }
                                let xr = Rect::from_center_size(del_rect.center(), Vec2::splat(9.5));
                                icons::paint_icon(
                                    ui.painter(),
                                    xr,
                                    Icon::Close,
                                    if over_del { p.danger } else { p.muted },
                                );
                            } else {
                                ui.painter().text(
                                    pos2(rect.right() - 8.0, rect.center().y),
                                    Align2::RIGHT_CENTER,
                                    format!("{count}"),
                                    FontId::proportional(10.0),
                                    if selected { p.accent } else { p.faint },
                                );
                            }

                            if resp.clicked() {
                                if over_del {
                                    playlist_to_delete = Some(pl_id);
                                } else {
                                    self.view = View::Playlist(pl_id);
                                    self.folder_filter = None;
                                    self.clear_selection();
                                }
                            }
                        });
                    }
                }
                if let Some(pl_id) = playlist_to_delete {
                    if self.view == View::Playlist(pl_id) {
                        self.view = View::Library;
                    }
                    if let Some(name) = self.store.active_mut().delete_playlist(pl_id) {
                        self.store.save();
                        self.set_status(format!("Deleted playlist \"{name}\""));
                    }
                }

                // DIRECTORIES
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new("DIRECTORIES")
                            .size(9.5)
                            .color(p.faint)
                            .strong(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(10.0);
                        if icons::icon_button(
                            ui,
                            Vec2::splat(20.0),
                            Icon::Plus,
                            11.5,
                            p.muted,
                            p.accent,
                            p.hover,
                            "Add music directory",
                        )
                        .clicked()
                        {
                            self.add_folder();
                        }
                        if !self.store.active().folders.is_empty()
                            && icons::icon_button(
                                ui,
                                Vec2::splat(20.0),
                                Icon::Refresh,
                                11.5,
                                p.muted,
                                p.text,
                                p.hover,
                                "Rescan all directories",
                            )
                            .clicked()
                        {
                            self.rescan();
                        }
                    });
                });

                let folders = self.store.active().folders.clone();
                let mut folder_to_remove: Option<PathBuf> = None;
                if folders.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new("Drop folders or click +")
                                .size(10.5)
                                .color(p.faint),
                        );
                    });
                } else {
                    for folder in folders {
                        let is_filtered = self.view == View::Library
                            && self.folder_filter.as_deref() == Some(folder.as_path());
                        let name = folder
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("Directory");
                        let count = self
                            .cached_folder_counts
                            .get(&folder)
                            .copied()
                            .unwrap_or(0);

                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            let (rect, resp) = ui.allocate_exact_size(
                                vec2(SIDEBAR_WIDTH - 20.0, 26.0),
                                Sense::click(),
                            );
                            let hovered = resp.hovered();
                            if is_filtered {
                                ui.painter().rect_filled(rect, 6.0, p.selected);
                                ui.painter().rect_stroke(
                                    rect,
                                    6.0,
                                    Stroke::new(1.0, p.selected_border),
                                    StrokeKind::Inside,
                                );
                            } else if hovered {
                                ui.painter().rect_filled(rect, 6.0, p.hover);
                            }
                            let icon_rect = Rect::from_center_size(
                                pos2(rect.left() + 14.0, rect.center().y),
                                Vec2::splat(13.0),
                            );
                            icons::paint_icon(
                                ui.painter(),
                                icon_rect,
                                Icon::Folder,
                                if is_filtered { p.accent } else { p.muted },
                            );

                            let del_rect = Rect::from_center_size(
                                pos2(rect.right() - 13.0, rect.center().y),
                                Vec2::splat(18.0),
                            );
                            let over_del = ui
                                .ctx()
                                .pointer_hover_pos()
                                .is_some_and(|pt| del_rect.contains(pt));

                            let text_rect = Rect::from_min_max(
                                pos2(rect.left() + 26.0, rect.top()),
                                pos2(rect.right() - 28.0, rect.bottom()),
                            );
                            ui.painter().with_clip_rect(text_rect).text(
                                pos2(text_rect.left(), rect.center().y),
                                Align2::LEFT_CENTER,
                                name,
                                FontId::proportional(11.5),
                                if is_filtered { p.text } else { p.text_secondary },
                            );

                            if hovered {
                                if over_del {
                                    ui.painter().rect_filled(del_rect, 4.0, p.danger_bg);
                                }
                                let xr = Rect::from_center_size(del_rect.center(), Vec2::splat(9.5));
                                icons::paint_icon(
                                    ui.painter(),
                                    xr,
                                    Icon::Close,
                                    if over_del { p.danger } else { p.muted },
                                );
                            } else {
                                ui.painter().text(
                                    pos2(rect.right() - 8.0, rect.center().y),
                                    Align2::RIGHT_CENTER,
                                    format!("{count}"),
                                    FontId::proportional(10.0),
                                    p.faint,
                                );
                            }

                            if resp.clicked() {
                                if over_del {
                                    folder_to_remove = Some(folder.clone());
                                } else if is_filtered {
                                    self.folder_filter = None;
                                } else {
                                    self.view = View::Library;
                                    self.folder_filter = Some(folder.clone());
                                    self.clear_selection();
                                }
                            }
                            resp.on_hover_text(format!(
                                "{}\n{} tracks · Click to filter · × to remove",
                                folder.display(),
                                count
                            ));
                        });
                    }
                }
                if let Some(folder) = folder_to_remove {
                    self.remove_folder_from_library(&folder);
                }
            });

        // Bottom Footer: Theme quick cycle + Settings + Scan indicator
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                if pill_button(ui, "Settings", Some(Icon::Settings), PillStyle::Secondary, &p)
                    .on_hover_text("Open Settings (Cmd+,)")
                    .clicked()
                {
                    self.show_settings = true;
                }
                if icons::icon_button(
                    ui,
                    Vec2::splat(24.0),
                    Icon::Palette,
                    14.0,
                    p.muted,
                    p.accent,
                    p.hover,
                    &format!("Theme: {} · Click to cycle", self.theme_id.name()),
                )
                .clicked()
                {
                    let idx = ThemeId::ALL
                        .iter()
                        .position(|&t| t == self.theme_id)
                        .unwrap_or(0);
                    let next = ThemeId::ALL[(idx + 1) % ThemeId::ALL.len()];
                    self.set_theme(ctx, next);
                }
                if icons::icon_button(
                    ui,
                    Vec2::splat(24.0),
                    Icon::Folder,
                    14.0,
                    p.muted,
                    p.text,
                    p.hover,
                    "Manage Directories",
                )
                .clicked()
                {
                    self.show_folders = true;
                }
            });

            if self.scanning {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            let label = if self.scan_total > 0 {
                                format!("Scanning {}/{}", self.scan_progress, self.scan_total)
                            } else {
                                format!("Scanning · {} found", self.scan_found)
                            };
                            ui.label(RichText::new(label).size(10.0).color(p.accent));
                        });
                        if self.scan_total > 0 {
                            let frac = (self.scan_progress as f32 / self.scan_total as f32)
                                .clamp(0.0, 1.0);
                            let (bar_rect, _) = ui.allocate_exact_size(
                                vec2(SIDEBAR_WIDTH - 28.0, 3.0),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(bar_rect, 1.5, p.surface);
                            let fill = Rect::from_min_max(
                                bar_rect.min,
                                pos2(bar_rect.left() + bar_rect.width() * frac, bar_rect.bottom()),
                            );
                            ui.painter().rect_filled(fill, 1.5, p.accent);
                        }
                    });
                });
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                let total_tracks = self.store.active().tracks.len();
                let hires = self.cached_hires_count;
                let summary = if hires > 0 {
                    format!("{total_tracks} tracks · {hires} Hi-Res")
                } else {
                    format!("{total_tracks} tracks")
                };
                ui.label(RichText::new(summary).size(10.0).color(p.faint));
            });
        });
    }

    fn nav_row(
        &mut self,
        ui: &mut Ui,
        label: &str,
        icon: Icon,
        count: usize,
        selected: bool,
        target: View,
    ) {
        let p = self.palette;
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            let (rect, resp) =
                ui.allocate_exact_size(vec2(SIDEBAR_WIDTH - 20.0, 28.0), Sense::click());
            let hovered = resp.hovered();
            if selected {
                ui.painter().rect_filled(rect, 6.0, p.selected);
                let bar = Rect::from_min_max(
                    pos2(rect.left() + 2.0, rect.top() + 6.0),
                    pos2(rect.left() + 5.0, rect.bottom() - 6.0),
                );
                ui.painter().rect_filled(bar, 1.5, p.accent);
            } else if hovered {
                ui.painter().rect_filled(rect, 6.0, p.hover);
            }
            let icon_rect =
                Rect::from_center_size(pos2(rect.left() + 17.0, rect.center().y), Vec2::splat(13.5));
            icons::paint_icon(
                ui.painter(),
                icon_rect,
                icon,
                if selected { p.accent } else { p.muted },
            );
            ui.painter().text(
                pos2(rect.left() + 33.0, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(12.0),
                if selected { p.text } else { p.text_secondary },
            );
            if count > 0 {
                ui.painter().text(
                    pos2(rect.right() - 10.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    format!("{count}"),
                    FontId::proportional(10.0),
                    if selected { p.accent } else { p.faint },
                );
            }
            if resp.clicked() {
                self.view = target;
                if target == View::Library {
                    self.folder_filter = None;
                }
                self.clear_selection();
            }
        });
    }

    fn main_view(&mut self, ui: &mut Ui) {
        self.ensure_visible_cache();
        let p = self.palette;
        let tracks = self.cached_visible.clone();
        let total_duration = self.cached_total_duration;

        // Seamless themed top window drag strip across the main panel (double-click to zoom)
        let (_, drag_resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::click_and_drag());
        if drag_resp.double_clicked() {
            let is_max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
        } else if drag_resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        // Header Row
        ui.horizontal(|ui| {
            let title = match self.view {
                View::Library => {
                    if let Some(folder) = &self.folder_filter {
                        folder
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("Directory")
                            .to_string()
                    } else {
                        self.store.active().name.clone()
                    }
                }
                View::Favorites => "Favorites".to_string(),
                View::Queue => "Play Queue".to_string(),
                View::Playlist(pl_id) => self
                    .store
                    .active()
                    .playlist(pl_id)
                    .map(|pl| pl.name.clone())
                    .unwrap_or_else(|| "Playlist".to_string()),
            };
            ui.label(RichText::new(title).size(22.0).strong().color(p.text));

            if self.view == View::Library
                && self.folder_filter.is_some()
                && pill_button(ui, "× All Tracks", None, PillStyle::Active, &p).clicked()
            {
                self.folder_filter = None;
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Concert Visualizer toggle pill
                if pill_button(
                    ui,
                    "Visualizer",
                    Some(Icon::Visualizer),
                    if self.show_concert_visualizer {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    },
                    &p,
                )
                .on_hover_text("Toggle expanded Concert Piano & Classical Visualizer")
                .clicked()
                {
                    self.show_concert_visualizer = !self.show_concert_visualizer;
                }

                // Quick Command Palette pill
                if pill_button(
                    ui,
                    &format!("{MOD_SHORT}K"),
                    Some(Icon::Search),
                    PillStyle::Secondary,
                    &p,
                )
                .on_hover_text(format!("Quick Command Palette ({MOD_SHORT}K)"))
                .clicked()
                {
                    self.command_palette.open_fresh();
                }

                // Search bar
                Frame::new()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(7.0)
                    .inner_margin(Margin::symmetric(8, 2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (ir, _) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::hover());
                            icons::paint_icon(ui.painter(), ir, Icon::Search, p.faint);
                            let search = ui.add_sized(
                                [156.0, 20.0],
                                egui::TextEdit::singleline(&mut self.search)
                                    .frame(Frame::NONE)
                                    .hint_text(
                                        RichText::new("Filter library… (⌘F)")
                                            .size(11.5)
                                            .color(p.faint),
                                    ),
                            );
                            if self.focus_search {
                                search.request_focus();
                                self.focus_search = false;
                            }
                            if search.changed() {
                                self.selected_queue_indices.clear();
                            }
                            if !self.search.is_empty()
                                && icons::icon_button(
                                    ui,
                                    Vec2::splat(16.0),
                                    Icon::Close,
                                    10.0,
                                    p.muted,
                                    p.text,
                                    p.hover,
                                    "Clear search",
                                )
                                .clicked()
                            {
                                self.search.clear();
                            }
                        });
                    });

                if self.view == View::Queue && !self.current_queue.is_empty() {
                    if pill_button(ui, "Clear Queue", Some(Icon::Trash), PillStyle::Danger, &p)
                        .clicked()
                    {
                        self.clear_queue();
                    }
                } else if !tracks.is_empty() {
                    if pill_button(ui, "Shuffle", Some(Icon::Shuffle), PillStyle::Secondary, &p)
                        .clicked()
                    {
                        self.shuffle = true;
                        let idx = rand::thread_rng().gen_range(0..tracks.len());
                        self.play_from_list(tracks[idx].id, &tracks);
                    }
                    if pill_button(ui, "Play All", Some(Icon::Play), PillStyle::Accent, &p)
                        .clicked()
                    {
                        self.play_from_list(tracks[0].id, &tracks);
                    }
                }
            });
        });

        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{} {}  ·  {}",
                    tracks.len(),
                    if tracks.len() == 1 { "track" } else { "tracks" },
                    format_library_duration(total_duration)
                ))
                .color(p.muted)
                .size(11.0),
            );
            if !tracks.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(
                            "Hover row for 1-click Play / Next / Tags  ·  ⌘K palette  ·  ⌘T tags  ·  V visualizer  ·  E DSP",
                        )
                        .color(p.faint)
                        .size(10.0),
                    );
                });
            }
        });

        ui.add_space(8.0);
        // Strictly Aligned Column Header (uses exact same TrackColumns as every row!)
        self.aligned_table_header(ui, &tracks);
        let sep_rect = ui.available_rect_before_wrap();
        ui.painter().line_segment(
            [pos2(sep_rect.left(), sep_rect.top()), pos2(sep_rect.right(), sep_rect.top())],
            Stroke::new(1.0, p.border),
        );
        ui.add_space(3.0);

        // Track Table ScrollArea (reserve bottom room if Concert Visualizer stage is open)
        let vis_reserve = if self.show_concert_visualizer {
            self.concert_visualizer_height + 34.0
        } else {
            0.0
        };
        let list_h = (ui.available_height() - vis_reserve).max(110.0);

        ScrollArea::vertical()
            .id_salt("aligned_track_table")
            .max_height(list_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().spacing.item_spacing.y = 2.0;
                if tracks.is_empty() {
                    self.empty_state(ui);
                } else {
                    self.aligned_track_rows(ui, &tracks);
                }
                // Extra padding so floating action dock never obscures the bottom row
                ui.add_space(54.0);
            });

        // Expandable Concert Classical & Piano Visualizer Stage
        if self.show_concert_visualizer {
            let enabled_modes = self.plugins.visualizers.enabled_modes();
            if !enabled_modes.contains(&self.visualizer.mode) {
                self.visualizer.mode = enabled_modes[0];
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("CONCERT VISUALIZER")
                        .size(9.5)
                        .strong()
                        .color(p.faint),
                );
                ui.add_space(6.0);
                for &m in &enabled_modes {
                    if pill_button(
                        ui,
                        m.short_name(),
                        None,
                        if self.visualizer.mode == m {
                            PillStyle::Active
                        } else {
                            PillStyle::Secondary
                        },
                        &p,
                    )
                    .on_hover_text(m.description())
                    .clicked()
                    {
                        self.visualizer.mode = m;
                    }
                }
                if self.visualizer.mode == VisualizerMode::PianoHarmonic
                    && pill_button(
                        ui,
                        "88 Keys",
                        None,
                        if self.visualizer.show_piano_keys {
                            PillStyle::Active
                        } else {
                            PillStyle::Secondary
                        },
                        &p,
                    )
                    .clicked()
                {
                    self.visualizer.show_piano_keys = !self.visualizer.show_piano_keys;
                }
                if matches!(
                    self.visualizer.mode,
                    VisualizerMode::PianoHarmonic | VisualizerMode::PianoRoll
                ) {
                    let eng = self.plugins.visualizers.piano_engine;
                    if pill_button(ui, eng.short_name(), Some(Icon::Plugin), PillStyle::Secondary, &p)
                        .on_hover_text(format!(
                            "{}\nClick to switch engine (or configure in Settings → Plugins)",
                            eng.description()
                        ))
                        .clicked()
                    {
                        self.plugins.visualizers.piano_engine = match eng {
                            PianoEngineId::PrecisionCqt => PianoEngineId::ClassicFft,
                            PianoEngineId::ClassicFft => PianoEngineId::PrecisionCqt,
                        };
                        self.set_status(format!(
                            "Piano Engine: {}",
                            self.plugins.visualizers.piano_engine.name()
                        ));
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icons::icon_button(
                        ui,
                        Vec2::splat(20.0),
                        Icon::Close,
                        10.0,
                        p.muted,
                        p.text,
                        p.hover,
                        "Hide Concert Visualizer (V)",
                    )
                    .clicked()
                    {
                        self.show_concert_visualizer = false;
                    }
                });
            });
            ui.add_space(3.0);
            self.visualizer.ui_concert_stage(
                ui,
                self.concert_visualizer_height,
                &p,
                &self.plugins.visualizers,
            );
            ui.add_space(6.0);
        }
    }

    fn aligned_table_header(&mut self, ui: &mut Ui, visible: &[Track]) {
        let p = self.palette;
        let sortable = matches!(self.view, View::Library | View::Favorites);
        let row_w = ui.available_width();
        let (row_rect, _) = ui.allocate_exact_size(vec2(row_w, 24.0), Sense::hover());
        let cols = TrackColumns::compute(row_rect);

        // Master select-all checkbox (strictly aligned with every row's checkbox!)
        let total = visible.len();
        let sel_count = if self.view == View::Queue {
            self.selected_queue_indices.len()
        } else {
            visible
                .iter()
                .filter(|t| self.selected_tracks.contains(&t.id))
                .count()
        };
        let all_checked = total > 0 && sel_count == total;
        let indeterminate = sel_count > 0 && sel_count < total;
        let cb_resp = ui.interact(
            cols.checkbox,
            ui.scope_id().with("header_select_all"),
            Sense::click(),
        );
        icons::paint_checkbox(
            ui.painter(),
            cols.checkbox,
            all_checked,
            indeterminate,
            cb_resp.hovered(),
            &p,
        );
        if cb_resp.clicked() && total > 0 {
            if all_checked {
                self.clear_selection();
            } else if self.view == View::Queue {
                self.selected_queue_indices = (0..total).collect();
            } else {
                self.selected_tracks = visible.iter().map(|t| t.id).collect();
            }
        }

        // Index column header
        ui.painter().text(
            cols.index.center(),
            Align2::CENTER_CENTER,
            "#",
            FontId::monospace(9.5),
            p.faint,
        );

        // Sortable column labels painted into exact TrackColumns rects
        let mut draw_col_header = |cell: Rect, label: &str, mode: SortMode, right_align: bool| {
            let active = sortable && self.sort == mode;
            let text = if active {
                format!("{} {}", label, if self.sort_ascending { "▲" } else { "▼" })
            } else {
                label.to_string()
            };
            let resp = ui.interact(
                cell,
                ui.scope_id().with(("col_hdr", label)),
                if sortable { Sense::click() } else { Sense::hover() },
            );
            let col = if active {
                p.accent
            } else if sortable && resp.hovered() {
                p.text
            } else {
                p.faint
            };
            let (pos, align) = if right_align {
                (pos2(cell.right() - 2.0, cell.center().y), Align2::RIGHT_CENTER)
            } else {
                (pos2(cell.left(), cell.center().y), Align2::LEFT_CENTER)
            };
            ui.painter()
                .text(pos, align, text, FontId::proportional(9.5), col);
            if sortable && resp.clicked() {
                self.toggle_sort(mode);
            }
        };

        draw_col_header(cols.title, "TITLE", SortMode::Title, false);
        draw_col_header(cols.artist, "ARTIST", SortMode::Artist, false);
        draw_col_header(cols.album, "ALBUM", SortMode::Album, false);
        draw_col_header(cols.format, "FORMAT", SortMode::Quality, false);
        draw_col_header(cols.duration, "TIME", SortMode::Duration, true);
    }

    fn aligned_track_rows(&mut self, ui: &mut Ui, tracks: &[Track]) {
        let p = self.palette;
        let current_id = self.current_id();
        let time = ui.ctx().input(|i| i.time);
        let row_h = self.row_density.row_height();
        let row_w = ui.available_width();
        let cmd = ui.ctx().input(|i| i.modifiers.command);
        let shift = ui.ctx().input(|i| i.modifiers.shift);

        let mut play_target: Option<(usize, u64)> = None;
        let mut toggle_current_play = false;
        let mut fav_toggle: Option<u64> = None;
        let mut quick_play_next: Option<u64> = None;
        let mut quick_enqueue: Option<u64> = None;
        let mut quick_tag: Option<u64> = None;
        let mut ctx_action: Option<ContextMenuAction> = None;

        // Hoisted once per frame instead of allocating per visible row!
        let playlists_snap: Vec<(u64, String)> = self
            .store
            .active()
            .playlists
            .iter()
            .map(|pl| (pl.id, pl.name.clone()))
            .collect();
        let current_view = self.view;

        for (row_idx, track) in tracks.iter().enumerate() {
            let is_current = if self.view == View::Queue {
                self.current_index == Some(row_idx)
            } else {
                current_id == Some(track.id)
            };
            let is_selected = if self.view == View::Queue {
                self.selected_queue_indices.contains(&row_idx)
            } else {
                self.selected_tracks.contains(&track.id)
            };
            let is_favorite = self.store.active().favorites.contains(&track.id);

            let (row_rect, _) = ui.allocate_exact_size(vec2(row_w, row_h), Sense::hover());
            if !ui.is_rect_visible(row_rect) {
                continue;
            }
            let cols = TrackColumns::compute(row_rect);

            let cb_id = ui.scope_id().with(("row_cb", row_idx, track.id));
            let play_id = ui.scope_id().with(("row_play", row_idx, track.id));
            let fav_id = ui.scope_id().with(("row_fav", row_idx, track.id));
            let body_id = ui.scope_id().with(("row_body", row_idx, track.id));

            // Determine row hover via pointer position so inline hover actions appear cleanly
            let row_pointer_over = ui
                .ctx()
                .pointer_hover_pos()
                .is_some_and(|pt| row_rect.contains(pt));

            // Inline hover quick-action buttons positioned cleanly inside the FORMAT column while hovered
            let mut hover_next_resp: Option<egui::Response> = None;
            let mut hover_queue_resp: Option<egui::Response> = None;
            let mut hover_tag_resp: Option<egui::Response> = None;
            let quick_btn_w = 24.0;
            let quick_gap = 4.0;
            let quick_right = cols.format.right();
            let q_tag_rect = Rect::from_center_size(
                pos2(quick_right - quick_btn_w * 0.5, row_rect.center().y),
                vec2(quick_btn_w, 22.0),
            );
            let q_queue_rect = Rect::from_center_size(
                pos2(q_tag_rect.left() - quick_gap - quick_btn_w * 0.5, row_rect.center().y),
                vec2(quick_btn_w, 22.0),
            );
            let q_next_rect = Rect::from_center_size(
                pos2(q_queue_rect.left() - quick_gap - quick_btn_w * 0.5, row_rect.center().y),
                vec2(quick_btn_w, 22.0),
            );

            // Non-overlapping hit regions: checkbox | 1-click play | body | (inline quick actions when hovered) | favorite heart
            let body_right = if row_pointer_over {
                q_next_rect.left() - 4.0
            } else {
                cols.favorite.left() - 2.0
            };
            let body_rect = Rect::from_min_max(
                pos2(cols.cover.left(), row_rect.top()),
                pos2(body_right.max(cols.cover.right()), row_rect.bottom()),
            );

            let cb_resp = ui.interact(cols.checkbox, cb_id, Sense::click());
            let play_resp = ui.interact(cols.index, play_id, Sense::click());
            let fav_resp = ui.interact(cols.favorite, fav_id, Sense::click());
            let body_resp = ui.interact(body_rect, body_id, Sense::click());

            if row_pointer_over {
                if current_view != View::Queue {
                    hover_next_resp = Some(
                        ui.interact(
                            q_next_rect,
                            ui.scope_id().with(("row_qn", row_idx, track.id)),
                            Sense::click(),
                        )
                        .on_hover_text("Play Next (1-click)"),
                    );
                    hover_queue_resp = Some(
                        ui.interact(
                            q_queue_rect,
                            ui.scope_id().with(("row_qq", row_idx, track.id)),
                            Sense::click(),
                        )
                        .on_hover_text("Add to Queue (1-click)"),
                    );
                }
                hover_tag_resp = Some(
                    ui.interact(
                        q_tag_rect,
                        ui.scope_id().with(("row_qt", row_idx, track.id)),
                        Sense::click(),
                    )
                    .on_hover_text("Edit Tags & Lookup (1-click)"),
                );
            }

            let row_hovered = row_pointer_over
                || cb_resp.hovered()
                || play_resp.hovered()
                || fav_resp.hovered()
                || body_resp.hovered();

            // Row background card
            let bg = if is_selected {
                p.selected
            } else if is_current {
                p.playing_row
            } else if row_hovered {
                p.hover
            } else if row_idx % 2 == 1 {
                if p.is_light {
                    Color32::from_rgba_unmultiplied(0, 0, 0, 6)
                } else {
                    Color32::from_rgba_unmultiplied(255, 255, 255, 3)
                }
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(row_rect, 6.0, bg);

            if is_selected {
                ui.painter().rect_stroke(
                    row_rect,
                    6.0,
                    Stroke::new(1.0, p.selected_border),
                    StrokeKind::Inside,
                );
                let accent_bar = Rect::from_min_max(
                    pos2(row_rect.left() + 1.5, row_rect.top() + 7.0),
                    pos2(row_rect.left() + 4.0, row_rect.bottom() - 7.0),
                );
                ui.painter().rect_filled(accent_bar, 1.5, p.accent);
            } else if is_current {
                ui.painter().rect_stroke(
                    row_rect,
                    6.0,
                    Stroke::new(1.0, p.accent.gamma_multiply(0.35)),
                    StrokeKind::Inside,
                );
            }

            // 1) Aligned 15x15 Checkbox
            icons::paint_checkbox(
                ui.painter(),
                cols.checkbox,
                is_selected,
                false,
                cb_resp.hovered(),
                &p,
            );

            // 2) Index / Animated Equalizer / 1-Click Hover Play Button!
            if row_hovered {
                if play_resp.hovered() {
                    ui.painter().circle_filled(cols.index.center(), 11.0, p.accent);
                }
                let ic = if is_current && self.playing {
                    Icon::Pause
                } else {
                    Icon::Play
                };
                let ic_col = if play_resp.hovered() {
                    p.on_accent()
                } else {
                    p.accent
                };
                icons::paint_icon(
                    ui.painter(),
                    Rect::from_center_size(cols.index.center(), Vec2::splat(12.0)),
                    ic,
                    ic_col,
                );
            } else if is_current {
                let eq_rect = Rect::from_center_size(cols.index.center(), vec2(12.0, 12.0));
                icons::paint_playing_bars(ui.painter(), eq_rect, time, self.playing, p.accent);
            } else {
                ui.painter().text(
                    cols.index.center(),
                    Align2::CENTER_CENTER,
                    format!("{:02}", row_idx + 1),
                    FontId::monospace(10.0),
                    if is_selected { p.accent } else { p.faint },
                );
            }

            // 3) Cover Thumbnail (28x28 dead-centered)
            let art_size = (row_h - 8.0).clamp(26.0, 32.0);
            let art_rect = Rect::from_center_size(cols.cover.center(), Vec2::splat(art_size));
            self.paint_cover_in_rect(ui, track, art_rect);

            // 4) Title, Artist, Album (strictly clipped & ellipsis-safe inside exact column rects)
            paint_clipped_cell_text(
                ui.painter(),
                cols.title,
                &track.title,
                FontId::proportional(12.5),
                if is_current { p.accent } else { p.text },
            );
            paint_clipped_cell_text(
                ui.painter(),
                cols.artist,
                &track.artist,
                FontId::proportional(11.5),
                if is_selected {
                    p.text_secondary
                } else {
                    p.muted
                },
            );
            paint_clipped_cell_text(
                ui.painter(),
                cols.album,
                &track.album,
                FontId::proportional(11.5),
                p.muted,
            );

            // 5) Format Badge OR 1-Click Inline Hover Actions (Play Next / Queue / Tags)
            if row_hovered {
                let paint_qbtn = |rect: Rect, icon: Icon, hovered: bool| {
                    ui.painter().rect_filled(
                        rect,
                        5.0,
                        if hovered { p.accent } else { p.surface_elevated },
                    );
                    icons::paint_icon(
                        ui.painter(),
                        Rect::from_center_size(rect.center(), Vec2::splat(12.0)),
                        icon,
                        if hovered { p.on_accent() } else { p.text_secondary },
                    );
                };
                if let Some(ref r) = hover_next_resp {
                    paint_qbtn(q_next_rect, Icon::PlayNext, r.hovered());
                }
                if let Some(ref r) = hover_queue_resp {
                    paint_qbtn(q_queue_rect, Icon::Plus, r.hovered());
                }
                if let Some(ref r) = hover_tag_resp {
                    paint_qbtn(q_tag_rect, Icon::Tag, r.hovered());
                }
            } else {
                paint_aligned_quality_badge(ui.painter(), cols.format, track, &p);
            }

            // 6) Tabular Monospace Duration (right-aligned)
            ui.painter().text(
                pos2(cols.duration.right() - 2.0, cols.duration.center().y),
                Align2::RIGHT_CENTER,
                format_duration(track.duration),
                FontId::monospace(11.0),
                p.muted,
            );

            // 7) Favorite Heart Button (centered)
            let heart_icon = if is_favorite {
                Icon::HeartFilled
            } else {
                Icon::HeartOutline
            };
            let heart_color = if fav_resp.hovered() {
                p.accent_bright
            } else if is_favorite {
                p.accent
            } else if row_hovered || is_selected {
                p.faint
            } else {
                Color32::TRANSPARENT
            };
            if fav_resp.hovered() {
                ui.painter().rect_filled(
                    Rect::from_center_size(cols.favorite.center(), Vec2::splat(22.0)),
                    5.0,
                    p.surface_elevated,
                );
            }
            icons::paint_icon(
                ui.painter(),
                Rect::from_center_size(cols.favorite.center(), Vec2::splat(13.0)),
                heart_icon,
                heart_color,
            );

            // Interactions
            if cb_resp.clicked() {
                self.toggle_row_checkbox(row_idx, track.id);
            } else if play_resp.clicked() {
                if is_current {
                    toggle_current_play = true;
                } else {
                    play_target = Some((row_idx, track.id));
                }
            } else if hover_next_resp.as_ref().is_some_and(egui::Response::clicked) {
                quick_play_next = Some(track.id);
            } else if hover_queue_resp.as_ref().is_some_and(egui::Response::clicked) {
                quick_enqueue = Some(track.id);
            } else if hover_tag_resp.as_ref().is_some_and(egui::Response::clicked) {
                quick_tag = Some(track.id);
            } else if fav_resp.clicked() {
                fav_toggle = Some(track.id);
            } else if body_resp.double_clicked() {
                play_target = Some((row_idx, track.id));
                self.select_row_click(row_idx, track.id, tracks, false, false);
            } else if body_resp.clicked() {
                self.select_row_click(row_idx, track.id, tracks, cmd, shift);
            }

            // Right-click Floating Context Menu directly at mouse pointer
            let tid = track.id;
            body_resp.context_menu(|ui| {
                if !is_selected {
                    self.select_row_click(row_idx, tid, tracks, false, false);
                }
                let sel_count = self.selected_count().max(1);
                if ui
                    .button(if sel_count > 1 {
                        format!("Play {sel_count} Selected Tracks")
                    } else if is_current && self.playing {
                        "Pause".into()
                    } else {
                        "Play".into()
                    })
                    .clicked()
                {
                    ctx_action = Some(ContextMenuAction::PlaySelected);
                    ui.close();
                }
                if current_view != View::Queue {
                    if ui.button("Play Next").clicked() {
                        ctx_action = Some(ContextMenuAction::PlayNext);
                        ui.close();
                    }
                    if ui.button("Add to Play Queue").clicked() {
                        ctx_action = Some(ContextMenuAction::AddToQueue);
                        ui.close();
                    }
                }
                ui.menu_button("Add to Playlist", |ui| {
                    for (pl_id, pl_name) in &playlists_snap {
                        if ui.button(pl_name).clicked() {
                            ctx_action = Some(ContextMenuAction::AddToPlaylist(*pl_id));
                            ui.close();
                        }
                    }
                    if !playlists_snap.is_empty() {
                        ui.separator();
                    }
                    if ui.button("+ New Playlist…").clicked() {
                        ctx_action = Some(ContextMenuAction::NewPlaylistWithSelected);
                        ui.close();
                    }
                });
                ui.separator();
                if ui.button("Edit Tags & Online Lookup…").clicked() {
                    ctx_action = Some(ContextMenuAction::EditTags);
                    ui.close();
                }
                if sel_count == 1 {
                    if ui.button("Track Info…").clicked() {
                        ctx_action = Some(ContextMenuAction::TrackInfo(tid));
                        ui.close();
                    }
                    if ui
                        .button(format!("Reveal in {FILE_MANAGER_NAME}"))
                        .clicked()
                    {
                        ctx_action = Some(ContextMenuAction::RevealInFinder(tid));
                        ui.close();
                    }
                }
                ui.separator();
                let remove_lbl = match current_view {
                    View::Playlist(_) => format!(
                        "Remove {} from Playlist",
                        if sel_count > 1 {
                            format!("{sel_count} Tracks")
                        } else {
                            "Track".into()
                        }
                    ),
                    View::Queue => format!(
                        "Remove {} from Queue",
                        if sel_count > 1 {
                            format!("{sel_count} Tracks")
                        } else {
                            "Track".into()
                        }
                    ),
                    _ => format!(
                        "Remove {} from Library",
                        if sel_count > 1 {
                            format!("{sel_count} Tracks")
                        } else {
                            "Track".into()
                        }
                    ),
                };
                if ui
                    .button(RichText::new(remove_lbl).color(p.danger))
                    .clicked()
                {
                    ctx_action = Some(ContextMenuAction::RemoveSelected);
                    ui.close();
                }
            });
        }

        if let Some(tid) = fav_toggle {
            self.store.active_mut().toggle_favorite(tid);
            self.mark_library_dirty();
            self.store.save();
        }
        if toggle_current_play {
            self.toggle_playback();
        } else if let Some((ridx, tid)) = play_target {
            if self.view == View::Queue {
                self.play_queue_at(ridx);
            } else {
                self.play_from_list(tid, tracks);
            }
        }
        if let Some(tid) = quick_play_next {
            self.play_next_ids(&[tid]);
        }
        if let Some(tid) = quick_enqueue {
            self.enqueue_ids(&[tid]);
        }
        if let Some(tid) = quick_tag {
            self.open_tag_editor_for_ids(&[tid]);
        }
        if let Some(act) = ctx_action {
            self.run_context_action(act, tracks);
        }
    }

    fn run_context_action(&mut self, action: ContextMenuAction, visible: &[Track]) {
        match action {
            ContextMenuAction::PlaySelected => self.play_selected(visible),
            ContextMenuAction::PlayNext => {
                let ids = self.selected_ordered_ids(visible);
                self.play_next_ids(&ids);
            }
            ContextMenuAction::AddToQueue => {
                let ids = self.selected_ordered_ids(visible);
                self.enqueue_ids(&ids);
            }
            ContextMenuAction::AddToPlaylist(pl_id) => {
                let ids = self.selected_ordered_ids(visible);
                let added = self
                    .store
                    .active_mut()
                    .add_tracks_to_playlist(pl_id, &ids);
                let pl_name = self
                    .store
                    .active()
                    .playlist(pl_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Playlist".into());
                self.mark_library_dirty();
                self.store.save();
                self.set_status(format!("Added {added} track(s) to \"{pl_name}\""));
            }
            ContextMenuAction::NewPlaylistWithSelected => {
                self.pending_playlist_tracks = self.selected_ordered_ids(visible);
                self.new_playlist_name.clear();
                self.show_new_playlist_modal = true;
            }
            ContextMenuAction::EditTags => {
                let ids = self.selected_ordered_ids(visible);
                self.open_tag_editor_for_ids(&ids);
            }
            ContextMenuAction::TrackInfo(id) => self.inspect_track_id = Some(id),
            ContextMenuAction::RevealInFinder(id) => {
                if let Some(t) = self.track(id) {
                    reveal_in_file_manager(&t.path);
                }
            }
            ContextMenuAction::RemoveSelected => self.remove_selected_in_current_view(visible),
        }
    }

    /// Sleek Floating Action Bar for 1 or N selected tracks (never expands row height!).
    fn floating_selection_dock(&mut self, context: &egui::Context) {
        let count = self.selected_count();
        if count == 0 {
            return;
        }
        let visible = self.visible_tracks();
        let selected_ids = self.selected_ordered_ids(&visible);
        if selected_ids.is_empty() {
            return;
        }
        let p = self.palette;
        let single_id = if selected_ids.len() == 1 {
            Some(selected_ids[0])
        } else {
            None
        };
        let is_single_playing =
            single_id.is_some() && self.current_id() == single_id && self.playing;
        let all_favorited = selected_ids
            .iter()
            .all(|id| self.store.active().favorites.contains(id));

        let bottom_offset = if self.show_concert_visualizer {
            -(TRANSPORT_HEIGHT + self.concert_visualizer_height + 46.0)
        } else {
            -(TRANSPORT_HEIGHT + 16.0)
        };

        let mut pending: Option<ContextMenuAction> = None;
        let mut reorder_dir: Option<isize> = None;
        let mut fav_all: Option<bool> = None;
        let mut clear_sel = false;

        let playlists_snap: Vec<(u64, String)> = self
            .store
            .active()
            .playlists
            .iter()
            .map(|pl| (pl.id, pl.name.clone()))
            .collect();

        egui::Area::new(egui::Id::unique("floating_track_action_dock"))
            .anchor(Align2::CENTER_BOTTOM, vec2(SIDEBAR_WIDTH * 0.5, bottom_offset))
            .order(egui::Order::Foreground)
            .show(context, |ui| {
                Frame::new()
                    .fill(p.surface_elevated)
                    .stroke(Stroke::new(1.2, p.selected_border))
                    .corner_radius(12.0)
                    .inner_margin(Margin::symmetric(10, 7))
                    .shadow(egui::Shadow {
                        offset: [0, 6],
                        blur: 18,
                        spread: 1,
                        color: Color32::from_rgba_unmultiplied(0, 0, 0, 120),
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.style_mut().spacing.item_spacing.x = 5.0;
                            // Selection count badge
                            Frame::new()
                                .fill(p.accent_subtle)
                                .corner_radius(6.0)
                                .inner_margin(Margin::symmetric(7, 3))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(if count == 1 {
                                            "1 selected".to_string()
                                        } else {
                                            format!("{count} selected")
                                        })
                                        .size(10.5)
                                        .strong()
                                        .color(p.accent),
                                    );
                                });

                            // Play / Pause
                            let play_lbl = if is_single_playing {
                                "Pause"
                            } else if count > 1 {
                                "Play Selected"
                            } else {
                                "Play"
                            };
                            let play_ic = if is_single_playing {
                                Icon::Pause
                            } else {
                                Icon::Play
                            };
                            if pill_button(ui, play_lbl, Some(play_ic), PillStyle::Accent, &p)
                                .clicked()
                            {
                                pending = Some(ContextMenuAction::PlaySelected);
                            }

                            if self.view != View::Queue {
                                if pill_button(
                                    ui,
                                    "Next",
                                    Some(Icon::PlayNext),
                                    PillStyle::Secondary,
                                    &p,
                                )
                                .on_hover_text("Play Next")
                                .clicked()
                                {
                                    pending = Some(ContextMenuAction::PlayNext);
                                }
                                if pill_button(
                                    ui,
                                    "Queue",
                                    Some(Icon::Plus),
                                    PillStyle::Secondary,
                                    &p,
                                )
                                .on_hover_text("Add to Play Queue")
                                .clicked()
                                {
                                    pending = Some(ContextMenuAction::AddToQueue);
                                }
                            } else if count == 1 {
                                if icons::icon_button(
                                    ui,
                                    Vec2::splat(24.0),
                                    Icon::ArrowUp,
                                    13.0,
                                    p.text_secondary,
                                    p.text,
                                    p.hover,
                                    "Move Up in Queue",
                                )
                                .clicked()
                                {
                                    reorder_dir = Some(-1);
                                }
                                if icons::icon_button(
                                    ui,
                                    Vec2::splat(24.0),
                                    Icon::ArrowDown,
                                    13.0,
                                    p.text_secondary,
                                    p.text,
                                    p.hover,
                                    "Move Down in Queue",
                                )
                                .clicked()
                                {
                                    reorder_dir = Some(1);
                                }
                            }

                            if matches!(self.view, View::Playlist(_)) && count == 1 {
                                if icons::icon_button(
                                    ui,
                                    Vec2::splat(24.0),
                                    Icon::ArrowUp,
                                    13.0,
                                    p.text_secondary,
                                    p.text,
                                    p.hover,
                                    "Move Up in Playlist",
                                )
                                .clicked()
                                {
                                    reorder_dir = Some(-1);
                                }
                                if icons::icon_button(
                                    ui,
                                    Vec2::splat(24.0),
                                    Icon::ArrowDown,
                                    13.0,
                                    p.text_secondary,
                                    p.text,
                                    p.hover,
                                    "Move Down in Playlist",
                                )
                                .clicked()
                                {
                                    reorder_dir = Some(1);
                                }
                            }

                            // Add to Playlist popup menu
                            ui.menu_button(
                                RichText::new("+ Playlist ▾")
                                    .size(11.0)
                                    .color(p.text_secondary),
                                |ui| {
                                    for (pl_id, pl_name) in &playlists_snap {
                                        if ui.button(pl_name).clicked() {
                                            pending = Some(ContextMenuAction::AddToPlaylist(*pl_id));
                                            ui.close();
                                        }
                                    }
                                    if !playlists_snap.is_empty() {
                                        ui.separator();
                                    }
                                    if ui.button("+ New Playlist…").clicked() {
                                        pending = Some(ContextMenuAction::NewPlaylistWithSelected);
                                        ui.close();
                                    }
                                },
                            );

                            // Favorite toggle
                            if pill_button(
                                ui,
                                if all_favorited { "Favorited" } else { "Favorite" },
                                Some(if all_favorited {
                                    Icon::HeartFilled
                                } else {
                                    Icon::HeartOutline
                                }),
                                if all_favorited {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                &p,
                            )
                            .clicked()
                            {
                                fav_all = Some(!all_favorited);
                            }

                            // Tag Editor
                            if pill_button(ui, "Tags", Some(Icon::Tag), PillStyle::Secondary, &p)
                                .on_hover_text("Edit metadata & lookup online tags")
                                .clicked()
                            {
                                pending = Some(ContextMenuAction::EditTags);
                            }

                            if let Some(sid) = single_id {
                                if icons::icon_button(
                                    ui,
                                    Vec2::splat(24.0),
                                    Icon::Info,
                                    13.5,
                                    p.text_secondary,
                                    p.text,
                                    p.hover,
                                    "Track Inspector",
                                )
                                .clicked()
                                {
                                    pending = Some(ContextMenuAction::TrackInfo(sid));
                                }
                                if icons::icon_button(
                                    ui,
                                    Vec2::splat(24.0),
                                    Icon::External,
                                    13.0,
                                    p.text_secondary,
                                    p.text,
                                    p.hover,
                                    "Reveal in Finder",
                                )
                                .clicked()
                                {
                                    pending = Some(ContextMenuAction::RevealInFinder(sid));
                                }
                            }

                            // Context-aware Remove button
                            let remove_label = match self.view {
                                View::Playlist(_) => "Remove from Playlist",
                                View::Queue => "Remove from Queue",
                                _ => "Remove",
                            };
                            if pill_button(
                                ui,
                                remove_label,
                                Some(Icon::Trash),
                                PillStyle::Danger,
                                &p,
                            )
                            .clicked()
                            {
                                pending = Some(ContextMenuAction::RemoveSelected);
                            }

                            if icons::icon_button(
                                ui,
                                Vec2::splat(22.0),
                                Icon::Close,
                                10.5,
                                p.muted,
                                p.text,
                                p.hover,
                                "Clear selection (Esc)",
                            )
                            .clicked()
                            {
                                clear_sel = true;
                            }
                        });
                    });
            });

        if let Some(fav) = fav_all {
            self.store.active_mut().set_favorites(&selected_ids, fav);
            self.mark_library_dirty();
            self.store.save();
        }
        if let Some(dir) = reorder_dir {
            match self.view {
                View::Queue => {
                    if let Some(&qidx) = self.selected_queue_indices.iter().next() {
                        let target = (qidx as isize + dir)
                            .clamp(0, self.current_queue.len().saturating_sub(1) as isize)
                            as usize;
                        self.move_queue_item(qidx, target);
                    }
                }
                View::Playlist(pl_id) => {
                    if let Some(sid) = single_id
                        && let Some(pos) = self
                            .store
                            .active()
                            .playlist(pl_id)
                            .and_then(|pl| pl.track_ids.iter().position(|&id| id == sid))
                    {
                        let len = self
                            .store
                            .active()
                            .playlist(pl_id)
                            .map(|pl| pl.track_ids.len())
                            .unwrap_or(0);
                        let target = (pos as isize + dir).clamp(0, len.saturating_sub(1) as isize)
                            as usize;
                        self.store
                            .active_mut()
                            .move_playlist_track(pl_id, pos, target);
                        self.mark_library_dirty();
                        self.store.save();
                    }
                }
                _ => {}
            }
        }
        if let Some(act) = pending {
            self.run_context_action(act, &visible);
        }
        if clear_sel {
            self.clear_selection();
        }
    }

    fn empty_state(&mut self, ui: &mut Ui) {
        let p = self.palette;
        let height = (ui.available_height() - 40.0).max(180.0);
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), height),
            Layout::top_down(Align::Center),
            |ui| {
                ui.add_space(36.0);
                let (mark_rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
                self.paint_brand_icon(ui.painter(), mark_rect);
                ui.add_space(12.0);
                let (title, subtitle) = match self.view {
                    View::Library if !self.search.trim().is_empty() => (
                        "No matching tracks",
                        "Try another title, composer, artist, album, or format.",
                    ),
                    View::Library => (
                        "High-resolution concert playback, ready when you are.",
                        "Add audio files, link a music directory, or drag and drop anywhere.",
                    ),
                    View::Favorites => (
                        "No favorite tracks yet",
                        "Click the heart on any track to collect your favorites here.",
                    ),
                    View::Queue => (
                        "Your play queue is empty",
                        "Play any track or click \"Queue\" to build an Up Next sequence.",
                    ),
                    View::Playlist(_) => (
                        "This playlist is empty",
                        "Select tracks in your library and click \"+ Playlist\" to add them here.",
                    ),
                };
                ui.label(RichText::new(title).size(17.5).strong().color(p.text));
                ui.add_space(4.0);
                ui.label(RichText::new(subtitle).size(12.0).color(p.muted));
                ui.add_space(14.0);
                if self.view == View::Library && self.search.trim().is_empty() {
                    ui.horizontal(|ui| {
                        let group_w = 240.0;
                        let pad = ((ui.available_width() - group_w) * 0.5).max(0.0);
                        ui.add_space(pad);
                        if sidebar_compact_action(ui, 114.0, "Add Tracks", Icon::TrackPlus, true, &p)
                            .clicked()
                        {
                            self.add_files();
                        }
                        ui.add_space(6.0);
                        if sidebar_compact_action(
                            ui,
                            114.0,
                            "Add Folder",
                            Icon::FolderPlus,
                            false,
                            &p,
                        )
                        .clicked()
                        {
                            self.add_folder();
                        }
                    });
                }
            },
        );
    }

    fn playback_panel(&mut self, context: &egui::Context, ui: &mut Ui) {
        let p = self.palette;
        let track = self.current_track().cloned();
        let total_width = ui.available_width();
        let left_w = (total_width * 0.28).max(205.0);
        let right_w = (total_width * 0.28).max(215.0);
        let center_w = (total_width - left_w - right_w - 16.0).max(280.0);

        ui.horizontal_centered(|ui| {
            // LEFT: Framed Album Art + Metadata + Quick Favorite
            ui.allocate_ui_with_layout(
                vec2(left_w, 84.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    if let Some(track) = &track {
                        let cover_resp = self.cover_clickable(ui, track, 58.0);
                        if cover_resp
                            .on_hover_text("Click to open Track Inspector")
                            .clicked()
                        {
                            self.inspect_track_id = Some(track.id);
                        }
                        ui.add_space(6.0);
                        let meta_w = (left_w - 98.0).max(86.0);
                        ui.allocate_ui_with_layout(
                            vec2(meta_w, 58.0),
                            Layout::top_down(Align::LEFT),
                            |ui| {
                                ui.add_space(4.0);
                                ui.style_mut().spacing.item_spacing.y = 2.0;
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&track.title)
                                            .size(13.0)
                                            .strong()
                                            .color(p.text),
                                    )
                                    .selectable(false)
                                    .truncate(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&track.artist)
                                            .size(11.0)
                                            .color(p.text_secondary),
                                    )
                                    .selectable(false)
                                    .truncate(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&track.album).size(10.0).color(p.faint),
                                    )
                                    .selectable(false)
                                    .truncate(),
                                );
                            },
                        );

                        let fav = self.store.active().favorites.contains(&track.id);
                        if icons::icon_button(
                            ui,
                            Vec2::splat(26.0),
                            if fav {
                                Icon::HeartFilled
                            } else {
                                Icon::HeartOutline
                            },
                            15.0,
                            if fav { p.accent } else { p.faint },
                            p.accent_bright,
                            p.hover,
                            if fav {
                                "Remove from Favorites"
                            } else {
                                "Add to Favorites"
                            },
                        )
                        .clicked()
                        {
                            self.store.active_mut().toggle_favorite(track.id);
                            self.store.save();
                        }
                    } else {
                        let (art_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
                        self.paint_brand_icon(ui.painter(), art_rect);
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new("No Track Loaded")
                                    .size(12.5)
                                    .strong()
                                    .color(p.text_secondary),
                            );
                            ui.label(
                                RichText::new("Select a piece to listen")
                                    .size(10.5)
                                    .color(p.faint),
                            );
                        });
                    }
                },
            );

            // CENTER: Precision Transport Controls + Acoustic Waveform Timeline Scrubber
            ui.allocate_ui_with_layout(
                vec2(center_w, 84.0),
                Layout::top_down(Align::Center),
                |ui| {
                    ui.add_space(2.0);
                    let controls_w = 196.0;
                    ui.horizontal(|ui| {
                        let pad = ((center_w - controls_w) * 0.5).max(0.0);
                        ui.add_space(pad);
                        ui.style_mut().spacing.item_spacing.x = 8.0;

                        if transport_toggle_button(ui, Icon::Shuffle, self.shuffle, "Shuffle", &p)
                            .clicked()
                        {
                            self.shuffle = !self.shuffle;
                        }
                        if icons::icon_button(
                            ui,
                            Vec2::splat(30.0),
                            Icon::Prev,
                            15.5,
                            p.text_secondary,
                            p.text,
                            p.hover,
                            "Previous track",
                        )
                        .clicked()
                        {
                            self.previous();
                        }
                        if play_pause_hero_button(ui, self.playing, &p).clicked() {
                            self.toggle_playback();
                        }
                        if icons::icon_button(
                            ui,
                            Vec2::splat(30.0),
                            Icon::Next,
                            15.5,
                            p.text_secondary,
                            p.text,
                            p.hover,
                            "Next track",
                        )
                        .clicked()
                        {
                            self.next(false);
                        }
                        let repeat_icon = match self.repeat {
                            RepeatMode::One => Icon::RepeatOne,
                            RepeatMode::Off | RepeatMode::All => Icon::Repeat,
                        };
                        let repeat_tip = match self.repeat {
                            RepeatMode::Off => "Repeat: Off",
                            RepeatMode::All => "Repeat: All",
                            RepeatMode::One => "Repeat: One",
                        };
                        if transport_toggle_button(
                            ui,
                            repeat_icon,
                            self.repeat != RepeatMode::Off,
                            repeat_tip,
                            &p,
                        )
                        .clicked()
                        {
                            self.repeat = match self.repeat {
                                RepeatMode::Off => RepeatMode::All,
                                RepeatMode::All => RepeatMode::One,
                                RepeatMode::One => RepeatMode::Off,
                            };
                        }
                    });

                    ui.add_space(4.0);
                    // Acoustic Waveform / Studio Precision Scrubber
                    ui.horizontal(|ui| {
                        let time_w = 42.0;
                        ui.allocate_ui_with_layout(
                            vec2(time_w, 28.0),
                            Layout::right_to_left(Align::Center),
                            |ui| {
                                ui.label(
                                    RichText::new(format_duration(Some(self.position)))
                                        .size(10.5)
                                        .monospace()
                                        .color(p.muted),
                                );
                            },
                        );

                        let bar_w = (center_w - time_w * 2.0 - 16.0).max(90.0);
                        let duration = self.duration.max(0.0);
                        let active = self.current_id().is_some() && duration > 0.01;
                        let waveform = self.current_id().and_then(|id| self.waveforms.get(&id));
                        if let Some(new_pos) = acoustic_timeline_scrubber(
                            ui,
                            bar_w,
                            self.position,
                            duration,
                            active,
                            waveform.map(Vec::as_slice),
                            self.progress_style,
                            &p,
                        ) {
                            self.audio.seek(new_pos);
                            self.position = new_pos;
                        }

                        ui.allocate_ui_with_layout(
                            vec2(time_w, 28.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                let right_text = if self.show_remaining && duration > 0.0 {
                                    format!(
                                        "-{}",
                                        format_duration(Some((duration - self.position).max(0.0)))
                                    )
                                } else {
                                    format_duration(if duration > 0.0 {
                                        Some(duration)
                                    } else {
                                        None
                                    })
                                };
                                let resp = ui.add(
                                    egui::Label::new(
                                        RichText::new(right_text)
                                            .size(10.5)
                                            .monospace()
                                            .color(p.muted),
                                    )
                                    .sense(Sense::click()),
                                );
                                if resp
                                    .on_hover_text("Click to toggle remaining / total time")
                                    .clicked()
                                {
                                    self.show_remaining = !self.show_remaining;
                                }
                            },
                        );
                    });
                },
            );

            // RIGHT: Compact Live Visualizer + Clickable Signal Path Telemetry + Device & Volume
            ui.allocate_ui_with_layout(
                vec2(right_w, 84.0),
                Layout::top_down(Align::RIGHT),
                |ui| {
                    ui.style_mut().spacing.item_spacing.y = 4.0;
                    let vis_w = (right_w - 8.0).clamp(144.0, 214.0);
                    self.visualizer
                        .ui_compact(ui, vec2(vis_w, 28.0), &p, &self.plugins.visualizers);

                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if let Some(info) = &self.stream_info {
                                let native = info.source_rate == info.output_rate;
                                let dsp_on = self.plugins.dsp.is_actively_processing();
                                let dot_color = if dsp_on {
                                    p.accent_secondary
                                } else if native {
                                    p.accent
                                } else {
                                    p.hires_gold
                                };
                                let bitrate_str = self
                                    .bitrate
                                    .map(|kbps| {
                                        let kind = if self.variable_bitrate { "VBR" } else { "CBR" };
                                        format!("{kind} {kbps} kbps · ")
                                    })
                                    .unwrap_or_default();
                                let bits = info
                                    .bit_depth
                                    .map(|b| format!("{b}b/"))
                                    .unwrap_or_default();
                                let rate_str = if native {
                                    format!("{bits}{}", format_sample_rate(info.source_rate))
                                } else {
                                    format!(
                                        "{bits}{} → {}",
                                        format_sample_rate(info.source_rate),
                                        format_sample_rate(info.output_rate)
                                    )
                                };
                                let dsp_tag = if dsp_on { " · DSP" } else { "" };
                                let sig_resp = ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(format!("{bitrate_str}{rate_str}{dsp_tag}"))
                                                .size(9.5)
                                                .monospace()
                                                .color(if self.show_signal_path {
                                                    p.accent
                                                } else {
                                                    p.muted
                                                }),
                                        )
                                        .sense(Sense::click()),
                                    )
                                    .on_hover_text(format!(
                                        "{} · {} ch · {} → {}\nClick to inspect Signal Path & Live DSP",
                                        info.device_name,
                                        info.channels,
                                        format_sample_rate(info.source_rate),
                                        format_sample_rate(info.output_rate)
                                    ));
                                self.signal_btn_rect = Some(sig_resp.rect);
                                if sig_resp.clicked() {
                                    self.show_signal_path = !self.show_signal_path;
                                    self.show_device_picker = false;
                                }
                                let (dot_rect, dot_resp) =
                                    ui.allocate_exact_size(Vec2::splat(9.0), Sense::click());
                                ui.painter().circle_filled(dot_rect.center(), 3.2, dot_color);
                                if dot_resp
                                    .on_hover_text("Click to open Signal Path & Live DSP")
                                    .clicked()
                                {
                                    self.show_signal_path = !self.show_signal_path;
                                    self.show_device_picker = false;
                                }
                            } else if let Some(track) = &track {
                                let resp = ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(track.quality_full())
                                                .size(9.5)
                                                .monospace()
                                                .color(p.faint),
                                        )
                                        .sense(Sense::click()),
                                    )
                                    .on_hover_text("Click to open Signal Path & Live DSP");
                                self.signal_btn_rect = Some(resp.rect);
                                if resp.clicked() {
                                    self.show_signal_path = !self.show_signal_path;
                                    self.show_device_picker = false;
                                }
                            } else {
                                let resp = ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(if self.plugins.dsp.enabled {
                                                "SIGNAL PATH · DSP ON"
                                            } else {
                                                "SIGNAL PATH · IDLE"
                                            })
                                            .size(9.5)
                                            .monospace()
                                            .color(if self.plugins.dsp.enabled {
                                                p.accent_secondary
                                            } else {
                                                p.faint
                                            }),
                                        )
                                        .sense(Sense::click()),
                                    )
                                    .on_hover_text("Click to open Signal Path & Live DSP");
                                self.signal_btn_rect = Some(resp.rect);
                                if resp.clicked() {
                                    self.show_signal_path = !self.show_signal_path;
                                    self.show_device_picker = false;
                                }
                            }
                        });
                    });

                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if let Some(new_vol) = custom_volume_bar(ui, 82.0, self.volume, &p) {
                                self.set_volume(new_vol);
                            }
                            let vol_icon = if self.volume <= 0.001 {
                                Icon::VolumeMuted
                            } else if self.volume < 0.5 {
                                Icon::VolumeLow
                            } else {
                                Icon::VolumeHigh
                            };
                            if icons::icon_button(
                                ui,
                                Vec2::splat(22.0),
                                vol_icon,
                                14.0,
                                if self.volume <= 0.001 {
                                    p.danger
                                } else {
                                    p.text_secondary
                                },
                                p.text,
                                p.hover,
                                "Mute / Unmute (M)",
                            )
                            .clicked()
                            {
                                self.toggle_mute();
                            }

                            ui.add_space(4.0);
                            let active_dev = self
                                .stream_info
                                .as_ref()
                                .map(|s| s.device_name.as_str())
                                .or(self.preferred_device.as_deref())
                                .unwrap_or("Output");
                            let short_dev: String = active_dev.chars().take(14).collect();
                            let dev_resp = pill_button(
                                ui,
                                &short_dev,
                                Some(Icon::Device),
                                if self.show_device_picker {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                &p,
                            )
                            .on_hover_text(format!("Audio Output: {active_dev}\nClick to switch"));
                            self.device_btn_rect = Some(dev_resp.rect);
                            if dev_resp.clicked() {
                                self.available_devices = list_output_devices();
                                self.show_device_picker = !self.show_device_picker;
                                if self.show_device_picker {
                                    self.show_signal_path = false;
                                }
                            }
                        });
                    });
                },
            );
        });

        if self.playing {
            context.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn show_modals(&mut self, context: &egui::Context) {
        let p = self.palette;

        // 1) Settings Window
        let settings_actions = modals::show_settings_window(
            context,
            &mut self.show_settings,
            &mut self.settings_tab,
            &p,
            self.theme_id,
            &mut self.progress_style,
            &mut self.row_density,
            &mut self.visualizer.mode,
            &mut self.show_concert_visualizer,
            &mut self.concert_visualizer_height,
            &mut self.visualizer.show_piano_keys,
            &mut self.visualizer.sensitivity,
            self.visualizer.last_compute_ms,
            &self.preferred_device,
            &mut self.available_devices,
            &mut self.linear_volume,
            &mut self.acoustid_api_key,
            &mut self.plugins,
            &self.tool_health,
        );
        if let Some(tid) = settings_actions.theme_changed {
            self.set_theme(context, tid);
        }
        if let Some(dev) = settings_actions.device_changed {
            self.preferred_device = dev.clone();
            self.audio.select_device(dev.clone());
            self.set_status(match dev {
                Some(name) => format!("Switched output to {name}"),
                None => "Switched output to System Default".into(),
            });
        }
        if let Some(lin) = settings_actions.linear_vol_changed {
            self.audio.set_linear_volume(lin);
        }
        if settings_actions.dsp_changed {
            self.audio.update_dsp(&self.plugins.dsp);
        }
        if settings_actions.open_folders {
            self.show_folders = true;
        }
        if settings_actions.open_libraries {
            self.show_libraries_modal = true;
        }
        if settings_actions.prune_missing {
            let removed = self.store.active_mut().prune_missing();
            self.purge_ids_from_queue(&removed);
            self.mark_library_dirty();
            self.store.save();
            self.set_status(if removed.is_empty() {
                "All library files are present on disk".into()
            } else {
                format!("Pruned {} missing track(s)", removed.len())
            });
        }

        // 2) Tag Editor & Online Matcher Window
        if let Some(save_ids) = modals::show_tag_editor_window(
            context,
            &mut self.tag_editor,
            &self.acoustid_api_key,
            &self.plugins.tag_providers,
            &p,
        ) {
            let tracks: Vec<Track> = save_ids
                .iter()
                .filter_map(|id| self.track(*id).cloned())
                .collect();
            self.tag_editor.trigger_save(tracks);
        }

        // 3) Track Inspector Window
        if let Some(tid) = self.inspect_track_id {
            if let Some(track) = self.track(tid).cloned() {
                let (open_stay, action) = modals::show_track_info_window(
                    context,
                    &track,
                    &p,
                    |ui, size| self.cover(ui, &track, size),
                    |ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(140.0, 20.0), Sense::hover());
                        paint_aligned_quality_badge(ui.painter(), r, &track, &p);
                    },
                );
                if !open_stay {
                    self.inspect_track_id = None;
                }
                if let Some(act) = action {
                    match act {
                        TrackInfoAction::Play => {
                            let visible = self.visible_tracks();
                            if visible.iter().any(|t| t.id == tid) {
                                self.play_from_list(tid, &visible);
                            } else {
                                self.current_queue = vec![tid];
                                self.current_index = Some(0);
                                self.start_current();
                            }
                        }
                        TrackInfoAction::EditTags => {
                            self.inspect_track_id = None;
                            self.open_tag_editor_for_ids(&[tid]);
                        }
                        TrackInfoAction::RevealInFinder => {
                            reveal_in_file_manager(&track.path);
                        }
                        TrackInfoAction::Remove => {
                            self.remove_tracks_from_library(&[tid]);
                        }
                    }
                }
            } else {
                self.inspect_track_id = None;
            }
        }

        // 4) Manage Music Directories Window
        let folders_with_counts: Vec<(PathBuf, usize)> = self
            .store
            .active()
            .folders
            .iter()
            .map(|f| (f.clone(), self.store.active().folder_track_count(f)))
            .collect();
        if let Some(act) =
            modals::show_folders_window(context, &mut self.show_folders, &folders_with_counts, &p)
        {
            match act {
                FolderModalAction::AddFolder => self.add_folder(),
                FolderModalAction::AddFiles => self.add_files(),
                FolderModalAction::RescanAll => self.rescan(),
                FolderModalAction::RescanOne(f) => self.start_scan(vec![f]),
                FolderModalAction::RemoveOne(f) => self.remove_folder_from_library(&f),
                FolderModalAction::PruneMissing => {
                    let removed = self.store.active_mut().prune_missing();
                    self.purge_ids_from_queue(&removed);
                    self.mark_library_dirty();
                    self.store.save();
                    self.set_status(if removed.is_empty() {
                        "All library files are present on disk".into()
                    } else {
                        format!("Pruned {} missing track(s)", removed.len())
                    });
                }
            }
        }

        // 5) Manage Libraries Window
        if self.show_libraries_modal {
            let mut open = self.show_libraries_modal;
            let mut switch_to: Option<u64> = None;
            let mut delete_id: Option<u64> = None;
            let mut create_clicked = false;
            let libs: Vec<(u64, String, usize, usize)> = self
                .store
                .libraries
                .iter()
                .map(|l| (l.id, l.name.clone(), l.tracks.len(), l.playlists.len()))
                .collect();

            egui::Window::new("Manage Libraries")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(440.0)
                .show(context, |ui| {
                    ui.label(
                        RichText::new(
                            "Separate your music collections (e.g. Classical & Piano, Jazz Hi-Res, Studio Masters).",
                        )
                        .size(11.0)
                        .color(p.muted),
                    );
                    ui.add_space(8.0);
                    for (id, name, track_n, pl_n) in &libs {
                        let is_active = *id == self.store.active_id;
                        Frame::new()
                            .fill(if is_active { p.selected } else { p.surface })
                            .stroke(Stroke::new(
                                1.0,
                                if is_active { p.accent } else { p.border },
                            ))
                            .corner_radius(6.0)
                            .inner_margin(Margin::symmetric(10, 6))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new(name)
                                                .size(12.5)
                                                .strong()
                                                .color(p.text),
                                        );
                                        ui.label(
                                            RichText::new(format!(
                                                "{track_n} tracks · {pl_n} playlists"
                                            ))
                                            .size(10.0)
                                            .color(p.muted),
                                        );
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if libs.len() > 1
                                            && pill_button(
                                                ui,
                                                "Delete",
                                                Some(Icon::Trash),
                                                PillStyle::Danger,
                                                &p,
                                            )
                                            .clicked()
                                        {
                                            delete_id = Some(*id);
                                        }
                                        if !is_active
                                            && pill_button(
                                                ui,
                                                "Switch",
                                                None,
                                                PillStyle::Active,
                                                &p,
                                            )
                                            .clicked()
                                        {
                                            switch_to = Some(*id);
                                        } else if is_active {
                                            ui.label(
                                                RichText::new("Active")
                                                    .size(10.5)
                                                    .strong()
                                                    .color(p.accent),
                                            );
                                        }
                                    });
                                });
                            });
                        ui.add_space(4.0);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let resp = ui.add_sized(
                            [280.0, 24.0],
                            egui::TextEdit::singleline(&mut self.new_library_name)
                                .hint_text("New library name (e.g. Classical Piano)"),
                        );
                        if pill_button(ui, "Create Library", Some(Icon::Plus), PillStyle::Accent, &p)
                            .clicked()
                            || (resp.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            create_clicked = true;
                        }
                    });
                });
            self.show_libraries_modal = open;
            if let Some(id) = switch_to {
                self.store.active_id = id;
                self.view = View::Library;
                self.folder_filter = None;
                self.clear_selection();
                self.store.save();
            }
            if let Some(id) = delete_id
                && let Some(name) = self.store.delete_library(id)
            {
                self.view = View::Library;
                self.folder_filter = None;
                self.clear_selection();
                self.store.save();
                self.set_status(format!("Deleted library \"{name}\""));
            }
            if create_clicked {
                let name = std::mem::take(&mut self.new_library_name);
                let id = self.store.create_library(name);
                self.view = View::Library;
                self.folder_filter = None;
                self.clear_selection();
                self.store.save();
                let created_name = self.store.active().name.clone();
                let _ = id;
                self.set_status(format!("Created & switched to \"{created_name}\""));
            }
        }

        // 6) New Playlist Dialog
        if self.show_new_playlist_modal {
            let mut open = self.show_new_playlist_modal;
            let mut create = false;
            egui::Window::new("New Playlist")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(340.0)
                .show(context, |ui| {
                    let resp = ui.add_sized(
                        [300.0, 26.0],
                        egui::TextEdit::singleline(&mut self.new_playlist_name)
                            .hint_text("Playlist name (e.g. Chopin Nocturnes)"),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if !self.pending_playlist_tracks.is_empty() {
                            ui.label(
                                RichText::new(format!(
                                    "Will include {} selected track(s)",
                                    self.pending_playlist_tracks.len()
                                ))
                                .size(10.5)
                                .color(p.muted),
                            );
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if pill_button(ui, "Create", Some(Icon::Check), PillStyle::Accent, &p)
                                .clicked()
                                || (resp.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                create = true;
                            }
                        });
                    });
                });
            self.show_new_playlist_modal = open;
            if create {
                let name = std::mem::take(&mut self.new_playlist_name);
                let tracks_to_add = std::mem::take(&mut self.pending_playlist_tracks);
                let pl_id = self.store.active_mut().create_playlist(name);
                if !tracks_to_add.is_empty() {
                    self.store
                        .active_mut()
                        .add_tracks_to_playlist(pl_id, &tracks_to_add);
                }
                self.store.save();
                self.mark_library_dirty();
                self.view = View::Playlist(pl_id);
                self.show_new_playlist_modal = false;
                let pl_name = self
                    .store
                    .active()
                    .playlist(pl_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Playlist".into());
                self.set_status(format!("Created playlist \"{pl_name}\""));
            }
        }

        // 7) Quick Output Device Picker Popover (anchored directly above the Output button)
        if let Some(chosen_device) = modals::show_device_picker_popover(
            context,
            &mut self.show_device_picker,
            self.device_btn_rect,
            TRANSPORT_HEIGHT,
            &self.preferred_device,
            &mut self.available_devices,
            &p,
        ) {
            self.preferred_device = chosen_device.clone();
            self.audio.select_device(chosen_device.clone());
            self.set_status(match chosen_device {
                Some(dev) => format!("Switched output to {dev}"),
                None => "Switched output to System Default".into(),
            });
        }

        // 8) Signal Path & Live DSP Popover (anchored directly above telemetry)
        if self.show_signal_path {
            let cur_codec = self
                .current_track()
                .map(|t| t.codec.clone())
                .unwrap_or_else(|| "PCM".into());
            let sig = self.stream_info.as_ref().map(|info| SignalPathDisplay {
                codec: cur_codec.as_str(),
                source_rate: info.source_rate,
                output_rate: info.output_rate,
                channels: info.channels,
                bit_depth: info.bit_depth,
                bitrate_kbps: self.bitrate,
                variable_bitrate: self.variable_bitrate,
                device_name: info.device_name.as_str(),
                vis_compute_ms: self.visualizer.last_compute_ms,
            });
            let (dsp_changed, open_dsp_settings) = modals::show_signal_path_popover(
                context,
                &mut self.show_signal_path,
                self.signal_btn_rect,
                TRANSPORT_HEIGHT,
                sig,
                &mut self.plugins,
                &p,
            );
            if dsp_changed {
                self.audio.update_dsp(&self.plugins.dsp);
            }
            if open_dsp_settings {
                self.show_signal_path = false;
                self.settings_tab = SettingsTab::Audio;
                self.show_settings = true;
            }
        }

        // 9) Cmd+K Quick Command Palette
        if self.command_palette.open {
            let entries = self.build_palette_entries();
            if let Some(cmd) =
                modals::show_command_palette(context, &mut self.command_palette, &entries, &p)
            {
                self.execute_palette_command(context, cmd);
            }
        }
    }

    fn build_palette_entries(&self) -> Vec<PaletteEntry> {
        let mut entries = Vec::with_capacity(64);
        entries.push(PaletteEntry {
            category: "ACTION",
            label: "Toggle Concert Visualizer".into(),
            subtitle: "Shortcut: V".into(),
            action: PaletteCommand::ToggleVisualizer,
        });
        entries.push(PaletteEntry {
            category: "ACTION",
            label: if self.plugins.dsp.enabled {
                "Bypass Audio DSP (Bit-Direct)".into()
            } else {
                "Enable Real-Time Audio DSP".into()
            },
            subtitle: "Shortcut: E".into(),
            action: PaletteCommand::ToggleDsp,
        });
        entries.push(PaletteEntry {
            category: "ACTION",
            label: "Edit Current Track Tags & Lookup".into(),
            subtitle: "Shortcut: Cmd+T".into(),
            action: PaletteCommand::EditCurrentTags,
        });
        entries.push(PaletteEntry {
            category: "ACTION",
            label: "Open Plugins & Modules Settings".into(),
            subtitle: "Piano engine, visualizers, DSP, tag providers".into(),
            action: PaletteCommand::OpenSettings(SettingsTab::Plugins),
        });
        entries.push(PaletteEntry {
            category: "ACTION",
            label: "Open Audio & EQ Settings".into(),
            subtitle: "10-band EQ, crossfeed, output device".into(),
            action: PaletteCommand::OpenSettings(SettingsTab::Audio),
        });
        entries.push(PaletteEntry {
            category: "ACTION",
            label: "Add Music Directory…".into(),
            subtitle: "Scan a folder into active library".into(),
            action: PaletteCommand::AddFolder,
        });
        for &m in VisualizerMode::ALL {
            entries.push(PaletteEntry {
                category: "VISUAL",
                label: m.name().into(),
                subtitle: "Switch Concert Visualizer mode".into(),
                action: PaletteCommand::SetVisualizer(m),
            });
        }
        for &preset in EqPreset::SELECTABLE {
            entries.push(PaletteEntry {
                category: "DSP EQ",
                label: preset.name().into(),
                subtitle: "Apply 10-band EQ curve".into(),
                action: PaletteCommand::ApplyEqPreset(preset),
            });
        }
        for &tid in ThemeId::ALL {
            entries.push(PaletteEntry {
                category: "THEME",
                label: tid.name().into(),
                subtitle: tid.subtitle().into(),
                action: PaletteCommand::SetTheme(tid),
            });
        }
        for pl in &self.store.active().playlists {
            entries.push(PaletteEntry {
                category: "PLAYLIST",
                label: pl.name.clone(),
                subtitle: format!("{} tracks", pl.track_ids.len()),
                action: PaletteCommand::OpenPlaylist(pl.id),
            });
        }
        for lib in &self.store.libraries {
            entries.push(PaletteEntry {
                category: "LIBRARY",
                label: lib.name.clone(),
                subtitle: format!("{} tracks", lib.tracks.len()),
                action: PaletteCommand::SwitchLibrary(lib.id),
            });
        }
        for track in &self.store.active().tracks {
            entries.push(PaletteEntry {
                category: "TRACK",
                label: track.title.clone(),
                subtitle: format!("{} · {}", track.artist, track.album),
                action: PaletteCommand::PlayTrack(track.id),
            });
        }
        entries
    }

    fn execute_palette_command(&mut self, ctx: &egui::Context, cmd: PaletteCommand) {
        match cmd {
            PaletteCommand::PlayTrack(id) => {
                let all = self.store.active().tracks.clone();
                self.play_from_list(id, &all);
            }
            PaletteCommand::OpenPlaylist(pl_id) => {
                self.view = View::Playlist(pl_id);
                self.folder_filter = None;
                self.clear_selection();
            }
            PaletteCommand::SwitchLibrary(lib_id) => {
                self.store.active_id = lib_id;
                self.view = View::Library;
                self.folder_filter = None;
                self.clear_selection();
                self.mark_library_dirty();
                self.store.save();
            }
            PaletteCommand::SetTheme(tid) => self.set_theme(ctx, tid),
            PaletteCommand::SetVisualizer(mode) => {
                self.visualizer.mode = mode;
                self.show_concert_visualizer = true;
            }
            PaletteCommand::ApplyEqPreset(preset) => {
                self.plugins.dsp.apply_preset(preset);
                self.plugins.dsp.enabled = preset != EqPreset::Flat || self.plugins.dsp.crossfeed_enabled;
                self.plugins.dsp.eq_enabled = true;
                self.audio.update_dsp(&self.plugins.dsp);
                self.set_status(format!("EQ Preset: {}", preset.name()));
            }
            PaletteCommand::ToggleDsp => {
                self.plugins.dsp.enabled = !self.plugins.dsp.enabled;
                self.audio.update_dsp(&self.plugins.dsp);
                self.set_status(format!("Audio DSP: {}", self.plugins.dsp.summary()));
            }
            PaletteCommand::ToggleVisualizer => {
                self.show_concert_visualizer = !self.show_concert_visualizer;
            }
            PaletteCommand::EditCurrentTags => {
                if let Some(cid) = self.current_id() {
                    self.open_tag_editor_for_ids(&[cid]);
                } else {
                    self.set_status("Select or play a track to edit tags".into());
                }
            }
            PaletteCommand::OpenSettings(tab) => {
                self.settings_tab = tab;
                self.show_settings = true;
            }
            PaletteCommand::AddFolder => self.add_folder(),
        }
    }

    fn drag_overlay(&self, context: &egui::Context) {
        if context.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let p = self.palette;
        let screen = context.content_rect();
        let painter = context.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("drop_overlay"),
        ));
        painter.rect_filled(
            screen,
            0.0,
            Color32::from_rgba_unmultiplied(p.background.r(), p.background.g(), p.background.b(), 205),
        );
        let card = Rect::from_center_size(screen.center(), vec2(360.0, 140.0));
        painter.rect_filled(card, 14.0, p.panel);
        painter.rect_stroke(card, 14.0, Stroke::new(2.0, p.accent), StrokeKind::Inside);
        let icon_rect =
            Rect::from_center_size(pos2(card.center().x, card.top() + 44.0), Vec2::splat(28.0));
        icons::paint_icon(&painter, icon_rect, Icon::FolderPlus, p.accent);
        painter.text(
            pos2(card.center().x, card.center().y + 12.0),
            Align2::CENTER_CENTER,
            "Drop Tracks or Directories",
            FontId::proportional(17.0),
            p.text,
        );
        painter.text(
            pos2(card.center().x, card.center().y + 36.0),
            Align2::CENTER_CENTER,
            "Files will be added immediately to the active library",
            FontId::proportional(11.5),
            p.muted,
        );
    }

    fn status_toast(&mut self, context: &egui::Context) {
        let Some((message, created)) = &self.status else {
            return;
        };
        if created.elapsed() > Duration::from_secs(4) {
            self.status = None;
            return;
        }
        let p = self.palette;
        egui::Area::new(egui::Id::unique("status_toast"))
            .anchor(Align2::RIGHT_TOP, vec2(-20.0, 16.0))
            .order(egui::Order::Foreground)
            .show(context, |ui| {
                Frame::new()
                    .fill(p.surface_elevated)
                    .stroke(Stroke::new(1.0, p.selected_border))
                    .corner_radius(18.0)
                    .inner_margin(Margin::symmetric(14, 6))
                    .show(ui, |ui| {
                        ui.label(RichText::new(message.as_str()).size(11.0).color(p.text));
                    });
            });
    }

    fn paint_brand_icon(&self, painter: &egui::Painter, rect: Rect) {
        if let Some(tex) = &self.brand_icon {
            painter.image(
                tex.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            icons::paint_brand_mark(painter, rect, &self.palette);
        }
    }

    fn cover(&mut self, ui: &mut Ui, track: &Track, size: f32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        self.paint_cover_in_rect(ui, track, rect);
    }

    fn cover_clickable(&mut self, ui: &mut Ui, track: &Track, size: f32) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
        self.paint_cover_in_rect(ui, track, rect);
        if resp.hovered() {
            ui.painter().rect_stroke(
                rect,
                (size * 0.14).clamp(4.0, 10.0),
                Stroke::new(1.5, self.palette.accent),
                StrokeKind::Inside,
            );
        }
        resp
    }

    fn paint_cover_in_rect(&mut self, ui: &mut Ui, track: &Track, rect: Rect) {
        let rounding = (rect.width() * 0.14).clamp(4.0, 10.0);
        if let Some(cover_path) = &track.cover_path {
            if let Some(entry) = self.artwork_by_path.get(cover_path) {
                if let Some(texture) = entry {
                    ui.painter().rect_filled(rect, rounding, self.palette.surface);
                    ui.painter().image(
                        texture.id(),
                        rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    ui.painter().rect_stroke(
                        rect,
                        rounding,
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 22)),
                        StrokeKind::Inside,
                    );
                    return;
                }
            } else if self.artwork_inflight.insert(cover_path.clone()) {
                let _ = self.artwork_req_tx.send(cover_path.clone());
            }
        }
        icons::paint_fallback_cover(ui.painter(), rect, track.id, &track.title);
    }
}

enum ContextMenuAction {
    PlaySelected,
    PlayNext,
    AddToQueue,
    AddToPlaylist(u64),
    NewPlaylistWithSelected,
    EditTags,
    TrackInfo(u64),
    RevealInFinder(u64),
    RemoveSelected,
}

impl eframe::App for TxPlayerApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = root.ctx().clone();
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.process_background_events(&context);
        self.process_input(&context);
        let paused = self.demo_piano_lock || (self.current_id().is_some() && !self.playing);
        self.visualizer
            .update(&self.visual_tap, dt, &self.plugins.visualizers, paused);

        let p = self.palette;

        egui::Panel::bottom("player")
            .exact_size(TRANSPORT_HEIGHT)
            .frame(
                Frame::new()
                    .fill(p.panel)
                    .inner_margin(Margin::symmetric(16, 10))
                    .stroke(Stroke::new(1.0, p.border)),
            )
            .show(root, |ui| self.playback_panel(&context, ui));

        egui::Panel::left("navigation")
            .exact_size(SIDEBAR_WIDTH)
            .frame(
                Frame::new()
                    .fill(p.sidebar)
                    .inner_margin(Margin::ZERO)
                    .stroke(Stroke::new(1.0, p.border)),
            )
            .show(root, |ui| self.sidebar(&context, ui));

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.background)
                    .inner_margin(Margin::symmetric(18, 0)),
            )
            .show(root, |ui| self.main_view(ui));

        self.floating_selection_dock(&context);
        self.show_modals(&context);
        self.drag_overlay(&context);
        self.status_toast(&context);

        #[cfg(not(target_os = "macos"))]
        handle_borderless_resize(&context);

        if self.scanning
            || self.dialog_open
            || self.status.is_some()
            || self.tag_editor.searching
            || self.tag_editor.hashing
            || self.tag_editor.fetching_cover
            || self.tag_editor.saving
            || self.waveform_inflight.is_some()
            || !self.artwork_inflight.is_empty()
        {
            context.request_repaint_after(Duration::from_millis(60));
        }

        // Built-in OpenGL post-render framebuffer screenshot capture (`--screenshot <path>`)
        if let Some(out_path) = self.screenshot_out.clone() {
            self.screenshot_frames += 1;
            if self.screenshot_frames == 22 {
                let rect = context.content_rect();
                let ppp = context.pixels_per_point();
                let w = (rect.width() * ppp).round().max(1.0) as i32;
                let h = (rect.height() * ppp).round().max(1.0) as i32;
                let cb = eframe::egui_glow::CallbackFn::new(move |_info, painter| {
                    use eframe::glow::{self, HasContext as _};
                    let gl = painter.gl();
                    let mut raw = vec![0u8; (w * h * 4) as usize];
                    unsafe {
                        gl.finish();
                        gl.read_pixels(
                            0,
                            0,
                            w,
                            h,
                            glow::RGBA,
                            glow::UNSIGNED_BYTE,
                            glow::PixelPackData::Slice(Some(&mut raw)),
                        );
                    }
                    let row_bytes = (w * 4) as usize;
                    let mut flipped = vec![0u8; raw.len()];
                    for y in 0..(h as usize) {
                        let src = (h as usize - 1 - y) * row_bytes;
                        let dst = y * row_bytes;
                        flipped[dst..dst + row_bytes].copy_from_slice(&raw[src..src + row_bytes]);
                    }
                    if let Some(parent) = out_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = image::save_buffer(
                        &out_path,
                        &flipped,
                        w as u32,
                        h as u32,
                        image::ColorType::Rgba8,
                    );
                });
                let top_painter = context.layer_painter(egui::LayerId::new(
                    egui::Order::Debug,
                    egui::Id::unique("screenshot_capture_layer"),
                ));
                top_painter.add(egui::PaintCallback {
                    rect,
                    callback: Arc::new(cb),
                });
                context.request_repaint_after(Duration::from_millis(40));
            } else if self.screenshot_frames > 23 {
                self.screenshot_out = None;
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                context.request_repaint_after(Duration::from_millis(25));
            }
        }

        // Guarantee standard arrow mouse cursor everywhere in the application.
        context.output_mut(|output| {
            output.cursor_icon = CursorIcon::Default;
        });
    }
}

impl Drop for TxPlayerApp {
    fn drop(&mut self) {
        if self.ephemeral_session {
            return;
        }
        self.store.save();
        Preferences {
            volume: self.volume,
            shuffle: self.shuffle,
            repeat: self.repeat,
            preferred_device: self.preferred_device.clone(),
            theme: self.theme_id,
            progress_style: self.progress_style,
            row_density: self.row_density,
            visualizer_mode: self.visualizer.mode,
            show_concert_visualizer: self.show_concert_visualizer,
            concert_visualizer_height: self.concert_visualizer_height,
            show_piano_keys: self.visualizer.show_piano_keys,
            visualizer_sensitivity: self.visualizer.sensitivity,
            linear_volume: self.linear_volume,
            acoustid_api_key: self.acoustid_api_key.clone(),
            plugins: self.plugins.clone(),
        }
        .save();
    }
}

fn paint_clipped_cell_text(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    font: FontId,
    color: Color32,
) {
    let clipped = painter.with_clip_rect(rect);
    clipped.text(
        pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        text,
        font,
        color,
    );
}

/// Strictly aligned fixed-height format badge painted directly into the FORMAT column rect.
fn paint_aligned_quality_badge(
    painter: &egui::Painter,
    cell: Rect,
    track: &Track,
    palette: &Palette,
) {
    let tier = track.quality_tier();
    let (bg, border, fg, label) = match tier {
        QualityTier::HiRes => (
            palette.hires_bg,
            palette.hires_border,
            palette.hires_gold,
            format!("HI-RES · {}", track.quality()),
        ),
        QualityTier::Lossless => (
            palette.accent_subtle,
            palette.accent.gamma_multiply(0.35),
            palette.accent,
            format!("{} · {}", track.codec, track.quality()),
        ),
        QualityTier::Standard => (
            palette.surface,
            palette.border,
            palette.muted,
            track.quality_full(),
        ),
    };

    let font = FontId::proportional(9.2);
    let galley = painter.layout_no_wrap(label.clone(), font.clone(), fg);
    let pill_w = (galley.size().x + 12.0).min(cell.width());
    let pill_h = 18.0;
    let pill_rect = Rect::from_min_size(
        pos2(cell.left(), cell.center().y - pill_h * 0.5),
        vec2(pill_w, pill_h),
    );
    painter.rect_filled(pill_rect, 4.0, bg);
    painter.rect_stroke(pill_rect, 4.0, Stroke::new(1.0, border), StrokeKind::Inside);
    painter.with_clip_rect(pill_rect.shrink2(vec2(4.0, 0.0))).text(
        pos2(pill_rect.left() + 6.0, pill_rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        fg,
    );
}

fn sidebar_compact_action(
    ui: &mut Ui,
    width: f32,
    label: &str,
    icon: Icon,
    primary: bool,
    p: &Palette,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let (fill, stroke, fg) = if primary {
            (
                if hovered { p.accent_bright } else { p.accent },
                p.accent,
                p.on_accent(),
            )
        } else {
            (
                if hovered { p.hover } else { p.surface },
                if hovered { p.border_strong } else { p.border },
                if hovered { p.text } else { p.text_secondary },
            )
        };
        ui.painter().rect_filled(rect, 7.0, fill);
        ui.painter()
            .rect_stroke(rect, 7.0, Stroke::new(1.0, stroke), StrokeKind::Inside);
        let ir = Rect::from_center_size(pos2(rect.left() + 15.0, rect.center().y), Vec2::splat(13.0));
        icons::paint_icon(ui.painter(), ir, icon, fg);
        ui.painter().text(
            pos2(rect.left() + 27.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(11.5),
            fg,
        );
    }
    resp
}

fn transport_toggle_button(
    ui: &mut Ui,
    icon: Icon,
    active: bool,
    tooltip: &str,
    p: &Palette,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        if hovered {
            ui.painter().rect_filled(rect, 6.0, p.hover);
        }
        let color = if active {
            p.accent
        } else if hovered {
            p.text
        } else {
            p.muted
        };
        let icon_rect = Rect::from_center_size(
            pos2(rect.center().x, rect.center().y - if active { 1.5 } else { 0.0 }),
            Vec2::splat(14.0),
        );
        icons::paint_icon(ui.painter(), icon_rect, icon, color);
        if active {
            ui.painter()
                .circle_filled(pos2(rect.center().x, rect.bottom() - 3.5), 1.8, p.accent);
        }
    }
    resp.on_hover_text(tooltip)
}

fn play_pause_hero_button(ui: &mut Ui, playing: bool, p: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let center = rect.center();
        let r = rect.width() * 0.5;
        if playing || hovered {
            ui.painter().circle_filled(
                center,
                r + 2.5,
                Color32::from_rgba_unmultiplied(
                    p.accent.r(),
                    p.accent.g(),
                    p.accent.b(),
                    if hovered { 48 } else { 28 },
                ),
            );
        }
        let bg = if hovered { p.accent_bright } else { p.accent };
        ui.painter().circle_filled(center, r, bg);
        let offset = if playing { 0.0 } else { 1.2 };
        let ir = Rect::from_center_size(pos2(center.x + offset, center.y), Vec2::splat(16.0));
        icons::paint_icon(
            ui.painter(),
            ir,
            if playing { Icon::Pause } else { Icon::Play },
            p.on_accent(),
        );
    }
    resp.on_hover_text(if playing { "Pause (Space)" } else { "Play (Space)" })
}

/// Studio-grade mirrored acoustic waveform timeline scrubber (replaces generic green progress bar).
#[allow(clippy::too_many_arguments)]
fn acoustic_timeline_scrubber(
    ui: &mut Ui,
    width: f32,
    position: f64,
    duration: f64,
    active: bool,
    waveform: Option<&[f32]>,
    style: ProgressBarStyle,
    p: &Palette,
) -> Option<f64> {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click_and_drag());
    let hovered = response.hovered();
    let dragging = response.dragged();
    let pointer_x = ui.ctx().pointer_hover_pos().map(|pt| pt.x);

    let frac = if active && duration > 0.0 {
        if dragging && let Some(px) = pointer_x {
            ((px - rect.left()) / rect.width()).clamp(0.0, 1.0)
        } else {
            (position / duration).clamp(0.0, 1.0) as f32
        }
    } else {
        0.0
    };

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let playhead_x = rect.left() + rect.width() * frac;

        match style {
            ProgressBarStyle::AcousticWaveform => {
                // Subtle framed dark acoustic track channel
                let bg_rect = rect.shrink2(vec2(0.0, 2.0));
                painter.rect_filled(bg_rect, 5.0, p.background);
                painter.rect_stroke(
                    bg_rect,
                    5.0,
                    Stroke::new(
                        0.8,
                        if hovered || dragging {
                            p.border_strong
                        } else {
                            p.border
                        },
                    ),
                    StrokeKind::Inside,
                );

                // Subtle center zero-axis hairline
                let cy = bg_rect.center().y;
                painter.line_segment(
                    [pos2(bg_rect.left() + 4.0, cy), pos2(bg_rect.right() - 4.0, cy)],
                    Stroke::new(0.6, p.border.gamma_multiply(0.5)),
                );

                let inner = bg_rect.shrink2(vec2(5.0, 3.0));
                let bar_count = ((inner.width() / 3.2).floor() as usize).clamp(32, WAVEFORM_BINS);
                let step = inner.width() / bar_count as f32;
                let bar_w = (step * 0.62).clamp(1.2, 2.4);
                let half_h = inner.height() * 0.5;

                for i in 0..bar_count {
                    let t = i as f32 / (bar_count - 1).max(1) as f32;
                    let amp = if let Some(wf) = waveform {
                        let idx = ((t * (wf.len() - 1) as f32).round() as usize)
                            .min(wf.len().saturating_sub(1));
                        wf[idx]
                    } else if active {
                        // Graceful subtle placeholder acoustic envelope while real waveform loads
                        let w1 = (t * 23.0).sin().abs() * 0.45;
                        let w2 = (t * 7.3 + 1.1).cos().abs() * 0.35;
                        (0.18 + w1 + w2).min(0.82)
                    } else {
                        0.12
                    };

                    let bx = inner.left() + i as f32 * step + (step - bar_w) * 0.5;
                    let played = active && bx <= playhead_x;
                    let hover_zone = active
                        && hovered
                        && pointer_x.is_some_and(|px| {
                            (bx >= playhead_x && bx <= px) || (bx <= playhead_x && bx >= px)
                        });

                    let bh = (half_h * amp).max(1.2);
                    let bar_rect = Rect::from_min_max(pos2(bx, cy - bh), pos2(bx + bar_w, cy + bh));

                    let col = if played {
                        // Luminous acoustic gradient along played region
                        let grad = if t < 0.5 {
                            lerp_c(p.accent_secondary, p.accent, t * 2.0)
                        } else {
                            lerp_c(p.accent, p.hires_gold, (t - 0.5) * 0.7)
                        };
                        if hovered || dragging {
                            lerp_c(grad, p.accent_bright, 0.25)
                        } else {
                            grad
                        }
                    } else if hover_zone {
                        p.accent.gamma_multiply(0.55)
                    } else {
                        p.border_strong.gamma_multiply(if active { 0.85 } else { 0.45 })
                    };
                    painter.rect_filled(bar_rect, bar_w * 0.45, col);
                }

                // Glowing vertical precision playhead cursor needle
                if active {
                    painter.rect_filled(
                        Rect::from_center_size(pos2(playhead_x, cy), vec2(6.0, bg_rect.height())),
                        3.0,
                        Color32::from_rgba_unmultiplied(
                            p.accent.r(),
                            p.accent.g(),
                            p.accent.b(),
                            if hovered || dragging { 70 } else { 36 },
                        ),
                    );
                    painter.line_segment(
                        [
                            pos2(playhead_x, bg_rect.top() + 1.5),
                            pos2(playhead_x, bg_rect.bottom() - 1.5),
                        ],
                        Stroke::new(1.8, p.accent_bright),
                    );
                }

                // Ghost hover cursor needle
                if active && hovered && !dragging && let Some(px) = pointer_x {
                    let cx = px.clamp(bg_rect.left() + 2.0, bg_rect.right() - 2.0);
                    painter.line_segment(
                        [pos2(cx, bg_rect.top() + 2.0), pos2(cx, bg_rect.bottom() - 2.0)],
                        Stroke::new(1.0, p.text.gamma_multiply(0.55)),
                    );
                }
            }
            ProgressBarStyle::StudioRail => {
                let bar_h = if hovered || dragging { 6.0 } else { 4.0 };
                let track_rect = Rect::from_center_size(rect.center(), vec2(rect.width(), bar_h));
                painter.rect_filled(track_rect, bar_h * 0.5, p.surface_elevated);
                if frac > 0.0 {
                    let fill_w = (track_rect.width() * frac).max(bar_h);
                    let fill_rect = Rect::from_min_size(track_rect.min, vec2(fill_w, bar_h));
                    painter.rect_filled(
                        fill_rect,
                        bar_h * 0.5,
                        if hovered || dragging {
                            p.accent_bright
                        } else {
                            p.accent
                        },
                    );
                }
                if active {
                    let knob_c = pos2(playhead_x, track_rect.center().y);
                    painter.circle_filled(
                        knob_c,
                        if hovered || dragging { 8.5 } else { 5.5 },
                        Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 55),
                    );
                    painter.circle_filled(
                        knob_c,
                        if hovered || dragging { 5.0 } else { 3.5 },
                        p.text,
                    );
                }
            }
        }
    }

    if active && hovered && let Some(px) = pointer_x {
        let hover_frac = ((px - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
        response
            .clone()
            .on_hover_text(format!("Seek to {}", format_duration(Some(hover_frac * duration))));
    }

    if active
        && (response.drag_stopped() || response.clicked())
        && let Some(px) = pointer_x.or_else(|| response.interact_pointer_pos().map(|pt| pt.x))
    {
        let target_frac = ((px - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
        return Some(target_frac * duration);
    }
    None
}

fn custom_volume_bar(ui: &mut Ui, width: f32, volume: f32, p: &Palette) -> Option<f32> {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
    let hovered = response.hovered();
    let dragging = response.dragged();

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let bar_h = if hovered || dragging { 5.0 } else { 3.5 };
        let track_rect = Rect::from_center_size(rect.center(), vec2(rect.width(), bar_h));
        painter.rect_filled(track_rect, bar_h * 0.5, p.surface_elevated);
        let fill_w = (track_rect.width() * volume.clamp(0.0, 1.0)).max(0.0);
        if fill_w > 0.5 {
            let fill_rect = Rect::from_min_size(track_rect.min, vec2(fill_w, bar_h));
            painter.rect_filled(
                fill_rect,
                bar_h * 0.5,
                if hovered || dragging {
                    p.accent_bright
                } else {
                    p.accent
                },
            );
        }
        if hovered || dragging {
            let knob_x = track_rect.left() + track_rect.width() * volume.clamp(0.0, 1.0);
            painter.circle_filled(pos2(knob_x, track_rect.center().y), 4.8, p.text);
        }
    }

    let pct = (volume * 100.0).round() as u32;
    let tip_resp = response.on_hover_text(format!("Volume: {pct}%"));
    if (tip_resp.clicked() || tip_resp.dragged())
        && let Some(pos) = tip_resp.interact_pointer_pos()
    {
        return Some(((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
    }
    None
}

fn lerp_c(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn format_library_duration(seconds: f64) -> String {
    let total_mins = (seconds / 60.0).round() as u64;
    let hours = total_mins / 60;
    let mins = total_mins % 60;
    if hours > 0 {
        format!("{hours} hr {mins} min")
    } else {
        format!("{mins} min")
    }
}

/// 8-way invisible window edge/corner resize handles for borderless Windows & Linux builds.
#[cfg(not(target_os = "macos"))]
fn handle_borderless_resize(ctx: &egui::Context) {
    use egui::ResizeDirection;
    let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    if !ctx.input(|i| i.pointer.primary_pressed()) {
        return;
    }
    let rect = ctx.content_rect();
    let edge = 6.0;
    let left = pos.x <= rect.left() + edge;
    let right = pos.x >= rect.right() - edge;
    let top = pos.y <= rect.top() + edge;
    let bottom = pos.y >= rect.bottom() - edge;
    let dir = match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (_, true, true, _) => Some(ResizeDirection::NorthEast),
        (true, _, _, true) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, _, _, _) => Some(ResizeDirection::West),
        (_, true, _, _) => Some(ResizeDirection::East),
        (_, _, true, _) => Some(ResizeDirection::North),
        (_, _, _, true) => Some(ResizeDirection::South),
        _ => None,
    };
    if let Some(d) = dir {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(d));
    }
}
