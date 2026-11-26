use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, CursorIcon, FontId, Frame, Layout, Margin, Pos2,
    Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2, pos2, vec2,
};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::audio::{
    AudioEvent, AudioHandle, PlaybackSpec, VisualTap, compute_waveform, list_output_devices,
};
use crate::icons::{self, Icon};
use crate::library::{LibraryStore, ScanEvent, cover_directory, scan};
use crate::lyrics::{TrackLyrics, load_lyrics_for_track};
use crate::media_controls::{OsMediaBridge, OsMediaCommand};
use crate::metadata::{
    QualityTier, SUPPORTED_EXTENSIONS, Track, format_duration, format_sample_rate,
};
use crate::modals::{
    self, CommandPaletteState, FolderModalAction, PaletteCommand, PaletteEntry, PillStyle,
    ProgressBarStyle, RowDensity, SettingsTab, SignalPathDisplay, TagEditorState, TagWorkerEvent,
    TrackInfoAction, pill_button,
};
use crate::platform::{FILE_MANAGER_NAME, MOD_SHORT, reveal_in_file_manager};
use crate::playlist_io::{export_playlist, import_playlist_paths};
use crate::plugins::{EqPreset, PianoEngineId, PluginConfig, ReplayGainMode, ToolHealth};
use crate::theme::{self, Palette, ThemeId};
use crate::tray::{SystemTrayBridge, TrayCommand};
use crate::visualizer::{VisualizerMode, VisualizerState};
use crate::watcher::FolderWatcher;

const SIDEBAR_WIDTH: f32 = 224.0;
const TRANSPORT_HEIGHT: f32 = 96.0;
const WAVEFORM_BINS: usize = 160;

pub const COMPACT_WIDTH: f32 = 468.0;
pub const COMPACT_COLLAPSED_HEIGHT: f32 = 200.0;
pub const COMPACT_EXPANDED_HEIGHT: f32 = 476.0;
pub const COMPACT_MIN_WIDTH: f32 = 380.0;
pub const COMPACT_MIN_HEIGHT: f32 = 186.0;
pub const FULL_WIDTH: f32 = 1240.0;
pub const FULL_HEIGHT: f32 = 790.0;
pub const FULL_MIN_WIDTH: f32 = 900.0;
pub const FULL_MIN_HEIGHT: f32 = 540.0;

pub fn startup_window_layout() -> (bool, bool, bool) {
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|a| a == "--full" || a == "--vis-mode" || a == "--demo-piano")
    {
        return (false, true, false);
    }
    if args.iter().any(|a| a == "--compact") {
        let prefs = Preferences::load();
        return (true, true, prefs.show_concert_visualizer);
    }
    if args.iter().any(|a| a == "--open-settings") {
        return (false, true, false);
    }
    let prefs = Preferences::load();
    (
        prefs.compact_mode,
        prefs.compact_show_playlist,
        prefs.show_concert_visualizer,
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SmartView {
    HiRes,
    RecentlyAdded,
    MostPlayed,
    TopRated,
}

impl SmartView {
    fn label(self) -> &'static str {
        match self {
            SmartView::HiRes => "Hi-Res Masters",
            SmartView::RecentlyAdded => "Recently Added",
            SmartView::MostPlayed => "Most Played",
            SmartView::TopRated => "Top Rated",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowseMode {
    Tracks,
    Albums,
    Artists,
    Folders,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Library,
    Favorites,
    Queue,
    Playlist(u64),
    Smart(SmartView),
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
    #[serde(default)]
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
    #[serde(default = "default_true")]
    compact_mode: bool,
    #[serde(default = "default_true")]
    compact_show_playlist: bool,
    #[serde(default)]
    close_to_tray: bool,
    #[serde(default = "default_true")]
    auto_watch_folders: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct SessionState {
    active_library_id: u64,
    queue_ids: Vec<u64>,
    current_index: Option<usize>,
    position_secs: f64,
}

impl SessionState {
    fn load() -> Option<Self> {
        let data = std::fs::read(crate::library::data_directory().join("session.json")).ok()?;
        serde_json::from_slice(&data).ok()
    }

    fn save(&self) {
        let dir = crate::library::data_directory();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(data) = serde_json::to_vec(self) {
            let _ = std::fs::write(dir.join("session.json"), data);
        }
    }
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
            show_concert_visualizer: false,
            concert_visualizer_height: default_concert_height(),
            show_piano_keys: true,
            visualizer_sensitivity: default_sensitivity(),
            linear_volume: false,
            acoustid_api_key: String::new(),
            plugins: PluginConfig::default(),
            compact_mode: true,
            compact_show_playlist: true,
            close_to_tray: false,
            auto_watch_folders: true,
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
    ImportPlaylist(Option<PathBuf>),
    ExportPlaylist {
        path: Option<PathBuf>,
        track_ids: Vec<u64>,
    },
}

#[derive(Clone)]
struct StreamInfo {
    source_rate: u32,
    output_rate: u32,
    channels: u16,
    bit_depth: Option<u32>,
    device_name: String,
    bit_perfect: bool,
    rg_gain_db: Option<f32>,
}

/// Deterministic column layout shared identically by table header AND every track row (no checkboxes).
struct TrackColumns {
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
        let mut x = row_rect.left() + 8.0;

        let mut take_fixed = |w: f32| -> Rect {
            let r = Rect::from_min_max(pos2(x, top), pos2(x + w, bot));
            x += w;
            r
        };

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

type AlbumGroup = (String, String, Track, usize, bool);

#[derive(Clone, PartialEq, Eq)]
struct VisibleCacheKey {
    library_rev: u64,
    queue_rev: u64,
    active_lib_id: u64,
    view: View,
    folder_filter: Option<PathBuf>,
    album_filter: Option<(String, String)>,
    artist_filter: Option<String>,
    sort: SortMode,
    sort_ascending: bool,
    search: String,
}

pub struct TxPlayerApp {
    audio: AudioHandle,
    visual_tap: Arc<VisualTap>,
    visualizer: VisualizerState,
    media_bridge: OsMediaBridge,
    tray_bridge: SystemTrayBridge,
    watcher: FolderWatcher,
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
    compact_mode: bool,
    compact_show_playlist: bool,
    compact_modal_expanded: bool,
    close_to_tray: bool,
    auto_watch_folders: bool,
    force_quit: bool,
    linear_volume: bool,
    acoustid_api_key: String,

    view: View,
    browse_mode: BrowseMode,
    folder_filter: Option<PathBuf>,
    album_filter: Option<(String, String)>,
    artist_filter: Option<String>,
    sort: SortMode,
    sort_ascending: bool,
    search: String,
    focus_search: bool,

    // Playback speed, A-B section loop & synced lyrics
    playback_speed: f32,
    loop_a: Option<f64>,
    loop_b: Option<f64>,
    show_lyrics: bool,
    cached_lyrics_id: Option<u64>,
    cached_lyrics: Option<Arc<TrackLyrics>>,
    counted_play_id: Option<u64>,

    // Cached filtered/sorted track table, derived browser views & sidebar stats (zero 60 Hz cloning/sorting!)
    library_rev: u64,
    queue_rev: u64,
    visible_cache_key: Option<VisibleCacheKey>,
    cached_visible: Arc<Vec<Track>>,
    cached_total_duration: f64,
    cached_albums: Arc<Vec<AlbumGroup>>,
    cached_artists: Arc<Vec<(String, usize, usize)>>,
    cached_dirs: Arc<Vec<(PathBuf, usize)>>,
    cached_stats_rev: u64,
    cached_stats_lib_id: u64,
    cached_hires_count: usize,
    cached_most_played_count: usize,
    cached_top_rated_count: usize,
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
    inspect_file_size: Option<(u64, Option<u64>)>,
    file_size_cache: HashMap<PathBuf, Option<u64>>,
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

        // Unified hi-def TxPlayer app icon texture (256x256 for crisp HiDPI scaling)
        let brand_icon = image::load_from_memory(crate::APP_ICON_PNG).ok().map(|img| {
            let thumb = img.thumbnail(256, 256).into_rgba8();
            let dims = [thumb.width() as usize, thumb.height() as usize];
            let color = ColorImage::from_rgba_unmultiplied(dims, thumb.as_raw());
            context.egui_ctx.load_texture(
                "txplayer-brand-icon",
                color,
                egui::TextureOptions::LINEAR,
            )
        });

        let arguments: Vec<String> = std::env::args().collect();
        let is_screenshot = arguments.iter().any(|a| a == "--screenshot");

        let media_bridge = OsMediaBridge::new(context);
        let tray_bridge = SystemTrayBridge::new(is_screenshot);
        let watcher = FolderWatcher::spawn(
            store.active().folders.clone(),
            preferences.auto_watch_folders && !is_screenshot,
        );

        let mut app = Self {
            audio,
            visual_tap,
            visualizer,
            media_bridge,
            tray_bridge,
            watcher,
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
            compact_mode: preferences.compact_mode,
            compact_show_playlist: preferences.compact_show_playlist,
            compact_modal_expanded: false,
            close_to_tray: preferences.close_to_tray,
            auto_watch_folders: preferences.auto_watch_folders,
            force_quit: false,
            linear_volume: preferences.linear_volume,
            acoustid_api_key: preferences.acoustid_api_key,
            view: View::Library,
            browse_mode: BrowseMode::Tracks,
            folder_filter: None,
            album_filter: None,
            artist_filter: None,
            sort: SortMode::Artist,
            sort_ascending: true,
            search: String::new(),
            focus_search: false,
            playback_speed: 1.0,
            loop_a: None,
            loop_b: None,
            show_lyrics: false,
            cached_lyrics_id: None,
            cached_lyrics: None,
            counted_play_id: None,
            library_rev: 1,
            queue_rev: 1,
            visible_cache_key: None,
            cached_visible: Arc::new(Vec::new()),
            cached_total_duration: 0.0,
            cached_albums: Arc::new(Vec::new()),
            cached_artists: Arc::new(Vec::new()),
            cached_dirs: Arc::new(Vec::new()),
            cached_stats_rev: 0,
            cached_stats_lib_id: 0,
            cached_hires_count: 0,
            cached_most_played_count: 0,
            cached_top_rated_count: 0,
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
            inspect_file_size: None,
            file_size_cache: HashMap::new(),
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

        let force_compact = arguments.iter().any(|a| a == "--compact");
        if force_compact {
            app.compact_mode = true;
            app.compact_show_playlist = true;
        }
        if arguments.iter().any(|a| a == "--full" || a == "--vis-mode" || a == "--demo-piano")
            || (!force_compact && arguments.iter().any(|a| a == "--open-settings"))
        {
            app.compact_mode = false;
        }
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
        } else if !app.ephemeral_session
            && let Some(sess) = SessionState::load()
            && sess.active_library_id == app.store.active_id
            && !sess.queue_ids.is_empty()
        {
            // Restore previous playback session (paused at exact saved position, ready to hit Space!)
            let valid_queue: Vec<u64> = sess
                .queue_ids
                .into_iter()
                .filter(|id| app.track(*id).is_some())
                .collect();
            if !valid_queue.is_empty() {
                app.current_queue = valid_queue;
                app.current_index = sess
                    .current_index
                    .filter(|&i| i < app.current_queue.len());
                app.mark_queue_dirty();
                if let Some(track) = app.current_track().cloned()
                    && track.path.exists()
                {
                    let resume_pos = sess
                        .position_secs
                        .clamp(0.0, track.duration.unwrap_or(sess.position_secs).max(0.0));
                    app.position = resume_pos;
                    app.duration = track.duration.unwrap_or(0.0);
                    app.request_waveform(&track);
                    app.sync_lyrics_for_current();
                    let spec = app.build_spec(&track, resume_pos, true);
                    app.audio.play_spec(spec);
                    app.preload_next_track();
                }
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
        self.store.active().get_by_id(id)
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
        let mut hires = 0usize;
        let mut most_played = 0usize;
        let mut top_rated = 0usize;
        for t in &lib.tracks {
            if t.is_hires() {
                hires += 1;
            }
            if t.play_count > 0 {
                most_played += 1;
            }
            if t.rating >= 4 {
                top_rated += 1;
            }
        }
        self.cached_hires_count = hires;
        self.cached_most_played_count = most_played;
        self.cached_top_rated_count = top_rated;
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
            album_filter: self.album_filter.clone(),
            artist_filter: self.artist_filter.clone(),
            sort: self.sort,
            sort_ascending: self.sort_ascending,
            search: self.search.clone(),
        };
        if self.visible_cache_key.as_ref() == Some(&key) {
            return;
        }
        let tracks = self.compute_visible_tracks();
        self.cached_total_duration = tracks.iter().filter_map(|t| t.duration).sum();

        // Precompute derived Albums / Artists / Folders views once on invalidation (zero 60 Hz HashMaps!)
        let mut album_groups: Vec<(String, String, Track, usize, bool)> = Vec::new();
        let mut album_idx: HashMap<(String, String), usize> = HashMap::new();
        let mut artist_counts: HashMap<String, (usize, HashSet<String>)> = HashMap::new();
        let mut dir_counts: HashMap<PathBuf, usize> = HashMap::new();
        for tr in &tracks {
            let akey = (tr.album.clone(), tr.artist.clone());
            if let Some(&gi) = album_idx.get(&akey) {
                album_groups[gi].3 += 1;
                album_groups[gi].4 |= tr.is_hires();
            } else {
                let gi = album_groups.len();
                album_idx.insert(akey.clone(), gi);
                album_groups.push((akey.0, akey.1, tr.clone(), 1, tr.is_hires()));
            }
            let ae = artist_counts
                .entry(tr.artist.clone())
                .or_insert_with(|| (0, HashSet::new()));
            ae.0 += 1;
            ae.1.insert(tr.album.clone());
            if let Some(parent) = tr.path.parent() {
                *dir_counts.entry(parent.to_path_buf()).or_insert(0) += 1;
            }
        }
        album_groups.sort_by_key(|a| a.0.to_lowercase());
        let mut artists: Vec<(String, usize, usize)> = artist_counts
            .into_iter()
            .map(|(name, (t_cnt, albs))| (name, t_cnt, albs.len()))
            .collect();
        artists.sort_by_key(|a| a.0.to_lowercase());
        let mut dirs: Vec<(PathBuf, usize)> = dir_counts.into_iter().collect();
        dirs.sort_by(|a, b| a.0.cmp(&b.0));

        self.cached_albums = Arc::new(album_groups);
        self.cached_artists = Arc::new(artists);
        self.cached_dirs = Arc::new(dirs);
        self.cached_visible = Arc::new(tracks);
        self.visible_cache_key = Some(key);
    }

    fn visible_tracks(&mut self) -> Arc<Vec<Track>> {
        self.ensure_visible_cache();
        Arc::clone(&self.cached_visible)
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
                .filter(|track| {
                    self.album_filter
                        .as_ref()
                        .is_none_or(|(alb, art)| &track.album == alb && &track.artist == art)
                })
                .filter(|track| {
                    self.artist_filter
                        .as_ref()
                        .is_none_or(|art| &track.artist == art)
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
            View::Smart(smart) => {
                let mut list: Vec<Track> = lib
                    .tracks
                    .iter()
                    .filter(|track| match smart {
                        SmartView::HiRes => track.is_hires(),
                        SmartView::RecentlyAdded => true,
                        SmartView::MostPlayed => track.play_count > 0,
                        SmartView::TopRated => track.rating >= 4,
                    })
                    .filter(|track| query.is_empty() || track.search_text().contains(&query))
                    .cloned()
                    .collect();
                match smart {
                    SmartView::RecentlyAdded => {
                        list.sort_by(|a, b| b.added_at.cmp(&a.added_at).then(a.title.cmp(&b.title)));
                        list.truncate(100);
                    }
                    SmartView::MostPlayed => {
                        list.sort_by(|a, b| {
                            b.play_count.cmp(&a.play_count).then(a.title.cmp(&b.title))
                        });
                    }
                    SmartView::TopRated => {
                        list.sort_by(|a, b| {
                            b.rating
                                .cmp(&a.rating)
                                .then(b.play_count.cmp(&a.play_count))
                                .then(a.title.cmp(&b.title))
                        });
                    }
                    SmartView::HiRes => {}
                }
                list
            }
        };

        if matches!(
            self.view,
            View::Library | View::Favorites | View::Smart(SmartView::HiRes)
        ) {
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
            View::Library | View::Favorites | View::Smart(_) => {
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

    fn compute_rg_for_track(&self, track: &Track) -> (f32, Option<f32>) {
        let dsp = &self.plugins.dsp;
        if dsp.bit_perfect_mode || dsp.replay_gain == ReplayGainMode::Off {
            return (1.0, None);
        }
        let (gain_db, peak) = match dsp.replay_gain {
            ReplayGainMode::Off => return (1.0, None),
            ReplayGainMode::Track => (
                track.rg_track_gain_db.or(track.rg_album_gain_db),
                track.rg_track_peak.or(track.rg_album_peak),
            ),
            ReplayGainMode::Album => (
                track.rg_album_gain_db.or(track.rg_track_gain_db),
                track.rg_album_peak.or(track.rg_track_peak),
            ),
        };
        let Some(db) = gain_db else {
            return (1.0, None);
        };
        let total_db = (db + dsp.rg_preamp_db).clamp(-18.0, 12.0);
        let mut linear = 10.0_f32.powf(total_db / 20.0);
        if dsp.rg_prevent_clipping
            && let Some(pk) = peak
            && pk > 0.01
            && linear * pk > 0.99
        {
            linear = 0.99 / pk;
        }
        let applied_db = 20.0 * linear.max(1e-4).log10();
        (linear, Some(applied_db))
    }

    fn build_spec(&self, track: &Track, start_offset: f64, start_paused: bool) -> PlaybackSpec {
        let (rg_gain_linear, rg_gain_db) = self.compute_rg_for_track(track);
        PlaybackSpec {
            path: track.path.clone(),
            cue_start: track.cue_start.unwrap_or(0.0),
            cue_end: track.cue_end,
            start_offset,
            start_paused,
            rg_gain_linear,
            rg_gain_db,
        }
    }

    fn peek_next_index(&self) -> Option<usize> {
        if self.current_queue.is_empty() {
            return None;
        }
        if self.repeat == RepeatMode::One {
            return self.current_index;
        }
        if self.shuffle {
            return None;
        }
        let cur = self.current_index?;
        if cur + 1 < self.current_queue.len() {
            Some(cur + 1)
        } else if self.repeat == RepeatMode::All {
            Some(0)
        } else {
            None
        }
    }

    fn preload_next_track(&self) {
        if !self.plugins.dsp.gapless_enabled {
            self.audio.preload_next(None);
            return;
        }
        let spec = self
            .peek_next_index()
            .and_then(|idx| self.current_queue.get(idx).copied())
            .and_then(|id| self.track(id))
            .filter(|t| t.path.exists())
            .map(|t| self.build_spec(t, 0.0, false));
        self.audio.preload_next(spec);
    }

    fn sync_lyrics_for_current(&mut self) {
        let cur_id = self.current_id();
        if self.cached_lyrics_id == cur_id {
            return;
        }
        self.cached_lyrics_id = cur_id;
        self.cached_lyrics = cur_id
            .and_then(|id| self.track(id))
            .and_then(|t| load_lyrics_for_track(&t.path, t.lyrics.as_deref()))
            .map(Arc::new);
    }

    fn apply_current_rg(&mut self) {
        if let Some(track) = self.current_track().cloned() {
            let (linear, db) = self.compute_rg_for_track(&track);
            self.audio.set_replay_gain_linear(linear);
            if let Some(ref mut info) = self.stream_info {
                info.rg_gain_db = db;
                info.bit_perfect = self.plugins.dsp.bit_perfect_mode
                    && info.source_rate == info.output_rate
                    && (self.playback_speed - 1.0).abs() < 0.001;
            }
        }
        self.preload_next_track();
    }

    fn set_playback_speed(&mut self, speed: f32) {
        let clamped = speed.clamp(0.5, 2.0);
        self.playback_speed = clamped;
        self.audio.set_speed(clamped);
        self.set_status(format!("Playback speed: {:.2}x", clamped));
    }

    fn set_loop_a(&mut self) {
        if self.current_id().is_none() {
            return;
        }
        self.loop_a = Some(self.position);
        if let Some(b) = self.loop_b
            && b <= self.position + 0.1
        {
            self.loop_b = None;
        }
        let range = self.loop_a.zip(self.loop_b);
        self.audio.set_ab_loop(range);
        self.set_status(format!("Loop A set at {}", format_duration(Some(self.position))));
    }

    fn set_loop_b(&mut self) {
        if self.current_id().is_none() {
            return;
        }
        let a = self.loop_a.unwrap_or(0.0);
        if self.position > a + 0.15 {
            self.loop_a = Some(a);
            self.loop_b = Some(self.position);
            self.audio.set_ab_loop(Some((a, self.position)));
            self.set_status(format!(
                "A-B Loop active: {} → {}",
                format_duration(Some(a)),
                format_duration(Some(self.position))
            ));
        }
    }

    fn clear_ab_loop(&mut self) {
        self.loop_a = None;
        self.loop_b = None;
        self.audio.set_ab_loop(None);
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
        self.clear_ab_loop();
        self.visualizer.reset();
        self.request_waveform(&track);
        self.sync_lyrics_for_current();
        self.counted_play_id = None;
        let spec = self.build_spec(&track, 0.0, false);
        self.audio.play_spec(spec);
        self.preload_next_track();
        self.playing = true;
        self.media_bridge.sync_metadata(
            track.id,
            &track.title,
            &track.artist,
            &track.album,
            track.duration,
            track.cover_path.as_deref(),
        );
        self.media_bridge.sync_playback(true, true, 0.0);
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

    fn import_playlist_dialog(&mut self) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Import Playlist (.m3u / .m3u8 / .pls)")
            .add_filter("Playlists", &["m3u", "m3u8", "pls"]);
        let events = self.dialog_sender.clone();
        thread::spawn(move || {
            let path = pollster::block_on(dialog.pick_file()).map(|h| h.path().to_path_buf());
            let _ = events.send(DialogEvent::ImportPlaylist(path));
        });
    }

    fn export_playlist_dialog(&mut self, default_name: &str, track_ids: Vec<u64>) {
        if self.dialog_open || track_ids.is_empty() {
            return;
        }
        self.dialog_open = true;
        let file_name = format!("{}.m3u8", default_name.replace(['/', '\\', ':'], "_"));
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Export Playlist (.m3u8 or .pls)")
            .set_file_name(&file_name)
            .add_filter("M3U8 Playlist", &["m3u8", "m3u"])
            .add_filter("PLS Playlist", &["pls"]);
        let events = self.dialog_sender.clone();
        thread::spawn(move || {
            let path = pollster::block_on(dialog.save_file()).map(|h| h.path().to_path_buf());
            let _ = events.send(DialogEvent::ExportPlaylist { path, track_ids });
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

        // OS Media Keys & System Now Playing events (SMTC / MPRIS / macOS Control Center)
        while let Some(cmd) = self.media_bridge.try_event() {
            match cmd {
                OsMediaCommand::Play => {
                    if !self.playing {
                        self.toggle_playback();
                    }
                }
                OsMediaCommand::Pause => {
                    if self.playing {
                        self.toggle_playback();
                    }
                }
                OsMediaCommand::Toggle => self.toggle_playback(),
                OsMediaCommand::Next => self.next(false),
                OsMediaCommand::Previous => self.previous(),
                OsMediaCommand::Stop => self.stop_playback(),
                OsMediaCommand::SeekAbs(pos) => {
                    if self.current_id().is_some() {
                        let target = pos.clamp(0.0, self.duration.max(pos));
                        self.audio.seek(target);
                        self.position = target;
                    }
                }
                OsMediaCommand::SeekRel(delta) => {
                    if self.current_id().is_some() {
                        let target = (self.position + delta).clamp(0.0, self.duration.max(0.0));
                        self.audio.seek(target);
                        self.position = target;
                    }
                }
            }
        }

        // System Tray / Menu Bar events
        while let Some(tcmd) = self.tray_bridge.poll() {
            match tcmd {
                TrayCommand::ShowWindow => {
                    context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    context.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    context.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayCommand::TogglePlay => self.toggle_playback(),
                TrayCommand::Next => self.next(false),
                TrayCommand::Previous => self.previous(),
                TrayCommand::Quit => {
                    self.force_quit = true;
                    context.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // Background Live Folder Watcher auto-sync
        while let Some(delta) = self.watcher.try_event() {
            if delta.removed_count > 0 {
                let removed = self.store.active_mut().prune_missing();
                if !removed.is_empty() {
                    self.purge_ids_from_queue(&removed);
                    self.mark_library_dirty();
                    self.store.save();
                }
            }
            if !delta.changed_or_added.is_empty() {
                self.start_scan(delta.changed_or_added);
            }
        }

        // Close-to-tray intercept on native window close button / Cmd+W / Alt+F4
        if self.close_to_tray
            && !self.force_quit
            && !self.ephemeral_session
            && context.input(|i| i.viewport().close_requested())
        {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
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
                    bit_perfect,
                    rg_gain_db,
                } => {
                    self.stream_info = Some(StreamInfo {
                        source_rate: source_sample_rate,
                        output_rate: output_sample_rate,
                        channels,
                        bit_depth,
                        device_name,
                        bit_perfect,
                        rg_gain_db,
                    });
                    if let Some(duration) = duration {
                        self.duration = duration;
                        if let Some(cid) = self.current_id()
                            && let Some(track) = self.store.active_mut().get_by_id_mut(cid)
                            && track.duration.is_none()
                        {
                            track.duration = Some(duration);
                            self.mark_library_dirty();
                        }
                    }
                    if let Some(t) = self.current_track().cloned() {
                        self.media_bridge.sync_metadata(
                            t.id,
                            &t.title,
                            &t.artist,
                            &t.album,
                            Some(self.duration),
                            t.cover_path.as_deref(),
                        );
                    }
                }
                AudioEvent::GaplessTransitioned {
                    source_sample_rate,
                    output_sample_rate,
                    channels,
                    bit_depth,
                    duration,
                    device_name,
                    bit_perfect,
                    rg_gain_db,
                } => {
                    if let Some(next_idx) = self.peek_next_index() {
                        self.current_index = Some(next_idx);
                    }
                    self.position = 0.0;
                    self.clear_ab_loop();
                    self.counted_play_id = None;
                    self.stream_info = Some(StreamInfo {
                        source_rate: source_sample_rate,
                        output_rate: output_sample_rate,
                        channels,
                        bit_depth,
                        device_name,
                        bit_perfect,
                        rg_gain_db,
                    });
                    if let Some(d) = duration {
                        self.duration = d;
                    } else if let Some(t) = self.current_track() {
                        self.duration = t.duration.unwrap_or(0.0);
                    }
                    if let Some(t) = self.current_track().cloned() {
                        self.request_waveform(&t);
                        self.sync_lyrics_for_current();
                        self.media_bridge.sync_metadata(
                            t.id,
                            &t.title,
                            &t.artist,
                            &t.album,
                            Some(self.duration),
                            t.cover_path.as_deref(),
                        );
                    }
                    self.preload_next_track();
                }
                AudioEvent::DeviceRouted(name) => {
                    self.available_devices = list_output_devices();
                    self.set_status(format!("Audio output switched to {name}"));
                }
                AudioEvent::Playing(playing) => {
                    self.playing = playing;
                    self.media_bridge
                        .sync_playback(self.current_id().is_some(), playing, self.position);
                }
                AudioEvent::Stopped => {
                    self.playing = false;
                    self.position = 0.0;
                    self.media_bridge.sync_playback(false, false, 0.0);
                }
                AudioEvent::Position(position) => {
                    self.position = position;
                    // Record play count once per track after 10s (or 30% for short tracks)
                    if let Some(cid) = self.current_id()
                        && self.counted_play_id != Some(cid)
                        && self.playing
                    {
                        let threshold = (self.duration * 0.30).clamp(3.0, 12.0);
                        if position >= threshold {
                            self.counted_play_id = Some(cid);
                            self.store.active_mut().increment_play_count(cid);
                            self.mark_library_dirty();
                            self.store.save();
                        }
                    }
                }
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
                        self.watcher.set_folders(self.store.active().folders.clone());
                        self.mark_library_dirty();
                        self.store.save();
                        self.start_scan(paths);
                    }
                }
                DialogEvent::ImportPlaylist(maybe_path) => {
                    if let Some(path) = maybe_path {
                        match import_playlist_paths(&path) {
                            Ok(found_paths) if !found_paths.is_empty() => {
                                let name = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("Imported Playlist")
                                    .to_string();
                                let pl_id = self.store.active_mut().create_playlist(name.clone());
                                let cover_dir = cover_directory();
                                let mut added_ids = Vec::new();
                                for p in &found_paths {
                                    let existing_id = self
                                        .store
                                        .active()
                                        .tracks
                                        .iter()
                                        .find(|t| &t.path == p)
                                        .map(|t| t.id);
                                    if let Some(id) = existing_id {
                                        added_ids.push(id);
                                    } else if let Ok(tr) = crate::metadata::read_track(p, &cover_dir)
                                    {
                                        let id = tr.id;
                                        self.store.active_mut().upsert(tr);
                                        added_ids.push(id);
                                    }
                                }
                                let count = self
                                    .store
                                    .active_mut()
                                    .add_tracks_to_playlist(pl_id, &added_ids);
                                self.mark_library_dirty();
                                self.store.save();
                                self.view = View::Playlist(pl_id);
                                self.set_status(format!(
                                    "Imported playlist \"{name}\" ({count} tracks)"
                                ));
                            }
                            Ok(_) => self.set_status("No existing audio files found in playlist".into()),
                            Err(e) => self.set_status(format!("Playlist import failed: {e}")),
                        }
                    }
                }
                DialogEvent::ExportPlaylist { path, track_ids } => {
                    if let Some(dest) = path {
                        let tracks: Vec<&Track> =
                            track_ids.iter().filter_map(|id| self.track(*id)).collect();
                        match export_playlist(&dest, &tracks) {
                            Ok(()) => self.set_status(format!(
                                "Exported {} tracks to {}",
                                tracks.len(),
                                dest.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("playlist")
                            )),
                            Err(e) => self.set_status(format!("Export failed: {e}")),
                        }
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
        if !cmd && context.input(|i| i.key_pressed(egui::Key::L)) {
            self.show_lyrics = !self.show_lyrics;
            self.sync_lyrics_for_current();
        }
        if !cmd && context.input(|i| i.key_pressed(egui::Key::OpenBracket)) {
            self.set_loop_a();
        }
        if !cmd && context.input(|i| i.key_pressed(egui::Key::CloseBracket)) {
            self.set_loop_b();
        }
        if !cmd && context.input(|i| i.key_pressed(egui::Key::Backslash)) {
            self.clear_ab_loop();
            self.set_status("A-B Loop cleared".into());
        }
        if cmd && context.input(|i| i.modifiers.shift && i.key_pressed(egui::Key::M)) {
            let next = !self.compact_mode;
            self.set_compact_mode(context, next);
            return;
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

    /// Unified 32px top window bar for Classic Mode — matches Compact Mode's chrome geometry and visual language.
    fn classic_top_bar(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let p = self.palette;
        let avail_w = ui.available_width();
        let (bar_rect, _) = ui.allocate_exact_size(vec2(avail_w, 32.0), Sense::hover());

        let is_focused = ctx.input(|i| i.focused);
        let is_maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let is_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        let alt_held = ctx.input(|i| i.modifiers.alt || i.modifiers.shift);

        // Background + bottom 1px hairline
        ui.painter().rect_filled(bar_rect, 0.0, p.sidebar);
        ui.painter().line_segment(
            [pos2(bar_rect.left(), bar_rect.bottom()), pos2(bar_rect.right(), bar_rect.bottom())],
            Stroke::new(1.0, p.border),
        );

        let cy = bar_rect.center().y;
        let dot_r = 5.5;
        let start_x = bar_rect.left() + 16.0;
        let spacing = 19.0;

        let close_rect = Rect::from_center_size(pos2(start_x, cy), Vec2::splat(16.0));
        let min_rect = Rect::from_center_size(pos2(start_x + spacing, cy), Vec2::splat(16.0));
        let zoom_rect =
            Rect::from_center_size(pos2(start_x + spacing * 2.0, cy), Vec2::splat(16.0));

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

        let cluster_rect = Rect::from_min_max(close_rect.min, zoom_rect.max).expand(2.0);
        let cluster_hovered = ctx
            .pointer_hover_pos()
            .is_some_and(|pt| cluster_rect.contains(pt));

        paint_traffic_dot(
            ui.painter(),
            close_rect.center(),
            dot_r,
            p.danger,
            is_focused,
            cluster_hovered,
            close_resp.hovered(),
            0,
            &p,
        );
        paint_traffic_dot(
            ui.painter(),
            min_rect.center(),
            dot_r,
            p.hires_gold,
            is_focused,
            cluster_hovered,
            min_resp.hovered(),
            1,
            &p,
        );
        paint_traffic_dot(
            ui.painter(),
            zoom_rect.center(),
            dot_r,
            p.accent,
            is_focused,
            cluster_hovered,
            zoom_resp.hovered(),
            2,
            &p,
        );

        // Brand mark + wordmark + version aligned cleanly right of traffic dots
        let mark_r = Rect::from_center_size(pos2(zoom_rect.right() + 18.0, cy), Vec2::splat(17.0));
        self.paint_brand_icon(ui.painter(), mark_r);
        let tx_g = ui.painter().layout_no_wrap(
            "Tx".into(),
            FontId::proportional(12.5),
            p.accent,
        );
        let tx_pos = pos2(mark_r.right() + 7.0, cy - tx_g.size().y * 0.5);
        let tx_w = tx_g.size().x;
        ui.painter().galley(tx_pos, tx_g, p.accent);
        let pl_g = ui.painter().layout_no_wrap(
            "Player".into(),
            FontId::proportional(12.5),
            p.text,
        );
        let pl_pos = pos2(tx_pos.x + tx_w, cy - pl_g.size().y * 0.5);
        let pl_w = pl_g.size().x;
        ui.painter().galley(pl_pos, pl_g, p.text);
        ui.painter().text(
            pos2(pl_pos.x + pl_w + 6.0, cy),
            Align2::LEFT_CENTER,
            format!("v{}", crate::APP_VERSION),
            FontId::monospace(9.5),
            p.faint,
        );

        // Right-aligned global utility cluster (EXACT same right-to-left order as Compact Mode!):
        // [Cmd+K] [Visualizer] [Lyrics] [Settings] [Pin] | [Compact]
        let mut rx = bar_rect.right() - 8.0;

        let mode_r = Rect::from_center_size(pos2(rx - 39.0, cy), vec2(78.0, 22.0));
        rx = mode_r.left() - 6.0;

        // Subtle vertical divider between utility icons and Mode Switch pill
        ui.painter().line_segment(
            [pos2(rx, bar_rect.top() + 8.0), pos2(rx, bar_rect.bottom() - 8.0)],
            Stroke::new(1.0, p.border),
        );
        rx -= 6.0;

        let pin_r = Rect::from_center_size(pos2(rx - 10.5, cy), Vec2::splat(21.0));
        rx = pin_r.left() - 2.0;
        let set_r = Rect::from_center_size(pos2(rx - 10.5, cy), Vec2::splat(21.0));
        rx = set_r.left() - 2.0;
        let lyr_r = Rect::from_center_size(pos2(rx - 10.5, cy), Vec2::splat(21.0));
        rx = lyr_r.left() - 2.0;
        let vis_r = Rect::from_center_size(pos2(rx - 10.5, cy), Vec2::splat(21.0));
        rx = vis_r.left() - 4.0;
        let cmd_r = Rect::from_center_size(pos2(rx - 26.0, cy), vec2(52.0, 22.0));

        let mode_clicked = paint_mode_switch_pill(
            ui,
            mode_r,
            "cls_to_compact",
            "Compact",
            Icon::Compact,
            &format!("Switch to Compact Mode ({MOD_SHORT}Shift+M)"),
            &p,
        );

        // Command palette quick pill
        let cmd_resp = ui
            .interact(cmd_r, ui.scope_id().with("cls_cmd_k"), Sense::click())
            .on_hover_text(format!("Command Palette ({MOD_SHORT}K)"));
        ui.painter().rect_filled(
            cmd_r,
            5.0,
            if cmd_resp.hovered() { p.hover } else { p.surface },
        );
        ui.painter().rect_stroke(
            cmd_r,
            5.0,
            Stroke::new(1.0, if cmd_resp.hovered() { p.border_strong } else { p.border }),
            StrokeKind::Inside,
        );
        icons::paint_icon(
            ui.painter(),
            Rect::from_center_size(pos2(cmd_r.left() + 11.0, cy), Vec2::splat(10.5)),
            Icon::Search,
            if cmd_resp.hovered() { p.text } else { p.muted },
        );
        ui.painter().text(
            pos2(cmd_r.left() + 20.0, cy),
            Align2::LEFT_CENTER,
            format!("{MOD_SHORT}K"),
            FontId::monospace(9.5),
            if cmd_resp.hovered() { p.text } else { p.muted },
        );

        let vis_clicked = paint_topbar_icon_btn(
            ui,
            vis_r,
            "cls_vis",
            Icon::Visualizer,
            self.show_concert_visualizer,
            "Toggle Visualizer (V)",
            &p,
        );
        let lyr_clicked = paint_topbar_icon_btn(
            ui,
            lyr_r,
            "cls_lyr",
            Icon::Lyrics,
            self.show_lyrics,
            "Toggle Synchronized Lyrics (L)",
            &p,
        );
        let set_clicked = paint_topbar_icon_btn(
            ui,
            set_r,
            "cls_set",
            Icon::Settings,
            self.show_settings,
            &format!("Settings ({MOD_SHORT},)"),
            &p,
        );
        let pin_clicked = paint_topbar_pin_btn(ui, pin_r, "cls_pin", self.always_on_top, &p);

        // Non-overlapping drag zone between traffic lights and right-hand cluster
        let drag_rect = Rect::from_min_max(
            pos2(zoom_rect.right() + 6.0, bar_rect.top()),
            pos2((cmd_r.left() - 6.0).max(zoom_rect.right() + 24.0), bar_rect.bottom()),
        );
        let drag_resp = ui.interact(
            drag_rect,
            ui.scope_id().with("cls_titlebar_drag"),
            Sense::click_and_drag(),
        );

        if close_resp.clicked() {
            if self.close_to_tray {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        } else if min_resp.clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        } else if zoom_resp.clicked() {
            if is_fullscreen || alt_held {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_fullscreen));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
            }
        } else if mode_clicked {
            self.set_compact_mode(ctx, true);
        } else if cmd_resp.clicked() {
            self.command_palette.open_fresh();
        } else if vis_clicked {
            self.show_concert_visualizer = !self.show_concert_visualizer;
        } else if lyr_clicked {
            self.show_lyrics = !self.show_lyrics;
            self.sync_lyrics_for_current();
        } else if set_clicked {
            self.show_settings = !self.show_settings;
        } else if pin_clicked {
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
        } else if drag_resp.double_clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maximized));
        } else if should_begin_window_drag(ctx, &drag_resp) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }

    fn sidebar(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        self.ensure_sidebar_stats();
        let p = self.palette;
        ui.add_space(10.0);
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
        let footer_reserve = if self.scanning { 76.0 } else { 40.0 };
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

                // SMART PLAYLISTS
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(RichText::new("SMART").size(9.5).color(p.faint).strong());
                });
                let hires_count = self.cached_hires_count;
                let most_played_count = self.cached_most_played_count;
                let top_rated_count = self.cached_top_rated_count;
                self.nav_row(
                    ui,
                    "Hi-Res Masters",
                    Icon::Sliders,
                    hires_count,
                    self.view == View::Smart(SmartView::HiRes),
                    View::Smart(SmartView::HiRes),
                );
                self.nav_row(
                    ui,
                    "Recently Added",
                    Icon::Refresh,
                    all_count.min(100),
                    self.view == View::Smart(SmartView::RecentlyAdded),
                    View::Smart(SmartView::RecentlyAdded),
                );
                self.nav_row(
                    ui,
                    "Most Played",
                    Icon::Play,
                    most_played_count,
                    self.view == View::Smart(SmartView::MostPlayed),
                    View::Smart(SmartView::MostPlayed),
                );
                self.nav_row(
                    ui,
                    "Top Rated",
                    Icon::StarFilled,
                    top_rated_count,
                    self.view == View::Smart(SmartView::TopRated),
                    View::Smart(SmartView::TopRated),
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
                        if icons::icon_button(
                            ui,
                            Vec2::splat(20.0),
                            Icon::FolderPlus,
                            11.5,
                            p.muted,
                            p.text,
                            p.hover,
                            "Import .m3u / .m3u8 / .pls playlist",
                        )
                        .clicked()
                        {
                            self.import_playlist_dialog();
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

        // Bottom Footer: Quiet library summary + Theme cycle + Directories
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                let total_tracks = self.store.active().tracks.len();
                let hires = self.cached_hires_count;
                let summary = if hires > 0 {
                    format!("{total_tracks} tracks · {hires} Hi-Res")
                } else {
                    format!("{total_tracks} tracks")
                };
                ui.label(RichText::new(summary).size(10.0).color(p.faint));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(10.0);
                    if icons::icon_button(
                        ui,
                        Vec2::splat(22.0),
                        Icon::Folder,
                        13.0,
                        p.muted,
                        p.text,
                        p.hover,
                        "Manage Directories",
                    )
                    .clicked()
                    {
                        self.show_folders = true;
                    }
                    if icons::icon_button(
                        ui,
                        Vec2::splat(22.0),
                        Icon::Palette,
                        13.0,
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
                });
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
                    self.album_filter = None;
                    self.artist_filter = None;
                }
                self.clear_selection();
            }
        });
    }

    fn main_view(&mut self, ui: &mut Ui) {
        self.ensure_visible_cache();
        let p = self.palette;
        let tracks = Arc::clone(&self.cached_visible);
        let total_duration = self.cached_total_duration;

        ui.add_space(12.0);
        // Header Row (view-local controls only; global chrome lives in unified classic_top_bar)
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
                View::Smart(smart) => smart.label().to_string(),
            };
            let title_resp = ui.label(RichText::new(title).size(20.0).strong().color(p.text));
            if let Some(folder) = &self.folder_filter {
                title_resp.on_hover_text(format!("Directory: {}", folder.display()));
            }

            if self.view == View::Library
                && (self.folder_filter.is_some()
                    || self.album_filter.is_some()
                    || self.artist_filter.is_some())
                && pill_button(ui, "× All Tracks", None, PillStyle::Active, &p).clicked()
            {
                self.folder_filter = None;
                self.album_filter = None;
                self.artist_filter = None;
            }

            if let View::Playlist(pl_id) = self.view
                && let Some(pl) = self.store.active().playlist(pl_id).cloned()
                && !pl.track_ids.is_empty()
                && pill_button(ui, "Export", Some(Icon::External), PillStyle::Secondary, &p)
                    .on_hover_text("Export playlist to .m3u8 or .pls")
                    .clicked()
            {
                self.export_playlist_dialog(&pl.name, pl.track_ids);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Strictly aligned 24px Filter Library input (exact match to pill_button height!)
                let search_w = 216.0;
                let (search_rect, _) = ui.allocate_exact_size(vec2(search_w, 24.0), Sense::hover());
                let search_has_focus = ui.ctx().memory(|m| m.focused()).is_some_and(|id| {
                    id == ui.scope_id().with("library_filter_input")
                });
                ui.painter().rect_filled(search_rect, 6.0, p.surface);
                ui.painter().rect_stroke(
                    search_rect,
                    6.0,
                    Stroke::new(
                        1.0,
                        if search_has_focus {
                            p.accent
                        } else {
                            p.border
                        },
                    ),
                    StrokeKind::Inside,
                );
                let icon_rect = Rect::from_center_size(
                    pos2(search_rect.left() + 13.0, search_rect.center().y),
                    Vec2::splat(12.0),
                );
                icons::paint_icon(
                    ui.painter(),
                    icon_rect,
                    Icon::Search,
                    if search_has_focus { p.accent } else { p.faint },
                );
                let clear_pad = if self.search.is_empty() { 6.0 } else { 22.0 };
                let edit_rect = Rect::from_min_max(
                    pos2(search_rect.left() + 24.0, search_rect.top()),
                    pos2(search_rect.right() - clear_pad, search_rect.bottom()),
                );
                let mut child_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(edit_rect)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                let search = child_ui.add_sized(
                    edit_rect.size(),
                    egui::TextEdit::singleline(&mut self.search)
                        .id(ui.scope_id().with("library_filter_input"))
                        .frame(Frame::NONE)
                        .font(FontId::proportional(11.5))
                        .align(Align2::LEFT_CENTER)
                        .hint_text(
                            RichText::new(format!("Filter library… ({MOD_SHORT}F)"))
                                .size(11.0)
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
                if !self.search.is_empty() {
                    let clear_r = Rect::from_center_size(
                        pos2(search_rect.right() - 12.0, search_rect.center().y),
                        Vec2::splat(16.0),
                    );
                    let clear_resp = ui
                        .interact(clear_r, ui.scope_id().with("library_filter_clear"), Sense::click())
                        .on_hover_text("Clear filter");
                    if clear_resp.hovered() {
                        ui.painter().rect_filled(clear_r, 4.0, p.hover);
                    }
                    icons::paint_icon(
                        ui.painter(),
                        Rect::from_center_size(clear_r.center(), Vec2::splat(9.0)),
                        Icon::Close,
                        if clear_resp.hovered() { p.text } else { p.muted },
                    );
                    if clear_resp.clicked() {
                        self.search.clear();
                    }
                }

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
            if let Some((ref alb, ref art)) = self.album_filter {
                ui.label(
                    RichText::new(format!("·  Album: {alb} ({art})"))
                        .color(p.accent)
                        .size(10.5),
                );
            } else if let Some(ref art) = self.artist_filter {
                ui.label(
                    RichText::new(format!("·  Artist: {art}"))
                        .color(p.accent)
                        .size(10.5),
                );
            } else if let Some(folder) = &self.folder_filter {
                ui.label(
                    RichText::new(format!("·  {}", folder.display()))
                        .color(p.faint)
                        .size(10.0)
                        .monospace(),
                );
            }

            // Library Browse Mode switcher (Tracks | Albums | Artists | Folders)
            if self.view == View::Library {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    for (mode, label, icon) in [
                        (BrowseMode::Folders, "Folders", Icon::Folder),
                        (BrowseMode::Artists, "Artists", Icon::Playlist),
                        (BrowseMode::Albums, "Albums", Icon::Grid),
                        (BrowseMode::Tracks, "Tracks", Icon::List),
                    ] {
                        if pill_button(
                            ui,
                            label,
                            Some(icon),
                            if self.browse_mode == mode {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            &p,
                        )
                        .clicked()
                        {
                            self.browse_mode = mode;
                            if mode != BrowseMode::Tracks {
                                self.album_filter = None;
                                self.artist_filter = None;
                            }
                        }
                    }
                });
            }
        });

        ui.add_space(6.0);

        let vis_reserve = if self.show_concert_visualizer {
            self.concert_visualizer_height + 44.0
        } else {
            0.0
        };
        let list_h = (ui.available_height() - vis_reserve).max(110.0);

        // Render chosen Library Browser mode (or Track Table for Playlists / Queue / Smart / Filtered)
        let show_grid_browser = self.view == View::Library
            && self.album_filter.is_none()
            && self.artist_filter.is_none()
            && self.browse_mode != BrowseMode::Tracks;

        if show_grid_browser {
            match self.browse_mode {
                BrowseMode::Albums => self.albums_grid_view(ui, &tracks, list_h),
                BrowseMode::Artists => self.artists_browser_view(ui, &tracks, list_h),
                BrowseMode::Folders => self.folders_tree_view(ui, &tracks, list_h),
                BrowseMode::Tracks => {}
            }
        } else {
            // Strictly Aligned Column Header (uses exact same TrackColumns as every row!)
            self.aligned_table_header(ui, &tracks);
            let sep_rect = ui.available_rect_before_wrap();
            ui.painter().line_segment(
                [pos2(sep_rect.left(), sep_rect.top()), pos2(sep_rect.right(), sep_rect.top())],
                Stroke::new(1.0, p.border),
            );
            ui.add_space(3.0);

            ScrollArea::vertical()
                .id_salt("aligned_track_table")
                .max_height((list_h - 28.0).max(90.0))
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
        }

        // Expandable Visualizer Stage
        if self.show_concert_visualizer {
            let enabled_modes = self.plugins.visualizers.enabled_modes();
            if !enabled_modes.contains(&self.visualizer.mode) {
                self.visualizer.mode = enabled_modes[0];
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("VISUALIZER")
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

    fn albums_grid_view(&mut self, ui: &mut Ui, tracks: &[Track], max_h: f32) {
        let p = self.palette;
        if tracks.is_empty() {
            self.empty_state(ui);
            return;
        }
        let groups = Arc::clone(&self.cached_albums);

        let card_w = 152.0_f32;
        let card_h = 204.0_f32;
        let gap = 12.0_f32;
        let cols = ((ui.available_width() + gap) / (card_w + gap)).floor().max(1.0) as usize;
        let mut open_album: Option<(String, String)> = None;
        let mut play_album: Option<(String, String)> = None;

        ScrollArea::vertical()
            .id_salt("albums_grid_scroll")
            .max_height(max_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in groups.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.style_mut().spacing.item_spacing.x = gap;
                        for (alb, art, rep, count, hires) in row {
                            let (card_rect, resp) =
                                ui.allocate_exact_size(vec2(card_w, card_h), Sense::click());
                            let hovered = resp.hovered();
                            ui.painter().rect_filled(
                                card_rect,
                                8.0,
                                if hovered { p.hover } else { p.surface },
                            );
                            ui.painter().rect_stroke(
                                card_rect,
                                8.0,
                                Stroke::new(1.0, if hovered { p.border_strong } else { p.border }),
                                StrokeKind::Inside,
                            );

                            let art_rect = Rect::from_min_size(
                                pos2(card_rect.left() + 8.0, card_rect.top() + 8.0),
                                Vec2::splat(card_w - 16.0),
                            );
                            self.paint_cover_in_rect(ui, rep, art_rect);

                            if *hires {
                                let badge = Rect::from_min_size(
                                    pos2(art_rect.left() + 5.0, art_rect.top() + 5.0),
                                    vec2(44.0, 15.0),
                                );
                                ui.painter().rect_filled(badge, 3.0, p.hires_bg);
                                ui.painter().text(
                                    badge.center(),
                                    Align2::CENTER_CENTER,
                                    "HI-RES",
                                    FontId::monospace(8.0),
                                    p.hires_gold,
                                );
                            }

                            // Hover 1-click Play Album circle button
                            let play_btn_r = Rect::from_center_size(
                                pos2(art_rect.right() - 18.0, art_rect.bottom() - 18.0),
                                Vec2::splat(28.0),
                            );
                            let over_play = ui
                                .ctx()
                                .pointer_hover_pos()
                                .is_some_and(|pt| play_btn_r.contains(pt));
                            if hovered {
                                ui.painter().circle_filled(
                                    play_btn_r.center(),
                                    14.0,
                                    if over_play { p.accent_bright } else { p.accent },
                                );
                                icons::paint_icon(
                                    ui.painter(),
                                    Rect::from_center_size(
                                        pos2(play_btn_r.center().x + 1.0, play_btn_r.center().y),
                                        Vec2::splat(13.0),
                                    ),
                                    Icon::Play,
                                    p.on_accent(),
                                );
                            }

                            let text_l = card_rect.left() + 8.0;
                            let text_r = card_rect.right() - 8.0;
                            let title_rect = Rect::from_min_max(
                                pos2(text_l, art_rect.bottom() + 6.0),
                                pos2(text_r, art_rect.bottom() + 24.0),
                            );
                            paint_clipped_cell_text(
                                ui.painter(),
                                title_rect,
                                alb,
                                FontId::proportional(11.5),
                                p.text,
                            );
                            let sub_rect = Rect::from_min_max(
                                pos2(text_l, art_rect.bottom() + 23.0),
                                pos2(text_r, art_rect.bottom() + 39.0),
                            );
                            paint_clipped_cell_text(
                                ui.painter(),
                                sub_rect,
                                &format!("{art} · {count}"),
                                FontId::proportional(10.0),
                                p.muted,
                            );

                            if resp.clicked() {
                                if over_play {
                                    play_album = Some((alb.clone(), art.clone()));
                                } else {
                                    open_album = Some((alb.clone(), art.clone()));
                                }
                            }
                        }
                    });
                    ui.add_space(gap);
                }
            });

        if let Some((alb, art)) = play_album {
            let alb_tracks: Vec<Track> = tracks
                .iter()
                .filter(|t| t.album == alb && t.artist == art)
                .cloned()
                .collect();
            if let Some(first) = alb_tracks.first() {
                self.play_from_list(first.id, &alb_tracks);
            }
        } else if let Some((alb, art)) = open_album {
            self.album_filter = Some((alb, art));
            self.browse_mode = BrowseMode::Tracks;
        }
    }

    fn artists_browser_view(&mut self, ui: &mut Ui, tracks: &[Track], max_h: f32) {
        let p = self.palette;
        if tracks.is_empty() {
            self.empty_state(ui);
            return;
        }
        let artists = Arc::clone(&self.cached_artists);

        let mut pick: Option<String> = None;
        ScrollArea::vertical()
            .id_salt("artists_list_scroll")
            .max_height(max_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().spacing.item_spacing.y = 3.0;
                let w = ui.available_width();
                for (name, t_cnt, a_cnt) in artists.iter() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    ui.painter().rect_filled(
                        rect,
                        6.0,
                        if resp.hovered() { p.hover } else { p.surface },
                    );
                    ui.painter().text(
                        pos2(rect.left() + 12.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        name,
                        FontId::proportional(12.5),
                        p.text,
                    );
                    ui.painter().text(
                        pos2(rect.right() - 12.0, rect.center().y),
                        Align2::RIGHT_CENTER,
                        format!(
                            "{a_cnt} {}  ·  {t_cnt} {}",
                            if *a_cnt == 1 { "album" } else { "albums" },
                            if *t_cnt == 1 { "track" } else { "tracks" }
                        ),
                        FontId::monospace(10.0),
                        p.muted,
                    );
                    if resp.clicked() {
                        pick = Some(name.clone());
                    }
                }
            });
        if let Some(art) = pick {
            self.artist_filter = Some(art);
            self.browse_mode = BrowseMode::Tracks;
        }
    }

    fn folders_tree_view(&mut self, ui: &mut Ui, tracks: &[Track], max_h: f32) {
        let p = self.palette;
        if tracks.is_empty() {
            self.empty_state(ui);
            return;
        }
        let dirs_sorted = Arc::clone(&self.cached_dirs);

        let mut filter_dir: Option<PathBuf> = None;
        ScrollArea::vertical()
            .id_salt("folders_tree_scroll")
            .max_height(max_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().spacing.item_spacing.y = 3.0;
                let w = ui.available_width();
                for (dir, cnt) in dirs_sorted.iter() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    ui.painter().rect_filled(
                        rect,
                        6.0,
                        if resp.hovered() { p.hover } else { p.surface },
                    );
                    icons::paint_icon(
                        ui.painter(),
                        Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), Vec2::splat(14.0)),
                        Icon::Folder,
                        p.accent,
                    );
                    let name = dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Directory");
                    ui.painter().text(
                        pos2(rect.left() + 34.0, rect.top() + 12.0),
                        Align2::LEFT_CENTER,
                        name,
                        FontId::proportional(12.0),
                        p.text,
                    );
                    let path_r = Rect::from_min_max(
                        pos2(rect.left() + 34.0, rect.top() + 20.0),
                        pos2(rect.right() - 100.0, rect.bottom() - 2.0),
                    );
                    paint_clipped_cell_text(
                        ui.painter(),
                        path_r,
                        &dir.display().to_string(),
                        FontId::monospace(9.5),
                        p.faint,
                    );
                    ui.painter().text(
                        pos2(rect.right() - 12.0, rect.center().y),
                        Align2::RIGHT_CENTER,
                        format!("{cnt} {}", if *cnt == 1 { "track" } else { "tracks" }),
                        FontId::monospace(10.0),
                        p.muted,
                    );
                    if resp.clicked() {
                        filter_dir = Some(dir.clone());
                    }
                    resp.context_menu(|ui| {
                        if ui
                            .button(format!("Reveal in {FILE_MANAGER_NAME}"))
                            .clicked()
                        {
                            reveal_in_file_manager(dir);
                            ui.close();
                        }
                    });
                }
            });
        if let Some(d) = filter_dir {
            self.folder_filter = Some(d);
            self.browse_mode = BrowseMode::Tracks;
        }
    }

    fn lyrics_panel_contents(&mut self, ui: &mut Ui) {
        self.sync_lyrics_for_current();
        let p = self.palette;
        ui.horizontal(|ui| {
            let (ir, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
            icons::paint_icon(ui.painter(), ir, Icon::Lyrics, p.accent);
            ui.label(RichText::new("Lyrics").size(12.5).strong().color(p.text));
            if let Some(ref lyr) = self.cached_lyrics {
                ui.label(
                    RichText::new(if lyr.synced { "SYNCED" } else { "PLAIN" })
                        .size(8.5)
                        .monospace()
                        .color(p.accent),
                )
                .on_hover_text(&lyr.source_label);
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
                    "Hide Lyrics (L)",
                )
                .clicked()
                {
                    self.show_lyrics = false;
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(6.0);

        let Some(lyrics) = self.cached_lyrics.as_ref().map(Arc::clone) else {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No lyrics found").size(12.0).color(p.muted));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Place a matching .lrc file next to the audio track or embed a LYRICS tag.")
                        .size(10.5)
                        .color(p.faint),
                );
            });
            return;
        };

        let active_idx = lyrics.active_index(self.position);
        let mut seek_to: Option<f64> = None;
        ScrollArea::vertical()
            .id_salt("synced_lyrics_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().spacing.item_spacing.y = 6.0;
                for (i, line) in lyrics.lines.iter().enumerate() {
                    let is_active = active_idx == Some(i);
                    let col = if is_active {
                        p.accent_bright
                    } else if active_idx.is_some_and(|a| i < a) {
                        p.faint
                    } else {
                        p.text_secondary
                    };
                    let size = if is_active { 13.5 } else { 11.5 };
                    let text = if is_active {
                        RichText::new(&line.text).size(size).strong().color(col)
                    } else {
                        RichText::new(&line.text).size(size).color(col)
                    };
                    let resp = ui.add(
                        egui::Label::new(text)
                            .wrap()
                            .sense(if line.time_secs.is_some() {
                                Sense::click()
                            } else {
                                Sense::hover()
                            }),
                    );
                    if is_active && self.playing {
                        resp.scroll_to_me(Some(Align::Center));
                    }
                    if let Some(ts) = line.time_secs
                        && resp
                            .on_hover_text(format!("Jump to {}", format_duration(Some(ts))))
                            .clicked()
                    {
                        seek_to = Some(ts);
                    }
                }
                ui.add_space(24.0);
            });
        if let Some(ts) = seek_to {
            self.audio.seek(ts);
            self.position = ts;
        }
    }

    fn aligned_table_header(&mut self, ui: &mut Ui, _visible: &[Track]) {
        let p = self.palette;
        let sortable = matches!(self.view, View::Library | View::Favorites);
        let row_w = ui.available_width();
        let (row_rect, _) = ui.allocate_exact_size(vec2(row_w, 24.0), Sense::hover());
        let cols = TrackColumns::compute(row_rect);

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
        let (rms_l, rms_r, _, _, _) = self.visual_tap.levels();
        let live_rms = ((rms_l + rms_r) * 1.6).clamp(0.0, 1.0);

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

            // Non-overlapping hit regions: 1-click play | body | (inline quick actions when hovered) | favorite heart
            let body_right = if row_pointer_over {
                q_next_rect.left() - 4.0
            } else {
                cols.favorite.left() - 2.0
            };
            let body_rect = Rect::from_min_max(
                pos2(cols.cover.left(), row_rect.top()),
                pos2(body_right.max(cols.cover.right()), row_rect.bottom()),
            );

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
                        .on_hover_text("Play Next"),
                    );
                    hover_queue_resp = Some(
                        ui.interact(
                            q_queue_rect,
                            ui.scope_id().with(("row_qq", row_idx, track.id)),
                            Sense::click(),
                        )
                        .on_hover_text("Add to Queue"),
                    );
                }
                hover_tag_resp = Some(
                    ui.interact(
                        q_tag_rect,
                        ui.scope_id().with(("row_qt", row_idx, track.id)),
                        Sense::click(),
                    )
                    .on_hover_text("Edit Tags & Lookup"),
                );
            }

            let row_hovered = row_pointer_over
                || play_resp.hovered()
                || fav_resp.hovered()
                || body_resp.hovered();

            // Distinct visual hierarchy:
            // - Playing track: rich accent-tinted fill + bright left accent bar + accent title
            // - Temporarily clicked row: lighter neutral tint + subtle thin neutral border (never confused with playing!)
            let bg = if is_current && is_selected {
                lerp_c(p.playing_row, p.clicked_row, 0.35)
            } else if is_current {
                p.playing_row
            } else if is_selected {
                p.clicked_row
            } else if row_hovered {
                p.hover
            } else if row_idx % 2 == 1 {
                if p.is_light {
                    Color32::from_rgba_unmultiplied(0, 0, 0, 5)
                } else {
                    Color32::from_rgba_unmultiplied(255, 255, 255, 3)
                }
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(row_rect, 5.0, bg);

            if is_current {
                ui.painter().rect_stroke(
                    row_rect,
                    5.0,
                    Stroke::new(
                        1.0,
                        if is_selected {
                            p.accent.gamma_multiply(0.75)
                        } else {
                            p.accent.gamma_multiply(0.45)
                        },
                    ),
                    StrokeKind::Inside,
                );
                let accent_bar = Rect::from_min_max(
                    pos2(row_rect.left() + 1.5, row_rect.top() + 6.0),
                    pos2(row_rect.left() + 4.5, row_rect.bottom() - 6.0),
                );
                ui.painter().rect_filled(accent_bar, 1.5, p.accent);
            } else if is_selected {
                ui.painter().rect_stroke(
                    row_rect,
                    5.0,
                    Stroke::new(1.0, p.border_strong),
                    StrokeKind::Inside,
                );
            }

            // 1) Index / Live Audio Equalizer / 1-Click Hover Play Button
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
                icons::paint_playing_bars(
                    ui.painter(),
                    eq_rect,
                    time,
                    self.playing,
                    live_rms,
                    p.accent,
                );
            } else {
                ui.painter().text(
                    cols.index.center(),
                    Align2::CENTER_CENTER,
                    format!("{:02}", row_idx + 1),
                    FontId::monospace(10.0),
                    if is_selected { p.text_secondary } else { p.faint },
                );
            }

            // 2) Cover Thumbnail (dead-centered)
            let art_size = (row_h - 8.0).clamp(26.0, 32.0);
            let art_rect = Rect::from_center_size(cols.cover.center(), Vec2::splat(art_size));
            self.paint_cover_in_rect(ui, track, art_rect);

            // 3) Title, Artist, Album (strictly clipped inside exact column rects)
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
                if is_current || is_selected {
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

            // 4) Format Badge OR 1-Click Inline Hover Actions (Play Next / Queue / Tags)
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

            // 5) Tabular Monospace Duration (right-aligned)
            ui.painter().text(
                pos2(cols.duration.right() - 2.0, cols.duration.center().y),
                Align2::RIGHT_CENTER,
                format_duration(track.duration),
                FontId::monospace(11.0),
                if is_current { p.accent } else { p.muted },
            );

            // 6) Favorite Heart Button (centered)
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
            if play_resp.clicked() {
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

            // Stylish theme-matched Track Hover Card (complete tags + audio spec + folder & file)
            let body_resp = body_resp.on_hover_ui(|ui| self.paint_track_hover_card(ui, track));

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
                    ui.menu_button("Rating", |ui| {
                        for stars in (1..=5u8).rev() {
                            let lbl = "★".repeat(stars as usize);
                            if ui.button(lbl).clicked() {
                                ctx_action = Some(ContextMenuAction::SetRating(tid, stars));
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui.button("Clear Rating").clicked() {
                            ctx_action = Some(ContextMenuAction::SetRating(tid, 0));
                            ui.close();
                        }
                    });
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
            ContextMenuAction::SetRating(id, stars) => {
                self.store.active_mut().set_rating(id, stars);
                self.mark_library_dirty();
                self.store.save();
            }
            ContextMenuAction::RemoveSelected => self.remove_selected_in_current_view(visible),
        }
    }

    /// Sleek Floating Action Bar for multi-track selection (2+ tracks; single track uses hover & right-click).
    fn floating_selection_dock(&mut self, context: &egui::Context) {
        let count = self.selected_count();
        if count < 2 {
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
                                    &format!("Reveal in {FILE_MANAGER_NAME}"),
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
                        "Your library is empty",
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
                    View::Smart(smart) => (
                        smart.label(),
                        "No tracks match this smart playlist yet.",
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
            // LEFT: Framed Album Art + Metadata + Directory Indicator + Quick Favorite
            ui.allocate_ui_with_layout(
                vec2(left_w, 76.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    if let Some(track) = &track {
                        let parent_dir = track
                            .path
                            .parent()
                            .map(|d| d.display().to_string())
                            .unwrap_or_else(|| track.path.display().to_string());
                        let folder_name = track
                            .path
                            .parent()
                            .and_then(|d| d.file_name())
                            .and_then(|n| n.to_str())
                            .unwrap_or("Folder");

                        let cover_resp = self.cover_clickable(ui, track, 54.0);
                        if cover_resp
                            .on_hover_ui(|ui| self.paint_track_hover_card(ui, track))
                            .clicked()
                        {
                            self.inspect_track_id = Some(track.id);
                        }
                        ui.add_space(6.0);
                        let meta_w = (left_w - 94.0).max(86.0);
                        ui.allocate_ui_with_layout(
                            vec2(meta_w, 56.0),
                            Layout::top_down(Align::LEFT),
                            |ui| {
                                ui.add_space(2.0);
                                ui.style_mut().spacing.item_spacing.y = 1.5;
                                let title_resp = ui.add(
                                    egui::Label::new(
                                        RichText::new(&track.title)
                                            .size(13.0)
                                            .strong()
                                            .color(p.text),
                                    )
                                    .selectable(false)
                                    .truncate(),
                                );
                                title_resp.on_hover_ui(|ui| self.paint_track_hover_card(ui, track));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{} · {}", track.artist, track.album))
                                            .size(10.5)
                                            .color(p.text_secondary),
                                    )
                                    .selectable(false)
                                    .truncate(),
                                );
                                // Subtle clickable directory row so user always sees where audio plays from!
                                ui.horizontal(|ui| {
                                    ui.style_mut().spacing.item_spacing.x = 4.0;
                                    let (fr, _) =
                                        ui.allocate_exact_size(Vec2::splat(11.0), Sense::hover());
                                    icons::paint_icon(ui.painter(), fr, Icon::Folder, p.faint);
                                    let dir_resp = ui
                                        .add(
                                            egui::Label::new(
                                                RichText::new(folder_name)
                                                    .size(10.0)
                                                    .color(p.muted),
                                            )
                                            .sense(Sense::click())
                                            .truncate(),
                                        )
                                        .on_hover_text(format!(
                                            "Playing from:\n{parent_dir}\nClick to reveal in {FILE_MANAGER_NAME}"
                                        ));
                                    if dir_resp.clicked() {
                                        reveal_in_file_manager(&track.path);
                                    }
                                });
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
                            self.mark_library_dirty();
                            self.store.save();
                        }
                    } else {
                        let (art_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
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
                                RichText::new("Select a track to play")
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
                            self.preload_next_track();
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
                            self.preload_next_track();
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
                            (self.loop_a, self.loop_b),
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

            // RIGHT: Clickable Signal Path Telemetry + Compact Output Icon + Volume
            ui.allocate_ui_with_layout(
                vec2(right_w, 76.0),
                Layout::top_down(Align::RIGHT),
                |ui| {
                    ui.add_space(12.0);
                    ui.style_mut().spacing.item_spacing.y = 8.0;

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
                                let dsp_tag = if info.bit_perfect {
                                    " · BIT-PERFECT"
                                } else if dsp_on {
                                    " · DSP"
                                } else {
                                    ""
                                };
                                let spd_tag = if (self.playback_speed - 1.0).abs() > 0.01 {
                                    format!(" · {:.2}x", self.playback_speed)
                                } else {
                                    String::new()
                                };
                                let ab_tag = if self.loop_a.is_some() && self.loop_b.is_some() {
                                    " · A-B"
                                } else {
                                    ""
                                };
                                let sig_resp = ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(format!(
                                                "{bitrate_str}{rate_str}{dsp_tag}{spd_tag}{ab_tag}"
                                            ))
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
                                                "DSP ON"
                                            } else {
                                                "SIGNAL PATH"
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
                            if let Some(new_vol) = custom_volume_bar(ui, 92.0, self.volume, &p) {
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

                            ui.add_space(2.0);
                            let active_dev = self
                                .stream_info
                                .as_ref()
                                .map(|s| s.device_name.as_str())
                                .or(self.preferred_device.as_deref())
                                .unwrap_or("System Default");
                            let dev_resp = icons::icon_button(
                                ui,
                                Vec2::splat(24.0),
                                Icon::Device,
                                14.0,
                                if self.show_device_picker || self.preferred_device.is_some() {
                                    p.accent
                                } else {
                                    p.muted
                                },
                                p.text,
                                p.hover,
                                &format!("Audio Output: {active_dev}\nClick to change device"),
                            );
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
            self.compact_mode,
            &mut self.progress_style,
            &mut self.row_density,
            &mut self.close_to_tray,
            &mut self.auto_watch_folders,
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
        if let Some(cm) = settings_actions.compact_mode_changed {
            self.set_compact_mode(context, cm);
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
            self.preload_next_track();
        }
        if settings_actions.rg_changed {
            self.apply_current_rg();
        }
        if let Some(watch_on) = settings_actions.watch_changed {
            self.watcher.set_enabled(watch_on);
        }
        if settings_actions.import_playlist {
            self.import_playlist_dialog();
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
                let file_size = match self.inspect_file_size {
                    Some((cached_id, sz)) if cached_id == tid => sz,
                    _ => {
                        let sz = track.file_size_bytes();
                        self.inspect_file_size = Some((tid, sz));
                        sz
                    }
                };
                let (open_stay, action) = modals::show_track_info_window(
                    context,
                    &track,
                    file_size,
                    &p,
                    |ui, size| self.cover(ui, &track, size),
                    |ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(140.0, 20.0), Sense::hover());
                        paint_aligned_quality_badge(ui.painter(), r, &track, &p);
                    },
                );
                if !open_stay {
                    self.inspect_track_id = None;
                    self.inspect_file_size = None;
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
                        TrackInfoAction::SetRating(stars) => {
                            self.store.active_mut().set_rating(tid, stars);
                            self.mark_library_dirty();
                            self.store.save();
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

            modals::themed_modal_window(
                context,
                "manage_libraries_modal",
                "Libraries",
                Icon::Library,
                None,
                [440.0, 320.0],
                None,
                false,
                &mut open,
                &p,
                |ui| {
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
            modals::themed_modal_window(
                context,
                "new_playlist_modal",
                "New Playlist",
                Icon::Playlist,
                None,
                [340.0, 128.0],
                None,
                false,
                &mut open,
                &p,
                |ui| {
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
                bit_perfect: info.bit_perfect,
                rg_gain_db: info.rg_gain_db,
            });
            let sig_actions = modals::show_signal_path_popover(
                context,
                &mut self.show_signal_path,
                self.signal_btn_rect,
                TRANSPORT_HEIGHT,
                sig,
                &mut self.plugins,
                self.playback_speed,
                (self.loop_a, self.loop_b),
                &p,
            );
            if sig_actions.dsp_changed {
                self.audio.update_dsp(&self.plugins.dsp);
                self.apply_current_rg();
            }
            if let Some(spd) = sig_actions.speed_changed {
                self.set_playback_speed(spd);
            }
            if sig_actions.set_loop_a {
                self.set_loop_a();
            }
            if sig_actions.set_loop_b {
                self.set_loop_b();
            }
            if sig_actions.clear_loop {
                self.clear_ab_loop();
                self.set_status("A-B Loop cleared".into());
            }
            if sig_actions.open_settings_dsp {
                self.show_signal_path = false;
                self.settings_tab = SettingsTab::Audio;
                self.show_settings = true;
            }
        }

        // 8b) Compact-mode Lyrics Window (in Full mode, Lyrics docks cleanly on the right panel)
        if self.compact_mode && self.show_lyrics {
            let mut open = self.show_lyrics;
            modals::themed_modal_window(
                context,
                "compact_lyrics_modal",
                "Lyrics",
                Icon::Lyrics,
                None,
                [360.0, 380.0],
                Some([280.0, 240.0]),
                true,
                &mut open,
                &p,
                |ui| self.lyrics_panel_contents(ui),
            );
            self.show_lyrics = open;
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
            label: if self.compact_mode {
                "Switch to Classic Mode".into()
            } else {
                "Switch to Compact Mode".into()
            },
            subtitle: format!("Shortcut: {MOD_SHORT}Shift+M"),
            action: PaletteCommand::ToggleCompactMode,
        });
        entries.push(PaletteEntry {
            category: "ACTION",
            label: "Toggle Visualizer".into(),
            subtitle: "Shortcut: V".into(),
            action: PaletteCommand::ToggleVisualizer,
        });
        entries.push(PaletteEntry {
            category: "ACTION",
            label: "Toggle Synchronized Lyrics".into(),
            subtitle: "Shortcut: L".into(),
            action: PaletteCommand::ToggleLyrics,
        });
        entries.push(PaletteEntry {
            category: "ACTION",
            label: "Import Playlist (.m3u / .m3u8 / .pls)…".into(),
            subtitle: "Create playlist from file".into(),
            action: PaletteCommand::ImportPlaylist,
        });
        entries.push(PaletteEntry {
            category: "ACTION",
            label: "Export Current View / Playlist (.m3u8 / .pls)…".into(),
            subtitle: "Save tracks to standard playlist".into(),
            action: PaletteCommand::ExportActivePlaylist,
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
            subtitle: format!("Shortcut: {MOD_SHORT}T"),
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
            PaletteCommand::ToggleCompactMode => {
                let next = !self.compact_mode;
                self.set_compact_mode(ctx, next);
            }
            PaletteCommand::ToggleLyrics => {
                self.show_lyrics = !self.show_lyrics;
                self.sync_lyrics_for_current();
            }
            PaletteCommand::ImportPlaylist => self.import_playlist_dialog(),
            PaletteCommand::ExportActivePlaylist => {
                let visible = self.visible_tracks();
                let ids: Vec<u64> = visible.iter().map(|t| t.id).collect();
                let name = match self.view {
                    View::Playlist(pl_id) => self
                        .store
                        .active()
                        .playlist(pl_id)
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| "Playlist".into()),
                    _ => self.store.active().name.clone(),
                };
                self.export_playlist_dialog(&name, ids);
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

    fn any_modal_open(&self) -> bool {
        self.show_settings
            || self.tag_editor.open
            || self.inspect_track_id.is_some()
            || self.show_folders
            || self.show_libraries_modal
            || self.show_new_playlist_modal
            || (self.compact_mode && self.show_lyrics)
            || self.command_palette.open
    }

    fn set_compact_mode(&mut self, ctx: &egui::Context, compact: bool) {
        self.compact_mode = compact;
        self.compact_modal_expanded = false;
        self.show_device_picker = false;
        self.show_signal_path = false;
        if compact {
            let h = if self.compact_show_playlist || self.any_modal_open() {
                COMPACT_EXPANDED_HEIGHT
            } else {
                COMPACT_COLLAPSED_HEIGHT
            } + if self.show_concert_visualizer { 62.0 } else { 0.0 };
            if !self.compact_show_playlist && self.any_modal_open() {
                self.compact_modal_expanded = true;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(
                COMPACT_MIN_WIDTH,
                COMPACT_MIN_HEIGHT,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(COMPACT_WIDTH, h)));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(vec2(
                FULL_MIN_WIDTH,
                FULL_MIN_HEIGHT,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(
                FULL_WIDTH,
                FULL_HEIGHT,
            )));
        }
    }

    fn sync_compact_height(&self, ctx: &egui::Context) {
        if !self.compact_mode {
            return;
        }
        let cur_w = ctx.content_rect().width().max(COMPACT_MIN_WIDTH);
        let target_h = if self.compact_show_playlist || self.compact_modal_expanded {
            COMPACT_EXPANDED_HEIGHT
        } else {
            COMPACT_COLLAPSED_HEIGHT
        } + if self.show_concert_visualizer { 62.0 } else { 0.0 };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(cur_w, target_h)));
    }

    /// Winamp / AIMP2-inspired Compact Mini-Player with unified 32px chrome, live VBR telemetry bar,
    /// clear Classic Mode switch, and optional collapsible playlist drawer.
    fn compact_view(&mut self, ctx: &egui::Context, ui: &mut Ui) {
        let p = self.palette;
        let track = self.current_track().cloned();
        let avail_w = ui.available_width();

        // 1) Unified 32px custom draggable top bar (exact height & visual grammar as Classic Mode)
        let (hdr_rect, _) = ui.allocate_exact_size(vec2(avail_w, 32.0), Sense::hover());
        let is_focused = ctx.input(|i| i.focused);
        ui.painter().rect_filled(hdr_rect, 0.0, p.sidebar);
        ui.painter().line_segment(
            [pos2(hdr_rect.left(), hdr_rect.bottom()), pos2(hdr_rect.right(), hdr_rect.bottom())],
            Stroke::new(1.0, p.border),
        );

        let cy = hdr_rect.center().y;
        let close_r = Rect::from_center_size(pos2(hdr_rect.left() + 15.0, cy), Vec2::splat(16.0));
        let min_r = Rect::from_center_size(pos2(hdr_rect.left() + 34.0, cy), Vec2::splat(16.0));
        let close_resp = ui
            .interact(close_r, ui.scope_id().with("cmp_close"), Sense::click())
            .on_hover_text("Close");
        let min_resp = ui
            .interact(min_r, ui.scope_id().with("cmp_min"), Sense::click())
            .on_hover_text("Minimize");

        let cluster_rect = Rect::from_min_max(close_r.min, min_r.max).expand(2.0);
        let cluster_hovered = ctx
            .pointer_hover_pos()
            .is_some_and(|pt| cluster_rect.contains(pt));

        paint_traffic_dot(
            ui.painter(),
            close_r.center(),
            5.5,
            p.danger,
            is_focused,
            cluster_hovered,
            close_resp.hovered(),
            0,
            &p,
        );
        paint_traffic_dot(
            ui.painter(),
            min_r.center(),
            5.5,
            p.hires_gold,
            is_focused,
            cluster_hovered,
            min_resp.hovered(),
            1,
            &p,
        );

        // Brand mini mark + dual-tone wordmark + version
        let mark_r = Rect::from_center_size(pos2(min_r.right() + 14.0, cy), Vec2::splat(16.0));
        self.paint_brand_icon(ui.painter(), mark_r);
        let tx_g = ui.painter().layout_no_wrap(
            "Tx".into(),
            FontId::proportional(12.0),
            p.accent,
        );
        let tx_pos = pos2(mark_r.right() + 6.0, cy - tx_g.size().y * 0.5);
        let tx_w = tx_g.size().x;
        ui.painter().galley(tx_pos, tx_g, p.accent);
        let pl_g = ui.painter().layout_no_wrap(
            "Player".into(),
            FontId::proportional(12.0),
            p.text,
        );
        let pl_pos = pos2(tx_pos.x + tx_w, cy - pl_g.size().y * 0.5);
        let pl_w = pl_g.size().x;
        ui.painter().galley(pl_pos, pl_g, p.text);
        ui.painter().text(
            pos2(pl_pos.x + pl_w + 5.0, cy),
            Align2::LEFT_CENTER,
            format!("v{}", crate::APP_VERSION),
            FontId::monospace(9.0),
            p.faint,
        );

        // Top-right cluster (EXACT same right-to-left order as Classic Mode!):
        // [List] [Visualizer] [Lyrics] [Settings] [Pin] | [Classic]
        let mut rx = hdr_rect.right() - 8.0;
        let btn_classic_r = Rect::from_center_size(pos2(rx - 39.0, cy), vec2(78.0, 22.0));
        rx = btn_classic_r.left() - 6.0;
        ui.painter().line_segment(
            [pos2(rx, hdr_rect.top() + 8.0), pos2(rx, hdr_rect.bottom() - 8.0)],
            Stroke::new(1.0, p.border),
        );
        rx -= 6.0;

        let btn_pin_r = Rect::from_center_size(pos2(rx - 10.5, cy), Vec2::splat(21.0));
        rx = btn_pin_r.left() - 2.0;
        let btn_set_r = Rect::from_center_size(pos2(rx - 10.5, cy), Vec2::splat(21.0));
        rx = btn_set_r.left() - 2.0;
        let btn_lyr_r = Rect::from_center_size(pos2(rx - 10.5, cy), Vec2::splat(21.0));
        rx = btn_lyr_r.left() - 2.0;
        let btn_vis_r = Rect::from_center_size(pos2(rx - 10.5, cy), Vec2::splat(21.0));
        rx = btn_vis_r.left() - 2.0;
        let btn_list_r = Rect::from_center_size(pos2(rx - 10.5, cy), Vec2::splat(21.0));

        let pin_clicked = paint_topbar_pin_btn(ui, btn_pin_r, "cmp_pin", self.always_on_top, &p);
        let lyr_clicked = paint_topbar_icon_btn(
            ui,
            btn_lyr_r,
            "cmp_lyr",
            Icon::Lyrics,
            self.show_lyrics,
            "Toggle Lyrics (L)",
            &p,
        );
        let vis_clicked = paint_topbar_icon_btn(
            ui,
            btn_vis_r,
            "cmp_vis",
            Icon::Visualizer,
            self.show_concert_visualizer,
            "Toggle Visualizer (V)",
            &p,
        );
        let list_clicked = paint_topbar_icon_btn(
            ui,
            btn_list_r,
            "cmp_list",
            Icon::List,
            self.compact_show_playlist,
            if self.compact_show_playlist {
                "Hide Playlist Drawer"
            } else {
                "Show Playlist Drawer"
            },
            &p,
        );
        let set_clicked = paint_topbar_icon_btn(
            ui,
            btn_set_r,
            "cmp_set",
            Icon::Settings,
            self.show_settings,
            &format!("Settings ({MOD_SHORT},)"),
            &p,
        );
        let classic_clicked = paint_mode_switch_pill(
            ui,
            btn_classic_r,
            "cmp_to_classic",
            "Classic",
            Icon::Expand,
            &format!("Switch to Classic Mode ({MOD_SHORT}Shift+M)"),
            &p,
        );

        let hdr_drag_rect = Rect::from_min_max(
            pos2(min_r.right() + 6.0, hdr_rect.top()),
            pos2((btn_list_r.left() - 6.0).max(min_r.right() + 24.0), hdr_rect.bottom()),
        );
        let hdr_resp = ui.interact(
            hdr_drag_rect,
            ui.scope_id().with("cmp_titlebar_drag"),
            Sense::click_and_drag(),
        );

        if close_resp.clicked() {
            if self.close_to_tray {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        } else if min_resp.clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        } else if pin_clicked {
            self.always_on_top = !self.always_on_top;
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if self.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));
        } else if lyr_clicked {
            self.show_lyrics = !self.show_lyrics;
            self.sync_lyrics_for_current();
        } else if vis_clicked {
            self.show_concert_visualizer = !self.show_concert_visualizer;
            self.sync_compact_height(ctx);
        } else if list_clicked {
            self.compact_show_playlist = !self.compact_show_playlist;
            self.sync_compact_height(ctx);
        } else if set_clicked {
            self.show_settings = !self.show_settings;
        } else if classic_clicked || hdr_resp.double_clicked() {
            self.set_compact_mode(ctx, false);
            return;
        } else if should_begin_window_drag(ctx, &hdr_resp) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        // 2) Main Compact Player Deck
        Frame::new()
            .fill(p.panel)
            .inner_margin(Margin::symmetric(12, 9))
            .show(ui, |ui| {
                // Row A: Crisp Cover + 2-Line Title / Artist·Album + Favorite Heart
                ui.horizontal(|ui| {
                    if let Some(ref t) = track {
                        if self
                            .cover_clickable(ui, t, 42.0)
                            .on_hover_ui(|ui| self.paint_track_hover_card(ui, t))
                            .clicked()
                        {
                            self.inspect_track_id = Some(t.id);
                        }
                        ui.add_space(6.0);
                        let meta_w = (ui.available_width() - 32.0).max(120.0);
                        ui.allocate_ui_with_layout(
                            vec2(meta_w, 42.0),
                            Layout::top_down(Align::LEFT),
                            |ui| {
                                ui.add_space(3.0);
                                ui.style_mut().spacing.item_spacing.y = 2.0;
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&t.title)
                                            .size(13.5)
                                            .strong()
                                            .color(p.text),
                                    )
                                    .selectable(false)
                                    .truncate(),
                                )
                                .on_hover_ui(|ui| self.paint_track_hover_card(ui, t));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{} · {}", t.artist, t.album))
                                            .size(10.5)
                                            .color(p.text_secondary),
                                    )
                                    .selectable(false)
                                    .truncate(),
                                );
                            },
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let fav = self.store.active().favorites.contains(&t.id);
                            if icons::icon_button(
                                ui,
                                Vec2::splat(24.0),
                                if fav {
                                    Icon::HeartFilled
                                } else {
                                    Icon::HeartOutline
                                },
                                14.0,
                                if fav { p.accent } else { p.faint },
                                p.accent_bright,
                                p.hover,
                                if fav { "Unfavorite" } else { "Favorite" },
                            )
                            .clicked()
                            {
                                self.store.active_mut().toggle_favorite(t.id);
                                self.mark_library_dirty();
                                self.store.save();
                            }
                        });
                    } else {
                        let (art_r, _) = ui.allocate_exact_size(Vec2::splat(42.0), Sense::hover());
                        self.paint_brand_icon(ui.painter(), art_r);
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("TxPlayer Ready")
                                    .size(13.0)
                                    .strong()
                                    .color(p.text_secondary),
                            );
                            ui.label(
                                RichText::new("Drop files or pick a track below")
                                    .size(10.0)
                                    .color(p.faint),
                            );
                        });
                    }
                });

                ui.add_space(6.0);
                // Row A2: Recessed Audiophile Live Telemetry Bar (live VBR/CBR kbps + codec + rate + folder)
                let (bar_rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
                ui.painter().rect_filled(bar_rect, 5.0, p.background);
                ui.painter().rect_stroke(
                    bar_rect,
                    5.0,
                    Stroke::new(1.0, p.border),
                    StrokeKind::Inside,
                );

                // Split bar cleanly: left = live stream/VBR telemetry, right = playing folder chip
                let folder_w = if track.is_some() { 112.0 } else { 0.0 };
                let tele_rect = Rect::from_min_max(
                    pos2(bar_rect.left() + 6.0, bar_rect.top()),
                    pos2((bar_rect.right() - folder_w - 6.0).max(bar_rect.left() + 80.0), bar_rect.bottom()),
                );
                let tele_resp = ui
                    .interact(tele_rect, ui.scope_id().with("cmp_live_tele"), Sense::click())
                    .on_hover_text(if let Some(info) = &self.stream_info {
                        format!(
                            "{} · {} ch · {} → {}\nClick for Signal Path & Live DSP",
                            info.device_name,
                            info.channels,
                            format_sample_rate(info.source_rate),
                            format_sample_rate(info.output_rate)
                        )
                    } else {
                        "Click to open Signal Path & Live DSP".to_string()
                    });
                self.signal_btn_rect = Some(tele_rect);
                if tele_resp.clicked() {
                    self.show_signal_path = !self.show_signal_path;
                    self.show_device_picker = false;
                }

                let (dot_col, tele_text, tele_fg) = if let Some(info) = &self.stream_info {
                    let native = info.source_rate == info.output_rate;
                    let dsp_on = self.plugins.dsp.is_actively_processing();
                    let dot = if dsp_on {
                        p.accent_secondary
                    } else if native {
                        p.accent
                    } else {
                        p.hires_gold
                    };
                    let codec = track
                        .as_ref()
                        .map(|t| format!("{} · ", t.codec))
                        .unwrap_or_default();
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
                            "{bits}{}→{}",
                            format_sample_rate(info.source_rate),
                            format_sample_rate(info.output_rate)
                        )
                    };
                    let tag = if info.bit_perfect {
                        " · BIT-PERFECT"
                    } else if dsp_on {
                        " · DSP"
                    } else {
                        ""
                    };
                    let spd = if (self.playback_speed - 1.0).abs() > 0.01 {
                        format!(" · {:.2}x", self.playback_speed)
                    } else {
                        String::new()
                    };
                    let ab = if self.loop_a.is_some() && self.loop_b.is_some() {
                        " · A-B"
                    } else {
                        ""
                    };
                    let fg = if self.show_signal_path || tele_resp.hovered() {
                        p.accent
                    } else {
                        p.text_secondary
                    };
                    (dot, format!("{codec}{bitrate_str}{rate_str}{tag}{spd}{ab}"), fg)
                } else if let Some(ref t) = track {
                    let dot = if t.is_hires() { p.hires_gold } else { p.faint };
                    (dot, t.quality_full(), if t.is_hires() { p.hires_gold } else { p.muted })
                } else {
                    let dsp_on = self.plugins.dsp.enabled;
                    (
                        if dsp_on { p.accent_secondary } else { p.faint },
                        if dsp_on {
                            "DSP ACTIVE · SIGNAL PATH".to_string()
                        } else {
                            "BIT-DIRECT ENGINE · READY".to_string()
                        },
                        p.faint,
                    )
                };

                ui.painter()
                    .circle_filled(pos2(tele_rect.left() + 4.0, tele_rect.center().y), 2.8, dot_col);
                let tele_text_r = Rect::from_min_max(
                    pos2(tele_rect.left() + 12.0, tele_rect.top()),
                    tele_rect.max,
                );
                ui.painter().with_clip_rect(tele_text_r).text(
                    pos2(tele_text_r.left(), tele_text_r.center().y),
                    Align2::LEFT_CENTER,
                    tele_text,
                    FontId::monospace(9.5),
                    tele_fg,
                );

                if let Some(ref t) = track {
                    let parent_dir = t
                        .path
                        .parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_else(|| t.path.display().to_string());
                    let folder_name = t
                        .path
                        .parent()
                        .and_then(|d| d.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("Folder");
                    let div_x = bar_rect.right() - folder_w - 2.0;
                    ui.painter().line_segment(
                        [pos2(div_x, bar_rect.top() + 4.0), pos2(div_x, bar_rect.bottom() - 4.0)],
                        Stroke::new(1.0, p.border),
                    );
                    let f_rect = Rect::from_min_max(
                        pos2(div_x + 4.0, bar_rect.top()),
                        pos2(bar_rect.right() - 5.0, bar_rect.bottom()),
                    );
                    let f_resp = ui
                        .interact(f_rect, ui.scope_id().with("cmp_folder_chip"), Sense::click())
                        .on_hover_text(format!(
                            "Playing from:\n{parent_dir}\nClick to reveal in {FILE_MANAGER_NAME}"
                        ));
                    icons::paint_icon(
                        ui.painter(),
                        Rect::from_center_size(pos2(f_rect.left() + 6.0, f_rect.center().y), Vec2::splat(10.0)),
                        Icon::Folder,
                        if f_resp.hovered() { p.accent } else { p.faint },
                    );
                    let f_txt = Rect::from_min_max(pos2(f_rect.left() + 14.0, f_rect.top()), f_rect.max);
                    ui.painter().with_clip_rect(f_txt).text(
                        pos2(f_txt.left(), f_txt.center().y),
                        Align2::LEFT_CENTER,
                        folder_name,
                        FontId::proportional(9.5),
                        if f_resp.hovered() { p.text } else { p.muted },
                    );
                    if f_resp.clicked() {
                        reveal_in_file_manager(&t.path);
                    }
                }

                ui.add_space(6.0);
                // Row B: Hi-Def Uniform Progress Scrubber
                let duration = self.duration.max(0.0);
                let active = self.current_id().is_some() && duration > 0.01;
                let waveform = self.current_id().and_then(|id| self.waveforms.get(&id));
                let full_w = ui.available_width();
                let time_w = 38.0;
                let bar_w = (full_w - time_w * 2.0 - 12.0).max(120.0);
                ui.horizontal(|ui| {
                    ui.style_mut().spacing.item_spacing.x = 6.0;
                    ui.allocate_ui_with_layout(
                        vec2(time_w, 26.0),
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(format_duration(Some(self.position)))
                                    .size(10.0)
                                    .monospace()
                                    .color(p.muted),
                            );
                        },
                    );
                    if let Some(new_pos) = acoustic_timeline_scrubber(
                        ui,
                        bar_w,
                        self.position,
                        duration,
                        active,
                        waveform.map(Vec::as_slice),
                        self.progress_style,
                        (self.loop_a, self.loop_b),
                        &p,
                    ) {
                        self.audio.seek(new_pos);
                        self.position = new_pos;
                    }
                    ui.allocate_ui_with_layout(
                        vec2(time_w, 26.0),
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
                            if ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(right_text)
                                            .size(10.0)
                                            .monospace()
                                            .color(p.muted),
                                    )
                                    .sense(Sense::click()),
                                )
                                .on_hover_text("Click to toggle remaining / total time")
                                .clicked()
                            {
                                self.show_remaining = !self.show_remaining;
                            }
                        },
                    );
                });

                ui.add_space(6.0);
                // Row C: Compact Tactile Transport (left) + Output Icon + Mute + Volume (right, matching Classic!)
                ui.horizontal(|ui| {
                    ui.style_mut().spacing.item_spacing.x = 5.0;
                    if transport_toggle_button(ui, Icon::Shuffle, self.shuffle, "Shuffle", &p)
                        .clicked()
                    {
                        self.shuffle = !self.shuffle;
                        self.preload_next_track();
                    }
                    if icons::icon_button(
                        ui,
                        Vec2::splat(26.0),
                        Icon::Prev,
                        14.0,
                        p.text_secondary,
                        p.text,
                        p.hover,
                        "Previous",
                    )
                    .clicked()
                    {
                        self.previous();
                    }
                    if play_pause_hero_button_sized(ui, 32.0, self.playing, &p).clicked() {
                        self.toggle_playback();
                    }
                    if icons::icon_button(
                        ui,
                        Vec2::splat(26.0),
                        Icon::Next,
                        14.0,
                        p.text_secondary,
                        p.text,
                        p.hover,
                        "Next",
                    )
                    .clicked()
                    {
                        self.next(false);
                    }
                    let repeat_icon = match self.repeat {
                        RepeatMode::One => Icon::RepeatOne,
                        RepeatMode::Off | RepeatMode::All => Icon::Repeat,
                    };
                    if transport_toggle_button(
                        ui,
                        repeat_icon,
                        self.repeat != RepeatMode::Off,
                        "Repeat",
                        &p,
                    )
                    .clicked()
                    {
                        self.repeat = match self.repeat {
                            RepeatMode::Off => RepeatMode::All,
                            RepeatMode::All => RepeatMode::One,
                            RepeatMode::One => RepeatMode::Off,
                        };
                        self.preload_next_track();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.style_mut().spacing.item_spacing.x = 4.0;
                        if let Some(new_vol) = custom_volume_bar(ui, 86.0, self.volume, &p) {
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
                            13.5,
                            if self.volume <= 0.001 {
                                p.danger
                            } else {
                                p.text_secondary
                            },
                            p.text,
                            p.hover,
                            "Mute (M)",
                        )
                        .clicked()
                        {
                            self.toggle_mute();
                        }

                        // Compact Output Device icon (matches Classic transport bar; Signal Path lives on telemetry bar above)
                        let active_dev = self
                            .stream_info
                            .as_ref()
                            .map(|s| s.device_name.as_str())
                            .or(self.preferred_device.as_deref())
                            .unwrap_or("System Default");
                        let dev_resp = icons::icon_button(
                            ui,
                            Vec2::splat(22.0),
                            Icon::Device,
                            13.5,
                            if self.show_device_picker || self.preferred_device.is_some() {
                                p.accent
                            } else {
                                p.muted
                            },
                            p.text,
                            p.hover,
                            &format!("Audio Output: {active_dev}\nClick to change"),
                        );
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
            });

        // 3) Optional Compact Live Visualizer Strip (only when toggled ON)
        if self.show_concert_visualizer {
            Frame::new()
                .fill(p.background)
                .inner_margin(Margin::symmetric(10, 4))
                .stroke(Stroke::new(1.0, p.border))
                .show(ui, |ui| {
                    let w = ui.available_width();
                    self.visualizer.ui_compact(
                        ui,
                        vec2(w, 52.0),
                        &p,
                        &self.plugins.visualizers,
                    );
                });
        }

        // 4) Collapsible Winamp / AIMP2-Style Playlist Drawer
        if self.compact_show_playlist {
            self.ensure_visible_cache();
            let tracks = Arc::clone(&self.cached_visible);
            let current_id = self.current_id();
            let time = ctx.input(|i| i.time);
            let (rms_l, rms_r, _, _, _) = self.visual_tap.levels();
            let live_rms = ((rms_l + rms_r) * 1.6).clamp(0.0, 1.0);
            let cmd = ctx.input(|i| i.modifiers.command);
            let shift = ctx.input(|i| i.modifiers.shift);

            let sep_r = ui.available_rect_before_wrap();
            ui.painter().line_segment(
                [pos2(sep_r.left(), sep_r.top()), pos2(sep_r.right(), sep_r.top())],
                Stroke::new(1.0, p.border),
            );

            Frame::new()
                .fill(p.background)
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    // Slim toolbar: aligned filter input + Add Tracks / Add Folder + count
                    ui.horizontal(|ui| {
                        let filter_w = (ui.available_width() - 120.0).max(120.0);
                        let (sr, _) = ui.allocate_exact_size(vec2(filter_w, 22.0), Sense::hover());
                        ui.painter().rect_filled(sr, 5.0, p.surface);
                        ui.painter().rect_stroke(sr, 5.0, Stroke::new(1.0, p.border), StrokeKind::Inside);
                        icons::paint_icon(
                            ui.painter(),
                            Rect::from_center_size(pos2(sr.left() + 11.0, sr.center().y), Vec2::splat(11.0)),
                            Icon::Search,
                            p.faint,
                        );
                        let edit_r = Rect::from_min_max(
                            pos2(sr.left() + 22.0, sr.top()),
                            pos2(sr.right() - 6.0, sr.bottom()),
                        );
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(edit_r)
                                .layout(Layout::left_to_right(Align::Center)),
                        );
                        let search_resp = child.add_sized(
                            edit_r.size(),
                            egui::TextEdit::singleline(&mut self.search)
                                .frame(Frame::NONE)
                                .font(FontId::proportional(11.0))
                                .align(Align2::LEFT_CENTER)
                                .hint_text(RichText::new("Filter tracks…").size(10.5).color(p.faint)),
                        );
                        if self.focus_search {
                            search_resp.request_focus();
                            self.focus_search = false;
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icons::icon_button(
                                ui,
                                Vec2::splat(22.0),
                                Icon::FolderPlus,
                                13.0,
                                p.muted,
                                p.accent,
                                p.hover,
                                "Add Folder",
                            )
                            .clicked()
                            {
                                self.add_folder();
                            }
                            if icons::icon_button(
                                ui,
                                Vec2::splat(22.0),
                                Icon::TrackPlus,
                                13.0,
                                p.muted,
                                p.accent,
                                p.hover,
                                "Add Tracks",
                            )
                            .clicked()
                            {
                                self.add_files();
                            }
                            ui.label(
                                RichText::new(format!("{}", tracks.len()))
                                    .size(10.0)
                                    .monospace()
                                    .color(p.faint),
                            );
                        });
                    });

                    ui.add_space(5.0);
                    let mut play_target: Option<(usize, u64)> = None;
                    let mut ctx_action: Option<ContextMenuAction> = None;
                    let playlists_snap: Vec<(u64, String)> = self
                        .store
                        .active()
                        .playlists
                        .iter()
                        .map(|pl| (pl.id, pl.name.clone()))
                        .collect();

                    ScrollArea::vertical()
                        .id_salt("compact_playlist_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.style_mut().spacing.item_spacing.y = 1.5;
                            if tracks.is_empty() {
                                ui.add_space(28.0);
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        RichText::new("Drop audio files or click + to add music")
                                            .size(11.0)
                                            .color(p.faint),
                                    );
                                });
                            } else {
                                let row_w = ui.available_width();
                                for (row_idx, tr) in tracks.iter().enumerate() {
                                    let is_current = current_id == Some(tr.id);
                                    let is_selected = self.selected_tracks.contains(&tr.id);
                                    let (row_rect, resp) =
                                        ui.allocate_exact_size(vec2(row_w, 26.0), Sense::click());
                                    if !ui.is_rect_visible(row_rect) {
                                        continue;
                                    }
                                    let hovered = resp.hovered();
                                    let bg = if is_current && is_selected {
                                        lerp_c(p.playing_row, p.clicked_row, 0.35)
                                    } else if is_current {
                                        p.playing_row
                                    } else if is_selected {
                                        p.clicked_row
                                    } else if hovered {
                                        p.hover
                                    } else {
                                        Color32::TRANSPARENT
                                    };
                                    ui.painter().rect_filled(row_rect, 4.0, bg);
                                    if is_current {
                                        let bar = Rect::from_min_max(
                                            pos2(row_rect.left() + 1.5, row_rect.top() + 5.0),
                                            pos2(row_rect.left() + 4.0, row_rect.bottom() - 5.0),
                                        );
                                        ui.painter().rect_filled(bar, 1.0, p.accent);
                                    } else if is_selected {
                                        ui.painter().rect_stroke(
                                            row_rect,
                                            4.0,
                                            Stroke::new(1.0, p.border_strong),
                                            StrokeKind::Inside,
                                        );
                                    }

                                    let idx_c = pos2(row_rect.left() + 18.0, row_rect.center().y);
                                    if is_current {
                                        icons::paint_playing_bars(
                                            ui.painter(),
                                            Rect::from_center_size(idx_c, vec2(10.0, 10.0)),
                                            time,
                                            self.playing,
                                            live_rms,
                                            p.accent,
                                        );
                                    } else {
                                        ui.painter().text(
                                            idx_c,
                                            Align2::CENTER_CENTER,
                                            format!("{:02}", row_idx + 1),
                                            FontId::monospace(9.5),
                                            p.faint,
                                        );
                                    }

                                    let text_r = Rect::from_min_max(
                                        pos2(row_rect.left() + 32.0, row_rect.top()),
                                        pos2(row_rect.right() - 48.0, row_rect.bottom()),
                                    );
                                    let line = format!("{} — {}", tr.title, tr.artist);
                                    ui.painter().with_clip_rect(text_r).text(
                                        pos2(text_r.left(), row_rect.center().y),
                                        Align2::LEFT_CENTER,
                                        line,
                                        FontId::proportional(11.5),
                                        if is_current { p.accent } else { p.text },
                                    );
                                    ui.painter().text(
                                        pos2(row_rect.right() - 6.0, row_rect.center().y),
                                        Align2::RIGHT_CENTER,
                                        format_duration(tr.duration),
                                        FontId::monospace(10.0),
                                        if is_current { p.accent } else { p.muted },
                                    );

                                    let resp =
                                        resp.on_hover_ui(|ui| self.paint_track_hover_card(ui, tr));

                                    if resp.double_clicked() {
                                        play_target = Some((row_idx, tr.id));
                                        self.select_row_click(row_idx, tr.id, &tracks, false, false);
                                    } else if resp.clicked() {
                                        self.select_row_click(row_idx, tr.id, &tracks, cmd, shift);
                                    }

                                    let tid = tr.id;
                                    resp.context_menu(|ui| {
                                        if !is_selected {
                                            self.select_row_click(row_idx, tid, &tracks, false, false);
                                        }
                                        if ui.button("Play").clicked() {
                                            ctx_action = Some(ContextMenuAction::PlaySelected);
                                            ui.close();
                                        }
                                        if ui.button("Play Next").clicked() {
                                            ctx_action = Some(ContextMenuAction::PlayNext);
                                            ui.close();
                                        }
                                        if ui.button("Add to Play Queue").clicked() {
                                            ctx_action = Some(ContextMenuAction::AddToQueue);
                                            ui.close();
                                        }
                                        ui.menu_button("Add to Playlist", |ui| {
                                            for (pl_id, pl_name) in &playlists_snap {
                                                if ui.button(pl_name).clicked() {
                                                    ctx_action =
                                                        Some(ContextMenuAction::AddToPlaylist(*pl_id));
                                                    ui.close();
                                                }
                                            }
                                            if !playlists_snap.is_empty() {
                                                ui.separator();
                                            }
                                            if ui.button("+ New Playlist…").clicked() {
                                                ctx_action =
                                                    Some(ContextMenuAction::NewPlaylistWithSelected);
                                                ui.close();
                                            }
                                        });
                                        ui.separator();
                                        if ui.button("Edit Tags & Lookup…").clicked() {
                                            ctx_action = Some(ContextMenuAction::EditTags);
                                            ui.close();
                                        }
                                        if ui.button("Track Info…").clicked() {
                                            ctx_action = Some(ContextMenuAction::TrackInfo(tid));
                                            ui.close();
                                        }
                                        if ui
                                            .button(format!("Reveal in {FILE_MANAGER_NAME}"))
                                            .clicked()
                                        {
                                            ctx_action =
                                                Some(ContextMenuAction::RevealInFinder(tid));
                                            ui.close();
                                        }
                                        ui.separator();
                                        if ui
                                            .button(
                                                RichText::new("Remove from Library").color(p.danger),
                                            )
                                            .clicked()
                                        {
                                            ctx_action = Some(ContextMenuAction::RemoveSelected);
                                            ui.close();
                                        }
                                    });
                                }
                            }
                        });

                    if let Some((_, tid)) = play_target {
                        self.play_from_list(tid, &tracks);
                    }
                    if let Some(act) = ctx_action {
                        self.run_context_action(act, &tracks);
                    }
                });
        }

        if self.playing {
            ctx.request_repaint_after(Duration::from_millis(16));
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

    fn cached_file_size(&mut self, path: &Path) -> Option<u64> {
        if let Some(&cached) = self.file_size_cache.get(path) {
            return cached;
        }
        let sz = std::fs::metadata(path).ok().map(|m| m.len());
        if self.file_size_cache.len() > 2048 {
            self.file_size_cache.clear();
        }
        self.file_size_cache.insert(path.to_path_buf(), sz);
        sz
    }

    /// Stylish, theme-aligned Track Hover Card displaying album art, quality tier, complete tag
    /// metadata, audio specs, ReplayGain/CUE info, and recessed folder + file readout.
    fn paint_track_hover_card(&mut self, ui: &mut Ui, track: &Track) {
        let p = self.palette;
        let card_w = (ui.ctx().content_rect().width() - 32.0).clamp(260.0, 356.0);
        ui.set_min_width(card_w.min(312.0));
        ui.set_max_width(card_w);

        let is_fav = self.store.active().favorites.contains(&track.id);
        let file_size = self.cached_file_size(&track.path);

        // 1) Top tier accent hairline (Gold for Hi-Res, Accent for Lossless, Subtle border for Standard)
        let tier_col = match track.quality_tier() {
            QualityTier::HiRes => p.hires_gold,
            QualityTier::Lossless => p.accent,
            QualityTier::Standard => p.border_strong,
        };
        let (bar_r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 2.5), Sense::hover());
        ui.painter().rect_filled(bar_r, 1.25, tier_col);
        ui.add_space(6.0);

        // 2) Hero header: crisp 54px cover + Title / Artist / Album + Quality pill + Duration + Rating
        ui.horizontal(|ui| {
            let (art_rect, _) = ui.allocate_exact_size(Vec2::splat(54.0), Sense::hover());
            self.paint_cover_in_rect(ui, track, art_rect);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.style_mut().spacing.item_spacing.y = 2.0;
                ui.add(
                    egui::Label::new(
                        RichText::new(&track.title)
                            .size(13.0)
                            .strong()
                            .color(p.text),
                    )
                    .wrap(),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(&track.artist)
                            .size(11.0)
                            .strong()
                            .color(p.accent),
                    )
                    .wrap(),
                );
                let album_line = match &track.year {
                    Some(y) if !y.is_empty() => format!("{}  ({y})", track.album),
                    _ => track.album.clone(),
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(album_line)
                            .size(10.5)
                            .color(p.text_secondary),
                    )
                    .wrap(),
                );
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.style_mut().spacing.item_spacing.x = 6.0;
                    let (badge_r, _) = ui.allocate_exact_size(vec2(126.0, 18.0), Sense::hover());
                    paint_aligned_quality_badge(ui.painter(), badge_r, track, &p);
                    ui.label(
                        RichText::new(format_duration(track.duration))
                            .size(10.0)
                            .monospace()
                            .color(p.muted),
                    );
                    if track.rating > 0 {
                        ui.label(
                            RichText::new("★".repeat(track.rating as usize))
                                .size(10.0)
                                .color(p.hires_gold),
                        );
                    }
                    if is_fav {
                        let (hr, _) = ui.allocate_exact_size(Vec2::splat(11.0), Sense::hover());
                        icons::paint_icon(ui.painter(), hr, Icon::HeartFilled, p.accent);
                    }
                });
            });
        });

        ui.add_space(6.0);
        let sep1 = ui.available_rect_before_wrap();
        ui.painter().line_segment(
            [pos2(sep1.left(), sep1.top()), pos2(sep1.right(), sep1.top())],
            Stroke::new(1.0, p.border),
        );
        ui.add_space(6.0);

        // 3) Structured Tag & Audio Technical Grid
        let mut audio_parts = vec![track.codec.clone()];
        if let Some(b) = track.bit_depth {
            audio_parts.push(format!("{b}-bit"));
        }
        if let Some(sr) = track.sample_rate {
            audio_parts.push(format_sample_rate(sr));
        }
        if let Some(ch) = track.channels {
            audio_parts.push(match ch {
                1 => "Mono".into(),
                2 => "Stereo".into(),
                n => format!("{n}ch"),
            });
        }
        if let Some(kbps) = track.nominal_bitrate {
            audio_parts.push(format!("{kbps} kbps"));
        }

        egui::Grid::new(ui.scope_id().with("track_hover_tag_grid"))
            .num_columns(2)
            .spacing([12.0, 3.0])
            .show(ui, |ui| {
                let mut row = |k: &str, v: String, mono: bool| {
                    ui.label(RichText::new(k).size(9.0).strong().color(p.faint));
                    let rt = if mono {
                        RichText::new(v)
                            .size(10.0)
                            .monospace()
                            .color(p.text_secondary)
                    } else {
                        RichText::new(v).size(10.5).color(p.text_secondary)
                    };
                    ui.add(egui::Label::new(rt).wrap());
                    ui.end_row();
                };

                if track.track_number.is_some() || track.disc_number.is_some() {
                    let tr_s = track
                        .track_number
                        .map(|n| format!("{n:02}"))
                        .unwrap_or_else(|| "—".into());
                    let disc_s = track
                        .disc_number
                        .map(|d| format!("  ·  Disc {d}"))
                        .unwrap_or_default();
                    row("TRACK", format!("{tr_s}{disc_s}"), true);
                }
                if let Some(aa) = &track.album_artist
                    && !aa.is_empty()
                    && aa != &track.artist
                {
                    row("ALBUM ARTIST", aa.clone(), false);
                }
                if let Some(genre) = &track.genre
                    && !genre.is_empty()
                {
                    row("GENRE", genre.clone(), false);
                }
                if let Some(year) = &track.year
                    && !year.is_empty()
                {
                    row("YEAR", year.clone(), true);
                }
                row("AUDIO", audio_parts.join("  ·  "), true);
                if track.rg_track_gain_db.is_some() || track.rg_album_gain_db.is_some() {
                    let rg_t = track
                        .rg_track_gain_db
                        .map(|g| format!("Track {g:+.2} dB"))
                        .unwrap_or_default();
                    let rg_a = track
                        .rg_album_gain_db
                        .map(|g| format!("Album {g:+.2} dB"))
                        .unwrap_or_default();
                    let joined = [rg_t, rg_a]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("  ·  ");
                    row("REPLAYGAIN", joined, true);
                }
                if let Some(cs) = track.cue_start {
                    let end_s = track
                        .cue_end
                        .map(|e| format_duration(Some(e)))
                        .unwrap_or_else(|| "End".into());
                    row("CUE SPAN", format!("{} → {end_s}", format_duration(Some(cs))), true);
                }
                if track.play_count > 0 || track.lyrics.is_some() {
                    let mut extras = Vec::new();
                    if track.play_count > 0 {
                        extras.push(format!("Played {}×", track.play_count));
                    }
                    if track.lyrics.is_some() {
                        extras.push("Embedded Lyrics".into());
                    }
                    row("INFO", extras.join("  ·  "), false);
                }
            });

        ui.add_space(6.0);

        // 4) Recessed File & Directory Card Footer
        let file_name = track
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown file");
        let dir_path = track
            .path
            .parent()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| track.path.display().to_string());

        Frame::new()
            .fill(p.background)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(6.0)
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.style_mut().spacing.item_spacing.y = 3.0;
                ui.horizontal(|ui| {
                    let (ic_r, _) = ui.allocate_exact_size(Vec2::splat(11.0), Sense::hover());
                    icons::paint_icon(ui.painter(), ic_r, Icon::Library, p.accent);
                    ui.add(
                        egui::Label::new(
                            RichText::new(file_name)
                                .size(10.0)
                                .monospace()
                                .strong()
                                .color(p.text),
                        )
                        .wrap(),
                    );
                    if let Some(bytes) = file_size {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(crate::metadata::format_file_size(bytes))
                                    .size(9.0)
                                    .monospace()
                                    .color(p.muted),
                            );
                        });
                    }
                });
                ui.horizontal(|ui| {
                    let (fr, _) = ui.allocate_exact_size(Vec2::splat(11.0), Sense::hover());
                    icons::paint_icon(ui.painter(), fr, Icon::Folder, p.faint);
                    ui.add(
                        egui::Label::new(
                            RichText::new(dir_path)
                                .size(9.0)
                                .monospace()
                                .color(p.muted),
                        )
                        .wrap(),
                    );
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
    SetRating(u64, u8),
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
        if self.show_concert_visualizer {
            self.visualizer
                .update(&self.visual_tap, dt, &self.plugins.visualizers, paused);
        }

        let p = self.palette;

        // Auto-expand collapsed Compact window while a modal is open so Settings/Tag Editor never feel cramped
        if self.compact_mode {
            let need_expand = !self.compact_show_playlist && self.any_modal_open();
            if need_expand && !self.compact_modal_expanded {
                self.compact_modal_expanded = true;
                self.sync_compact_height(&context);
            } else if !need_expand && self.compact_modal_expanded {
                self.compact_modal_expanded = false;
                self.sync_compact_height(&context);
            }

            egui::CentralPanel::default()
                .frame(Frame::new().fill(p.background).inner_margin(Margin::ZERO))
                .show(root, |ui| self.compact_view(&context, ui));
        } else {
            egui::Panel::top("classic_top_bar")
                .exact_size(32.0)
                .frame(Frame::new().fill(p.sidebar).inner_margin(Margin::ZERO))
                .show(root, |ui| self.classic_top_bar(&context, ui));

            egui::Panel::bottom("player")
                .exact_size(TRANSPORT_HEIGHT)
                .frame(
                    Frame::new()
                        .fill(p.panel)
                        .inner_margin(Margin::symmetric(16, 8))
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

            if self.show_lyrics {
                egui::Panel::right("lyrics_panel")
                    .exact_size(276.0)
                    .frame(
                        Frame::new()
                            .fill(p.sidebar)
                            .inner_margin(Margin::symmetric(12, 12))
                            .stroke(Stroke::new(1.0, p.border)),
                    )
                    .show(root, |ui| self.lyrics_panel_contents(ui));
            }

            egui::CentralPanel::default()
                .frame(
                    Frame::new()
                        .fill(p.background)
                        .inner_margin(Margin::symmetric(18, 0)),
                )
                .show(root, |ui| self.main_view(ui));

            self.floating_selection_dock(&context);
        }

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
        self.store.save_sync();
        SessionState {
            active_library_id: self.store.active_id,
            queue_ids: self.current_queue.clone(),
            current_index: self.current_index,
            position_secs: self.position,
        }
        .save();
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
            compact_mode: self.compact_mode,
            compact_show_playlist: self.compact_show_playlist,
            close_to_tray: self.close_to_tray,
            auto_watch_folders: self.auto_watch_folders,
        }
        .save();
    }
}

/// Platform-accurate window drag trigger:
/// - On macOS, `winit`'s `drag_window()` forwards `NSApp.currentEvent` to
///   `[NSWindow performWindowDragWithEvent:]`, which requires an `NSLeftMouseDown` event.
///   Sending `StartDrag` after egui's drag threshold (during `NSLeftMouseDragged`) causes
///   AppKit to log `Warning: Window move completed without beginning` when the drag finishes.
///   Triggering on the primary press frame (excluding double-clicks) passes `NSLeftMouseDown` cleanly.
/// - On Windows/Linux, `drag_started_by(Primary)` avoids intercepting clicks/double-clicks.
fn should_begin_window_drag(ctx: &egui::Context, resp: &egui::Response) -> bool {
    #[cfg(target_os = "macos")]
    {
        resp.is_pointer_button_down_on()
            && ctx.input(|i| {
                i.pointer.primary_pressed()
                    && !i.pointer.button_double_clicked(egui::PointerButton::Primary)
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ctx;
        resp.drag_started_by(egui::PointerButton::Primary)
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_traffic_dot(
    painter: &egui::Painter,
    center: Pos2,
    dot_r: f32,
    base_col: Color32,
    is_focused: bool,
    cluster_hovered: bool,
    hovered: bool,
    kind: u8,
    p: &Palette,
) {
    let glyph_col = if p.is_light {
        Color32::from_rgb(24, 34, 38)
    } else {
        Color32::from_rgb(12, 16, 18)
    };
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
        let st = Stroke::new(1.25, glyph_col);
        match kind {
            // Close (×)
            0 => {
                let d = 2.4;
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
                let d = 2.9;
                painter.line_segment([pos2(center.x - d, center.y), pos2(center.x + d, center.y)], st);
            }
            // Zoom / Fullscreen (⤢)
            _ => {
                let d = 2.4;
                painter.line_segment(
                    [pos2(center.x - d, center.y + d), pos2(center.x + d, center.y - d)],
                    st,
                );
                painter.line_segment(
                    [pos2(center.x + d - 2.0, center.y - d), pos2(center.x + d, center.y - d)],
                    st,
                );
                painter.line_segment(
                    [pos2(center.x + d, center.y - d), pos2(center.x + d, center.y - d + 2.0)],
                    st,
                );
                painter.line_segment(
                    [pos2(center.x - d, center.y + d), pos2(center.x - d + 2.0, center.y + d)],
                    st,
                );
                painter.line_segment(
                    [pos2(center.x - d, center.y + d), pos2(center.x - d, center.y + d - 2.0)],
                    st,
                );
            }
        }
    }
}

fn paint_topbar_icon_btn(
    ui: &mut Ui,
    rect: Rect,
    id: &str,
    icon: Icon,
    active: bool,
    tip: &str,
    p: &Palette,
) -> bool {
    let resp = ui
        .interact(rect, ui.scope_id().with(id), Sense::click())
        .on_hover_text(tip);
    if active {
        ui.painter().rect_filled(rect, 4.0, p.accent_subtle);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, p.hover);
    }
    icons::paint_icon(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(12.0)),
        icon,
        if active {
            p.accent
        } else if resp.hovered() {
            p.text
        } else {
            p.muted
        },
    );
    resp.clicked()
}

fn paint_topbar_pin_btn(
    ui: &mut Ui,
    rect: Rect,
    id: &str,
    always_on_top: bool,
    p: &Palette,
) -> bool {
    let resp = ui
        .interact(rect, ui.scope_id().with(id), Sense::click())
        .on_hover_text(if always_on_top {
            "Always on Top: On (Click to unpin)"
        } else {
            "Pin Window Always on Top"
        });
    if always_on_top || resp.hovered() {
        ui.painter().rect_filled(
            rect,
            4.0,
            if always_on_top { p.accent_subtle } else { p.hover },
        );
    }
    let c = rect.center();
    let col = if always_on_top {
        p.accent
    } else if resp.hovered() {
        p.text
    } else {
        p.faint
    };
    ui.painter().circle_filled(pos2(c.x, c.y - 1.8), 2.3, col);
    ui.painter().line_segment(
        [pos2(c.x, c.y + 0.5), pos2(c.x, c.y + 4.4)],
        Stroke::new(1.2, col),
    );
    resp.clicked()
}

fn paint_mode_switch_pill(
    ui: &mut Ui,
    rect: Rect,
    id: &str,
    label: &str,
    icon: Icon,
    tip: &str,
    p: &Palette,
) -> bool {
    let resp = ui
        .interact(rect, ui.scope_id().with(id), Sense::click())
        .on_hover_text(tip);
    let hovered = resp.hovered();
    ui.painter().rect_filled(
        rect,
        5.0,
        if hovered { p.accent_subtle } else { p.surface },
    );
    ui.painter().rect_stroke(
        rect,
        5.0,
        Stroke::new(1.0, if hovered { p.accent } else { p.border }),
        StrokeKind::Inside,
    );
    let fg = if hovered { p.accent } else { p.text_secondary };
    let font = FontId::proportional(10.5);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    let icon_sz = 11.0;
    let gap = 5.0;
    let content_w = icon_sz + gap + galley.size().x;
    let start_x = rect.center().x - content_w * 0.5;
    icons::paint_icon(
        ui.painter(),
        Rect::from_center_size(pos2(start_x + icon_sz * 0.5, rect.center().y), Vec2::splat(icon_sz)),
        icon,
        fg,
    );
    ui.painter().galley(
        pos2(start_x + icon_sz + gap, rect.center().y - galley.size().y * 0.5),
        galley,
        fg,
    );
    resp.clicked()
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
    play_pause_hero_button_sized(ui, 36.0, playing, p)
}

fn play_pause_hero_button_sized(
    ui: &mut Ui,
    size: f32,
    playing: bool,
    p: &Palette,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let center = rect.center();
        let r = rect.width() * 0.5;
        if playing || hovered {
            ui.painter().circle_filled(
                center,
                r + 2.0,
                Color32::from_rgba_unmultiplied(
                    p.accent.r(),
                    p.accent.g(),
                    p.accent.b(),
                    if hovered { 42 } else { 24 },
                ),
            );
        }
        let bg = if hovered { p.accent_bright } else { p.accent };
        ui.painter().circle_filled(center, r, bg);
        let offset = if playing { 0.0 } else { 1.1 };
        let icon_sz = (size * 0.44).clamp(13.0, 16.0);
        let ir = Rect::from_center_size(pos2(center.x + offset, center.y), Vec2::splat(icon_sz));
        icons::paint_icon(
            ui.painter(),
            ir,
            if playing { Icon::Pause } else { Icon::Play },
            p.on_accent(),
        );
    }
    resp.on_hover_text(if playing { "Pause (Space)" } else { "Play (Space)" })
}

/// High-definition track progress scrubber with three crisp modes:
/// - `AcousticWaveform`: smooth continuous mirrored acoustic silhouette with exact sub-pixel playhead split
/// - `PrecisionBars`: strictly pixel-snapped uniform-width bars (zero Moiré/uneven width on 100%/125% displays)
/// - `StudioRail`: minimal precision rail
///
/// All modes show a crisp floating seek timestamp pill directly above the cursor while hovering/scrubbing.
#[allow(clippy::too_many_arguments)]
fn acoustic_timeline_scrubber(
    ui: &mut Ui,
    width: f32,
    position: f64,
    duration: f64,
    active: bool,
    waveform: Option<&[f32]>,
    style: ProgressBarStyle,
    ab_loop: (Option<f64>, Option<f64>),
    p: &Palette,
) -> Option<f64> {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click_and_drag());
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
        let ppp = painter.pixels_per_point().max(1.0);
        let bg_rect = icons::snap_rect(rect.shrink2(vec2(0.0, 2.0)), ppp);
        let playhead_x = (bg_rect.left() + bg_rect.width() * frac).clamp(bg_rect.left(), bg_rect.right());
        let cy = icons::snap_px(bg_rect.top() + bg_rect.height() * 0.62, ppp);

        // Sample smooth interpolated waveform amplitude at normalized t in [0, 1]
        let sample_amp = |t: f32| -> f32 {
            let Some(wf) = waveform else {
                return 0.0;
            };
            if wf.is_empty() {
                return 0.0;
            }
            let pos = t.clamp(0.0, 1.0) * (wf.len() - 1) as f32;
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(wf.len() - 1);
            let f = pos - i0 as f32;
            (wf[i0] * (1.0 - f) + wf[i1] * f).clamp(0.05, 1.0)
        };

        match style {
            ProgressBarStyle::AcousticWaveform => {
                painter.rect_filled(bg_rect, 5.0, p.background);
                painter.rect_stroke(
                    bg_rect,
                    5.0,
                    Stroke::new(
                        1.0,
                        if hovered || dragging {
                            p.border_strong
                        } else {
                            p.border
                        },
                    ),
                    StrokeKind::Inside,
                );

                // Clean center baseline axis
                let inner_l = bg_rect.left() + 4.0;
                let inner_r = bg_rect.right() - 4.0;
                let inner_w = (inner_r - inner_l).max(8.0);
                let up_h = (cy - bg_rect.top() - 2.5).max(4.0);
                let dn_h = (bg_rect.bottom() - cy - 2.0).max(2.5);
                let split_x = (inner_l + inner_w * frac).clamp(inner_l, inner_r);

                painter.line_segment(
                    [pos2(inner_l, cy), pos2(inner_r, cy)],
                    Stroke::new(1.0 / ppp, p.border),
                );

                if waveform.is_some() {
                    // Build high-resolution continuous mirrored ribbon (upper 66% + lower 34% reflection)
                    let steps = ((inner_w / 2.0).round() as usize).clamp(48, 220);
                    let unplayed_up = Color32::from_rgba_unmultiplied(
                        p.muted.r(),
                        p.muted.g(),
                        p.muted.b(),
                        if p.is_light { 95 } else { 78 },
                    );
                    let unplayed_dn = Color32::from_rgba_unmultiplied(
                        p.muted.r(),
                        p.muted.g(),
                        p.muted.b(),
                        if p.is_light { 45 } else { 36 },
                    );
                    let played_up = if hovered || dragging {
                        Color32::from_rgba_unmultiplied(
                            p.accent_bright.r(),
                            p.accent_bright.g(),
                            p.accent_bright.b(),
                            225,
                        )
                    } else {
                        Color32::from_rgba_unmultiplied(
                            p.accent.r(),
                            p.accent.g(),
                            p.accent.b(),
                            210,
                        )
                    };
                    let played_dn = Color32::from_rgba_unmultiplied(
                        p.accent.r(),
                        p.accent.g(),
                        p.accent.b(),
                        86,
                    );
                    let hover_up = Color32::from_rgba_unmultiplied(
                        p.accent.r(),
                        p.accent.g(),
                        p.accent.b(),
                        128,
                    );

                    let mut played_crest: Vec<Pos2> = Vec::with_capacity(steps);
                    let mut unplayed_crest: Vec<Pos2> = Vec::with_capacity(steps);
                    // Single pre-sized GPU Mesh replaces ~440 per-frame Shape::convex_polygon Vec allocations!
                    let mut wf_mesh = egui::Mesh::default();
                    wf_mesh.vertices.reserve((steps + 2) * 8);
                    wf_mesh.indices.reserve((steps + 2) * 12);

                    let mut push_quad = |p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, col: Color32| {
                        let base = wf_mesh.vertices.len() as u32;
                        wf_mesh.colored_vertex(p0, col);
                        wf_mesh.colored_vertex(p1, col);
                        wf_mesh.colored_vertex(p2, col);
                        wf_mesh.colored_vertex(p3, col);
                        wf_mesh.add_triangle(base, base + 1, base + 2);
                        wf_mesh.add_triangle(base, base + 2, base + 3);
                    };

                    let mut prev_x = inner_l;
                    let mut prev_a = sample_amp(0.0);
                    for i in 1..=steps {
                        let t = i as f32 / steps as f32;
                        let x = inner_l + inner_w * t;
                        let a = sample_amp(t);

                        let mut draw_quad =
                            |x0: f32, a0: f32, x1: f32, a1: f32, is_played: bool, is_hov: bool| {
                                let y0_u = cy - up_h * a0;
                                let y1_u = cy - up_h * a1;
                                let y0_d = cy + dn_h * a0;
                                let y1_d = cy + dn_h * a1;
                                let col_u = if is_played {
                                    played_up
                                } else if is_hov {
                                    hover_up
                                } else {
                                    unplayed_up
                                };
                                let col_d = if is_played { played_dn } else { unplayed_dn };
                                push_quad(
                                    pos2(x0, cy),
                                    pos2(x0, y0_u),
                                    pos2(x1, y1_u),
                                    pos2(x1, cy),
                                    col_u,
                                );
                                push_quad(
                                    pos2(x0, cy),
                                    pos2(x1, cy),
                                    pos2(x1, y1_d),
                                    pos2(x0, y0_d),
                                    col_d,
                                );
                            };

                        let in_hover = |mid_x: f32| -> bool {
                            active
                                && hovered
                                && pointer_x.is_some_and(|px| {
                                    (mid_x >= split_x && mid_x <= px)
                                        || (mid_x <= split_x && mid_x >= px)
                                })
                        };

                        if active && prev_x < split_x && x > split_x {
                            // Exact sub-pixel split right at playhead!
                            let alpha = ((split_x - prev_x) / (x - prev_x)).clamp(0.0, 1.0);
                            let mid_a = prev_a + (a - prev_a) * alpha;
                            draw_quad(prev_x, prev_a, split_x, mid_a, true, false);
                            draw_quad(split_x, mid_a, x, a, false, in_hover((split_x + x) * 0.5));
                            played_crest.push(pos2(prev_x, cy - up_h * prev_a));
                            played_crest.push(pos2(split_x, cy - up_h * mid_a));
                            unplayed_crest.push(pos2(split_x, cy - up_h * mid_a));
                            unplayed_crest.push(pos2(x, cy - up_h * a));
                        } else {
                            let is_played = active && x <= split_x;
                            draw_quad(prev_x, prev_a, x, a, is_played, in_hover((prev_x + x) * 0.5));
                            if is_played {
                                if played_crest.is_empty() {
                                    played_crest.push(pos2(prev_x, cy - up_h * prev_a));
                                }
                                played_crest.push(pos2(x, cy - up_h * a));
                            } else {
                                if unplayed_crest.is_empty() {
                                    unplayed_crest.push(pos2(prev_x, cy - up_h * prev_a));
                                }
                                unplayed_crest.push(pos2(x, cy - up_h * a));
                            }
                        }

                        prev_x = x;
                        prev_a = a;
                    }

                    if !wf_mesh.is_empty() {
                        painter.add(egui::Shape::mesh(wf_mesh));
                    }

                    if unplayed_crest.len() >= 2 {
                        painter.add(egui::Shape::line(
                            unplayed_crest,
                            Stroke::new(1.0, p.muted.gamma_multiply(0.7)),
                        ));
                    }
                    if played_crest.len() >= 2 {
                        painter.add(egui::Shape::line(
                            played_crest,
                            Stroke::new(1.3, p.accent_bright),
                        ));
                    }
                } else if active {
                    // Clean quiet rail while waveform decodes (ZERO fake sine wave!)
                    let rail_h = 3.0;
                    let rail_r = Rect::from_min_max(
                        pos2(inner_l, cy - rail_h * 0.5),
                        pos2(split_x.max(inner_l + 2.0), cy + rail_h * 0.5),
                    );
                    painter.rect_filled(rail_r, 1.5, p.accent);
                }

                // Precision vertical playhead needle
                if active {
                    let px_snap = icons::snap_px(split_x, ppp);
                    painter.line_segment(
                        [pos2(px_snap, bg_rect.top() + 1.5), pos2(px_snap, bg_rect.bottom() - 1.5)],
                        Stroke::new(1.6, p.accent_bright),
                    );
                }
            }
            ProgressBarStyle::PrecisionBars => {
                painter.rect_filled(bg_rect, 5.0, p.background);
                painter.rect_stroke(
                    bg_rect,
                    5.0,
                    Stroke::new(
                        1.0,
                        if hovered || dragging {
                            p.border_strong
                        } else {
                            p.border
                        },
                    ),
                    StrokeKind::Inside,
                );

                // Strictly pixel-snapped uniform bar width & uniform gap (zero Moiré on 1x Windows displays!)
                let inner = bg_rect.shrink2(vec2(5.0, 3.0));
                let bar_w = icons::snap_px(2.0, ppp).max(1.0 / ppp);
                let gap = icons::snap_px(2.0, ppp).max(1.0 / ppp);
                let pitch = bar_w + gap;
                let bar_count = ((inner.width() / pitch).floor() as usize).clamp(24, 240);
                let total_w = bar_count as f32 * bar_w + (bar_count.saturating_sub(1)) as f32 * gap;
                let start_x = icons::snap_px(inner.left() + (inner.width() - total_w) * 0.5, ppp);
                let half_h = inner.height() * 0.5;
                let mid_y = icons::snap_px(inner.center().y, ppp);

                for i in 0..bar_count {
                    let t = i as f32 / (bar_count - 1).max(1) as f32;
                    let amp = if waveform.is_some() {
                        sample_amp(t)
                    } else {
                        0.12
                    };
                    let bx = icons::snap_px(start_x + i as f32 * pitch, ppp);
                    let played = active && bx + bar_w * 0.5 <= playhead_x;
                    let hover_zone = active
                        && hovered
                        && pointer_x.is_some_and(|px| {
                            (bx >= playhead_x && bx <= px) || (bx <= playhead_x && bx >= px)
                        });
                    let bh = icons::snap_px((half_h * amp).max(1.5), ppp);
                    let bar_rect =
                        Rect::from_min_max(pos2(bx, mid_y - bh), pos2(bx + bar_w, mid_y + bh));
                    let col = if played {
                        if hovered || dragging {
                            p.accent_bright
                        } else {
                            p.accent
                        }
                    } else if hover_zone {
                        p.accent.gamma_multiply(0.55)
                    } else {
                        p.border_strong.gamma_multiply(if active { 0.85 } else { 0.45 })
                    };
                    painter.rect_filled(bar_rect, 0.8, col);
                }

                if active {
                    let px_snap = icons::snap_px(playhead_x, ppp);
                    painter.line_segment(
                        [pos2(px_snap, bg_rect.top() + 1.5), pos2(px_snap, bg_rect.bottom() - 1.5)],
                        Stroke::new(1.6, p.accent_bright),
                    );
                }
            }
            ProgressBarStyle::StudioRail => {
                let bar_h = if hovered || dragging { 5.0 } else { 3.5 };
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
                        if hovered || dragging { 5.2 } else { 3.5 },
                        p.text,
                    );
                }
            }
        }

        // Subtle shaded A-B section loop bracket overlay when active
        if active && duration > 0.01 {
            if let (Some(a), Some(b)) = ab_loop
                && b > a
            {
                let ax = bg_rect.left() + bg_rect.width() * (a / duration).clamp(0.0, 1.0) as f32;
                let bx = bg_rect.left() + bg_rect.width() * (b / duration).clamp(0.0, 1.0) as f32;
                let span = Rect::from_min_max(pos2(ax, bg_rect.top() + 1.0), pos2(bx, bg_rect.bottom() - 1.0));
                painter.rect_filled(
                    span,
                    2.0,
                    Color32::from_rgba_unmultiplied(p.hires_gold.r(), p.hires_gold.g(), p.hires_gold.b(), 34),
                );
                painter.line_segment([pos2(ax, bg_rect.top() + 1.0), pos2(ax, bg_rect.bottom() - 1.0)], Stroke::new(1.5, p.hires_gold));
                painter.line_segment([pos2(bx, bg_rect.top() + 1.0), pos2(bx, bg_rect.bottom() - 1.0)], Stroke::new(1.5, p.hires_gold));
            } else if let Some(a) = ab_loop.0 {
                let ax = bg_rect.left() + bg_rect.width() * (a / duration).clamp(0.0, 1.0) as f32;
                painter.line_segment([pos2(ax, bg_rect.top() + 1.0), pos2(ax, bg_rect.bottom() - 1.0)], Stroke::new(1.5, p.hires_gold));
            }
        }

        // Crisp floating seek time pill directly above cursor on hover/scrub (zero delayed OS tooltip!)
        if active && (hovered || dragging) && let Some(px) = pointer_x {
            let cx = px.clamp(bg_rect.left() + 2.0, bg_rect.right() - 2.0);
            if !dragging && style != ProgressBarStyle::StudioRail {
                painter.line_segment(
                    [pos2(cx, bg_rect.top() + 2.0), pos2(cx, bg_rect.bottom() - 2.0)],
                    Stroke::new(1.0, p.text.gamma_multiply(0.55)),
                );
            }
            let hover_frac = ((cx - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64;
            let time_label = format_duration(Some(hover_frac * duration));
            let top_painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::unique("seek_hover_time_pill"),
            ));
            let font = FontId::monospace(9.5);
            let galley = top_painter.layout_no_wrap(time_label.clone(), font.clone(), p.text);
            let pill_w = galley.size().x + 10.0;
            let pill_h = 17.0;
            let pill_cx = cx.clamp(bg_rect.left() + pill_w * 0.5, bg_rect.right() - pill_w * 0.5);
            let pill_r = Rect::from_center_size(pos2(pill_cx, bg_rect.top() - 11.0), vec2(pill_w, pill_h));
            top_painter.rect_filled(pill_r, 4.0, p.surface_elevated);
            top_painter.rect_stroke(pill_r, 4.0, Stroke::new(1.0, p.selected_border), StrokeKind::Inside);
            top_painter.text(pill_r.center(), Align2::CENTER_CENTER, time_label, font, p.text);
        }
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
