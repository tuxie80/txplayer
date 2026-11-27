use std::path::PathBuf;
use std::thread;

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use crate::abx::{AbxLabState, AbxSlot, compute_loudness_match_trims};
use crate::analyzer::{AudioAnalysis, HiResVerdict};
use crate::audio::{default_output_device_name, list_output_devices};
use crate::dsp::compute_eq_response_curve;
use crate::icons::{self, Icon};
use crate::library::{
    DuplicateGroup, LibraryStats, PlayHistoryEntry, QueryField, QueryOp, QueryRule, SavedView,
    cover_directory,
};
use crate::metadata::{Track, format_duration, format_file_size, format_sample_rate};
use crate::platform::{
    AUDIO_BACKEND_NAME, FILE_MANAGER_NAME, MOD_SHORT, open_file_with_default_app,
    reveal_in_file_manager,
};
use crate::plugins::{
    ConcertTuning, CrossfeedPreset, DitherMode, DspConfig, EQ_FREQ_LABELS, EQ_FREQS_HZ, EqPreset,
    HeadphonePreset, IrPreset, OversamplingMode, PianoEngineId, PluginConfig, ReplayGainMode,
    ResamplerQuality, TagProviderConfig, ToolHealth, TrackBookmark,
};
use crate::tagger::{
    BatchTagTransform, EditableTags, LookupStrategy, TagCandidate, TrackHashes,
    apply_batch_transform, compute_fast_audio_hash, download_cover_artwork, lookup_online,
    record_hash_cache, write_tags_to_file,
};
use crate::theme::{Palette, ThemeId};
use crate::visualizer::VisualizerMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum ProgressBarStyle {
    #[default]
    AcousticWaveform,
    PrecisionBars,
    StudioRail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum RowDensity {
    #[default]
    Compact,
    Comfortable,
}

impl RowDensity {
    pub fn row_height(self) -> f32 {
        match self {
            RowDensity::Compact => 36.0,
            RowDensity::Comfortable => 42.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Appearance,
    Visualizer,
    Audio,
    Plugins,
    LibraryTags,
}

#[derive(Clone, Copy)]
pub struct FieldPick {
    pub title: bool,
    pub artist: bool,
    pub album: bool,
    pub album_artist: bool,
    pub year: bool,
    pub genre: bool,
    pub track_number: bool,
    pub disc_number: bool,
}

impl Default for FieldPick {
    fn default() -> Self {
        Self {
            title: true,
            artist: true,
            album: true,
            album_artist: true,
            year: true,
            genre: true,
            track_number: true,
            disc_number: true,
        }
    }
}

pub enum TagWorkerEvent {
    HashesReady {
        track_id: u64,
        hashes: Result<TrackHashes, String>,
    },
    LookupReady {
        candidates: Vec<TagCandidate>,
        note: Option<String>,
        updated_hashes: Option<TrackHashes>,
    },
    CoverReady {
        track_ids: Vec<u64>,
        result: Result<PathBuf, String>,
    },
    SaveDone {
        updated_tracks: Vec<Track>,
        disk_written: usize,
        disk_errors: Vec<String>,
    },
}

pub struct TagEditorState {
    pub open: bool,
    pub target_ids: Vec<u64>,
    pub primary_track: Option<Track>,
    pub form: EditableTags,
    pub query_title: String,
    pub query_artist: String,
    pub hashes: Option<TrackHashes>,
    pub hashing: bool,
    pub hash_error: Option<String>,
    pub candidates: Vec<TagCandidate>,
    pub selected_candidate: Option<usize>,
    pub field_pick: FieldPick,
    pub searching: bool,
    pub fetching_cover: bool,
    pub lookup_note: Option<String>,
    pub write_to_file: bool,
    pub saving: bool,
    pub find_text: String,
    pub replace_text: String,
    pub batch_tag_input: String,
    pub tx: Sender<TagWorkerEvent>,
    pub rx: Receiver<TagWorkerEvent>,
}

pub enum TagEditorAction {
    Save(Vec<u64>),
    BatchTransform {
        ids: Vec<u64>,
        transform: BatchTagTransform,
    },
}

impl TagEditorState {
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            open: false,
            target_ids: Vec::new(),
            primary_track: None,
            form: EditableTags::default(),
            query_title: String::new(),
            query_artist: String::new(),
            hashes: None,
            hashing: false,
            hash_error: None,
            candidates: Vec::new(),
            selected_candidate: None,
            field_pick: FieldPick::default(),
            searching: false,
            fetching_cover: false,
            lookup_note: None,
            write_to_file: true,
            saving: false,
            find_text: String::new(),
            replace_text: String::new(),
            batch_tag_input: String::new(),
            tx,
            rx,
        }
    }

    pub fn open_for(&mut self, tracks: &[Track]) {
        if tracks.is_empty() {
            return;
        }
        self.open = true;
        self.target_ids = tracks.iter().map(|t| t.id).collect();
        let primary = tracks[0].clone();
        if tracks.len() == 1 {
            self.form = EditableTags::from_track(&primary);
        } else {
            let common = |f: fn(&Track) -> &str| -> String {
                let first = f(&tracks[0]);
                if tracks.iter().all(|t| f(t) == first) {
                    first.to_string()
                } else {
                    String::new()
                }
            };
            self.form = EditableTags {
                title: String::new(),
                artist: common(|t| &t.artist),
                album: common(|t| &t.album),
                album_artist: common(|t| t.album_artist.as_deref().unwrap_or("")),
                composer: common(|t| t.composer.as_deref().unwrap_or("")),
                conductor: common(|t| t.conductor.as_deref().unwrap_or("")),
                orchestra: common(|t| t.orchestra.as_deref().unwrap_or("")),
                work: common(|t| t.work.as_deref().unwrap_or("")),
                label: common(|t| t.label.as_deref().unwrap_or("")),
                catalog_number: common(|t| t.catalog_number.as_deref().unwrap_or("")),
                comment: common(|t| t.comment.as_deref().unwrap_or("")),
                custom_tags: String::new(),
                year: common(|t| t.year.as_deref().unwrap_or("")),
                genre: common(|t| t.genre.as_deref().unwrap_or("")),
                track_number: String::new(),
                disc_number: String::new(),
            };
        }
        self.query_title = primary.title.clone();
        self.query_artist = if primary.artist.eq_ignore_ascii_case("unknown artist") {
            String::new()
        } else {
            primary.artist.clone()
        };
        self.candidates.clear();
        self.selected_candidate = None;
        self.lookup_note = None;
        self.hashes = None;
        self.hash_error = None;
        self.find_text.clear();
        self.replace_text.clear();
        self.batch_tag_input.clear();
        self.hashing = true;
        self.searching = false;
        self.fetching_cover = false;
        self.saving = false;
        self.primary_track = Some(primary.clone());

        // Fast 256 KB audio-body hash (<1 ms even on huge Hi-Res FLACs); full SHA-256 + fpcalc run lazily on Hash lookup!
        let tx = self.tx.clone();
        let path = primary.path.clone();
        let dur = primary.duration;
        let tid = primary.id;
        thread::spawn(move || {
            let res = compute_fast_audio_hash(&path, dur).map_err(|e| e.to_string());
            let _ = tx.send(TagWorkerEvent::HashesReady {
                track_id: tid,
                hashes: res,
            });
        });
    }

    pub fn trigger_lookup(
        &mut self,
        strategy: LookupStrategy,
        acoustid_key: &str,
        providers: TagProviderConfig,
    ) {
        let Some(track) = self.primary_track.clone() else {
            return;
        };
        if self.searching {
            return;
        }
        self.searching = true;
        self.lookup_note = None;
        self.candidates.clear();
        self.selected_candidate = None;

        let tx = self.tx.clone();
        let q_title = self.query_title.clone();
        let q_artist = self.query_artist.clone();
        let hashes = self.hashes.clone();
        let key = acoustid_key.to_string();

        thread::spawn(move || {
            let (candidates, note, updated_hashes) = lookup_online(
                &track,
                &q_title,
                &q_artist,
                hashes.as_ref(),
                strategy,
                &key,
                &providers,
            );
            let _ = tx.send(TagWorkerEvent::LookupReady {
                candidates,
                note,
                updated_hashes,
            });
        });
    }

    pub fn trigger_cover_fetch(&mut self, artwork_url: String) {
        if self.fetching_cover || self.target_ids.is_empty() {
            return;
        }
        let Some(primary) = &self.primary_track else {
            return;
        };
        self.fetching_cover = true;
        let tid = primary.id;
        let targets = self.target_ids.clone();
        let tx = self.tx.clone();
        let dir = cover_directory();
        thread::spawn(move || {
            let res = download_cover_artwork(&artwork_url, tid, &dir).map_err(|e| e.to_string());
            let _ = tx.send(TagWorkerEvent::CoverReady {
                track_ids: targets,
                result: res,
            });
        });
    }

    pub fn apply_candidate(&mut self, index: usize, all_fields: bool) {
        let Some(c) = self.candidates.get(index).cloned() else {
            return;
        };
        let pick = if all_fields {
            FieldPick::default()
        } else {
            self.field_pick
        };
        if pick.title
            && let Some(v) = c.title
        {
            self.form.title = v;
        }
        if pick.artist
            && let Some(v) = c.artist
        {
            self.form.artist = v;
        }
        if pick.album
            && let Some(v) = c.album
        {
            self.form.album = v;
        }
        if pick.album_artist
            && let Some(v) = c.album_artist
        {
            self.form.album_artist = v;
        }
        if pick.year
            && let Some(v) = c.year
        {
            self.form.year = v;
        }
        if pick.genre
            && let Some(v) = c.genre
        {
            self.form.genre = v;
        }
        if pick.track_number
            && let Some(v) = c.track_number
        {
            self.form.track_number = v.to_string();
        }
        if pick.disc_number
            && let Some(v) = c.disc_number
        {
            self.form.disc_number = v.to_string();
        }
    }

    pub fn trigger_save(&mut self, source_tracks: Vec<Track>) {
        if self.saving || source_tracks.is_empty() {
            return;
        }
        self.saving = true;
        let batch_mode = source_tracks.len() > 1;
        let form = self.form.clone();
        let write_disk = self.write_to_file;
        let hashes = self.hashes.clone();
        let primary_id = self.primary_track.as_ref().map(|t| t.id);
        let tx = self.tx.clone();

        thread::spawn(move || {
            let mut updated = Vec::with_capacity(source_tracks.len());
            let mut disk_written = 0usize;
            let mut disk_errors = Vec::new();
            for mut track in source_tracks {
                form.apply_to_track(&mut track, batch_mode);
                if write_disk {
                    match write_tags_to_file(&track, !batch_mode) {
                        Ok(()) => disk_written += 1,
                        Err(e) => disk_errors.push(format!("{}: {e}", track.title)),
                    }
                }
                if Some(track.id) == primary_id
                    && let Some(h) = &hashes
                {
                    record_hash_cache(h, &track);
                }
                updated.push(track);
            }
            let _ = tx.send(TagWorkerEvent::SaveDone {
                updated_tracks: updated,
                disk_written,
                disk_errors,
            });
        });
    }

    pub fn trigger_batch_transform(
        &mut self,
        source_tracks: Vec<Track>,
        transform: BatchTagTransform,
    ) {
        if self.saving || source_tracks.is_empty() {
            return;
        }
        self.saving = true;
        let write_disk = self.write_to_file;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let mut updated = Vec::with_capacity(source_tracks.len());
            let mut disk_written = 0usize;
            let mut disk_errors = Vec::new();
            for (idx, mut track) in source_tracks.into_iter().enumerate() {
                let changed = apply_batch_transform(&mut track, &transform, idx);
                if changed && write_disk {
                    match write_tags_to_file(&track, false) {
                        Ok(()) => disk_written += 1,
                        Err(e) => disk_errors.push(format!("{}: {e}", track.title)),
                    }
                }
                updated.push(track);
            }
            let _ = tx.send(TagWorkerEvent::SaveDone {
                updated_tracks: updated,
                disk_written,
                disk_errors,
            });
        });
    }
}

/// Renders an aligned checkbox + label pair with strict 22px row height.
pub fn aligned_checkbox_row(
    ui: &mut Ui,
    checked: &mut bool,
    label: &str,
    palette: &Palette,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width().min(540.0), 22.0), Sense::click());
    if response.clicked() {
        *checked = !*checked;
    }
    if ui.is_rect_visible(rect) {
        let cb_cell = Rect::from_min_size(rect.min, vec2(22.0, rect.height()));
        icons::paint_checkbox(ui.painter(), cb_cell, *checked, false, response.hovered(), palette);
        ui.painter().text(
            pos2(cb_cell.right() + 6.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(11.5),
            if response.hovered() {
                palette.text
            } else {
                palette.text_secondary
            },
        );
    }
    response
}

/// Renders a compact inline checkbox + label with exact uniform 22px height (for tag field pickers).
pub fn aligned_inline_checkbox(
    ui: &mut Ui,
    checked: &mut bool,
    label: &str,
    width: f32,
    palette: &Palette,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click());
    if response.clicked() {
        *checked = !*checked;
    }
    if ui.is_rect_visible(rect) {
        let cb_cell = Rect::from_min_size(rect.min, vec2(20.0, rect.height()));
        icons::paint_checkbox(ui.painter(), cb_cell, *checked, false, response.hovered(), palette);
        ui.painter().text(
            pos2(cb_cell.right() + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(11.0),
            if *checked {
                palette.text
            } else {
                palette.muted
            },
        );
    }
    response
}

#[derive(Clone, Copy)]
pub enum PillStyle {
    Accent,
    Active,
    Secondary,
    Danger,
}

pub fn pill_button(
    ui: &mut Ui,
    label: &str,
    icon: Option<Icon>,
    style: PillStyle,
    palette: &Palette,
) -> egui::Response {
    let font = FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), palette.text);
    let icon_space = if icon.is_some() { 18.0 } else { 0.0 };
    let width = galley.size().x + 18.0 + icon_space;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let (fill, stroke_col, fg) = match style {
            PillStyle::Accent => (
                if hovered {
                    palette.accent_bright
                } else {
                    palette.accent
                },
                palette.accent,
                palette.on_accent(),
            ),
            PillStyle::Active => (
                if hovered {
                    palette.accent_dark
                } else {
                    palette.accent_subtle
                },
                palette.selected_border,
                if palette.is_light {
                    palette.accent
                } else {
                    palette.accent_bright
                },
            ),
            PillStyle::Secondary => (
                if hovered {
                    palette.hover
                } else {
                    palette.surface
                },
                if hovered {
                    palette.border_strong
                } else {
                    palette.border
                },
                if hovered {
                    palette.text
                } else {
                    palette.text_secondary
                },
            ),
            PillStyle::Danger => (
                if hovered {
                    palette.danger_bg
                } else {
                    palette.surface
                },
                if hovered {
                    palette.danger_border
                } else {
                    palette.border
                },
                if hovered {
                    palette.danger
                } else {
                    palette.muted
                },
            ),
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        ui.painter()
            .rect_stroke(rect, 6.0, Stroke::new(1.0, stroke_col), StrokeKind::Inside);
        let mut text_x = rect.left() + 9.0;
        if let Some(ic) = icon {
            let icon_rect =
                Rect::from_center_size(pos2(rect.left() + 14.0, rect.center().y), Vec2::splat(12.0));
            icons::paint_icon(ui.painter(), icon_rect, ic, fg);
            text_x = rect.left() + 24.0;
        }
        ui.painter()
            .text(pos2(text_x, rect.center().y), Align2::LEFT_CENTER, label, font, fg);
    }
    response
}

/// Renders a crisp, theme-aligned modal window with a slim 28px custom header bar.
/// Automatically clamps dimensions to the active viewport (so modals NEVER overflow Compact Mode!)
/// and wraps body content in a smooth scroll area when constrained.
#[allow(clippy::too_many_arguments)]
pub fn themed_modal_window(
    context: &egui::Context,
    id: &str,
    title: &str,
    icon: Icon,
    subtitle_right: Option<&str>,
    default_size: [f32; 2],
    min_size: Option<[f32; 2]>,
    resizable: bool,
    open: &mut bool,
    palette: &Palette,
    content: impl FnOnce(&mut Ui),
) {
    if !*open {
        return;
    }
    let screen = context.content_rect();
    let max_w = (screen.width() - 16.0).max(280.0);
    let max_h = (screen.height() - 16.0).max(180.0);
    let win_w = default_size[0].min(max_w);
    let win_h = default_size[1].min(max_h);
    let is_tight = win_w < default_size[0] - 4.0 || win_h < default_size[1] - 4.0;

    let pos_x = (screen.center().x - win_w * 0.5).clamp(screen.left() + 8.0, (screen.right() - win_w - 8.0).max(screen.left() + 8.0));
    let pos_y = (screen.center().y - win_h * 0.5).clamp(screen.top() + 8.0, (screen.bottom() - win_h - 8.0).max(screen.top() + 8.0));

    let mut close_clicked = false;
    let frame = Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(1.0, palette.border_strong))
        .corner_radius(8.0)
        .inner_margin(Margin::ZERO)
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 20,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(
                0,
                0,
                0,
                if palette.is_light { 45 } else { 135 },
            ),
        });

    let mut win = egui::Window::new(id)
        .id(egui::Id::unique(id))
        .title_bar(false)
        .collapsible(false)
        .movable(true)
        .resizable(resizable && !is_tight)
        .frame(frame)
        .constrain_to(screen.shrink(6.0))
        .max_size([max_w, max_h])
        .default_pos(pos2(pos_x, pos_y));

    if is_tight || !resizable {
        win = win.fixed_size([win_w, win_h]);
    } else {
        win = win.default_size([win_w, win_h]);
        if let Some(ms) = min_size {
            win = win.min_size([ms[0].min(max_w), ms[1].min(max_h)]);
        }
    }

    win.show(context, |ui| {
        // Slim 28px custom draggable title strip
        let hdr_h = 28.0;
        let avail_w = ui.available_width();
        let (hdr_rect, hdr_resp) = ui.allocate_exact_size(vec2(avail_w, hdr_h), Sense::hover());
        if hdr_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        let p = ui.painter();
        p.rect_filled(
            hdr_rect,
            egui::CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            },
            palette.sidebar,
        );
        p.line_segment(
            [
                pos2(hdr_rect.left(), hdr_rect.bottom()),
                pos2(hdr_rect.right(), hdr_rect.bottom()),
            ],
            Stroke::new(1.0, palette.border),
        );

        let icon_rect = Rect::from_center_size(
            pos2(hdr_rect.left() + 15.0, hdr_rect.center().y),
            Vec2::splat(12.5),
        );
        icons::paint_icon(p, icon_rect, icon, palette.accent);

        p.text(
            pos2(hdr_rect.left() + 27.0, hdr_rect.center().y),
            Align2::LEFT_CENTER,
            title,
            FontId::proportional(12.0),
            palette.text,
        );

        let close_rect = Rect::from_center_size(
            pos2(hdr_rect.right() - 15.0, hdr_rect.center().y),
            Vec2::splat(20.0),
        );
        let close_resp = ui
            .interact(close_rect, ui.scope_id().with((id, "hdr_close")), Sense::click())
            .on_hover_text("Close (Esc)");
        if close_resp.hovered() {
            ui.painter().rect_filled(close_rect, 4.0, palette.hover);
        }
        icons::paint_icon(
            ui.painter(),
            Rect::from_center_size(close_rect.center(), Vec2::splat(9.5)),
            Icon::Close,
            if close_resp.hovered() {
                palette.text
            } else {
                palette.muted
            },
        );
        if close_resp.clicked() {
            close_clicked = true;
        }

        if let Some(sub) = subtitle_right {
            ui.painter().text(
                pos2(close_rect.left() - 8.0, hdr_rect.center().y),
                Align2::RIGHT_CENTER,
                sub,
                FontId::monospace(9.5),
                palette.faint,
            );
        }

        // Body content with crisp padding & viewport-safe vertical scrolling
        let body_max_h = (win_h - hdr_h - 20.0).max(120.0);
        Frame::new()
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt((id, "modal_body_scroll"))
                    .max_height(body_max_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        content(ui);
                    });
            });
    });

    if close_clicked {
        *open = false;
    }
}

pub struct SettingsViewActions {
    pub theme_changed: Option<ThemeId>,
    pub compact_mode_changed: Option<bool>,
    pub device_changed: Option<Option<String>>,
    pub linear_vol_changed: Option<bool>,
    pub dsp_changed: bool,
    pub rg_changed: bool,
    pub watch_changed: Option<bool>,
    pub open_folders: bool,
    pub open_libraries: bool,
    pub prune_missing: bool,
    pub import_playlist: bool,
    pub import_autoeq: bool,
    pub import_wav_ir: bool,
    pub lan_remote_changed: bool,
    pub save_device_profile: Option<String>,
    pub clear_device_profile: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn show_settings_window(
    context: &egui::Context,
    open: &mut bool,
    tab: &mut SettingsTab,
    palette: &Palette,
    theme_id: ThemeId,
    compact_mode: bool,
    progress_style: &mut ProgressBarStyle,
    row_density: &mut RowDensity,
    close_to_tray: &mut bool,
    auto_watch_folders: &mut bool,
    vis_mode: &mut VisualizerMode,
    show_concert_vis: &mut bool,
    concert_vis_height: &mut f32,
    show_piano_keys: &mut bool,
    vis_sensitivity: &mut f32,
    vis_compute_ms: f32,
    preferred_device: &Option<String>,
    available_devices: &mut Vec<String>,
    linear_volume: &mut bool,
    acoustid_key: &mut String,
    plugins: &mut PluginConfig,
    tool_health: &ToolHealth,
    lan_remote_url: Option<&str>,
    lan_remote_err: Option<&str>,
) -> SettingsViewActions {
    let mut actions = SettingsViewActions {
        theme_changed: None,
        compact_mode_changed: None,
        device_changed: None,
        linear_vol_changed: None,
        dsp_changed: false,
        rg_changed: false,
        watch_changed: None,
        open_folders: false,
        open_libraries: false,
        prune_missing: false,
        import_playlist: false,
        import_autoeq: false,
        import_wav_ir: false,
        lan_remote_changed: false,
        save_device_profile: None,
        clear_device_profile: None,
    };
    if !*open {
        return actions;
    }

    let mut is_open = *open;
    let ver_badge = format!("v{}", env!("CARGO_PKG_VERSION"));
    themed_modal_window(
        context,
        "settings_modal",
        "Settings",
        Icon::Settings,
        Some(&ver_badge),
        [636.0, 498.0],
        None,
        false,
        &mut is_open,
        palette,
        |ui| {
            // Responsive segmented tab bar (wraps cleanly in Compact Mode!)
            let narrow = ui.available_width() < 520.0;
            ui.horizontal_wrapped(|ui| {
                for (t, label, icon) in [
                    (SettingsTab::Appearance, "Appearance", Icon::Palette),
                    (SettingsTab::Visualizer, "Visualizer", Icon::Visualizer),
                    (SettingsTab::Audio, "Audio & DSP", Icon::Sliders),
                    (SettingsTab::Plugins, "Modules", Icon::Plugin),
                    (SettingsTab::LibraryTags, "Library", Icon::Tag),
                ] {
                    let style = if *tab == t {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    };
                    if pill_button(ui, label, Some(icon), style, palette).clicked() {
                        *tab = t;
                    }
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(8.0);

            match *tab {
                SettingsTab::Appearance => {
                    ui.label(
                        RichText::new("WINDOW LAYOUT")
                            .size(10.0)
                            .strong()
                            .color(palette.faint),
                    );
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if pill_button(
                            ui,
                            "Compact Mode",
                            Some(Icon::Compact),
                            if compact_mode {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            palette,
                        )
                        .on_hover_text("Compact mini-player with optional playlist drawer")
                        .clicked()
                            && !compact_mode
                        {
                            actions.compact_mode_changed = Some(true);
                        }
                        if pill_button(
                            ui,
                            "Classic Mode",
                            Some(Icon::Expand),
                            if !compact_mode {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            palette,
                        )
                        .on_hover_text("Full library browser with sidebar, albums, and lyrics")
                        .clicked()
                            && compact_mode
                        {
                            actions.compact_mode_changed = Some(false);
                        }
                    });

                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("COLOR THEME")
                            .size(10.0)
                            .strong()
                            .color(palette.faint),
                    );
                    ui.add_space(4.0);
                    let cols = if narrow { 1 } else { 2 };
                    let card_w = if narrow {
                        (ui.available_width() - 4.0).max(240.0)
                    } else {
                        ((ui.available_width() - 12.0) * 0.5).clamp(220.0, 290.0)
                    };
                    let card_h = if narrow { 40.0 } else { 44.0 };
                    egui::Grid::new("theme_grid")
                        .num_columns(cols)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            for (idx, &tid) in ThemeId::ALL.iter().enumerate() {
                                let tp = tid.palette();
                                let active = tid == theme_id;
                                let (rect, resp) =
                                    ui.allocate_exact_size(vec2(card_w, card_h), Sense::click());
                                let hovered = resp.hovered();
                                ui.painter().rect_filled(
                                    rect,
                                    6.0,
                                    if active {
                                        palette.selected
                                    } else if hovered {
                                        palette.hover
                                    } else {
                                        palette.surface
                                    },
                                );
                                ui.painter().rect_stroke(
                                    rect,
                                    6.0,
                                    Stroke::new(
                                        if active { 1.4 } else { 1.0 },
                                        if active {
                                            palette.accent
                                        } else {
                                            palette.border
                                        },
                                    ),
                                    StrokeKind::Inside,
                                );
                                // Swatch preview chips
                                let swatches = [tp.background, tp.sidebar, tp.accent, tp.hires_gold];
                                for (si, col) in swatches.iter().enumerate() {
                                    let sr = Rect::from_min_size(
                                        pos2(rect.left() + 10.0 + si as f32 * 13.0, rect.center().y - 5.5),
                                        Vec2::splat(11.0),
                                    );
                                    ui.painter().rect_filled(sr, 3.0, *col);
                                    ui.painter().rect_stroke(
                                        sr,
                                        3.0,
                                        Stroke::new(0.6, palette.border_strong),
                                        StrokeKind::Inside,
                                    );
                                }
                                let text_clip = Rect::from_min_max(
                                    pos2(rect.left() + 66.0, rect.top()),
                                    pos2(rect.right() - 8.0, rect.bottom()),
                                );
                                ui.painter().with_clip_rect(text_clip).text(
                                    pos2(rect.left() + 68.0, rect.top() + card_h * 0.34),
                                    Align2::LEFT_CENTER,
                                    tid.name(),
                                    FontId::proportional(11.5),
                                    palette.text,
                                );
                                ui.painter().with_clip_rect(text_clip).text(
                                    pos2(rect.left() + 68.0, rect.top() + card_h * 0.70),
                                    Align2::LEFT_CENTER,
                                    tid.subtitle(),
                                    FontId::proportional(9.5),
                                    palette.muted,
                                );
                                if resp.clicked() {
                                    actions.theme_changed = Some(tid);
                                }
                                if cols == 1 || idx % cols == cols - 1 {
                                    ui.end_row();
                                }
                            }
                        });

                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("TIMELINE & TRACK LIST")
                            .size(10.0)
                            .strong()
                            .color(palette.faint),
                    );
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Track Progress:")
                                .size(11.5)
                                .color(palette.text_secondary),
                        );
                        for (lbl, st) in [
                            ("Waveform", ProgressBarStyle::AcousticWaveform),
                            ("Uniform Bars", ProgressBarStyle::PrecisionBars),
                            ("Minimal Rail", ProgressBarStyle::StudioRail),
                        ] {
                            if pill_button(
                                ui,
                                lbl,
                                None,
                                if *progress_style == st {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .clicked()
                            {
                                *progress_style = st;
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Row Density:")
                                .size(11.5)
                                .color(palette.text_secondary),
                        );
                        if pill_button(
                            ui,
                            "Compact (36px)",
                            None,
                            if *row_density == RowDensity::Compact {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            palette,
                        )
                        .clicked()
                        {
                            *row_density = RowDensity::Compact;
                        }
                        if pill_button(
                            ui,
                            "Comfortable (42px)",
                            None,
                            if *row_density == RowDensity::Comfortable {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            palette,
                        )
                        .clicked()
                        {
                            *row_density = RowDensity::Comfortable;
                        }
                    });
                    ui.add_space(6.0);
                    aligned_checkbox_row(
                        ui,
                        close_to_tray,
                        "Minimize to system tray when closing window",
                        palette,
                    );
                }
                SettingsTab::Visualizer => {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("VISUALIZER")
                                .size(10.0)
                                .strong()
                                .color(palette.faint),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.2} ms/frame", vis_compute_ms))
                                    .size(9.5)
                                    .monospace()
                                    .color(palette.accent),
                            );
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        for &m in VisualizerMode::ALL {
                            if pill_button(
                                ui,
                                m.name(),
                                None,
                                if *vis_mode == m {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .clicked()
                            {
                                *vis_mode = m;
                            }
                        }
                    });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("88-KEY PIANO DETECTION ENGINE")
                            .size(10.0)
                            .strong()
                            .color(palette.faint),
                    );
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for &eng in PianoEngineId::ALL {
                            if pill_button(
                                ui,
                                eng.name(),
                                None,
                                if plugins.visualizers.piano_engine == eng {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .on_hover_text(eng.description())
                            .clicked()
                            {
                                plugins.visualizers.piano_engine = eng;
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(plugins.visualizers.piano_engine.description())
                            .size(10.5)
                            .color(palette.muted),
                    );
                    ui.add_space(6.0);
                    aligned_checkbox_row(
                        ui,
                        show_concert_vis,
                        "Show expanded Concert Visualizer stage above transport deck",
                        palette,
                    );
                    aligned_checkbox_row(
                        ui,
                        show_piano_keys,
                        "Show live 88-key Concert Grand Piano keybed in Piano Ribbon mode",
                        palette,
                    );
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.visualizers.harmonic_cancellation,
                        "Enable iterative harmonic overtone cancellation (suppresses false octave ghosts)",
                        palette,
                    );
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.visualizers.show_chord_readout,
                        "Show real-time detected notes & chord badge on Piano stage",
                        palette,
                    );
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Concert Stage Height:")
                                .size(11.5)
                                .color(palette.text_secondary),
                        );
                        for (label, h) in [("Compact", 96.0_f32), ("Studio", 132.0), ("Grand", 176.0)] {
                            if pill_button(
                                ui,
                                label,
                                None,
                                if (*concert_vis_height - h).abs() < 4.0 {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .clicked()
                            {
                                *concert_vis_height = h;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("Harmonic Sensitivity: {:.0}%", *vis_sensitivity * 100.0))
                                .size(11.5)
                                .color(palette.text_secondary),
                        );
                        ui.add(
                            egui::Slider::new(vis_sensitivity, 0.5..=2.0)
                                .show_value(false)
                                .trailing_fill(true),
                        );
                    });
                }
                SettingsTab::Audio => {
                    ui.label(
                        RichText::new(format!("{} OUTPUT DEVICE", AUDIO_BACKEND_NAME.to_uppercase()))
                            .size(10.0)
                            .strong()
                            .color(palette.faint),
                    );
                    ui.add_space(4.0);
                    Frame::new()
                        .fill(palette.background)
                        .stroke(Stroke::new(1.0, palette.border))
                        .corner_radius(6.0)
                        .inner_margin(Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ScrollArea::vertical().max_height(105.0).show(ui, |ui| {
                                let is_default = preferred_device.is_none();
                                let default_label = match default_output_device_name() {
                                    Some(name) => format!("System Default ({name})"),
                                    None => "System Default (Auto-follow headphones & Bluetooth)".into(),
                                };
                                if ui.selectable_label(is_default, default_label).clicked() {
                                    actions.device_changed = Some(None);
                                }
                                ui.separator();
                                for dev in available_devices.iter() {
                                    let selected = preferred_device.as_deref() == Some(dev.as_str());
                                    if ui.selectable_label(selected, dev).clicked() {
                                        actions.device_changed = Some(Some(dev.clone()));
                                    }
                                }
                            });
                        });
                    ui.add_space(5.0);
                    ui.horizontal_wrapped(|ui| {
                        if pill_button(
                            ui,
                            "Refresh Output Devices",
                            Some(Icon::Refresh),
                            PillStyle::Secondary,
                            palette,
                        )
                        .clicked()
                        {
                            *available_devices = list_output_devices();
                        }
                        let prev_lin = *linear_volume;
                        aligned_checkbox_row(
                            ui,
                            linear_volume,
                            "Linear volume curve",
                            palette,
                        );
                        if *linear_volume != prev_lin {
                            actions.linear_vol_changed = Some(*linear_volume);
                        }
                    });

                    // Per-Output Device Audio Profile Bar
                    let active_dev_name = preferred_device
                        .clone()
                        .or_else(default_output_device_name)
                        .unwrap_or_else(|| "System Default".to_string());
                    ui.add_space(6.0);
                    Frame::new()
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0, palette.border))
                        .corner_radius(6.0)
                        .inner_margin(Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new("DEVICE PROFILE")
                                        .size(9.5)
                                        .strong()
                                        .color(palette.faint),
                                );
                                ui.label(
                                    RichText::new(&active_dev_name)
                                        .size(10.5)
                                        .strong()
                                        .color(palette.text),
                                );
                                if let Some(saved) = plugins.device_profiles.get(&active_dev_name) {
                                    ui.label(
                                        RichText::new(format!("({})", saved.summary()))
                                            .size(10.0)
                                            .monospace()
                                            .color(palette.accent),
                                    );
                                } else {
                                    ui.label(
                                        RichText::new("(No saved profile)")
                                            .size(10.0)
                                            .color(palette.muted),
                                    );
                                }
                            });
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                if pill_button(
                                    ui,
                                    "Save Current Audio Setup for Device",
                                    Some(Icon::Device),
                                    PillStyle::Active,
                                    palette,
                                )
                                .on_hover_text(
                                    "Auto-applies Exclusive Mode, 0 dB Lock, Oversampling, Crossfeed & Headphone AutoEQ whenever this output is active",
                                )
                                .clicked()
                                {
                                    actions.save_device_profile = Some(active_dev_name.clone());
                                }
                                if plugins.device_profiles.contains_key(&active_dev_name)
                                    && pill_button(
                                        ui,
                                        "Clear Profile",
                                        Some(Icon::Close),
                                        PillStyle::Danger,
                                        palette,
                                    )
                                    .clicked()
                                {
                                    actions.clear_device_profile = Some(active_dev_name.clone());
                                }
                            });
                        });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);

                    // Headphone AutoEQ & 16-Band Parametric EQ
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("HEADPHONE AUTOEQ & PARAMETRIC EQ (64-BIT)")
                                .size(10.0)
                                .strong()
                                .color(palette.faint),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if pill_button(
                                ui,
                                "Import AutoEQ .txt…",
                                Some(Icon::FolderPlus),
                                PillStyle::Secondary,
                                palette,
                            )
                            .on_hover_text(
                                "Import any Oratory1990 / AutoEQ / EqualizerAPO ParametricEQ .txt headphone correction profile",
                            )
                            .clicked()
                            {
                                actions.import_autoeq = true;
                            }
                            if !plugins.dsp.peq_bands.is_empty()
                                && pill_button(
                                    ui,
                                    if plugins.dsp.peq_enabled && plugins.dsp.enabled {
                                        "PEQ: On"
                                    } else {
                                        "PEQ: Off"
                                    },
                                    None,
                                    if plugins.dsp.peq_enabled && plugins.dsp.enabled {
                                        PillStyle::Accent
                                    } else {
                                        PillStyle::Secondary
                                    },
                                    palette,
                                )
                                .clicked()
                            {
                                plugins.dsp.peq_enabled = !plugins.dsp.peq_enabled;
                                if plugins.dsp.peq_enabled {
                                    plugins.dsp.enabled = true;
                                    plugins.dsp.bit_perfect_mode = false;
                                }
                                actions.dsp_changed = true;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for &hp in HeadphonePreset::ALL {
                            let active = plugins.dsp.peq_enabled
                                && plugins.dsp.peq_profile_name == hp.profile().0;
                            if pill_button(
                                ui,
                                hp.name(),
                                None,
                                if active {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .clicked()
                            {
                                plugins.dsp.apply_headphone_preset(hp);
                                actions.dsp_changed = true;
                            }
                        }
                        if !plugins.dsp.peq_bands.is_empty()
                            && pill_button(ui, "Clear PEQ", None, PillStyle::Danger, palette)
                                .clicked()
                        {
                            plugins.dsp.peq_enabled = false;
                            plugins.dsp.peq_profile_name.clear();
                            plugins.dsp.peq_bands.clear();
                            plugins.dsp.peq_preamp_db = 0.0;
                            actions.dsp_changed = true;
                        }
                    });
                    if !plugins.dsp.peq_bands.is_empty() {
                        ui.add_space(4.0);
                        let summary_line = plugins
                            .dsp
                            .peq_bands
                            .iter()
                            .filter(|b| b.enabled)
                            .take(6)
                            .map(|b| {
                                format!(
                                    "{} {:.0}Hz {:+.1}dB",
                                    b.kind.short_label(),
                                    b.freq_hz,
                                    b.gain_db
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" · ");
                        ui.label(
                            RichText::new(format!(
                                "{} · Preamp {:+.1} dB · {} bands ({summary_line})",
                                if plugins.dsp.peq_profile_name.is_empty() {
                                    "Parametric Profile"
                                } else {
                                    &plugins.dsp.peq_profile_name
                                },
                                plugins.dsp.peq_preamp_db,
                                plugins.dsp.peq_bands.len(),
                            ))
                            .size(9.8)
                            .monospace()
                            .color(if plugins.dsp.peq_enabled {
                                palette.accent
                            } else {
                                palette.muted
                            }),
                        );
                    }

                    // Live interactive drag-to-shape Graphic + Parametric Frequency Response Curve
                    ui.add_space(6.0);
                    if interactive_eq_response_canvas(ui, &mut plugins.dsp, palette) {
                        actions.dsp_changed = true;
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);

                    // DSP & Equalizer
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("EQUALIZER & CROSSFEED")
                                .size(10.0)
                                .strong()
                                .color(palette.faint),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if pill_button(
                                ui,
                                if plugins.dsp.enabled {
                                    "DSP Active"
                                } else {
                                    "DSP Bypassed"
                                },
                                Some(Icon::Sliders),
                                if plugins.dsp.enabled {
                                    PillStyle::Accent
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .clicked()
                            {
                                plugins.dsp.enabled = !plugins.dsp.enabled;
                                actions.dsp_changed = true;
                            }
                        });
                    });
                    ui.add_space(6.0);

                    if paint_dsp_controls(ui, &mut plugins.dsp, palette) {
                        actions.dsp_changed = true;
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("PLAYBACK ENGINE · GAPLESS · BIT-PERFECT · REPLAYGAIN")
                            .size(10.0)
                            .strong()
                            .color(palette.faint),
                    );
                    ui.add_space(4.0);
                    let prev_excl = plugins.dsp.exclusive_mode;
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.dsp.exclusive_mode,
                        "Exclusive DAC Mode (macOS CoreAudio Hog Mode + Hardware Auto Clock Switching)",
                        palette,
                    );
                    let prev_bp = plugins.dsp.bit_perfect_mode;
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.dsp.bit_perfect_mode,
                        "Bit-Perfect Direct Output (bypass DSP & ReplayGain, native 1:1 DAC sample rate)",
                        palette,
                    );
                    let prev_dvl = plugins.dsp.direct_volume_lock;
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.dsp.direct_volume_lock,
                        "Direct DAC Volume Lock (0.0 dBFS fixed digital output for external hardware amp/DAC)",
                        palette,
                    );
                    let prev_dop = plugins.dsp.dsd_dop;
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.dsp.dsd_dop,
                        "Native DSD over PCM (DoP v1.1 · 0x05/0xFA framing at 176.4/352.8 kHz when Exclusive DAC locked)",
                        palette,
                    );
                    if plugins.dsp.dsd_dop != prev_dop {
                        actions.dsp_changed = true;
                    }
                    let prev_ram = plugins.dsp.ram_playback;
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.dsp.ram_playback,
                        "Play from RAM (instant-start async buffering + background next-track preload · auto-capped to free RAM)",
                        palette,
                    );
                    if plugins.dsp.ram_playback {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(22.0);
                            ui.label(
                                RichText::new("RAM Buffer Limit:")
                                    .size(10.5)
                                    .color(palette.muted),
                            );
                            for &(mb, lbl) in &[
                                (256_u32, "256 MB"),
                                (512, "512 MB"),
                                (1024, "1 GB"),
                                (2048, "2 GB"),
                            ] {
                                if pill_button(
                                    ui,
                                    lbl,
                                    None,
                                    if plugins.dsp.ram_buffer_mb == mb {
                                        PillStyle::Active
                                    } else {
                                        PillStyle::Secondary
                                    },
                                    palette,
                                )
                                .clicked()
                                {
                                    plugins.dsp.ram_buffer_mb = mb;
                                    actions.dsp_changed = true;
                                }
                            }
                        });
                    }
                    let prev_gap = plugins.dsp.gapless_enabled;
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.dsp.gapless_enabled,
                        "True Gapless Playback (zero-silence transition between tracks & CUE sheets)",
                        palette,
                    );
                    if plugins.dsp.exclusive_mode != prev_excl
                        || plugins.dsp.bit_perfect_mode != prev_bp
                        || plugins.dsp.direct_volume_lock != prev_dvl
                        || plugins.dsp.ram_playback != prev_ram
                        || plugins.dsp.gapless_enabled != prev_gap
                    {
                        actions.dsp_changed = true;
                        actions.rg_changed = true;
                    }

                    ui.add_space(5.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Oversampling:")
                                .size(11.0)
                                .color(palette.text_secondary),
                        );
                        for &mode in OversamplingMode::ALL {
                            let active = if plugins.dsp.bit_perfect_mode {
                                mode == OversamplingMode::Native
                            } else {
                                plugins.dsp.oversampling == mode
                            };
                            if pill_button(
                                ui,
                                mode.name(),
                                None,
                                if active {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .on_hover_text(mode.description())
                            .clicked()
                            {
                                plugins.dsp.oversampling = mode;
                                if mode != OversamplingMode::Native {
                                    plugins.dsp.bit_perfect_mode = false;
                                }
                                actions.dsp_changed = true;
                            }
                        }
                    });
                    if !plugins.dsp.bit_perfect_mode && plugins.dsp.oversampling != OversamplingMode::Native {
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(16.0);
                            ui.label(
                                RichText::new("Sinc Filter:")
                                    .size(10.5)
                                    .color(palette.muted),
                            );
                            for &q in ResamplerQuality::ALL {
                                if pill_button(
                                    ui,
                                    q.name(),
                                    None,
                                    if plugins.dsp.resampler_quality == q {
                                        PillStyle::Active
                                    } else {
                                        PillStyle::Secondary
                                    },
                                    palette,
                                )
                                .on_hover_text(q.detail())
                                .clicked()
                                {
                                    plugins.dsp.resampler_quality = q;
                                    actions.dsp_changed = true;
                                }
                            }
                        });
                    }

                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Output Dither:")
                                .size(11.0)
                                .color(palette.text_secondary),
                        );
                        for &dm in DitherMode::ALL {
                            let active = if plugins.dsp.bit_perfect_mode {
                                dm == DitherMode::Off
                            } else {
                                plugins.dsp.dither == dm
                            };
                            if pill_button(
                                ui,
                                dm.name(),
                                None,
                                if active {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .on_hover_text(dm.detail())
                            .clicked()
                            {
                                plugins.dsp.dither = dm;
                                if dm != DitherMode::Off {
                                    plugins.dsp.bit_perfect_mode = false;
                                }
                                actions.dsp_changed = true;
                            }
                        }
                    });

                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Convolution IR:")
                                .size(11.0)
                                .color(palette.text_secondary),
                        );
                        let active_ir = if plugins.dsp.bit_perfect_mode || !plugins.dsp.ir_enabled {
                            IrPreset::Off
                        } else {
                            plugins.dsp.ir_preset
                        };
                        for &ir in IrPreset::BUILTIN {
                            if pill_button(
                                ui,
                                ir.name(),
                                None,
                                if active_ir == ir {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .on_hover_text(
                                "Real-time 64-bit partitioned FFT stereo convolution engine",
                            )
                            .clicked()
                            {
                                plugins.dsp.ir_preset = ir;
                                plugins.dsp.ir_enabled = ir != IrPreset::Off;
                                if ir != IrPreset::Off {
                                    plugins.dsp.enabled = true;
                                    plugins.dsp.bit_perfect_mode = false;
                                }
                                actions.dsp_changed = true;
                            }
                        }
                        if pill_button(
                            ui,
                            if active_ir == IrPreset::CustomWav && !plugins.dsp.ir_name.is_empty() {
                                "Change .WAV IR…"
                            } else {
                                "Load .WAV IR…"
                            },
                            Some(Icon::Folder),
                            if active_ir == IrPreset::CustomWav {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            palette,
                        )
                        .on_hover_text(
                            "Load a stereo or mono .wav Impulse Response (room correction, speaker/headphone EQ, or hall reverb)",
                        )
                        .clicked()
                        {
                            actions.import_wav_ir = true;
                        }
                        if active_ir == IrPreset::CustomWav && !plugins.dsp.ir_name.is_empty() {
                            ui.label(
                                RichText::new(format!("Active: {}", plugins.dsp.ir_name))
                                    .size(10.0)
                                    .color(palette.hires_gold),
                            );
                        }
                    });
                    if plugins.dsp.ir_enabled
                        && plugins.dsp.ir_preset != IrPreset::Off
                        && !plugins.dsp.bit_perfect_mode
                    {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "IR Wet/Dry: {:.0}%",
                                    plugins.dsp.ir_mix * 100.0
                                ))
                                .size(11.0)
                                .color(palette.text_secondary),
                            );
                            if ui
                                .add(
                                    egui::Slider::new(&mut plugins.dsp.ir_mix, 0.0..=1.0)
                                        .step_by(0.05)
                                        .show_value(false),
                                )
                                .changed()
                            {
                                actions.dsp_changed = true;
                            }
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(format!(
                                    "IR Gain: {:+.1} dB",
                                    plugins.dsp.ir_gain_db
                                ))
                                .size(11.0)
                                .color(palette.text_secondary),
                            );
                            if ui
                                .add(
                                    egui::Slider::new(&mut plugins.dsp.ir_gain_db, -12.0..=6.0)
                                        .step_by(0.5)
                                        .show_value(false),
                                )
                                .changed()
                            {
                                actions.dsp_changed = true;
                            }
                        });
                    }

                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        let bal = plugins.dsp.balance;
                        let bal_lbl = if bal.abs() < 0.015 {
                            "Balance: Center".to_string()
                        } else if bal < 0.0 {
                            format!("Balance: L {:.0}%", -bal * 100.0)
                        } else {
                            format!("Balance: R {:.0}%", bal * 100.0)
                        };
                        ui.label(
                            RichText::new(bal_lbl)
                                .size(11.0)
                                .color(palette.text_secondary),
                        );
                        let bal_resp = ui
                            .add(
                                egui::Slider::new(&mut plugins.dsp.balance, -1.0..=1.0)
                                    .step_by(0.05)
                                    .show_value(false),
                            )
                            .on_hover_text("Constant-power L/R balance (double-click to center)");
                        if bal_resp.double_clicked() {
                            plugins.dsp.balance = 0.0;
                            actions.dsp_changed = true;
                        } else if bal_resp.changed() {
                            if plugins.dsp.balance.abs() >= 0.01 {
                                plugins.dsp.enabled = true;
                                plugins.dsp.bit_perfect_mode = false;
                            }
                            actions.dsp_changed = true;
                        }
                        if pill_button(
                            ui,
                            "Mono",
                            None,
                            if plugins.dsp.mono_fold && !plugins.dsp.bit_perfect_mode {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            palette,
                        )
                        .on_hover_text("Fold stereo to mono (0.5*(L+R)) for phase & mix checks")
                        .clicked()
                        {
                            plugins.dsp.mono_fold = !plugins.dsp.mono_fold;
                            if plugins.dsp.mono_fold {
                                plugins.dsp.enabled = true;
                                plugins.dsp.bit_perfect_mode = false;
                            }
                            actions.dsp_changed = true;
                        }
                        if pill_button(
                            ui,
                            "Ø Polarity",
                            None,
                            if plugins.dsp.invert_polarity && !plugins.dsp.bit_perfect_mode {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            palette,
                        )
                        .on_hover_text("Invert absolute phase (180° polarity flip)")
                        .clicked()
                        {
                            plugins.dsp.invert_polarity = !plugins.dsp.invert_polarity;
                            if plugins.dsp.invert_polarity {
                                plugins.dsp.enabled = true;
                                plugins.dsp.bit_perfect_mode = false;
                            }
                            actions.dsp_changed = true;
                        }
                        if pill_button(
                            ui,
                            "L ↔ R",
                            None,
                            if plugins.dsp.swap_channels && !plugins.dsp.bit_perfect_mode {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            palette,
                        )
                        .on_hover_text("Swap Left and Right stereo channels")
                        .clicked()
                        {
                            plugins.dsp.swap_channels = !plugins.dsp.swap_channels;
                            if plugins.dsp.swap_channels {
                                plugins.dsp.enabled = true;
                                plugins.dsp.bit_perfect_mode = false;
                            }
                            actions.dsp_changed = true;
                        }
                    });

                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "Pitch: {:+.1} st",
                                plugins.dsp.pitch_semitones
                            ))
                            .size(11.0)
                            .color(palette.text_secondary),
                        );
                        let p_resp = ui
                            .add(
                                egui::Slider::new(&mut plugins.dsp.pitch_semitones, -12.0..=12.0)
                                    .step_by(0.5)
                                    .show_value(false),
                            )
                            .on_hover_text(
                                "Real-time musical transpose (-12..+12 semitones, double-click to reset)",
                            );
                        if p_resp.double_clicked() {
                            plugins.dsp.pitch_semitones = 0.0;
                            actions.dsp_changed = true;
                        } else if p_resp.changed() {
                            if plugins.dsp.pitch_semitones.abs() >= 0.05 {
                                plugins.dsp.enabled = true;
                                plugins.dsp.bit_perfect_mode = false;
                            }
                            actions.dsp_changed = true;
                        }
                        if pill_button(
                            ui,
                            "Pitch-Lock",
                            None,
                            if plugins.dsp.preserve_pitch && !plugins.dsp.bit_perfect_mode {
                                PillStyle::Active
                            } else {
                                PillStyle::Secondary
                            },
                            palette,
                        )
                        .on_hover_text(
                            "Preserve original musical key when slowing down or speeding up for practice",
                        )
                        .clicked()
                        {
                            plugins.dsp.preserve_pitch = !plugins.dsp.preserve_pitch;
                            if plugins.dsp.preserve_pitch {
                                plugins.dsp.enabled = true;
                                plugins.dsp.bit_perfect_mode = false;
                            }
                            actions.dsp_changed = true;
                        }
                        for &ct in ConcertTuning::ALL {
                            if pill_button(
                                ui,
                                ct.name(),
                                None,
                                if plugins.dsp.concert_tuning == ct && !plugins.dsp.bit_perfect_mode
                                {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .on_hover_text("Concert reference pitch (Standard 440, Verdi 432, Baroque 415 Hz)")
                            .clicked()
                            {
                                plugins.dsp.concert_tuning = ct;
                                if ct != ConcertTuning::Standard440 {
                                    plugins.dsp.enabled = true;
                                    plugins.dsp.bit_perfect_mode = false;
                                }
                                actions.dsp_changed = true;
                            }
                        }
                    });

                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("Crossfade: {:.1}s", plugins.dsp.crossfade_secs))
                                .size(11.0)
                                .color(palette.text_secondary),
                        );
                        if ui
                            .add(
                                egui::Slider::new(&mut plugins.dsp.crossfade_secs, 0.0..=6.0)
                                    .step_by(0.5)
                                    .show_value(false),
                            )
                            .changed()
                        {
                            actions.dsp_changed = true;
                        }
                        ui.label(
                            RichText::new(if plugins.dsp.crossfade_secs <= 0.05 {
                                "Off (pure gapless)"
                            } else {
                                "Equal-power fade"
                            })
                            .size(10.0)
                            .color(palette.faint),
                        );
                    });

                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("ReplayGain:")
                                .size(11.0)
                                .color(palette.text_secondary),
                        );
                        for &mode in ReplayGainMode::ALL {
                            if pill_button(
                                ui,
                                mode.name(),
                                None,
                                if plugins.dsp.replay_gain == mode {
                                    PillStyle::Active
                                } else {
                                    PillStyle::Secondary
                                },
                                palette,
                            )
                            .clicked()
                            {
                                plugins.dsp.replay_gain = mode;
                                actions.rg_changed = true;
                            }
                        }
                        ui.add_space(6.0);
                        let prev_clip = plugins.dsp.rg_prevent_clipping;
                        aligned_inline_checkbox(
                            ui,
                            &mut plugins.dsp.rg_prevent_clipping,
                            "Peak Guard",
                            98.0,
                            palette,
                        );
                        if plugins.dsp.rg_prevent_clipping != prev_clip {
                            actions.rg_changed = true;
                        }
                    });
                    if plugins.dsp.replay_gain != ReplayGainMode::Off {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "RG Preamp: {:+.1} dB",
                                    plugins.dsp.rg_preamp_db
                                ))
                                .size(11.0)
                                .color(palette.text_secondary),
                            );
                            if ui
                                .add(
                                    egui::Slider::new(&mut plugins.dsp.rg_preamp_db, -6.0..=6.0)
                                        .step_by(0.5)
                                        .show_value(false),
                                )
                                .changed()
                            {
                                actions.rg_changed = true;
                            }
                        });
                    }
                }
                SettingsTab::Plugins => {
                    ui.label(
                        RichText::new("MODULES & TOOLS")
                            .size(10.0)
                            .strong()
                            .color(palette.faint),
                    );
                    ui.add_space(6.0);

                    // 1. Piano Pitch Detection Engine Module
                    plugin_section_card(ui, "PIANO PITCH ENGINE", palette, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for &eng in PianoEngineId::ALL {
                                if pill_button(
                                    ui,
                                    eng.short_name(),
                                    None,
                                    if plugins.visualizers.piano_engine == eng {
                                        PillStyle::Active
                                    } else {
                                        PillStyle::Secondary
                                    },
                                    palette,
                                )
                                .clicked()
                                {
                                    plugins.visualizers.piano_engine = eng;
                                }
                            }
                            status_badge(
                                ui,
                                &format!("{:.2} ms/f", vis_compute_ms),
                                true,
                                palette,
                            );
                        });
                        ui.label(
                            RichText::new(plugins.visualizers.piano_engine.description())
                                .size(10.5)
                                .color(palette.muted),
                        );
                    });
                    ui.add_space(6.0);

                    // 2. Visualizer Modules
                    plugin_section_card(ui, "VISUALIZER MODULES (QUICK-CYCLE ROSTER)", palette, |ui| {
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.visualizers.enable_piano_harmonic,
                            "Piano & Harmonic Ribbon (88-key spline + live keybed)",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.visualizers.enable_piano_roll,
                            "88-Key Concert Piano Roll (real-time note waterfall)",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.visualizers.enable_concert_spectrum,
                            "64-Band Concert Spectrum (27.5 Hz – 18 kHz)",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.visualizers.enable_spectrogram,
                            "Live Acoustic Spectrogram (2D scrolling sonogram)",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.visualizers.enable_stereo_scope,
                            "Stereo Scope & M/S Phase Goniometer",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.visualizers.enable_mastering_vu,
                            "Mastering VU & Phase Correlation",
                            palette,
                        );
                    });
                    ui.add_space(6.0);

                    // 3. Real-Time Audio DSP Modules
                    plugin_section_card(ui, "AUDIO DSP MODULES", palette, |ui| {
                        let prev_en = plugins.dsp.enabled;
                        let prev_eq = plugins.dsp.eq_enabled;
                        let prev_cf = plugins.dsp.crossfeed_enabled;
                        let prev_lim = plugins.dsp.soft_limiter;
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.dsp.enabled,
                            "Master Audio DSP Chain (disable for bit-direct bypass)",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.dsp.eq_enabled,
                            "10-Band Audiophile Parametric EQ (64-bit Direct Form II)",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.dsp.crossfeed_enabled,
                            "Headphone Acoustic Crossfeed (natural speaker soundstage)",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.dsp.soft_limiter,
                            "Transparent Soft-Knee Limiter (-0.7 dBFS clip guard)",
                            palette,
                        );
                        if plugins.dsp.enabled != prev_en
                            || plugins.dsp.eq_enabled != prev_eq
                            || plugins.dsp.crossfeed_enabled != prev_cf
                            || plugins.dsp.soft_limiter != prev_lim
                        {
                            actions.dsp_changed = true;
                        }
                    });
                    ui.add_space(6.0);

                    // 4. Metadata & Tag Provider Modules + Tool Health
                    plugin_section_card(ui, "TAG PROVIDERS & SYSTEM TOOLS", palette, |ui| {
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.tag_providers.local_hash_cache,
                            "Local Hash Cache (instant offline repeat matching)",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.tag_providers.musicbrainz,
                            "MusicBrainz Web Service (classical & recording metadata)",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.tag_providers.itunes,
                            "iTunes Search + 600px Cover Art Fetcher",
                            palette,
                        );
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.tag_providers.acoustid,
                            "AcoustID Chromaprint Fingerprinting",
                            palette,
                        );
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            status_badge(
                                ui,
                                if tool_health.ffmpeg_path.is_some() {
                                    "ffmpeg ✓ (tag writes)"
                                } else {
                                    "ffmpeg ✗ (optional)"
                                },
                                tool_health.ffmpeg_path.is_some(),
                                palette,
                            );
                            status_badge(
                                ui,
                                if tool_health.fpcalc_path.is_some() {
                                    "fpcalc ✓ (AcoustID)"
                                } else {
                                    "fpcalc ✗ (optional)"
                                },
                                tool_health.fpcalc_path.is_some(),
                                palette,
                            );
                            status_badge(
                                ui,
                                if tool_health.curl_available {
                                    "curl ✓ (online lookup)"
                                } else {
                                    "curl ✗"
                                },
                                tool_health.curl_available,
                                palette,
                            );
                        });
                    });
                    ui.add_space(6.0);

                    // 5. Optional 100% Local LAN Mobile Remote Control
                    plugin_section_card(ui, "LOCAL LAN MOBILE REMOTE (ZERO CLOUD)", palette, |ui| {
                        let prev_en = plugins.lan_remote_enabled;
                        aligned_checkbox_row(
                            ui,
                            &mut plugins.lan_remote_enabled,
                            "Enable Local Wi-Fi Web Remote (control from phone/tablet on your LAN)",
                            palette,
                        );
                        if plugins.lan_remote_enabled != prev_en {
                            actions.lan_remote_changed = true;
                        }
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new("Port:")
                                    .size(11.0)
                                    .color(palette.text_secondary),
                            );
                            let mut port_val = plugins.lan_remote_port as i32;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut port_val)
                                        .range(1024..=65535)
                                        .speed(1),
                                )
                                .changed()
                            {
                                plugins.lan_remote_port = port_val as u16;
                                if plugins.lan_remote_enabled {
                                    actions.lan_remote_changed = true;
                                }
                            }
                            if let Some(url) = lan_remote_url {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(format!("Live: {url}"))
                                        .size(11.0)
                                        .monospace()
                                        .color(palette.hires_gold),
                                );
                                if pill_button(ui, "Copy URL", None, PillStyle::Active, palette)
                                    .clicked()
                                {
                                    ui.ctx().copy_text(url.to_string());
                                }
                            } else if let Some(err) = lan_remote_err {
                                ui.label(RichText::new(err).size(10.5).color(palette.danger));
                            }
                        });
                    });
                }
                SettingsTab::LibraryTags => {
                    ui.label(
                        RichText::new("LIBRARIES & DIRECTORIES")
                            .size(10.0)
                            .strong()
                            .color(palette.faint),
                    );
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if pill_button(
                            ui,
                            "Manage Libraries…",
                            Some(Icon::Library),
                            PillStyle::Secondary,
                            palette,
                        )
                        .clicked()
                        {
                            actions.open_libraries = true;
                        }
                        if pill_button(
                            ui,
                            "Manage Directories…",
                            Some(Icon::Folder),
                            PillStyle::Secondary,
                            palette,
                        )
                        .clicked()
                        {
                            actions.open_folders = true;
                        }
                        if pill_button(
                            ui,
                            "Prune Missing Files",
                            Some(Icon::Trash),
                            PillStyle::Danger,
                            palette,
                        )
                        .clicked()
                        {
                            actions.prune_missing = true;
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if pill_button(
                            ui,
                            "Import Playlist (.m3u / .pls)…",
                            Some(Icon::Playlist),
                            PillStyle::Secondary,
                            palette,
                        )
                        .clicked()
                        {
                            actions.import_playlist = true;
                        }
                    });
                    ui.add_space(6.0);
                    let prev_watch = *auto_watch_folders;
                    aligned_checkbox_row(
                        ui,
                        auto_watch_folders,
                        "Auto-sync linked directories in background (live folder watching)",
                        palette,
                    );
                    if *auto_watch_folders != prev_watch {
                        actions.watch_changed = Some(*auto_watch_folders);
                    }
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new("ONLINE TAG SERVICES")
                            .size(10.0)
                            .strong()
                            .color(palette.faint),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "MusicBrainz and iTunes Search work out of the box. Optionally add an AcoustID API key (with fpcalc installed) for acoustic fingerprint matching.",
                        )
                        .size(11.0)
                        .color(palette.muted),
                    );
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("AcoustID API Key:")
                                .size(11.5)
                                .color(palette.text_secondary),
                        );
                        let key_w = (ui.available_width() - 8.0).clamp(160.0, 260.0);
                        ui.add_sized(
                            [key_w, 24.0],
                            egui::TextEdit::singleline(acoustid_key)
                                .hint_text("Optional (uses default demo key if blank)"),
                        );
                    });
                }
            }
        });
    *open = is_open;
    actions
}

pub fn show_tag_editor_window(
    context: &egui::Context,
    state: &mut TagEditorState,
    acoustid_key: &str,
    providers: &TagProviderConfig,
    palette: &Palette,
) -> Option<TagEditorAction> {
    if !state.open {
        return None;
    }
    let mut action: Option<TagEditorAction> = None;
    let mut is_open = state.open;
    let batch_count = state.target_ids.len();
    let title = if batch_count > 1 {
        format!("Tag Editor · {batch_count} Tracks Selected")
    } else {
        "Tag Editor & Online Metadata Matcher".to_string()
    };

    // Cmd+Enter saves tags immediately without moving mouse
    if context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) && !state.saving {
        action = Some(TagEditorAction::Save(state.target_ids.clone()));
    }

    themed_modal_window(
        context,
        "tag_editor_modal",
        &title,
        Icon::Tag,
        Some("ESC to close"),
        [740.0, 565.0],
        Some([640.0, 460.0]),
        true,
        &mut is_open,
        palette,
        |ui| {
            // Top Hash & File Identity bar
            if let Some(track) = &state.primary_track {
                Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(8.0)
                    .inner_margin(Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(if batch_count > 1 {
                                    format!("Batch editing {batch_count} tracks · Reference: {}", track.title)
                                } else {
                                    track.path.display().to_string()
                                })
                                .size(10.5)
                                .monospace()
                                .color(palette.text_secondary),
                            );
                        });
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            if state.hashing {
                                ui.spinner();
                                ui.label(
                                    RichText::new("Computing Fast Audio Identity Hash…")
                                        .size(10.0)
                                        .color(palette.muted),
                                );
                            } else if let Some(h) = &state.hashes {
                                let file_part = if h.file_sha256.len() >= 16 {
                                    format!("FILE SHA-256: {}…   ·   ", &h.file_sha256[..16])
                                } else {
                                    String::new()
                                };
                                ui.label(
                                    RichText::new(format!(
                                        "{file_part}AUDIO HASH: {}…   ·   {}",
                                        &h.audio_sha256[..16.min(h.audio_sha256.len())],
                                        if h.chromaprint.is_some() {
                                            "Chromaprint Ready"
                                        } else {
                                            "Instant Audio Hash"
                                        }
                                    ))
                                    .size(9.5)
                                    .monospace()
                                    .color(palette.accent),
                                );
                            } else if let Some(err) = &state.hash_error {
                                ui.label(RichText::new(err).size(10.0).color(palette.danger));
                            }
                        });
                    });
            }

            ui.add_space(8.0);
            // Editable Metadata Form (adapts cleanly to 2 columns in Compact Mode)
            let tag_narrow = ui.available_width() < 540.0;
            let field_w = if tag_narrow {
                (ui.available_width() - 96.0).clamp(140.0, 290.0)
            } else {
                225.0
            };
            egui::Grid::new("tag_editor_form")
                .num_columns(if tag_narrow { 2 } else { 4 })
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    let lbl = |ui: &mut Ui, text: &str| {
                        ui.label(RichText::new(text).size(11.0).color(palette.muted));
                    };
                    if batch_count == 1 {
                        lbl(ui, "Title");
                        ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut state.form.title));
                    } else {
                        lbl(ui, "Title");
                        ui.label(
                            RichText::new("(Preserved per track in batch edit)")
                                .size(10.5)
                                .color(palette.faint),
                        );
                    }
                    if tag_narrow {
                        ui.end_row();
                    }
                    lbl(ui, "Artist");
                    ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut state.form.artist));
                    ui.end_row();

                    lbl(ui, "Album");
                    ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut state.form.album));
                    if tag_narrow {
                        ui.end_row();
                    }
                    lbl(ui, "Album Artist");
                    ui.add_sized(
                        [field_w, 24.0],
                        egui::TextEdit::singleline(&mut state.form.album_artist),
                    );
                    ui.end_row();

                    lbl(ui, "Year");
                    ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut state.form.year));
                    if tag_narrow {
                        ui.end_row();
                    }
                    lbl(ui, "Genre");
                    ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut state.form.genre));
                    ui.end_row();

                    lbl(ui, "Composer");
                    ui.add_sized(
                        [field_w, 24.0],
                        egui::TextEdit::singleline(&mut state.form.composer)
                            .hint_text("e.g. Ludwig van Beethoven"),
                    );
                    if tag_narrow {
                        ui.end_row();
                    }
                    lbl(ui, "Conductor");
                    ui.add_sized(
                        [field_w, 24.0],
                        egui::TextEdit::singleline(&mut state.form.conductor)
                            .hint_text("e.g. Herbert von Karajan"),
                    );
                    ui.end_row();

                    lbl(ui, "Orchestra");
                    ui.add_sized(
                        [field_w, 24.0],
                        egui::TextEdit::singleline(&mut state.form.orchestra)
                            .hint_text("Ensemble / Orchestra"),
                    );
                    if tag_narrow {
                        ui.end_row();
                    }
                    lbl(ui, "Work / Opus");
                    ui.add_sized(
                        [field_w, 24.0],
                        egui::TextEdit::singleline(&mut state.form.work)
                            .hint_text("e.g. Piano Sonata No. 14, Op. 27/2"),
                    );
                    ui.end_row();

                    lbl(ui, "Record Label");
                    ui.add_sized(
                        [field_w, 24.0],
                        egui::TextEdit::singleline(&mut state.form.label)
                            .hint_text("e.g. Deutsche Grammophon"),
                    );
                    if tag_narrow {
                        ui.end_row();
                    }
                    lbl(ui, "Catalog #");
                    ui.add_sized(
                        [field_w, 24.0],
                        egui::TextEdit::singleline(&mut state.form.catalog_number),
                    );
                    ui.end_row();

                    lbl(ui, "Comment");
                    ui.add_sized(
                        [field_w, 24.0],
                        egui::TextEdit::singleline(&mut state.form.comment),
                    );
                    if tag_narrow {
                        ui.end_row();
                    }
                    lbl(ui, "#Custom Tags");
                    ui.add_sized(
                        [field_w, 24.0],
                        egui::TextEdit::singleline(&mut state.form.custom_tags)
                            .hint_text("reference, solo-piano, late-night"),
                    );
                    ui.end_row();

                    if batch_count == 1 {
                        lbl(ui, "Track #");
                        ui.add_sized(
                            [field_w, 24.0],
                            egui::TextEdit::singleline(&mut state.form.track_number),
                        );
                        if tag_narrow {
                            ui.end_row();
                        }
                        lbl(ui, "Disc #");
                        ui.add_sized(
                            [field_w, 24.0],
                            egui::TextEdit::singleline(&mut state.form.disc_number),
                        );
                        ui.end_row();
                    }
                });

            ui.add_space(8.0);
            // Batch Multi-Track Tag Transformation Tools (1-click classical & archive cleanup)
            Frame::new()
                .fill(palette.surface)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(7.0)
                .inner_margin(Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("BATCH TRANSFORMS:")
                                .size(9.5)
                                .strong()
                                .color(palette.faint),
                        );
                        if pill_button(ui, "Filename → Tags", None, PillStyle::Secondary, palette)
                            .on_hover_text("Parse Track #, Artist, and Title from filenames (e.g. '01 - Artist - Title')")
                            .clicked()
                            && !state.saving
                        {
                            action = Some(TagEditorAction::BatchTransform {
                                ids: state.target_ids.clone(),
                                transform: BatchTagTransform::ParseFromFilename,
                            });
                        }
                        if pill_button(
                            ui,
                            "Swap Artist ↔ Composer",
                            None,
                            PillStyle::Secondary,
                            palette,
                        )
                        .on_hover_text("Swap Artist and Composer fields (ideal for classical rips)")
                        .clicked()
                            && !state.saving
                        {
                            action = Some(TagEditorAction::BatchTransform {
                                ids: state.target_ids.clone(),
                                transform: BatchTagTransform::SwapArtistComposer,
                            });
                        }
                        if pill_button(ui, "Strip Track #", None, PillStyle::Secondary, palette)
                            .on_hover_text("Strip leading track number prefixes ('01 - ', '02. ') from Titles")
                            .clicked()
                            && !state.saving
                        {
                            action = Some(TagEditorAction::BatchTransform {
                                ids: state.target_ids.clone(),
                                transform: BatchTagTransform::StripLeadingTrackNumbers,
                            });
                        }
                        if pill_button(ui, "Smart Title Case", None, PillStyle::Secondary, palette)
                            .on_hover_text("Normalize Title & Album casing while preserving Op., BWV, Roman numerals")
                            .clicked()
                            && !state.saving
                        {
                            action = Some(TagEditorAction::BatchTransform {
                                ids: state.target_ids.clone(),
                                transform: BatchTagTransform::SmartTitleCase,
                            });
                        }
                        if pill_button(ui, "Auto-Number 1..N", None, PillStyle::Secondary, palette)
                            .on_hover_text("Assign sequential Track # 1..N in current selection order")
                            .clicked()
                            && !state.saving
                        {
                            action = Some(TagEditorAction::BatchTransform {
                                ids: state.target_ids.clone(),
                                transform: BatchTagTransform::AutoNumberTracks,
                            });
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.add_sized(
                            [130.0, 22.0],
                            egui::TextEdit::singleline(&mut state.find_text)
                                .hint_text("Find text…"),
                        );
                        ui.add_sized(
                            [130.0, 22.0],
                            egui::TextEdit::singleline(&mut state.replace_text)
                                .hint_text("Replace with…"),
                        );
                        if pill_button(ui, "Replace All", None, PillStyle::Active, palette)
                            .on_hover_text("Find & replace across Title, Artist, Album, Composer & Work")
                            .clicked()
                            && !state.saving
                            && !state.find_text.is_empty()
                        {
                            action = Some(TagEditorAction::BatchTransform {
                                ids: state.target_ids.clone(),
                                transform: BatchTagTransform::FindReplace {
                                    find: state.find_text.clone(),
                                    replace: state.replace_text.clone(),
                                },
                            });
                        }
                        ui.add_space(8.0);
                        ui.add_sized(
                            [115.0, 22.0],
                            egui::TextEdit::singleline(&mut state.batch_tag_input)
                                .hint_text("#custom-tag"),
                        );
                        if pill_button(ui, "+ Tag All", None, PillStyle::Secondary, palette)
                            .on_hover_text("Append a #custom-tag to every selected track")
                            .clicked()
                            && !state.saving
                            && !state.batch_tag_input.trim().is_empty()
                        {
                            let t = state.batch_tag_input.trim().to_string();
                            state.batch_tag_input.clear();
                            action = Some(TagEditorAction::BatchTransform {
                                ids: state.target_ids.clone(),
                                transform: BatchTagTransform::AddCustomTag(t),
                            });
                        }
                    });
                });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            // Online Lookup Services Section
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("ONLINE TAG LOOKUP (MUSICBRAINZ · ITUNES · ACOUSTID · HASH)")
                        .size(10.0)
                        .strong()
                        .color(palette.faint),
                );
                if state.searching {
                    ui.spinner();
                    ui.label(
                        RichText::new("Querying online databases…")
                            .size(10.5)
                            .color(palette.accent),
                    );
                }
            });
            ui.add_space(5.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_sized(
                    [if tag_narrow { 175.0 } else { 190.0 }, 24.0],
                    egui::TextEdit::singleline(&mut state.query_title)
                        .hint_text("Title or work (e.g. Nocturne Op. 9)"),
                );
                ui.add_sized(
                    [155.0, 24.0],
                    egui::TextEdit::singleline(&mut state.query_artist)
                        .hint_text("Artist / Composer"),
                );
                if pill_button(ui, "By Name", Some(Icon::Search), PillStyle::Active, palette)
                    .on_hover_text("Search MusicBrainz & iTunes by title and artist/composer")
                    .clicked()
                {
                    state.trigger_lookup(LookupStrategy::ByName, acoustid_key, providers.clone());
                }
                if pill_button(ui, "By Hash", Some(Icon::Tag), PillStyle::Secondary, palette)
                    .on_hover_text("Match via File/Audio Hash cache, AcoustID fingerprint, and exact duration")
                    .clicked()
                {
                    state.trigger_lookup(LookupStrategy::ByHash, acoustid_key, providers.clone());
                }
                if pill_button(ui, "Search All", None, PillStyle::Accent, palette)
                    .on_hover_text("Run both Name and Hash/Duration online lookups")
                    .clicked()
                {
                    state.trigger_lookup(LookupStrategy::All, acoustid_key, providers.clone());
                }
            });

            if let Some(note) = &state.lookup_note {
                ui.add_space(4.0);
                ui.label(RichText::new(note).size(10.5).color(palette.danger));
            }

            ui.add_space(6.0);
            // Candidate Results Table
            Frame::new()
                .fill(palette.background)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(8.0)
                .inner_margin(Margin::symmetric(6, 6))
                .show(ui, |ui| {
                    if state.candidates.is_empty() {
                        ui.allocate_ui_with_layout(
                            vec2(ui.available_width(), 120.0),
                            Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                ui.label(
                                    RichText::new(if state.searching {
                                        "Searching MusicBrainz, iTunes & Hash records…"
                                    } else {
                                        "Click \"By Name\", \"By Hash\", or \"Search All\" to fetch verified online tags."
                                    })
                                    .size(11.5)
                                    .color(palette.faint),
                                );
                            },
                        );
                    } else {
                        let mut dbl_apply_idx: Option<usize> = None;
                        ScrollArea::vertical()
                            .id_salt("tag_candidates_scroll")
                            .max_height(150.0)
                            .show(ui, |ui| {
                                ui.style_mut().spacing.item_spacing.y = 3.0;
                                for (idx, cand) in state.candidates.iter().enumerate() {
                                    let is_sel = state.selected_candidate == Some(idx);
                                    let (rect, resp) = ui.allocate_exact_size(
                                        vec2(ui.available_width(), 30.0),
                                        Sense::click(),
                                    );
                                    let hovered = resp.hovered();
                                    let bg = if is_sel {
                                        palette.selected
                                    } else if hovered {
                                        palette.hover
                                    } else {
                                        Color32::TRANSPARENT
                                    };
                                    ui.painter().rect_filled(rect, 6.0, bg);
                                    if is_sel {
                                        ui.painter().rect_stroke(
                                            rect,
                                            6.0,
                                            Stroke::new(1.0, palette.accent),
                                            StrokeKind::Inside,
                                        );
                                    }
                                    // Source badge
                                    let badge_r = Rect::from_min_size(
                                        pos2(rect.left() + 6.0, rect.center().y - 9.0),
                                        vec2(106.0, 18.0),
                                    );
                                    ui.painter().rect_filled(badge_r, 4.0, palette.accent_subtle);
                                    ui.painter().text(
                                        badge_r.center(),
                                        Align2::CENTER_CENTER,
                                        format!("{} {}%", cand.source, cand.score),
                                        FontId::proportional(9.0),
                                        palette.accent,
                                    );

                                    // Title · Artist · Album · Year
                                    let info_rect = Rect::from_min_max(
                                        pos2(badge_r.right() + 8.0, rect.top()),
                                        pos2(rect.right() - 56.0, rect.bottom()),
                                    );
                                    let line = format!(
                                        "{}  —  {}  ·  {}{}",
                                        cand.title.as_deref().unwrap_or("—"),
                                        cand.artist.as_deref().unwrap_or("—"),
                                        cand.album.as_deref().unwrap_or("—"),
                                        cand.year
                                            .as_deref()
                                            .map(|y| format!(" ({y})"))
                                            .unwrap_or_default()
                                    );
                                    ui.painter().with_clip_rect(info_rect).text(
                                        pos2(info_rect.left(), rect.center().y),
                                        Align2::LEFT_CENTER,
                                        line,
                                        FontId::proportional(11.0),
                                        palette.text,
                                    );

                                    // Duration
                                    ui.painter().text(
                                        pos2(rect.right() - 8.0, rect.center().y),
                                        Align2::RIGHT_CENTER,
                                        format_duration(cand.duration_secs),
                                        FontId::monospace(10.0),
                                        palette.muted,
                                    );

                                    if resp.clicked() {
                                        state.selected_candidate = Some(idx);
                                    }
                                    if resp.double_clicked() {
                                        state.selected_candidate = Some(idx);
                                        dbl_apply_idx = Some(idx);
                                    }
                                }
                            });
                        if let Some(idx) = dbl_apply_idx {
                            state.apply_candidate(idx, true);
                        }
                    }
                });

            // Selective Field Chooser when a candidate is highlighted
            if let Some(sel_idx) = state.selected_candidate {
                let cover_url = state
                    .candidates
                    .get(sel_idx)
                    .and_then(|c| c.artwork_url.clone());
                ui.add_space(6.0);
                Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(6.0)
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Choose fields to copy from selected match:")
                                    .size(10.5)
                                    .color(palette.muted),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if pill_button(
                                    ui,
                                    "Apply & Save",
                                    Some(Icon::Check),
                                    PillStyle::Accent,
                                    palette,
                                )
                                .on_hover_text("Apply all match fields and save immediately (1-click)")
                                .clicked()
                                {
                                    state.apply_candidate(sel_idx, true);
                                    action = Some(TagEditorAction::Save(state.target_ids.clone()));
                                }
                                if pill_button(
                                    ui,
                                    "Apply Checked",
                                    None,
                                    PillStyle::Active,
                                    palette,
                                )
                                .clicked()
                                {
                                    state.apply_candidate(sel_idx, false);
                                }
                                if let Some(url) = cover_url {
                                    if state.fetching_cover {
                                        ui.spinner();
                                    } else if pill_button(
                                        ui,
                                        "Fetch Cover",
                                        Some(Icon::Palette),
                                        PillStyle::Secondary,
                                        palette,
                                    )
                                    .on_hover_text("Download 600px album artwork from iTunes")
                                    .clicked()
                                    {
                                        state.trigger_cover_fetch(url);
                                    }
                                }
                            });
                        });
                        ui.add_space(2.0);
                        ui.horizontal_wrapped(|ui| {
                            aligned_inline_checkbox(ui, &mut state.field_pick.title, "Title", 72.0, palette);
                            aligned_inline_checkbox(ui, &mut state.field_pick.artist, "Artist", 76.0, palette);
                            aligned_inline_checkbox(ui, &mut state.field_pick.album, "Album", 76.0, palette);
                            aligned_inline_checkbox(ui, &mut state.field_pick.album_artist, "Album Artist", 104.0, palette);
                            aligned_inline_checkbox(ui, &mut state.field_pick.year, "Year", 64.0, palette);
                            aligned_inline_checkbox(ui, &mut state.field_pick.genre, "Genre", 72.0, palette);
                            aligned_inline_checkbox(ui, &mut state.field_pick.track_number, "Track #", 78.0, palette);
                            aligned_inline_checkbox(ui, &mut state.field_pick.disc_number, "Disc #", 72.0, palette);
                        });
                    });
            }

            ui.add_space(10.0);
            // Footer Save bar
            ui.horizontal(|ui| {
                aligned_checkbox_row(
                    ui,
                    &mut state.write_to_file,
                    "Write tags losslessly into audio file (ffmpeg -c copy)",
                    palette,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if state.saving {
                        ui.spinner();
                        ui.label(RichText::new("Saving tags…").size(11.0).color(palette.accent));
                    } else if pill_button(
                        ui,
                        if batch_count > 1 {
                            "Save Tags to Selected Tracks"
                        } else {
                            "Save Tags"
                        },
                        Some(Icon::Check),
                        PillStyle::Accent,
                        palette,
                    )
                    .clicked()
                    {
                        action = Some(TagEditorAction::Save(state.target_ids.clone()));
                    }
                });
            });
        });

    state.open = is_open;
    action
}

#[allow(clippy::too_many_arguments)]
pub fn show_track_info_window(
    context: &egui::Context,
    track: &Track,
    file_size_bytes: Option<u64>,
    analysis: Option<&AudioAnalysis>,
    analyzing: bool,
    analysis_error: Option<&str>,
    palette: &Palette,
    cover_paint: impl FnOnce(&mut Ui, f32),
    quality_paint: impl FnOnce(&mut Ui),
) -> (bool, Option<TrackInfoAction>) {
    let mut open = true;
    let mut action = None;

    themed_modal_window(
        context,
        "track_inspector_modal",
        "Track Info & Audio Analysis",
        Icon::Info,
        None,
        [516.0, 495.0],
        None,
        false,
        &mut open,
        palette,
        |ui| {
            ui.horizontal(|ui| {
                cover_paint(ui, 112.0);
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(&track.title)
                            .size(17.0)
                            .strong()
                            .color(palette.text),
                    );
                    ui.label(
                        RichText::new(&track.artist)
                            .size(13.0)
                            .color(palette.text_secondary),
                    );
                    ui.label(
                        RichText::new(&track.album)
                            .size(11.5)
                            .color(palette.muted),
                    );
                    ui.add_space(8.0);
                    quality_paint(ui);
                });
            });
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            egui::Grid::new("track_inspector_grid")
                .num_columns(2)
                .spacing([18.0, 5.0])
                .show(ui, |ui| {
                    let mut row = |k: &str, v: String| {
                        ui.label(RichText::new(k).size(11.0).color(palette.faint));
                        ui.label(RichText::new(v).size(11.5).color(palette.text_secondary));
                        ui.end_row();
                    };
                    row("Format", track.quality_full());
                    if let Some(ch) = track.channels {
                        row(
                            "Channels",
                            match ch {
                                1 => "1 (Mono)".into(),
                                2 => "2 (Stereo)".into(),
                                n => format!("{n} channels"),
                            },
                        );
                    }
                    row("Duration", format_duration(track.duration));
                    if let Some(bytes) = file_size_bytes {
                        row("File Size", format_file_size(bytes));
                    }
                    if let Some(num) = track.track_number {
                        let disc = track
                            .disc_number
                            .map(|d| format!(" · Disc {d}"))
                            .unwrap_or_default();
                        row("Track #", format!("{num}{disc}"));
                    }
                    if let Some(year) = &track.year {
                        row("Year", year.clone());
                    }
                    if let Some(genre) = &track.genre {
                        row("Genre", genre.clone());
                    }
                    if let Some(aa) = &track.album_artist {
                        row("Album Artist", aa.clone());
                    }
                    if let Some(comp) = &track.composer {
                        row("Composer", comp.clone());
                    }
                    if let Some(cond) = &track.conductor {
                        row("Conductor", cond.clone());
                    }
                    if let Some(orch) = &track.orchestra {
                        row("Orchestra", orch.clone());
                    }
                    if let Some(work) = &track.work {
                        row("Work / Opus", work.clone());
                    }
                    if let Some(lbl) = &track.label {
                        let cat = track
                            .catalog_number
                            .as_deref()
                            .map(|c| format!(" · {c}"))
                            .unwrap_or_default();
                        row("Label", format!("{lbl}{cat}"));
                    }
                    if let Some(dr) = track.dr_score {
                        let lufs = track
                            .integrated_lufs
                            .map(|l| format!(" · {l:.1} LUFS"))
                            .unwrap_or_default();
                        let auth = track
                            .authenticity
                            .as_deref()
                            .map(|a| format!(" · {a}"))
                            .unwrap_or_default();
                        row("DB Quality", format!("DR{dr}{lufs}{auth}"));
                    }
                    if track.codec.starts_with("DSD")
                        && let Some(sr) = track.sample_rate
                        && let Some(dop_rate) = crate::dsd::dop_pcm_sample_rate(sr)
                    {
                        row(
                            "DoP v1.1",
                            format!("Supported ({} 24-bit 0x05/0xFA)", format_sample_rate(dop_rate)),
                        );
                    }
                    if track.play_count > 0 {
                        row("Plays", track.play_count.to_string());
                    }
                    if track.rg_track_gain_db.is_some() || track.rg_album_gain_db.is_some() {
                        let rg_t = track
                            .rg_track_gain_db
                            .map(|g| format!("Track {:+.2} dB", g))
                            .unwrap_or_default();
                        let rg_a = track
                            .rg_album_gain_db
                            .map(|g| format!("Album {:+.2} dB", g))
                            .unwrap_or_default();
                        let joined = [rg_t, rg_a]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join("  ·  ");
                        row("ReplayGain", joined);
                    }
                    if let Some(cs) = track.cue_start {
                        let end_s = track
                            .cue_end
                            .map(|e| format_duration(Some(e)))
                            .unwrap_or_else(|| "End".into());
                        row("CUE Span", format!("{} → {}", format_duration(Some(cs)), end_s));
                    }
                    ui.label(RichText::new("Tags").size(11.0).color(palette.faint));
                    ui.horizontal_wrapped(|ui| {
                        for tag in &track.custom_tags {
                            if pill_button(
                                ui,
                                &format!("#{tag} ×"),
                                None,
                                PillStyle::Secondary,
                                palette,
                            )
                            .on_hover_text("Click to remove custom tag")
                            .clicked()
                            {
                                action = Some(TrackInfoAction::RemoveCustomTag(tag.clone()));
                            }
                        }
                        let tag_id = egui::Id::unique(("track_info_new_tag", track.id));
                        let mut draft: String = ui
                            .ctx()
                            .data_mut(|d| d.get_temp::<String>(tag_id).unwrap_or_default());
                        let resp = ui.add_sized(
                            [116.0, 20.0],
                            egui::TextEdit::singleline(&mut draft).hint_text("+ #tag + Enter"),
                        );
                        if resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            && !draft.trim().is_empty()
                        {
                            action = Some(TrackInfoAction::AddCustomTag(draft.trim().to_string()));
                            draft.clear();
                        }
                        ui.ctx().data_mut(|d| d.insert_temp(tag_id, draft));
                    });
                    ui.end_row();
                    ui.label(RichText::new("Rating").size(11.0).color(palette.faint));
                    ui.horizontal(|ui| {
                        ui.style_mut().spacing.item_spacing.x = 6.0;
                        if let Some(next_half) = icons::interactive_half_star_rating(
                            ui,
                            track.rating,
                            6.5,
                            3.0,
                            palette.hires_gold,
                            palette.faint,
                        ) {
                            action = Some(TrackInfoAction::SetRating(next_half));
                        }
                        let rating_txt = if track.rating > 0 {
                            format!("{:.1} / 5.0", track.rating as f32 * 0.5)
                        } else {
                            "Unrated".to_string()
                        };
                        ui.label(RichText::new(rating_txt).size(10.5).monospace().color(palette.muted));
                    });
                    ui.end_row();
                    ui.label(RichText::new("Location").size(11.0).color(palette.faint));
                    ui.add(
                        egui::Label::new(
                            RichText::new(track.path.display().to_string())
                                .size(10.5)
                                .monospace()
                                .color(palette.muted),
                        )
                        .wrap(),
                    );
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);

            // Audirvāna-style Audio Quality & Hi-Res Authenticity Analyzer Card
            Frame::new()
                .fill(palette.background)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(7.0)
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("AUDIO QUALITY & HI-RES AUTHENTICITY")
                                .size(9.5)
                                .strong()
                                .color(palette.faint),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if analyzing {
                                ui.spinner();
                                ui.label(
                                    RichText::new("Analyzing…")
                                        .size(10.0)
                                        .color(palette.accent),
                                );
                            } else if analysis.is_none()
                                && pill_button(
                                    ui,
                                    "Analyze Track",
                                    Some(Icon::Visualizer),
                                    PillStyle::Active,
                                    palette,
                                )
                                .on_hover_text(
                                    "Measure Dynamic Range (DR), EBU R128 LUFS, 4x True Peak, and ultrasonic Hi-Res spectrum",
                                )
                                .clicked()
                            {
                                action = Some(TrackInfoAction::AnalyzeQuality);
                            }
                        });
                    });

                    if let Some(err) = analysis_error {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("Analysis error: {err}"))
                                .size(10.5)
                                .color(palette.danger),
                        );
                    } else if let Some(a) = analysis {
                        ui.add_space(6.0);
                        // Primary Audiophile Metrics Strip: DR Score | LUFS | True Peak | Clips | Authenticity
                        ui.horizontal_wrapped(|ui| {
                            let (dr_col, dr_bg) = if a.dr_score >= 12 {
                                (palette.accent_bright, palette.accent_subtle)
                            } else if a.dr_score >= 8 {
                                (palette.hires_gold, palette.hires_bg)
                            } else {
                                (palette.danger, palette.danger_bg)
                            };
                            Frame::new()
                                .fill(dr_bg)
                                .stroke(Stroke::new(1.0, dr_col.gamma_multiply(0.6)))
                                .corner_radius(5.0)
                                .inner_margin(Margin::symmetric(8, 3))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(format!("DR{}", a.dr_score))
                                            .size(12.0)
                                            .strong()
                                            .monospace()
                                            .color(dr_col),
                                    )
                                    .on_hover_text(format!(
                                        "Dynamic Range: {:.1} dB (TT/Pleasurize top-20% RMS crest factor)",
                                        a.dr_precise
                                    ));
                                });

                            let mut metric_chip = |label: &str, val: String, col: Color32| {
                                Frame::new()
                                    .fill(palette.surface)
                                    .stroke(Stroke::new(1.0, palette.border))
                                    .corner_radius(5.0)
                                    .inner_margin(Margin::symmetric(7, 3))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.style_mut().spacing.item_spacing.x = 4.0;
                                            ui.label(
                                                RichText::new(label).size(9.5).color(palette.faint),
                                            );
                                            ui.label(
                                                RichText::new(val)
                                                    .size(10.5)
                                                    .monospace()
                                                    .strong()
                                                    .color(col),
                                            );
                                        });
                                    });
                            };

                            metric_chip(
                                "LUFS",
                                format!("{:+.1}", a.integrated_lufs),
                                palette.text,
                            );
                            metric_chip(
                                "Peak",
                                format!("{:+.2} dB", a.sample_peak_db),
                                palette.text_secondary,
                            );
                            metric_chip(
                                "True Peak",
                                format!("{:+.2} dBTP", a.true_peak_dbtp),
                                if a.true_peak_dbtp > -0.1 {
                                    palette.danger
                                } else {
                                    palette.text_secondary
                                },
                            );
                            metric_chip(
                                "BW",
                                format!("{:.1} kHz", a.effective_bandwidth_hz / 1000.0),
                                palette.accent_bright,
                            );
                            metric_chip(
                                "Clips",
                                format!("{}", a.clipped_samples),
                                if a.clipped_samples > 0 {
                                    palette.danger
                                } else {
                                    palette.accent
                                },
                            );

                            let (v_col, v_bg) = match a.verdict {
                                HiResVerdict::AuthenticHiRes => {
                                    (palette.hires_gold, palette.hires_bg)
                                }
                                HiResVerdict::LosslessCdQuality => {
                                    (palette.accent, palette.accent_subtle)
                                }
                                HiResVerdict::SuspectedUpsample => {
                                    (palette.hires_gold, palette.surface)
                                }
                                HiResVerdict::LossyTranscode => {
                                    (palette.danger, palette.danger_bg)
                                }
                            };
                            Frame::new()
                                .fill(v_bg)
                                .stroke(Stroke::new(1.0, v_col.gamma_multiply(0.55)))
                                .corner_radius(5.0)
                                .inner_margin(Margin::symmetric(7, 3))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(a.verdict.badge())
                                            .size(9.5)
                                            .strong()
                                            .monospace()
                                            .color(v_col),
                                    );
                                });
                        });

                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(&a.verdict_detail)
                                .size(10.5)
                                .color(palette.text_secondary),
                        );

                        // Average Spectral Distribution Plot (0 Hz .. Nyquist) with 22 kHz CD marker
                        ui.add_space(6.0);
                        paint_analysis_spectrum(ui, a, palette);
                    } else if !analyzing {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Inspect Dynamic Range (DR), EBU R128 loudness, inter-sample True Peak, and check whether Hi-Res tracks contain genuine ultrasonic spectrum above 22 kHz.",
                            )
                            .size(10.5)
                            .color(palette.muted),
                        );
                    }
                });

            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if pill_button(ui, "Play", Some(Icon::Play), PillStyle::Accent, palette).clicked() {
                    action = Some(TrackInfoAction::Play);
                }
                if pill_button(ui, "Edit Tags…", Some(Icon::Tag), PillStyle::Active, palette)
                    .clicked()
                {
                    action = Some(TrackInfoAction::EditTags);
                }
                if pill_button(
                    ui,
                    FILE_MANAGER_NAME,
                    Some(Icon::External),
                    PillStyle::Secondary,
                    palette,
                )
                .clicked()
                {
                    action = Some(TrackInfoAction::RevealInFinder);
                }
                if pill_button(ui, "Remove", Some(Icon::Trash), PillStyle::Danger, palette)
                    .clicked()
                {
                    action = Some(TrackInfoAction::Remove);
                }
            });
        });

    (open, action)
}

fn paint_analysis_spectrum(ui: &mut Ui, analysis: &AudioAnalysis, palette: &Palette) {
    let w = ui.available_width().max(180.0);
    let h = 72.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 5.0, palette.panel);
    painter.rect_stroke(rect, 5.0, Stroke::new(1.0, palette.border), StrokeKind::Inside);

    let plot = Rect::from_min_max(
        pos2(rect.left() + 6.0, rect.top() + 8.0),
        pos2(rect.right() - 6.0, rect.bottom() - 16.0),
    );
    let nyquist_khz = (analysis.sample_rate as f32 * 0.5) / 1000.0;

    // Horizontal dB reference guides (-30 dB, -60 dB)
    for db in [-30.0_f32, -60.0] {
        let frac = ((db + 96.0) / 96.0).clamp(0.0, 1.0);
        let y = plot.bottom() - frac * plot.height();
        painter.line_segment(
            [pos2(plot.left(), y), pos2(plot.right(), y)],
            Stroke::new(0.7, palette.border),
        );
    }

    // Spectrum filled bars + smooth crest polyline
    let bins = analysis.spectrum_db.len();
    if bins >= 2 {
        let bar_w = plot.width() / bins as f32;
        let mut pts = Vec::with_capacity(bins);
        for (i, &db) in analysis.spectrum_db.iter().enumerate() {
            let norm = ((db + 96.0) / 96.0).clamp(0.0, 1.0);
            let x0 = plot.left() + i as f32 * bar_w;
            let x1 = (x0 + bar_w - 1.0).max(x0 + 1.0);
            let y = plot.bottom() - norm * plot.height();
            let freq_khz = ((i as f32 + 0.5) / bins as f32) * nyquist_khz;
            let bar_col = if freq_khz > 22.05 {
                palette.hires_gold.gamma_multiply(0.38)
            } else {
                palette.accent.gamma_multiply(0.32)
            };
            painter.rect_filled(
                Rect::from_min_max(pos2(x0, y), pos2(x1, plot.bottom())),
                1.0,
                bar_col,
            );
            pts.push(pos2((x0 + x1) * 0.5, y));
        }
        painter.add(egui::Shape::line(pts, Stroke::new(1.4, palette.accent_bright)));
    }

    // Vertical 22.05 kHz CD-Nyquist reference line for Hi-Res tracks
    if nyquist_khz > 24.5 {
        let x22 = plot.left() + (22.05 / nyquist_khz).clamp(0.0, 1.0) * plot.width();
        painter.line_segment(
            [pos2(x22, plot.top()), pos2(x22, plot.bottom())],
            Stroke::new(1.2, palette.hires_gold),
        );
        painter.text(
            pos2(x22 + 3.0, plot.top() + 5.0),
            Align2::LEFT_CENTER,
            "22 kHz",
            FontId::monospace(8.5),
            palette.hires_gold,
        );
    }

    // Frequency axis labels
    let axis_y = rect.bottom() - 7.5;
    painter.text(
        pos2(plot.left(), axis_y),
        Align2::LEFT_CENTER,
        "0 kHz",
        FontId::monospace(8.5),
        palette.faint,
    );
    painter.text(
        pos2(plot.center().x, axis_y),
        Align2::CENTER_CENTER,
        format!("{:.1} kHz", nyquist_khz * 0.5),
        FontId::monospace(8.5),
        palette.faint,
    );
    painter.text(
        pos2(plot.right(), axis_y),
        Align2::RIGHT_CENTER,
        format!("{:.1} kHz", nyquist_khz),
        FontId::monospace(8.5),
        palette.faint,
    );
}

pub enum TrackInfoAction {
    Play,
    EditTags,
    RevealInFinder,
    Remove,
    SetRating(u8),
    AnalyzeQuality,
    RemoveCustomTag(String),
    AddCustomTag(String),
}

pub enum FolderModalAction {
    AddFolder,
    AddFiles,
    RescanAll,
    RescanOne(PathBuf),
    RemoveOne(PathBuf),
    PruneMissing,
}

pub fn show_folders_window(
    context: &egui::Context,
    open: &mut bool,
    folders: &[(PathBuf, usize)],
    palette: &Palette,
) -> Option<FolderModalAction> {
    if !*open {
        return None;
    }
    let mut is_open = *open;
    let mut action = None;

    themed_modal_window(
        context,
        "folders_modal",
        "Music Directories",
        Icon::Folder,
        None,
        [560.0, 320.0],
        Some([460.0, 260.0]),
        true,
        &mut is_open,
        palette,
        |ui| {
            ui.label(
                RichText::new(
                    "Directories are indexed in place — TxPlayer never moves or deletes your audio files.",
                )
                .size(11.5)
                .color(palette.muted),
            );
            ui.add_space(8.0);
            if folders.is_empty() {
                Frame::new()
                    .fill(palette.surface)
                    .corner_radius(8.0)
                    .inner_margin(Margin::symmetric(16, 24))
                    .show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new("No directories linked to this library yet.")
                                    .size(12.0)
                                    .color(palette.muted),
                            );
                        });
                    });
            } else {
                ScrollArea::vertical().max_height(190.0).show(ui, |ui| {
                    for (folder, count) in folders {
                        Frame::new()
                            .fill(palette.surface)
                            .stroke(Stroke::new(1.0, palette.border))
                            .corner_radius(6.0)
                            .inner_margin(Margin::symmetric(10, 6))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        let name = folder
                                            .file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("Folder");
                                        ui.label(
                                            RichText::new(format!("{name}  ({count} tracks)"))
                                                .size(12.0)
                                                .strong()
                                                .color(palette.text),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(folder.display().to_string())
                                                    .size(10.0)
                                                    .monospace()
                                                    .color(palette.faint),
                                            )
                                            .truncate(),
                                        );
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if pill_button(
                                            ui,
                                            "Remove",
                                            Some(Icon::Trash),
                                            PillStyle::Danger,
                                            palette,
                                        )
                                        .clicked()
                                        {
                                            action = Some(FolderModalAction::RemoveOne(folder.clone()));
                                        }
                                        if pill_button(
                                            ui,
                                            FILE_MANAGER_NAME,
                                            Some(Icon::External),
                                            PillStyle::Secondary,
                                            palette,
                                        )
                                        .clicked()
                                        {
                                            reveal_in_file_manager(folder);
                                        }
                                        if pill_button(
                                            ui,
                                            "Rescan",
                                            Some(Icon::Refresh),
                                            PillStyle::Secondary,
                                            palette,
                                        )
                                        .clicked()
                                        {
                                            action = Some(FolderModalAction::RescanOne(folder.clone()));
                                        }
                                    });
                                });
                            });
                        ui.add_space(4.0);
                    }
                });
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if pill_button(ui, "Add Folder", Some(Icon::FolderPlus), PillStyle::Accent, palette)
                    .clicked()
                {
                    action = Some(FolderModalAction::AddFolder);
                }
                if pill_button(ui, "Add Tracks", Some(Icon::TrackPlus), PillStyle::Secondary, palette)
                    .clicked()
                {
                    action = Some(FolderModalAction::AddFiles);
                }
                if !folders.is_empty()
                    && pill_button(ui, "Rescan All", Some(Icon::Refresh), PillStyle::Secondary, palette)
                        .clicked()
                {
                    action = Some(FolderModalAction::RescanAll);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "Prune Missing", Some(Icon::Trash), PillStyle::Danger, palette)
                        .clicked()
                    {
                        action = Some(FolderModalAction::PruneMissing);
                    }
                });
            });
        });

    *open = is_open;
    action
}


fn plugin_section_card(
    ui: &mut Ui,
    heading: &str,
    palette: &Palette,
    content: impl FnOnce(&mut Ui),
) {
    Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.label(
                RichText::new(heading)
                    .size(9.5)
                    .strong()
                    .color(palette.faint),
            );
            ui.add_space(4.0);
            content(ui);
        });
}

fn status_badge(ui: &mut Ui, label: &str, ok: bool, palette: &Palette) {
    let (bg, border, fg) = if ok {
        (
            palette.accent_subtle,
            palette.selected_border,
            palette.accent,
        )
    } else {
        (palette.danger_bg, palette.danger_border, palette.danger)
    };
    Frame::new()
        .fill(bg)
        .stroke(Stroke::new(0.8, border))
        .corner_radius(4.0)
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(9.5).monospace().color(fg));
        });
}

/// Renders the 10-band Parametric EQ + Crossfeed + Limiter controls. Returns true if DSP parameters changed.
pub fn paint_dsp_controls(ui: &mut Ui, dsp: &mut DspConfig, palette: &Palette) -> bool {
    let mut changed = false;

    ui.horizontal_wrapped(|ui| {
        for &preset in EqPreset::SELECTABLE {
            if pill_button(
                ui,
                preset.name(),
                None,
                if dsp.eq_preset == preset {
                    PillStyle::Active
                } else {
                    PillStyle::Secondary
                },
                palette,
            )
            .clicked()
            {
                dsp.apply_preset(preset);
                dsp.enabled = true;
                dsp.eq_enabled = true;
                changed = true;
            }
        }
    });
    ui.add_space(6.0);

    // 10-band vertical EQ bank (responsive band width so it fits both Compact & Classic modes!)
    let eq_band_w = ((ui.available_width() - 48.0) / 10.0).clamp(30.0, 48.0);
    let eq_gap = if eq_band_w < 38.0 { 2.0 } else { 5.0 };
    Frame::new()
        .fill(palette.background)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(6.0)
        .inner_margin(Margin::symmetric(8, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = eq_gap;
                for (i, label) in EQ_FREQ_LABELS.iter().enumerate() {
                    ui.vertical(|ui| {
                        ui.set_width(eq_band_w);
                        ui.with_layout(Layout::top_down(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:+.1}", dsp.eq_bands_db[i]))
                                    .size(9.0)
                                    .monospace()
                                    .color(if dsp.eq_bands_db[i].abs() > 0.1 {
                                        palette.accent
                                    } else {
                                        palette.faint
                                    }),
                            );
                            let resp = ui.add(
                                egui::Slider::new(&mut dsp.eq_bands_db[i], -9.0..=9.0)
                                    .vertical()
                                    .show_value(false)
                                    .step_by(0.5),
                            );
                            if resp.changed() {
                                dsp.eq_preset = EqPreset::Custom;
                                dsp.enabled = true;
                                dsp.eq_enabled = true;
                                changed = true;
                            }
                            if resp.double_clicked() {
                                dsp.eq_bands_db[i] = 0.0;
                                dsp.eq_preset = EqPreset::Custom;
                                changed = true;
                            }
                            ui.label(
                                RichText::new(*label)
                                    .size(9.5)
                                    .monospace()
                                    .color(palette.muted),
                            );
                        });
                    });
                }
            });
        });

    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        let prev_cf = dsp.crossfeed_enabled;
        aligned_checkbox_row(
            ui,
            &mut dsp.crossfeed_enabled,
            "Headphone Crossfeed",
            palette,
        );
        if dsp.crossfeed_enabled != prev_cf {
            if dsp.crossfeed_enabled {
                dsp.enabled = true;
            }
            changed = true;
        }
        for &preset in CrossfeedPreset::SELECTABLE {
            let active = dsp.crossfeed_enabled && dsp.crossfeed_preset == preset;
            if pill_button(
                ui,
                preset.name(),
                None,
                if active {
                    PillStyle::Active
                } else {
                    PillStyle::Secondary
                },
                palette,
            )
            .on_hover_text(preset.detail())
            .clicked()
            {
                dsp.apply_crossfeed_preset(preset);
                changed = true;
            }
        }
        if dsp.crossfeed_enabled {
            let cf_resp = ui.add(
                egui::Slider::new(&mut dsp.crossfeed_amount, 0.05..=1.0)
                    .show_value(false)
                    .trailing_fill(true),
            );
            ui.label(
                RichText::new(format!(
                    "{:.0}% · {:.0} Hz",
                    dsp.crossfeed_amount * 100.0,
                    dsp.crossfeed_cutoff_hz
                ))
                .size(10.0)
                .monospace()
                .color(palette.accent),
            );
            if cf_resp.changed() {
                dsp.crossfeed_preset = CrossfeedPreset::Custom;
                changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("Preamp: {:+.1} dB", dsp.preamp_db))
                .size(11.0)
                .color(palette.text_secondary),
        );
        if ui
            .add(
                egui::Slider::new(&mut dsp.preamp_db, -6.0..=6.0)
                    .step_by(0.5)
                    .show_value(false),
            )
            .changed()
        {
            changed = true;
        }
        let prev_lim = dsp.soft_limiter;
        aligned_checkbox_row(ui, &mut dsp.soft_limiter, "Soft Limiter", palette);
        if dsp.soft_limiter != prev_lim {
            changed = true;
        }
    });
    changed
}

/// Interactive 20 Hz .. 20 kHz studio frequency response canvas with 10 draggable EQ band nodes.
/// Drag any node vertically to shape the live EQ curve (-12..+12 dB); double-click a node to reset to 0 dB.
fn interactive_eq_response_canvas(
    ui: &mut Ui,
    dsp: &mut DspConfig,
    palette: &Palette,
) -> bool {
    let w = ui.available_width().max(220.0);
    let h = 104.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
    let plot = Rect::from_min_max(
        pos2(rect.left() + 26.0, rect.top() + 10.0),
        pos2(rect.right() - 10.0, rect.bottom() - 16.0),
    );
    let zero_y = plot.center().y;
    let half_h = plot.height() * 0.46;
    let log_min = 20.0_f32.ln();
    let log_max = 20_000.0_f32.ln();
    let freq_to_x = |hz: f32| -> f32 {
        let t = ((hz.ln() - log_min) / (log_max - log_min)).clamp(0.0, 1.0);
        plot.left() + t * plot.width()
    };
    let db_to_y = |db: f32| -> f32 {
        let norm = (db / 12.0).clamp(-1.0, 1.0);
        (zero_y - norm * half_h).clamp(plot.top(), plot.bottom())
    };
    let y_to_db = |y: f32| -> f32 {
        let raw = ((zero_y - y) / half_h).clamp(-1.0, 1.0) * 12.0;
        (raw * 2.0).round() * 0.5
    };

    // Precompute screen coordinates for all 10 graphic EQ nodes
    let mut node_pts = [pos2(0.0, 0.0); 10];
    for (i, &hz) in EQ_FREQS_HZ.iter().enumerate() {
        node_pts[i] = pos2(freq_to_x(hz), db_to_y(dsp.eq_bands_db[i]));
    }

    let drag_id = ui.scope_id().with("interactive_eq_drag_band");
    let mut dragged_band: Option<usize> = ui.ctx().data(|d| d.get_temp(drag_id));
    let mut changed = false;

    let nearest_band = |pos: egui::Pos2| -> Option<(usize, f32)> {
        node_pts
            .iter()
            .enumerate()
            .map(|(i, pt)| (i, pt.distance(pos)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    };

    let hovered_band = resp
        .hover_pos()
        .and_then(|pos| nearest_band(pos).filter(|(_, d)| *d <= 18.0).map(|(i, _)| i));

    if resp.double_clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some((idx, dist)) = nearest_band(pos)
        && dist <= 22.0
    {
        dsp.eq_bands_db[idx] = 0.0;
        dsp.eq_preset = EqPreset::Custom;
        changed = true;
    } else if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some((idx, _)) = nearest_band(pos)
    {
        dragged_band = Some(idx);
        ui.ctx().data_mut(|d| d.insert_temp(drag_id, idx));
    }

    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let idx = dragged_band.or_else(|| nearest_band(pos).map(|(i, _)| i));
        if let Some(i) = idx {
            let next_db = y_to_db(pos.y);
            if (dsp.eq_bands_db[i] - next_db).abs() > 0.01 {
                dsp.eq_bands_db[i] = next_db;
                dsp.eq_preset = EqPreset::Custom;
                dsp.eq_enabled = true;
                dsp.enabled = true;
                dsp.bit_perfect_mode = false;
                node_pts[i].y = db_to_y(next_db);
                changed = true;
            }
        }
    }

    if resp.drag_stopped() {
        dragged_band = None;
        ui.ctx().data_mut(|d| d.remove::<usize>(drag_id));
    }

    let painter = ui.painter();
    painter.rect_filled(rect, 6.0, palette.background);
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0, palette.border), StrokeKind::Inside);

    // Horizontal dB gridlines (+12 / +6 / 0 / -6 / -12 dB)
    for (db, label) in [(12.0_f32, "+12"), (6.0, "+6"), (0.0, "0"), (-6.0, "-6"), (-12.0, "-12")]
    {
        let gy = db_to_y(db);
        let stroke = if db.abs() < 0.1 {
            Stroke::new(0.9, palette.border_strong)
        } else {
            Stroke::new(0.5, palette.border.gamma_multiply(0.45))
        };
        painter.line_segment([pos2(plot.left(), gy), pos2(plot.right(), gy)], stroke);
        painter.text(
            pos2(plot.left() - 4.0, gy),
            Align2::RIGHT_CENTER,
            label,
            FontId::monospace(7.5),
            palette.faint,
        );
    }

    // Subtle vertical frequency gridlines
    for hz in [62.0_f32, 250.0, 1000.0, 4000.0, 16000.0] {
        let gx = freq_to_x(hz);
        painter.line_segment(
            [pos2(gx, plot.top()), pos2(gx, plot.bottom())],
            Stroke::new(0.5, palette.border.gamma_multiply(0.35)),
        );
    }

    let curve = compute_eq_response_curve(dsp, 96);
    let active = dsp.enabled && !dsp.bit_perfect_mode;
    let line_col = if active {
        palette.accent_bright
    } else {
        palette.faint
    };
    let mut pts = Vec::with_capacity(curve.len());
    for (i, &db) in curve.iter().enumerate() {
        let x = plot.left() + (i as f32 / (curve.len() - 1).max(1) as f32) * plot.width();
        pts.push(pos2(x, db_to_y(db)));
    }

    // Translucent area shading between curve and 0 dB axis
    if pts.len() >= 2 {
        let fill_col = Color32::from_rgba_unmultiplied(
            line_col.r(),
            line_col.g(),
            line_col.b(),
            if active { 28 } else { 12 },
        );
        for pair in pts.windows(2) {
            let p0 = pair[0];
            let p1 = pair[1];
            let y_top = p0.y.min(p1.y).min(zero_y);
            let y_bot = p0.y.max(p1.y).max(zero_y);
            if y_bot - y_top > 0.5 {
                painter.rect_filled(
                    Rect::from_min_max(pos2(p0.x, y_top), pos2(p1.x + 0.5, y_bot)),
                    0.0,
                    fill_col,
                );
            }
        }
        painter.add(egui::Shape::line(pts, Stroke::new(1.8, line_col)));
    }

    // 10 Draggable EQ Band Control Nodes
    let focus_band = dragged_band.or(hovered_band);
    for (i, pt) in node_pts.iter().enumerate() {
        let is_focus = focus_band == Some(i);
        let non_zero = dsp.eq_bands_db[i].abs() >= 0.25;
        let r = if is_focus { 5.2 } else if non_zero { 4.0 } else { 3.2 };
        let fill = if is_focus {
            palette.hires_gold
        } else if non_zero && active {
            palette.accent_bright
        } else {
            palette.surface_elevated
        };
        if is_focus {
            painter.circle_filled(
                *pt,
                8.5,
                Color32::from_rgba_unmultiplied(
                    palette.hires_gold.r(),
                    palette.hires_gold.g(),
                    palette.hires_gold.b(),
                    55,
                ),
            );
        }
        painter.circle_filled(*pt, r, fill);
        painter.circle_stroke(
            *pt,
            r,
            Stroke::new(
                1.0,
                if is_focus {
                    palette.text
                } else if active {
                    palette.accent
                } else {
                    palette.border_strong
                },
            ),
        );
    }

    // Floating readout pill for hovered/dragged EQ node
    if let Some(bi) = focus_band {
        let pt = node_pts[bi];
        let tag = format!("{} · {:+.1} dB", EQ_FREQ_LABELS[bi], dsp.eq_bands_db[bi]);
        let pill_w = 78.0;
        let px = (pt.x - pill_w * 0.5).clamp(plot.left(), plot.right() - pill_w);
        let py = if pt.y > plot.top() + 24.0 {
            pt.y - 22.0
        } else {
            pt.y + 10.0
        };
        let pill = Rect::from_min_size(pos2(px, py), vec2(pill_w, 16.0));
        painter.rect_filled(pill, 4.0, palette.surface_elevated);
        painter.rect_stroke(pill, 4.0, Stroke::new(0.8, palette.hires_gold), StrokeKind::Inside);
        painter.text(
            pill.center(),
            Align2::CENTER_CENTER,
            tag,
            FontId::monospace(8.5),
            palette.hires_gold,
        );
    }

    // Frequency axis labels
    let axis_y = rect.bottom() - 7.5;
    for (lbl, hz) in [
        ("31", 31.0_f32),
        ("125", 125.0),
        ("500", 500.0),
        ("2k", 2000.0),
        ("8k", 8000.0),
        ("16k", 16000.0),
    ] {
        painter.text(
            pos2(freq_to_x(hz), axis_y),
            Align2::CENTER_CENTER,
            lbl,
            FontId::monospace(8.0),
            palette.faint,
        );
    }

    let _ = resp.on_hover_text(
        "Drag any frequency node to sculpt EQ live · Double-click a node to reset to 0.0 dB",
    );
    changed
}

pub struct SignalPathDisplay<'a> {
    pub codec: &'a str,
    pub source_rate: u32,
    pub output_rate: u32,
    pub output_format: &'a str,
    pub channels: u16,
    pub bit_depth: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub variable_bitrate: bool,
    pub device_name: &'a str,
    pub bit_perfect: bool,
    pub exclusive_active: bool,
    pub hw_rate_switched: bool,
    pub rg_gain_db: Option<f32>,
    pub ram_bytes: Option<usize>,
    pub oversampling: OversamplingMode,
    pub resampler_quality: ResamplerQuality,
    pub direct_volume_lock: bool,
    pub volume: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepTimerPreset {
    Off,
    Minutes(u32),
    AfterTrack,
}

pub struct SignalPathActions {
    pub dsp_changed: bool,
    pub open_settings_dsp: bool,
    pub speed_changed: Option<f32>,
    pub set_loop_a: bool,
    pub set_loop_b: bool,
    pub clear_loop: bool,
    pub sleep_preset: Option<SleepTimerPreset>,
    pub add_bookmark: bool,
    pub jump_bookmark_secs: Option<f64>,
    pub remove_bookmark_idx: Option<usize>,
}

/// Renders a sleek, themed popover card anchored directly above `anchor_rect` with a downward pointer caret.
/// Closes automatically on Escape or clicking outside both the popover and its trigger button.
#[allow(clippy::too_many_arguments)]
fn anchored_popover(
    context: &egui::Context,
    id: &str,
    title: &str,
    icon: Icon,
    width: f32,
    anchor_rect: Option<Rect>,
    fallback_bottom_offset: f32,
    open: &mut bool,
    palette: &Palette,
    content: impl FnOnce(&mut Ui),
) {
    if !*open {
        return;
    }
    if context.input(|i| i.key_pressed(egui::Key::Escape)) {
        *open = false;
        return;
    }

    let screen = context.content_rect();
    let caret_h = 7.0;
    let margin = 10.0;
    let width = width.min((screen.width() - margin * 2.0).max(260.0));

    let (anchor_cx, bottom_y) = if let Some(ar) = anchor_rect {
        (ar.center().x, ar.top() - caret_h - 4.0)
    } else {
        (screen.right() - width * 0.5 - 16.0, screen.bottom() - fallback_bottom_offset)
    };

    let left_x = (anchor_cx - width * 0.5)
        .clamp(screen.left() + margin, (screen.right() - width - margin).max(screen.left() + margin));

    // Position initially above the trigger button via `default_pos` + `pivot`, while remaining freely movable!
    let mut close_clicked = false;
    let resp = egui::Area::new(egui::Id::unique(id))
        .default_pos(pos2(left_x, bottom_y))
        .pivot(Align2::LEFT_BOTTOM)
        .movable(true)
        .constrain_to(screen.shrink(6.0))
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(context, |ui| {
            Frame::new()
                .fill(palette.panel)
                .stroke(Stroke::new(1.0, palette.selected_border))
                .corner_radius(8.0)
                .inner_margin(Margin::ZERO)
                .shadow(egui::Shadow {
                    offset: [0, 6],
                    blur: 18,
                    spread: 0,
                    color: Color32::from_rgba_unmultiplied(
                        0,
                        0,
                        0,
                        if palette.is_light { 45 } else { 130 },
                    ),
                })
                .show(ui, |ui| {
                    ui.set_width(width);
                    // Slim 26px draggable popover header
                    let (hdr_rect, hdr_resp) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
                    if hdr_resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    ui.painter().rect_filled(
                        hdr_rect,
                        egui::CornerRadius {
                            nw: 8,
                            ne: 8,
                            sw: 0,
                            se: 0,
                        },
                        palette.sidebar,
                    );
                    ui.painter().line_segment(
                        [
                            pos2(hdr_rect.left(), hdr_rect.bottom()),
                            pos2(hdr_rect.right(), hdr_rect.bottom()),
                        ],
                        Stroke::new(1.0, palette.border),
                    );
                    let ir = Rect::from_center_size(
                        pos2(hdr_rect.left() + 14.0, hdr_rect.center().y),
                        Vec2::splat(12.5),
                    );
                    icons::paint_icon(ui.painter(), ir, icon, palette.accent);
                    ui.painter().text(
                        pos2(hdr_rect.left() + 25.0, hdr_rect.center().y),
                        Align2::LEFT_CENTER,
                        title,
                        FontId::proportional(11.5),
                        palette.text,
                    );
                    let close_r = Rect::from_center_size(
                        pos2(hdr_rect.right() - 14.0, hdr_rect.center().y),
                        Vec2::splat(18.0),
                    );
                    let close_resp = ui
                        .interact(close_r, ui.scope_id().with((id, "pop_close")), Sense::click())
                        .on_hover_text("Close (Esc)");
                    if close_resp.hovered() {
                        ui.painter().rect_filled(close_r, 4.0, palette.hover);
                    }
                    icons::paint_icon(
                        ui.painter(),
                        Rect::from_center_size(close_r.center(), Vec2::splat(9.0)),
                        Icon::Close,
                        if close_resp.hovered() {
                            palette.text
                        } else {
                            palette.muted
                        },
                    );
                    if close_resp.clicked() {
                        close_clicked = true;
                    }

                    Frame::new()
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            content(ui);
                        });
                });
        });

    let card_rect = resp.response.rect;

    // Draw crisp downward pointer caret visually binding the popover to its trigger button!
    let tip_x = anchor_cx.clamp(card_rect.left() + 18.0, card_rect.right() - 18.0);
    let base_y = card_rect.bottom() - 1.0;
    let caret_pts = vec![
        pos2(tip_x - 7.5, base_y),
        pos2(tip_x + 7.5, base_y),
        pos2(tip_x, base_y + caret_h),
    ];
    let painter = context.layer_painter(resp.response.layer_id);
    painter.add(egui::Shape::convex_polygon(
        caret_pts.clone(),
        palette.panel,
        Stroke::NONE,
    ));
    painter.line_segment(
        [caret_pts[0], caret_pts[2]],
        Stroke::new(1.2, palette.selected_border),
    );
    painter.line_segment(
        [caret_pts[1], caret_pts[2]],
        Stroke::new(1.2, palette.selected_border),
    );

    // Click outside both card AND trigger button closes the popover cleanly
    if context.input(|i| i.pointer.any_pressed())
        && let Some(pos) = context.input(|i| i.pointer.interact_pos())
    {
        let in_card = card_rect.expand(2.0).contains(pos);
        let in_anchor = anchor_rect.is_some_and(|r| r.expand(2.0).contains(pos));
        if !in_card && !in_anchor {
            *open = false;
        }
    }

    if close_clicked {
        *open = false;
    }
}

/// Sleek Audio Output Device popover visually anchored directly above the Output button.
/// Returns `Some(device_choice)` if user selected a device (`None` = System Default).
pub fn show_device_picker_popover(
    context: &egui::Context,
    open: &mut bool,
    anchor_rect: Option<Rect>,
    transport_height: f32,
    preferred_device: &Option<String>,
    available_devices: &mut Vec<String>,
    palette: &Palette,
) -> Option<Option<String>> {
    if !*open {
        return None;
    }
    let mut chosen: Option<Option<String>> = None;
    let mut should_close = false;

    anchored_popover(
        context,
        "audio_output_popover",
        "Audio Output Device",
        Icon::Device,
        310.0,
        anchor_rect,
        transport_height + 12.0,
        open,
        palette,
        |ui| {
            ui.label(
                RichText::new(format!(
                    "Hot-switch {AUDIO_BACKEND_NAME} output without interrupting playback."
                ))
                .size(10.5)
                .color(palette.muted),
            );
            ui.add_space(6.0);

            let draw_device_row = |ui: &mut Ui, label: &str, selected: bool| -> bool {
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                if selected {
                    ui.painter().rect_filled(rect, 6.0, palette.selected);
                    ui.painter().rect_stroke(
                        rect,
                        6.0,
                        Stroke::new(1.0, palette.selected_border),
                        StrokeKind::Inside,
                    );
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 6.0, palette.hover);
                }
                let text_r = Rect::from_min_max(
                    pos2(rect.left() + 8.0, rect.top()),
                    pos2(rect.right() - 24.0, rect.bottom()),
                );
                ui.painter().with_clip_rect(text_r).text(
                    pos2(text_r.left(), rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(11.5),
                    if selected {
                        palette.text
                    } else {
                        palette.text_secondary
                    },
                );
                if selected {
                    let chk_r = Rect::from_center_size(
                        pos2(rect.right() - 13.0, rect.center().y),
                        Vec2::splat(12.0),
                    );
                    icons::paint_icon(ui.painter(), chk_r, Icon::Check, palette.accent);
                }
                resp.clicked()
            };

            ScrollArea::vertical()
                .id_salt("device_popover_scroll")
                .max_height(155.0)
                .show(ui, |ui| {
                    ui.style_mut().spacing.item_spacing.y = 3.0;
                    let is_default = preferred_device.is_none();
                    let default_label = match default_output_device_name() {
                        Some(name) => format!("System Default ({name})"),
                        None => "System Default (Auto-follow)".into(),
                    };
                    if draw_device_row(ui, &default_label, is_default) {
                        chosen = Some(None);
                        should_close = true;
                    }
                    ui.add_space(2.0);
                    for dev in available_devices.iter() {
                        let selected = preferred_device.as_deref() == Some(dev.as_str());
                        if draw_device_row(ui, dev, selected) {
                            chosen = Some(Some(dev.clone()));
                            should_close = true;
                        }
                    }
                });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if pill_button(
                    ui,
                    "Refresh Devices",
                    Some(Icon::Refresh),
                    PillStyle::Secondary,
                    palette,
                )
                .clicked()
                {
                    *available_devices = list_output_devices();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("ESC to close")
                            .size(9.5)
                            .monospace()
                            .color(palette.faint),
                    );
                });
            });
        },
    );

    if should_close {
        *open = false;
    }
    chosen
}

/// Row snapshot passed into [`show_album_audit_window`].
pub struct AlbumAuditRow<'a> {
    pub index: usize,
    pub track: &'a Track,
    pub analysis: Option<&'a AudioAnalysis>,
    pub analyzing: bool,
    pub error: Option<&'a str>,
}

/// Batch Album / Selection TrueHz & Dynamic Range Authenticity Auditor modal.
/// Returns `(open_stay, inspect_track_id)` so clicking a track row opens its full spectrum & DR breakdown.
pub fn show_album_audit_window(
    context: &egui::Context,
    title: &str,
    rows: &[AlbumAuditRow<'_>],
    palette: &Palette,
) -> (bool, Option<u64>) {
    let mut open = true;
    let mut close_clicked = false;
    let mut inspect_id: Option<u64> = None;

    let total = rows.len().max(1);
    let mut done = 0usize;
    let mut dr_sum = 0.0_f32;
    let mut lufs_sum = 0.0_f32;
    let mut max_tp = -99.0_f32;
    let mut cnt_hires = 0usize;
    let mut cnt_cd = 0usize;
    let mut cnt_upscale = 0usize;
    let mut cnt_lossy = 0usize;
    let mut cnt_err = 0usize;
    for row in rows {
        if let Some(a) = row.analysis {
            done += 1;
            dr_sum += a.dr_precise;
            lufs_sum += a.integrated_lufs;
            max_tp = max_tp.max(a.true_peak_dbtp);
            match a.verdict {
                HiResVerdict::AuthenticHiRes => cnt_hires += 1,
                HiResVerdict::LosslessCdQuality => cnt_cd += 1,
                HiResVerdict::SuspectedUpsample => cnt_upscale += 1,
                HiResVerdict::LossyTranscode => cnt_lossy += 1,
            }
        } else if row.error.is_some() {
            cnt_err += 1;
        }
    }
    let finished = done + cnt_err;

    let status_sub = if finished < rows.len() {
        format!("Scanning {finished} / {} tracks…", rows.len())
    } else {
        format!("Verified {} track(s)", rows.len())
    };
    themed_modal_window(
        context,
        "album_audit_modal",
        "TrueHz & Dynamic Range Authenticity Audit",
        Icon::Sliders,
        Some(status_sub.as_str()),
        [720.0, 540.0],
        Some([540.0, 380.0]),
        true,
        &mut open,
        palette,
        |ui| {
        // Header: Album Title + live scan progress
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(title)
                    .size(14.0)
                    .strong()
                    .color(palette.text),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if finished < rows.len() {
                    ui.label(
                        RichText::new(format!("Scanning {finished} / {} tracks…", rows.len()))
                            .size(10.5)
                            .monospace()
                            .color(palette.accent_bright),
                    );
                } else {
                    ui.label(
                        RichText::new(format!("✓ Verified {} track(s)", rows.len()))
                            .size(10.5)
                            .monospace()
                            .color(palette.hires_gold),
                    );
                }
            });
        });

        // Progress bar while background analyzer is working
        if finished < rows.len() {
            ui.add_space(4.0);
            let frac = (finished as f32 / total as f32).clamp(0.0, 1.0);
            let (bar_r, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
            ui.painter().rect_filled(bar_r, 2.0, palette.surface);
            ui.painter().rect_filled(
                Rect::from_min_max(
                    bar_r.min,
                    pos2(bar_r.left() + bar_r.width() * frac, bar_r.bottom()),
                ),
                2.0,
                palette.accent,
            );
        }

        ui.add_space(8.0);

        // Aggregate KPI Cards
        if done > 0 {
            let avg_dr = dr_sum / done as f32;
            let avg_dr_int = avg_dr.round() as u8;
            let avg_lufs = lufs_sum / done as f32;
            let dr_col = if avg_dr_int >= 11 {
                palette.hires_gold
            } else if avg_dr_int >= 8 {
                palette.accent_bright
            } else {
                palette.danger
            };
            let tp_col = if max_tp > -0.3 {
                palette.danger
            } else if max_tp > -1.0 {
                palette.hires_gold
            } else {
                palette.accent
            };
            let (verdict_txt, verdict_col) = if cnt_upscale > 0 || cnt_lossy > 0 {
                (
                    format!("{} Flagged ({} Upscale · {} Lossy)", cnt_upscale + cnt_lossy, cnt_upscale, cnt_lossy),
                    palette.danger,
                )
            } else if cnt_hires > 0 && cnt_cd == 0 {
                (
                    format!("{cnt_hires}/{done} Authentic Hi-Res"),
                    palette.hires_gold,
                )
            } else if cnt_hires > 0 {
                (
                    format!("{cnt_hires} Hi-Res · {cnt_cd} Lossless CD"),
                    palette.hires_gold,
                )
            } else {
                (
                    format!("{cnt_cd}/{done} True Lossless CD"),
                    palette.accent_bright,
                )
            };

            ui.horizontal_wrapped(|ui| {
                let kpi = |ui: &mut Ui, label: &str, value: &str, col: Color32| {
                    Frame::new()
                        .fill(palette.background)
                        .stroke(Stroke::new(1.0, palette.border))
                        .corner_radius(6.0)
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(label)
                                        .size(9.0)
                                        .strong()
                                        .color(palette.faint),
                                );
                                ui.label(
                                    RichText::new(value)
                                        .size(12.5)
                                        .strong()
                                        .monospace()
                                        .color(col),
                                );
                            });
                        });
                };
                kpi(ui, "ALBUM DR", &format!("DR{avg_dr_int} ({avg_dr:.1})"), dr_col);
                kpi(ui, "AVG LOUDNESS", &format!("{avg_lufs:.1} LUFS"), palette.text);
                kpi(ui, "MAX TRUE PEAK", &format!("{max_tp:+.1} dBTP"), tp_col);
                kpi(ui, "AUTHENTICITY", &verdict_txt, verdict_col);
            });
            ui.add_space(8.0);
        }

        // Column header
        ui.horizontal(|ui| {
            ui.add_sized([28.0, 16.0], egui::Label::new(RichText::new("#").size(9.5).color(palette.faint)));
            ui.add_sized([210.0, 16.0], egui::Label::new(RichText::new("TRACK").size(9.5).color(palette.faint)));
            ui.add_sized([82.0, 16.0], egui::Label::new(RichText::new("FORMAT").size(9.5).color(palette.faint)));
            ui.add_sized([48.0, 16.0], egui::Label::new(RichText::new("DR").size(9.5).color(palette.faint)));
            ui.add_sized([68.0, 16.0], egui::Label::new(RichText::new("LUFS").size(9.5).color(palette.faint)));
            ui.add_sized([66.0, 16.0], egui::Label::new(RichText::new("PEAK").size(9.5).color(palette.faint)));
            ui.add_sized([54.0, 16.0], egui::Label::new(RichText::new("BW").size(9.5).color(palette.faint)));
            ui.label(RichText::new("VERDICT").size(9.5).color(palette.faint));
        });
        ui.separator();

        // Scrollable per-track table
        ScrollArea::vertical()
            .id_salt("album_audit_scroll")
            .max_height(330.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in rows {
                    let idx = row.index;
                    let track = row.track;
                    let analysis = row.analysis;
                    let analyzing = row.analyzing;
                    let err = row.error;
                    let row_h = 26.0;
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 4.0, palette.hover);
                    } else if idx % 2 == 1 {
                        ui.painter()
                            .rect_filled(rect, 4.0, palette.surface.gamma_multiply(0.45));
                    }

                    let cy = rect.center().y;
                    let mut x = rect.left() + 4.0;
                    let num_str = track
                        .track_number
                        .map(|n| format!("{n:02}"))
                        .unwrap_or_else(|| format!("{:02}", idx + 1));
                    ui.painter().text(
                        pos2(x, cy),
                        Align2::LEFT_CENTER,
                        num_str,
                        FontId::monospace(10.0),
                        palette.faint,
                    );
                    x += 28.0;

                    let title_clip = Rect::from_min_max(pos2(x, rect.top()), pos2(x + 206.0, rect.bottom()));
                    ui.painter().with_clip_rect(title_clip).text(
                        pos2(x, cy),
                        Align2::LEFT_CENTER,
                        &track.title,
                        FontId::proportional(11.0),
                        palette.text,
                    );
                    x += 214.0;

                    let fmt_str = format!(
                        "{} {}",
                        track.codec,
                        track
                            .sample_rate
                            .map(format_sample_rate)
                            .unwrap_or_default()
                    );
                    let fmt_clip = Rect::from_min_max(pos2(x, rect.top()), pos2(x + 78.0, rect.bottom()));
                    ui.painter().with_clip_rect(fmt_clip).text(
                        pos2(x, cy),
                        Align2::LEFT_CENTER,
                        fmt_str,
                        FontId::monospace(9.2),
                        palette.muted,
                    );
                    x += 84.0;

                    if let Some(a) = analysis {
                        let dr_col = if a.dr_score >= 11 {
                            palette.hires_gold
                        } else if a.dr_score >= 8 {
                            palette.accent_bright
                        } else {
                            palette.danger
                        };
                        ui.painter().text(
                            pos2(x, cy),
                            Align2::LEFT_CENTER,
                            format!("DR{}", a.dr_score),
                            FontId::monospace(10.5),
                            dr_col,
                        );
                        x += 50.0;

                        ui.painter().text(
                            pos2(x, cy),
                            Align2::LEFT_CENTER,
                            format!("{:5.1}", a.integrated_lufs),
                            FontId::monospace(10.0),
                            palette.text_secondary,
                        );
                        x += 70.0;

                        let tp_col = if a.true_peak_dbtp > -0.3 {
                            palette.danger
                        } else if a.true_peak_dbtp > -1.0 {
                            palette.hires_gold
                        } else {
                            palette.accent
                        };
                        ui.painter().text(
                            pos2(x, cy),
                            Align2::LEFT_CENTER,
                            format!("{:+.1} dB", a.true_peak_dbtp),
                            FontId::monospace(10.0),
                            tp_col,
                        );
                        x += 68.0;

                        ui.painter().text(
                            pos2(x, cy),
                            Align2::LEFT_CENTER,
                            format!("{:.1}k", a.effective_bandwidth_hz / 1000.0),
                            FontId::monospace(9.8),
                            palette.muted,
                        );
                        x += 54.0;

                        let (v_lbl, v_col) = match a.verdict {
                            HiResVerdict::AuthenticHiRes => ("HI-RES ✓", palette.hires_gold),
                            HiResVerdict::LosslessCdQuality => ("LOSSLESS ✓", palette.accent_bright),
                            HiResVerdict::SuspectedUpsample => ("UPSCALE ?", palette.danger),
                            HiResVerdict::LossyTranscode => ("LOSSY !", palette.danger),
                        };
                        ui.painter().text(
                            pos2(x, cy),
                            Align2::LEFT_CENTER,
                            v_lbl,
                            FontId::monospace(9.8),
                            v_col,
                        );
                    } else if let Some(e) = err {
                        ui.painter().text(
                            pos2(x, cy),
                            Align2::LEFT_CENTER,
                            format!("Error: {}", e.chars().take(42).collect::<String>()),
                            FontId::monospace(9.5),
                            palette.danger,
                        );
                    } else {
                        ui.painter().text(
                            pos2(x, cy),
                            Align2::LEFT_CENTER,
                            if analyzing { "Analyzing spectrum & DR…" } else { "Queued…" },
                            FontId::monospace(9.5),
                            palette.faint,
                        );
                    }

                    if resp.on_hover_text("Click to open full Spectrum & DR Track Inspector").clicked() {
                        inspect_id = Some(track.id);
                    }
                }
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Tip: Click any track row to inspect its 64-bin ultrasonic spectrum graph.")
                    .size(10.0)
                    .color(palette.faint),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill_button(ui, "Close", None, PillStyle::Secondary, palette).clicked() {
                    close_clicked = true;
                }
            });
        });
    });

    if close_clicked {
        open = false;
    }
    (open, inspect_id)
}

/// High-resolution Album Artwork & Booklet Scans Gallery modal.
pub fn show_artwork_gallery_window(
    context: &egui::Context,
    title: &str,
    images: &[PathBuf],
    pdfs: &[PathBuf],
    selected_idx: &mut usize,
    texture: Option<&egui::TextureHandle>,
    palette: &Palette,
) -> bool {
    let mut open = true;
    let mut close_clicked = false;
    let total = images.len();
    if total > 0 && *selected_idx >= total {
        *selected_idx = 0;
    }
    let sub = if total > 0 {
        format!("Image {} / {}", *selected_idx + 1, total)
    } else {
        format!("{} Booklet PDF(s)", pdfs.len())
    };
    themed_modal_window(
        context,
        "album_artwork_gallery",
        title,
        Icon::Grid,
        Some(sub.as_str()),
        [660.0, 560.0],
        Some([460.0, 400.0]),
        true,
        &mut open,
        palette,
        |ui| {
            if !pdfs.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("Digital Booklets:")
                            .size(10.5)
                            .strong()
                            .color(palette.muted),
                    );
                    for pdf in pdfs {
                        let name = pdf
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Booklet.pdf");
                        if pill_button(ui, name, Some(Icon::Folder), PillStyle::Active, palette)
                            .on_hover_text(format!("Open {}", pdf.display()))
                            .clicked()
                        {
                            open_file_with_default_app(pdf);
                        }
                    }
                });
                ui.add_space(6.0);
            }

            if total > 0 {
                let avail_w = ui.available_width().max(240.0);
                let avail_h = (ui.available_height() - 48.0).max(220.0);
                Frame::new()
                    .fill(palette.background)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(6.0)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_size(vec2(avail_w - 16.0, avail_h - 16.0));
                        ui.centered_and_justified(|ui| {
                            if let Some(tex) = texture {
                                let tsz = tex.size_vec2();
                                let max_w = (avail_w - 24.0).max(100.0);
                                let max_h = (avail_h - 24.0).max(100.0);
                                let scale = (max_w / tsz.x.max(1.0))
                                    .min(max_h / tsz.y.max(1.0))
                                    .min(1.0);
                                let draw_sz = tsz * scale;
                                ui.image((tex.id(), draw_sz));
                            } else {
                                ui.label(
                                    RichText::new("Loading high-resolution scan…")
                                        .size(11.5)
                                        .color(palette.muted),
                                );
                            }
                        });
                    });

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if total > 1
                        && pill_button(ui, "← Prev", None, PillStyle::Secondary, palette).clicked()
                    {
                        *selected_idx = if *selected_idx == 0 {
                            total - 1
                        } else {
                            *selected_idx - 1
                        };
                    }
                    if total > 1
                        && pill_button(ui, "Next →", None, PillStyle::Secondary, palette).clicked()
                    {
                        *selected_idx = (*selected_idx + 1) % total;
                    }
                    if let Some(cur_path) = images.get(*selected_idx) {
                        let fname = cur_path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("scan");
                        ui.label(
                            RichText::new(fname)
                                .size(10.5)
                                .monospace()
                                .color(palette.muted),
                        );
                        if pill_button(ui, "Open File", None, PillStyle::Secondary, palette)
                            .clicked()
                        {
                            open_file_with_default_app(cur_path);
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if pill_button(ui, "Close", None, PillStyle::Secondary, palette).clicked() {
                            close_clicked = true;
                        }
                    });
                });
            } else {
                ui.label(
                    RichText::new("No additional cover or booklet image scans found in this album folder.")
                        .size(11.0)
                        .color(palette.muted),
                );
            }
        },
    );
    if close_clicked {
        open = false;
    }
    open
}

/// Curated studio swatch colors for user Collections.
pub fn collection_accent_color(idx: u8, palette: &Palette) -> Color32 {
    match idx % 6 {
        0 => palette.accent,
        1 => palette.hires_gold,
        2 => palette.accent_secondary,
        3 => Color32::from_rgb(0x4E, 0xB8, 0x89),
        4 => Color32::from_rgb(0xD9, 0x77, 0x57),
        _ => Color32::from_rgb(0x9A, 0x7E, 0xE6),
    }
}

/// Modal to create a new curated Collection (with name, description, and color badge).
#[allow(clippy::too_many_arguments)]
pub fn show_new_collection_modal(
    context: &egui::Context,
    open: &mut bool,
    name: &mut String,
    description: &mut String,
    accent_idx: &mut u8,
    pending_tracks_count: usize,
    palette: &Palette,
) -> bool {
    if !*open {
        return false;
    }
    let mut create = false;
    let mut cancel = false;
    let sub = if pending_tracks_count > 0 {
        Some(format!("+ {pending_tracks_count} track(s)"))
    } else {
        None
    };
    themed_modal_window(
        context,
        "new_collection_modal",
        "New Collection",
        Icon::Grid,
        sub.as_deref(),
        [420.0, 265.0],
        Some([360.0, 240.0]),
        false,
        open,
        palette,
        |ui| {
            ui.label(
                RichText::new("Collection Name")
                    .size(10.5)
                    .color(palette.muted),
            );
            let name_resp = ui.add(
                egui::TextEdit::singleline(name)
                    .hint_text("e.g. Beethoven Complete Sonatas, Reference Demos…")
                    .desired_width(ui.available_width()),
            );
            if name_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                create = true;
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new("Description (optional)")
                    .size(10.5)
                    .color(palette.muted),
            );
            ui.add(
                egui::TextEdit::singleline(description)
                    .hint_text("Notes, label series, recording era…")
                    .desired_width(ui.available_width()),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Badge Color:").size(10.5).color(palette.muted));
                for idx in 0..6u8 {
                    let col = collection_accent_color(idx, palette);
                    let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 7.5, col);
                    if *accent_idx == idx {
                        ui.painter()
                            .circle_stroke(r.center(), 9.5, Stroke::new(1.5, palette.text));
                    }
                    if resp.clicked() {
                        *accent_idx = idx;
                    }
                }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "Create Collection", None, PillStyle::Accent, palette)
                        .clicked()
                    {
                        create = true;
                    }
                    if pill_button(ui, "Cancel", None, PillStyle::Secondary, palette).clicked() {
                        cancel = true;
                    }
                });
            });
        },
    );
    if cancel || create {
        *open = false;
    }
    create
}

/// Editable draft for the Visual Database Smart View Query Builder modal.
#[derive(Debug, Clone)]
pub struct SmartViewDraft {
    pub id: u64,
    pub name: String,
    pub match_all: bool,
    pub rules: Vec<QueryRule>,
}

impl Default for SmartViewDraft {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            match_all: true,
            rules: vec![QueryRule {
                field: QueryField::Composer,
                op: QueryOp::Contains,
                value: String::new(),
            }],
        }
    }
}

/// Visual Rule-Based Smart View Query Builder modal.
/// Returns `(keep_open, saved_view_if_submitted)`.
pub fn show_smart_view_builder_modal(
    context: &egui::Context,
    draft: &mut SmartViewDraft,
    live_match_count: usize,
    palette: &Palette,
) -> (bool, Option<SavedView>) {
    let mut open = true;
    let mut save_clicked = false;
    let mut cancel_clicked = false;
    let sub = format!("Matches {live_match_count} track(s)");

    themed_modal_window(
        context,
        "smart_view_builder_modal",
        if draft.id == 0 {
            "New Smart Database View"
        } else {
            "Edit Smart Database View"
        },
        Icon::Sliders,
        Some(sub.as_str()),
        [620.0, 440.0],
        Some([500.0, 340.0]),
        true,
        &mut open,
        palette,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("View Name:").size(11.0).color(palette.muted));
                ui.add(
                    egui::TextEdit::singleline(&mut draft.name)
                        .hint_text("e.g. High-DR Beethoven, DSD Masters, Unplayed Hi-Res…")
                        .desired_width((ui.available_width() - 180.0).max(180.0)),
                );
                if pill_button(
                    ui,
                    "Match ALL",
                    None,
                    if draft.match_all {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    },
                    palette,
                )
                .clicked()
                {
                    draft.match_all = true;
                }
                if pill_button(
                    ui,
                    "Match ANY",
                    None,
                    if !draft.match_all {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    },
                    palette,
                )
                .clicked()
                {
                    draft.match_all = false;
                }
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Quick Presets:").size(10.0).color(palette.faint));
                for (lbl, preset_name, field, op, val) in [
                    ("High-DR ≥12", "High-DR Audiophile (DR ≥ 12)", QueryField::DynamicRange, QueryOp::GreaterOrEq, "12"),
                    ("Hi-Res ≥96k", "Hi-Res ≥ 96 kHz", QueryField::SampleRateHz, QueryOp::GreaterOrEq, "96000"),
                    ("DSD Masters", "DSD Masters", QueryField::Codec, QueryOp::Contains, "DSD"),
                    ("Unplayed", "Unplayed Tracks", QueryField::PlayCount, QueryOp::Equals, "0"),
                    ("Verified Hi-Res", "Verified Authentic Hi-Res", QueryField::Authenticity, QueryOp::Contains, "AuthenticHiRes"),
                    ("Needs ReplayGain", "Needs ReplayGain Scan", QueryField::HasReplayGain, QueryOp::IsFalse, ""),
                ] {
                    if pill_button(ui, lbl, None, PillStyle::Secondary, palette).clicked() {
                        draft.name = preset_name.into();
                        draft.match_all = true;
                        draft.rules = vec![QueryRule {
                            field,
                            op,
                            value: val.into(),
                        }];
                    }
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            let mut remove_idx: Option<usize> = None;
            ScrollArea::vertical()
                .id_salt("smart_view_rules_scroll")
                .max_height(210.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (idx, rule) in draft.rules.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt(("sv_field", idx))
                                .width(138.0)
                                .selected_text(rule.field.label())
                                .show_ui(ui, |ui| {
                                    for &f in QueryField::ALL {
                                        if ui.selectable_value(&mut rule.field, f, f.label()).changed() {
                                            if f.is_bool() {
                                                rule.op = QueryOp::IsTrue;
                                                rule.value.clear();
                                            } else if f.is_numeric()
                                                && matches!(rule.op, QueryOp::Contains | QueryOp::IsTrue | QueryOp::IsFalse)
                                            {
                                                rule.op = QueryOp::GreaterOrEq;
                                            }
                                        }
                                    }
                                });

                            egui::ComboBox::from_id_salt(("sv_op", idx))
                                .width(92.0)
                                .selected_text(rule.op.label())
                                .show_ui(ui, |ui| {
                                    for &op in QueryOp::ALL {
                                        ui.selectable_value(&mut rule.op, op, op.label());
                                    }
                                });

                            if !matches!(rule.op, QueryOp::IsTrue | QueryOp::IsFalse) {
                                let hint = match rule.field {
                                    QueryField::SampleRateHz => "e.g. 96000",
                                    QueryField::BitDepth => "e.g. 24",
                                    QueryField::DynamicRange => "e.g. 12",
                                    QueryField::Rating => "1..5",
                                    QueryField::Year => "e.g. 1975",
                                    QueryField::Codec => "FLAC / DSD / ALAC",
                                    _ => "Value…",
                                };
                                ui.add(
                                    egui::TextEdit::singleline(&mut rule.value)
                                        .hint_text(hint)
                                        .desired_width((ui.available_width() - 40.0).max(120.0)),
                                );
                            }
                            if pill_button(ui, "×", None, PillStyle::Danger, palette).clicked() {
                                remove_idx = Some(idx);
                            }
                        });
                        ui.add_space(4.0);
                    }
                });
            if let Some(i) = remove_idx
                && i < draft.rules.len()
            {
                draft.rules.remove(i);
            }

            ui.add_space(4.0);
            if pill_button(ui, "+ Add Rule", Some(Icon::Plus), PillStyle::Secondary, palette)
                .clicked()
            {
                draft.rules.push(QueryRule {
                    field: QueryField::Genre,
                    op: QueryOp::Contains,
                    value: String::new(),
                });
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Live preview: {live_match_count} matching track(s)"))
                        .size(10.5)
                        .monospace()
                        .color(palette.accent_bright),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "Save Smart View", None, PillStyle::Accent, palette)
                        .clicked()
                    {
                        save_clicked = true;
                    }
                    if pill_button(ui, "Cancel", None, PillStyle::Secondary, palette).clicked() {
                        cancel_clicked = true;
                    }
                });
            });
        },
    );

    if cancel_clicked {
        return (false, None);
    }
    if save_clicked {
        let name = if draft.name.trim().is_empty() {
            "Custom Smart View".to_string()
        } else {
            draft.name.trim().to_string()
        };
        return (
            false,
            Some(SavedView {
                id: draft.id,
                name,
                match_all: draft.match_all,
                rules: draft.rules.clone(),
            }),
        );
    }
    (open, None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryInsightsAction {
    ExportCsv,
    ExportCatalogHtml,
    ExportCatalogMarkdown,
    BackupJson,
    RestoreJson,
    OpenDbHealth,
    InspectSacdIso,
    OpenDbFolder,
    FilterSearch(String),
    OpenYearsBrowser,
}

/// Library Database Analytics, Format Distribution, Listening History & Backup Dashboard modal.
pub fn show_library_insights_modal(
    context: &egui::Context,
    library_name: &str,
    stats: &LibraryStats,
    recent_history: &[PlayHistoryEntry],
    schema_version: u32,
    palette: &Palette,
) -> (bool, Option<LibraryInsightsAction>) {
    let mut open = true;
    let mut close_clicked = false;
    let mut action: Option<LibraryInsightsAction> = None;
    let sub = format!("Schema v{schema_version} · Rolling .bak enabled");

    themed_modal_window(
        context,
        "library_insights_modal",
        &format!("Library Database & Insights — {library_name}"),
        Icon::Sliders,
        Some(sub.as_str()),
        [680.0, 520.0],
        Some([520.0, 400.0]),
        true,
        &mut open,
        palette,
        |ui| {
            // 1) Top KPI Grid
            let total_hours = stats.total_duration_secs / 3600.0;
            let kpi = |ui: &mut Ui, label: &str, value: String, col: Color32| {
                Frame::new()
                    .fill(palette.background)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(6.0)
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_min_width(116.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(label).size(9.0).strong().color(palette.faint));
                            ui.label(
                                RichText::new(value)
                                    .size(13.0)
                                    .strong()
                                    .monospace()
                                    .color(col),
                            );
                        });
                    });
            };
            ui.horizontal_wrapped(|ui| {
                kpi(ui, "TRACKS", stats.total_tracks.to_string(), palette.text);
                kpi(ui, "ALBUMS", stats.total_albums.to_string(), palette.text);
                kpi(
                    ui,
                    "ARTISTS / COMP.",
                    format!("{} / {}", stats.total_artists, stats.total_composers),
                    palette.accent_bright,
                );
                kpi(
                    ui,
                    "RUNTIME",
                    format!("{total_hours:.1} hrs"),
                    palette.text,
                );
                kpi(
                    ui,
                    "HI-RES / DSD",
                    format!("{} / {}", stats.hires_tracks, stats.dsd_tracks),
                    palette.hires_gold,
                );
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                kpi(
                    ui,
                    "AVG DR",
                    stats
                        .avg_dr
                        .map(|d| format!("DR{d:.1}"))
                        .unwrap_or_else(|| "—".into()),
                    palette.hires_gold,
                );
                kpi(
                    ui,
                    "VERIFIED HI-RES",
                    format!("{}/{}", stats.verified_hires_tracks, stats.dr_analyzed_tracks),
                    palette.accent_bright,
                );
                kpi(
                    ui,
                    "REPLAYGAIN",
                    format!("{}/{}", stats.rg_scanned_tracks, stats.total_tracks),
                    palette.accent,
                );
                kpi(
                    ui,
                    "COLLECTIONS",
                    stats.collections_count.to_string(),
                    palette.accent_secondary,
                );
                kpi(
                    ui,
                    "SMART VIEWS",
                    stats.saved_views_count.to_string(),
                    palette.accent,
                );
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "{} lossless tracks  ·  {} genres  ·  {} record labels  ·  {} playlists",
                    stats.lossless_tracks,
                    stats.total_genres,
                    stats.total_labels,
                    stats.playlists_count
                ))
                .size(10.0)
                .monospace()
                .color(palette.muted),
            );

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            // 2) Interactive Collector Analytics (Codec, Sample Rate, Decades, DR Health, Top Composers & Labels)
            let mut clicked_action: Option<LibraryInsightsAction> = None;
            ScrollArea::vertical()
                .id_salt("insights_charts_scroll")
                .max_height(265.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut interactive_bars = |ui: &mut Ui,
                                                heading: &str,
                                                rows: &[(String, usize)],
                                                max_n: usize,
                                                col: Color32,
                                                make_action: &dyn Fn(&str) -> Option<LibraryInsightsAction>| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(heading)
                                    .size(9.5)
                                    .strong()
                                    .color(palette.faint),
                            );
                            ui.add_space(3.0);
                            if rows.is_empty() {
                                ui.label(
                                    RichText::new("No data yet").size(10.0).color(palette.muted),
                                );
                                return;
                            }
                            let denom = max_n.max(1) as f32;
                            for (lbl, cnt) in rows.iter().take(6) {
                                let act_opt = make_action(lbl);
                                let resp = ui
                                    .horizontal(|ui| {
                                        let short_lbl = if lbl.chars().count() > 14 {
                                            format!(
                                                "{}…",
                                                lbl.chars().take(13).collect::<String>()
                                            )
                                        } else {
                                            lbl.clone()
                                        };
                                        let lresp = ui.add_sized(
                                            [96.0, 16.0],
                                            egui::Label::new(
                                                RichText::new(short_lbl)
                                                    .size(9.8)
                                                    .monospace()
                                                    .color(palette.text_secondary),
                                            )
                                            .sense(Sense::click()),
                                        );
                                        let frac = (*cnt as f32 / denom).clamp(0.03, 1.0);
                                        let (bar_r, bresp) = ui.allocate_exact_size(
                                            vec2(132.0, 10.0),
                                            Sense::click(),
                                        );
                                        let hov = lresp.hovered() || bresp.hovered();
                                        ui.painter().rect_filled(
                                            bar_r,
                                            3.0,
                                            if hov && act_opt.is_some() {
                                                palette.hover
                                            } else {
                                                palette.background
                                            },
                                        );
                                        ui.painter().rect_filled(
                                            Rect::from_min_max(
                                                bar_r.min,
                                                pos2(
                                                    bar_r.left() + bar_r.width() * frac,
                                                    bar_r.bottom(),
                                                ),
                                            ),
                                            3.0,
                                            if hov && act_opt.is_some() {
                                                palette.accent_bright
                                            } else {
                                                col
                                            },
                                        );
                                        ui.label(
                                            RichText::new(format!("{cnt}"))
                                                .size(9.8)
                                                .monospace()
                                                .color(palette.muted),
                                        );
                                        lresp.clicked() || bresp.clicked()
                                    })
                                    .inner;
                                if resp && let Some(a) = act_opt {
                                    clicked_action = Some(a);
                                }
                            }
                        });
                    };

                    ui.columns(2, |cols| {
                        interactive_bars(
                            &mut cols[0],
                            "CODEC BREAKDOWN (CLICK TO FILTER)",
                            &stats.codec_counts,
                            stats.total_tracks,
                            palette.accent,
                            &|lbl| Some(LibraryInsightsAction::FilterSearch(format!("codec:{lbl}"))),
                        );
                        interactive_bars(
                            &mut cols[1],
                            "SAMPLE RATE DISTRIBUTION",
                            &stats.rate_buckets,
                            stats.total_tracks,
                            palette.hires_gold,
                            &|lbl| match lbl {
                                "DSD (1-bit)" => {
                                    Some(LibraryInsightsAction::FilterSearch("codec:dsd".into()))
                                }
                                "≥ 176.4 kHz" => {
                                    Some(LibraryInsightsAction::FilterSearch("rate>=176".into()))
                                }
                                "88.2 – 96 kHz" => {
                                    Some(LibraryInsightsAction::FilterSearch("rate>=88".into()))
                                }
                                _ => None,
                            },
                        );
                    });

                    if !stats.decade_counts.is_empty() || !stats.dr_buckets.is_empty() {
                        ui.add_space(8.0);
                        ui.columns(2, |cols| {
                            interactive_bars(
                                &mut cols[0],
                                "RECORDING DECADES TIMELINE",
                                &stats.decade_counts,
                                stats.decade_counts.iter().map(|(_, c)| *c).max().unwrap_or(1),
                                palette.accent_secondary,
                                &|_| Some(LibraryInsightsAction::OpenYearsBrowser),
                            );
                            interactive_bars(
                                &mut cols[1],
                                "DYNAMIC RANGE (DR) HEALTH",
                                &stats.dr_buckets,
                                stats.dr_analyzed_tracks.max(1),
                                palette.hires_gold,
                                &|lbl| {
                                    if lbl.starts_with("DR 14+") {
                                        Some(LibraryInsightsAction::FilterSearch("dr>=14".into()))
                                    } else if lbl.starts_with("DR 11") {
                                        Some(LibraryInsightsAction::FilterSearch("dr>=11".into()))
                                    } else if lbl.starts_with("DR 8") {
                                        Some(LibraryInsightsAction::FilterSearch("dr>=8".into()))
                                    } else {
                                        Some(LibraryInsightsAction::FilterSearch("dr<=7".into()))
                                    }
                                },
                            );
                        });
                    }

                    if !stats.top_composers.is_empty() || !stats.top_labels.is_empty() {
                        ui.add_space(8.0);
                        ui.columns(2, |cols| {
                            interactive_bars(
                                &mut cols[0],
                                "TOP COMPOSERS (CLICK TO FILTER)",
                                &stats.top_composers,
                                stats.top_composers.first().map(|(_, c)| *c).unwrap_or(1),
                                palette.accent_bright,
                                &|lbl| {
                                    let tok = lbl.split_whitespace().last().unwrap_or(lbl);
                                    Some(LibraryInsightsAction::FilterSearch(format!(
                                        "composer:{tok}"
                                    )))
                                },
                            );
                            interactive_bars(
                                &mut cols[1],
                                "TOP RECORD LABELS (CLICK TO FILTER)",
                                &stats.top_labels,
                                stats.top_labels.first().map(|(_, c)| *c).unwrap_or(1),
                                palette.accent,
                                &|lbl| {
                                    let tok = lbl.split_whitespace().next().unwrap_or(lbl);
                                    Some(LibraryInsightsAction::FilterSearch(format!(
                                        "label:{tok}"
                                    )))
                                },
                            );
                        });
                    }

                    if !recent_history.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "RECENT LISTENING HISTORY ({} LOGGED PLAYS)",
                                recent_history.len()
                            ))
                            .size(9.5)
                            .strong()
                            .color(palette.faint),
                        );
                        ui.add_space(3.0);
                        for ev in recent_history.iter().rev().take(15) {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&ev.title)
                                        .size(10.5)
                                        .color(palette.text),
                                );
                                ui.label(
                                    RichText::new(format!("— {} · {}", ev.artist, ev.album))
                                        .size(10.0)
                                        .color(palette.muted),
                                );
                            });
                        }
                    }
                });
            if let Some(ca) = clicked_action {
                action = Some(ca);
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);

            // 3) Database Export & Backup Actions
            ui.horizontal_wrapped(|ui| {
                if pill_button(
                    ui,
                    "HTML Dossier (.html)",
                    Some(Icon::Info),
                    PillStyle::Accent,
                    palette,
                )
                .on_hover_text("Export a print-ready portable HTML audiophile catalog with DR badges & liner notes")
                .clicked()
                {
                    action = Some(LibraryInsightsAction::ExportCatalogHtml);
                }
                if pill_button(
                    ui,
                    "Markdown (.md)",
                    Some(Icon::List),
                    PillStyle::Active,
                    palette,
                )
                .on_hover_text("Export a clean Markdown listening catalog grouped by album with liner notes")
                .clicked()
                {
                    action = Some(LibraryInsightsAction::ExportCatalogMarkdown);
                }
                if pill_button(
                    ui,
                    "Spreadsheet (.csv)",
                    Some(Icon::List),
                    PillStyle::Secondary,
                    palette,
                )
                .on_hover_text("Export complete track metadata, DR, LUFS, and tags to a CSV spreadsheet")
                .clicked()
                {
                    action = Some(LibraryInsightsAction::ExportCsv);
                }
                if pill_button(
                    ui,
                    "Backup (.json)",
                    Some(Icon::Folder),
                    PillStyle::Secondary,
                    palette,
                )
                .on_hover_text("Save a full portable backup of all libraries, collections, playlists & smart views")
                .clicked()
                {
                    action = Some(LibraryInsightsAction::BackupJson);
                }
                if pill_button(
                    ui,
                    "Restore (.json)",
                    Some(Icon::Refresh),
                    PillStyle::Secondary,
                    palette,
                )
                .on_hover_text("Restore a previously saved TxPlayer JSON database backup")
                .clicked()
                {
                    action = Some(LibraryInsightsAction::RestoreJson);
                }
                if pill_button(
                    ui,
                    "Duplicates & Missing…",
                    Some(Icon::Sliders),
                    PillStyle::Active,
                    palette,
                )
                .on_hover_text("Audit duplicate tracks across formats (keeps highest-res master) & missing files")
                .clicked()
                {
                    action = Some(LibraryInsightsAction::OpenDbHealth);
                }
                if pill_button(
                    ui,
                    "Inspect SACD .iso…",
                    Some(Icon::Info),
                    PillStyle::Secondary,
                    palette,
                )
                .on_hover_text("Inspect Scarlet Book Master TOC metadata inside a SACD .iso disc image")
                .clicked()
                {
                    action = Some(LibraryInsightsAction::InspectSacdIso);
                }
                if pill_button(
                    ui,
                    "DB Folder",
                    None,
                    PillStyle::Secondary,
                    palette,
                )
                .clicked()
                {
                    action = Some(LibraryInsightsAction::OpenDbFolder);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "Close", None, PillStyle::Secondary, palette).clicked() {
                        close_clicked = true;
                    }
                });
            });
        },
    );

    if close_clicked {
        open = false;
    }
    (open, action)
}

pub enum DbHealthAction {
    RemoveTrackIds(Vec<u64>),
    PruneAllMissing,
}

/// Library Database Integrity, Duplicate Quality Auditor & Missing-Files Cleaner modal.
pub fn show_db_health_modal(
    context: &egui::Context,
    duplicates: &[DuplicateGroup],
    missing_ids: &[u64],
    lookup_track: impl Fn(u64) -> Option<Track>,
    palette: &Palette,
) -> (bool, Option<DbHealthAction>) {
    let mut open = true;
    let mut close_clicked = false;
    let mut action: Option<DbHealthAction> = None;

    let extra_dup_ids: Vec<u64> = duplicates
        .iter()
        .flat_map(|g| {
            g.track_ids
                .iter()
                .copied()
                .filter(move |&id| id != g.best_track_id)
        })
        .collect();
    let sub = format!(
        "{} duplicate group(s) · {} missing file(s)",
        duplicates.len(),
        missing_ids.len()
    );

    themed_modal_window(
        context,
        "db_health_modal",
        "Library Database Health — Duplicates & Missing Files",
        Icon::Sliders,
        Some(sub.as_str()),
        [720.0, 540.0],
        Some([540.0, 380.0]),
        true,
        &mut open,
        palette,
        |ui| {
            // Top quick-action bar
            ui.horizontal_wrapped(|ui| {
                if !extra_dup_ids.is_empty()
                    && pill_button(
                        ui,
                        &format!("Keep Best Quality (Remove {} Lower-Res)", extra_dup_ids.len()),
                        None,
                        PillStyle::Accent,
                        palette,
                    )
                    .clicked()
                {
                    action = Some(DbHealthAction::RemoveTrackIds(extra_dup_ids.clone()));
                }
                if !missing_ids.is_empty()
                    && pill_button(
                        ui,
                        &format!("Prune {} Missing File(s)", missing_ids.len()),
                        Some(Icon::Close),
                        PillStyle::Danger,
                        palette,
                    )
                    .clicked()
                {
                    action = Some(DbHealthAction::PruneAllMissing);
                }
                if extra_dup_ids.is_empty() && missing_ids.is_empty() {
                    ui.label(
                        RichText::new("✓ Library database is spotless — zero duplicates and zero missing files!")
                            .size(11.5)
                            .strong()
                            .color(palette.hires_gold),
                    );
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            ScrollArea::vertical()
                .id_salt("db_health_scroll")
                .max_height(390.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if !duplicates.is_empty() {
                        ui.label(
                            RichText::new("DUPLICATE TRACK GROUPS (RANKED BY AUDIO FIDELITY)")
                                .size(9.5)
                                .strong()
                                .color(palette.faint),
                        );
                        ui.add_space(4.0);
                        for group in duplicates {
                            Frame::new()
                                .fill(palette.background)
                                .stroke(Stroke::new(1.0, palette.border))
                                .corner_radius(6.0)
                                .inner_margin(Margin::symmetric(10, 7))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(&group.key_label)
                                            .size(11.5)
                                            .strong()
                                            .color(palette.text),
                                    );
                                    ui.add_space(3.0);
                                    for &tid in &group.track_ids {
                                        let Some(tr) = lookup_track(tid) else {
                                            continue;
                                        };
                                        let is_best = tid == group.best_track_id;
                                        ui.horizontal(|ui| {
                                            if is_best {
                                                Frame::new()
                                                    .fill(palette.hires_bg)
                                                    .corner_radius(3.0)
                                                    .inner_margin(Margin::symmetric(5, 1))
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            RichText::new("BEST")
                                                                .size(8.5)
                                                                .monospace()
                                                                .strong()
                                                                .color(palette.hires_gold),
                                                        );
                                                    });
                                            } else {
                                                ui.add_space(38.0);
                                            }
                                            ui.label(
                                                RichText::new(tr.quality_full())
                                                    .size(10.0)
                                                    .monospace()
                                                    .color(if is_best {
                                                        palette.hires_gold
                                                    } else {
                                                        palette.accent_bright
                                                    }),
                                            );
                                            ui.label(
                                                RichText::new(tr.path.display().to_string())
                                                    .size(9.5)
                                                    .monospace()
                                                    .color(palette.muted),
                                            );
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if !is_best
                                                        && pill_button(
                                                            ui,
                                                            "Remove",
                                                            None,
                                                            PillStyle::Danger,
                                                            palette,
                                                        )
                                                        .clicked()
                                                    {
                                                        action = Some(
                                                            DbHealthAction::RemoveTrackIds(vec![
                                                                tid,
                                                            ]),
                                                        );
                                                    }
                                                },
                                            );
                                        });
                                    }
                                });
                            ui.add_space(6.0);
                        }
                    }

                    if !missing_ids.is_empty() {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("MISSING FILES ON DISK")
                                .size(9.5)
                                .strong()
                                .color(palette.danger),
                        );
                        ui.add_space(4.0);
                        for &tid in missing_ids.iter().take(80) {
                            if let Some(tr) = lookup_track(tid) {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("{} — {}", tr.title, tr.artist))
                                            .size(10.5)
                                            .color(palette.text_secondary),
                                    );
                                    ui.label(
                                        RichText::new(tr.path.display().to_string())
                                            .size(9.5)
                                            .monospace()
                                            .color(palette.faint),
                                    );
                                });
                            }
                        }
                    }
                });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "Close", None, PillStyle::Secondary, palette).clicked() {
                        close_clicked = true;
                    }
                });
            });
        },
    );

    if close_clicked {
        open = false;
    }
    (open, action)
}

/// Audirvana-inspired Signal Path Inspector + Quick DSP Popover anchored directly above telemetry.
/// Returns `(dsp_changed, open_full_dsp_settings)`.
#[allow(clippy::too_many_arguments)]
pub fn show_signal_path_popover(
    context: &egui::Context,
    open: &mut bool,
    anchor_rect: Option<Rect>,
    transport_height: f32,
    signal: Option<SignalPathDisplay<'_>>,
    plugins: &mut PluginConfig,
    playback_speed: f32,
    ab_loop: (Option<f64>, Option<f64>),
    sleep_status: Option<&str>,
    sleep_after_track: bool,
    current_pos_secs: f64,
    has_current_track: bool,
    bookmarks: &[TrackBookmark],
    palette: &Palette,
) -> SignalPathActions {
    let mut out = SignalPathActions {
        dsp_changed: false,
        open_settings_dsp: false,
        speed_changed: None,
        set_loop_a: false,
        set_loop_b: false,
        clear_loop: false,
        sleep_preset: None,
        add_bookmark: false,
        jump_bookmark_secs: None,
        remove_bookmark_idx: None,
    };
    if !*open {
        return out;
    }

    anchored_popover(
        context,
        "signal_path_popover",
        "Signal Path & Live DSP",
        Icon::Sliders,
        380.0,
        anchor_rect,
        transport_height + 12.0,
        open,
        palette,
        |ui| {
            let stage_row =
                |ui: &mut Ui, step: &str, title: &str, detail: String, accent: Color32, is_last: bool| {
                    ui.horizontal(|ui| {
                        let (dot_r, _) = ui.allocate_exact_size(vec2(16.0, 24.0), Sense::hover());
                        if !is_last {
                            ui.painter().line_segment(
                                [
                                    pos2(dot_r.center().x, dot_r.center().y + 5.0),
                                    pos2(dot_r.center().x, dot_r.bottom() + 6.0),
                                ],
                                Stroke::new(1.2, palette.border_strong),
                            );
                        }
                        ui.painter()
                            .circle_filled(dot_r.center(), 6.0, accent.gamma_multiply(0.22));
                        ui.painter().circle_filled(dot_r.center(), 3.6, accent);
                        ui.vertical(|ui| {
                            ui.style_mut().spacing.item_spacing.y = 0.0;
                            ui.label(
                                RichText::new(format!("{step}  ·  {title}"))
                                    .size(10.5)
                                    .strong()
                                    .color(palette.text),
                            );
                            ui.label(
                                RichText::new(detail)
                                    .size(9.8)
                                    .monospace()
                                    .color(palette.muted),
                            );
                        });
                    });
                };

            if let Some(s) = signal {
                // Top-Level Signal Path Verdict Banner (Decaud / Audirvana-class instant clarity)
                let native = s.source_rate == s.output_rate && (playback_speed - 1.0).abs() < 0.001;
                let dsp_active = plugins.dsp.is_actively_processing() && !plugins.dsp.bit_perfect_mode;
                let (verdict_title, verdict_sub, verdict_col) =
                    if s.output_format.starts_with("DoP") {
                        (
                            "NATIVE DSD OVER PCM (DoP v1.1)",
                            "Bit-Exact 1-Bit DSD · 0x05/0xFA Framing · 0.0 dBFS Direct DAC Lock",
                            palette.hires_gold,
                        )
                    } else if s.exclusive_active && s.bit_perfect && s.direct_volume_lock {
                        (
                            "BIT-PERFECT EXCLUSIVE",
                            "Hardware Hog Mode · 1:1 Clock · 0.0 dBFS Direct Pass-Through",
                            palette.hires_gold,
                        )
                    } else if s.bit_perfect && native {
                        (
                            "BIT-PERFECT DIRECT",
                            "Zero DSP · 1:1 Sample Rate · Unaltered Bit-Stream",
                            palette.hires_gold,
                        )
                    } else if s.exclusive_active {
                        (
                            "EXCLUSIVE LOSSLESS",
                            "Direct Hardware DAC Lock · Dedicated Audio Clock",
                            palette.hires_gold,
                        )
                    } else if dsp_active {
                        (
                            "STUDIO ENHANCED (64-BIT DSP)",
                            "Precision 64-bit Floating-Point Processing Active",
                            palette.accent_secondary,
                        )
                    } else if s.oversampling != OversamplingMode::Native && s.output_rate > s.source_rate {
                        (
                            "OVERSAMPLED STUDIO",
                            "High-Precision Sinc Upsampling Active",
                            palette.accent_bright,
                        )
                    } else if native {
                        (
                            "LOSSLESS NATIVE",
                            "Bit-Matched Sample Rate · DSP Bypassed",
                            palette.accent,
                        )
                    } else {
                        (
                            "RESAMPLED PLAYBACK",
                            "High-Precision Rubato Sinc Conversion Active",
                            palette.accent_bright,
                        )
                    };
                Frame::new()
                    .fill(verdict_col.gamma_multiply(0.11))
                    .stroke(Stroke::new(1.0, verdict_col.gamma_multiply(0.55)))
                    .corner_radius(6.0)
                    .inner_margin(Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                            ui.painter()
                                .circle_filled(dot.center(), 5.0, verdict_col.gamma_multiply(0.28));
                            ui.painter().circle_filled(dot.center(), 3.2, verdict_col);
                            ui.vertical(|ui| {
                                ui.style_mut().spacing.item_spacing.y = 0.5;
                                ui.label(
                                    RichText::new(verdict_title)
                                        .size(10.5)
                                        .strong()
                                        .monospace()
                                        .color(verdict_col),
                                );
                                ui.label(
                                    RichText::new(verdict_sub)
                                        .size(9.5)
                                        .color(palette.text_secondary),
                                );
                            });
                        });
                    });
                ui.add_space(7.0);

                let bits = s.bit_depth.map(|b| format!("{b}-bit · ")).unwrap_or_default();
                let br = s
                    .bitrate_kbps
                    .map(|k| format!(" · {} {k} kbps", if s.variable_bitrate { "VBR" } else { "CBR" }))
                    .unwrap_or_default();
                stage_row(
                    ui,
                    "1",
                    "Source Decoder",
                    format!(
                        "{} · {}{} · {} ch{}",
                        s.codec,
                        bits,
                        format_sample_rate(s.source_rate),
                        s.channels,
                        br
                    ),
                    palette.accent,
                    false,
                );
                ui.add_space(2.0);
                let (mem_detail, mem_col) = if let Some(bytes) = s.ram_bytes {
                    (
                        format!(
                            "Play from RAM ({} · Zero Disk I/O)",
                            format_file_size(bytes as u64)
                        ),
                        palette.hires_gold,
                    )
                } else {
                    (
                        "Streaming Lock-Free RingBuffer (3.0s)".to_string(),
                        palette.accent,
                    )
                };
                stage_row(ui, "2", "Memory Engine", mem_detail, mem_col, false);
                ui.add_space(2.0);
                let native = s.source_rate == s.output_rate && (playback_speed - 1.0).abs() < 0.001;
                stage_row(
                    ui,
                    "3",
                    "Oversampling & Tempo",
                    if s.bit_perfect {
                        format!("BIT-PERFECT 1:1 ({})", format_sample_rate(s.output_rate))
                    } else if native {
                        format!("Native Bit-Matched ({})", format_sample_rate(s.output_rate))
                    } else if s.oversampling != OversamplingMode::Native && (playback_speed - 1.0).abs() < 0.001 {
                        format!(
                            "{} · {} ({} → {})",
                            s.oversampling.name(),
                            s.resampler_quality.name(),
                            format_sample_rate(s.source_rate),
                            format_sample_rate(s.output_rate),
                        )
                    } else {
                        format!(
                            "Rubato Sinc ({} → {}, {:.2}x)",
                            format_sample_rate(s.source_rate),
                            format_sample_rate(s.output_rate),
                            playback_speed
                        )
                    },
                    if s.bit_perfect {
                        palette.hires_gold
                    } else if native {
                        palette.accent
                    } else {
                        palette.accent_bright
                    },
                    false,
                );
                ui.add_space(2.0);
                let rg_note = match s.rg_gain_db {
                    Some(db) => format!(" · RG {:+.1} dB", db),
                    None => String::new(),
                };
                stage_row(
                    ui,
                    "4",
                    "DSP & ReplayGain (64-bit f64)",
                    if plugins.dsp.bit_perfect_mode {
                        "Bypassed (Bit-Perfect Direct)".into()
                    } else {
                        format!("{}{}", plugins.dsp.summary(), rg_note)
                    },
                    if plugins.dsp.is_actively_processing() && !plugins.dsp.bit_perfect_mode {
                        palette.accent_secondary
                    } else {
                        palette.accent
                    },
                    false,
                );
                ui.add_space(2.0);
                let vol_stage = if s.direct_volume_lock {
                    "0.0 dBFS Direct Lock".to_string()
                } else if s.volume >= 0.999 {
                    "Software Vol 100%".to_string()
                } else {
                    format!("Software Vol {:.0}%", s.volume * 100.0)
                };
                let excl_tag = if s.exclusive_active {
                    "Exclusive Hog Mode"
                } else if s.hw_rate_switched {
                    "HW Clock Sync"
                } else {
                    "Shared"
                };
                stage_row(
                    ui,
                    "5",
                    &format!("{AUDIO_BACKEND_NAME} Hardware Output ({excl_tag})"),
                    format!(
                        "{} · {} · {} · {}",
                        s.device_name,
                        s.output_format,
                        format_sample_rate(s.output_rate),
                        vol_stage
                    ),
                    if s.exclusive_active || (s.direct_volume_lock && s.bit_perfect) {
                        palette.hires_gold
                    } else {
                        palette.accent_bright
                    },
                    true,
                );
            } else {
                ui.label(
                    RichText::new("No active audio stream. Play a track to inspect live bit-path.")
                        .size(11.0)
                        .color(palette.muted),
                );
            }

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(5.0);
            // Quick Speed & A-B Section Loop row
            ui.horizontal(|ui| {
                ui.label(RichText::new("Speed:").size(10.5).color(palette.muted));
                for &spd in &[0.75_f32, 0.85, 1.0, 1.25, 1.5] {
                    if pill_button(
                        ui,
                        &format!("{spd}x"),
                        None,
                        if (playback_speed - spd).abs() < 0.02 {
                            PillStyle::Active
                        } else {
                            PillStyle::Secondary
                        },
                        palette,
                    )
                    .clicked()
                    {
                        out.speed_changed = Some(spd);
                    }
                }
                if pill_button(
                    ui,
                    "Pitch-Lock",
                    None,
                    if plugins.dsp.preserve_pitch && !plugins.dsp.bit_perfect_mode {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    },
                    palette,
                )
                .on_hover_text("Keep original musical key when changing speed")
                .clicked()
                {
                    plugins.dsp.preserve_pitch = !plugins.dsp.preserve_pitch;
                    if plugins.dsp.preserve_pitch {
                        plugins.dsp.enabled = true;
                        plugins.dsp.bit_perfect_mode = false;
                    }
                    out.dsp_changed = true;
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("A-B Loop:").size(10.5).color(palette.muted));
                let a_lbl = ab_loop
                    .0
                    .map(|a| format!("A {}", format_duration(Some(a))))
                    .unwrap_or_else(|| "Set A ([)".into());
                let b_lbl = ab_loop
                    .1
                    .map(|b| format!("B {}", format_duration(Some(b))))
                    .unwrap_or_else(|| "Set B (])".into());
                if pill_button(
                    ui,
                    &a_lbl,
                    None,
                    if ab_loop.0.is_some() {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    },
                    palette,
                )
                .clicked()
                {
                    out.set_loop_a = true;
                }
                if pill_button(
                    ui,
                    &b_lbl,
                    None,
                    if ab_loop.1.is_some() {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    },
                    palette,
                )
                .clicked()
                {
                    out.set_loop_b = true;
                }
                if (ab_loop.0.is_some() || ab_loop.1.is_some())
                    && pill_button(ui, "Clear", Some(Icon::Close), PillStyle::Danger, palette)
                        .clicked()
                {
                    out.clear_loop = true;
                }
            });
            ui.add_space(4.0);
            // Sleep Timer with gentle 5-second fade-out
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Sleep:").size(10.5).color(palette.muted));
                for (lbl, preset, active) in [
                    ("Off", SleepTimerPreset::Off, sleep_status.is_none()),
                    ("15m", SleepTimerPreset::Minutes(15), false),
                    ("30m", SleepTimerPreset::Minutes(30), false),
                    ("45m", SleepTimerPreset::Minutes(45), false),
                    ("60m", SleepTimerPreset::Minutes(60), false),
                    ("End", SleepTimerPreset::AfterTrack, sleep_after_track),
                ] {
                    if pill_button(
                        ui,
                        lbl,
                        None,
                        if active {
                            PillStyle::Active
                        } else {
                            PillStyle::Secondary
                        },
                        palette,
                    )
                    .clicked()
                    {
                        out.sleep_preset = Some(preset);
                    }
                }
                if let Some(rem) = sleep_status {
                    ui.label(
                        RichText::new(rem)
                            .size(10.0)
                            .monospace()
                            .strong()
                            .color(palette.hires_gold),
                    );
                }
            });
            if has_current_track {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Bookmarks:").size(10.5).color(palette.muted));
                    if pill_button(
                        ui,
                        &format!("+ {}", format_duration(Some(current_pos_secs))),
                        None,
                        PillStyle::Accent,
                        palette,
                    )
                    .on_hover_text("Drop a timestamp bookmark at the current playback position")
                    .clicked()
                    {
                        out.add_bookmark = true;
                    }
                    for (idx, bm) in bookmarks.iter().enumerate() {
                        let lbl = if bm.label.is_empty() {
                            format_duration(Some(bm.position_secs))
                        } else {
                            format!("{} {}", format_duration(Some(bm.position_secs)), bm.label)
                        };
                        let resp = pill_button(ui, &lbl, None, PillStyle::Secondary, palette)
                            .on_hover_text("Click to jump · Right-click to remove bookmark");
                        if resp.clicked() {
                            out.jump_bookmark_secs = Some(bm.position_secs);
                        }
                        if resp.secondary_clicked() {
                            out.remove_bookmark_idx = Some(idx);
                        }
                    }
                });
            }

            ui.add_space(6.0);
            ui.separator();
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                if pill_button(
                    ui,
                    if plugins.dsp.bit_perfect_mode {
                        "Bit-Perfect"
                    } else if plugins.dsp.enabled {
                        "DSP: On"
                    } else {
                        "DSP: Bypass"
                    },
                    Some(Icon::Sliders),
                    if plugins.dsp.bit_perfect_mode {
                        PillStyle::Active
                    } else if plugins.dsp.enabled {
                        PillStyle::Accent
                    } else {
                        PillStyle::Secondary
                    },
                    palette,
                )
                .clicked()
                {
                    if plugins.dsp.bit_perfect_mode {
                        plugins.dsp.bit_perfect_mode = false;
                    } else {
                        plugins.dsp.enabled = !plugins.dsp.enabled;
                    }
                    out.dsp_changed = true;
                }
                if pill_button(
                    ui,
                    "Crossfeed",
                    None,
                    if !plugins.dsp.bit_perfect_mode
                        && plugins.dsp.enabled
                        && plugins.dsp.crossfeed_enabled
                    {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    },
                    palette,
                )
                .clicked()
                {
                    plugins.dsp.bit_perfect_mode = false;
                    plugins.dsp.crossfeed_enabled = !plugins.dsp.crossfeed_enabled;
                    if plugins.dsp.crossfeed_enabled {
                        plugins.dsp.enabled = true;
                    }
                    out.dsp_changed = true;
                }
                if pill_button(
                    ui,
                    if plugins.dsp.direct_volume_lock {
                        "0 dB Lock"
                    } else {
                        "DAC 0 dB"
                    },
                    None,
                    if plugins.dsp.direct_volume_lock {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    },
                    palette,
                )
                .on_hover_text("Bypass software volume attenuation (fixed 0.0 dBFS pass-through for external DAC/amp)")
                .clicked()
                {
                    plugins.dsp.direct_volume_lock = !plugins.dsp.direct_volume_lock;
                    out.dsp_changed = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "Audio…", None, PillStyle::Secondary, palette).clicked() {
                        out.open_settings_dsp = true;
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for &preset in &[
                    EqPreset::Flat,
                    EqPreset::ConcertHall,
                    EqPreset::PianoWarmth,
                    EqPreset::LateNight,
                ] {
                    if pill_button(
                        ui,
                        preset.name(),
                        None,
                        if !plugins.dsp.bit_perfect_mode
                            && plugins.dsp.enabled
                            && plugins.dsp.eq_preset == preset
                        {
                            PillStyle::Active
                        } else {
                            PillStyle::Secondary
                        },
                        palette,
                    )
                    .clicked()
                    {
                        plugins.dsp.bit_perfect_mode = false;
                        plugins.dsp.apply_preset(preset);
                        plugins.dsp.enabled =
                            preset != EqPreset::Flat || plugins.dsp.crossfeed_enabled;
                        plugins.dsp.eq_enabled = true;
                        out.dsp_changed = true;
                    }
                }
            });
        },
    );

    out
}

#[derive(Clone)]
pub enum PaletteCommand {
    PlayTrack(u64),
    OpenPlaylist(u64),
    OpenCollection(u64),
    OpenSavedView(u64),
    SwitchLibrary(u64),
    SetTheme(ThemeId),
    SetVisualizer(VisualizerMode),
    ApplyEqPreset(EqPreset),
    ToggleDsp,
    ToggleVisualizer,
    ToggleCompactMode,
    ToggleLyrics,
    ToggleStopAfterCurrent,
    JumpToPlaying,
    SaveQueueAsPlaylist,
    UndoPlaylist,
    ImportPlaylist,
    ExportActivePlaylist,
    EditCurrentTags,
    OpenSettings(SettingsTab),
    AddFolder,
    OpenLibraryInsights,
    OpenDbHealth,
    NewSmartView,
    NewCollection,
    DropBookmark,
    SetSleepTimer(SleepTimerPreset),
    VerifyCurrentAlbumAudit,
    ScanCurrentAlbumReplayGain,
    StartLocalRadio,
    SetLoopA,
    SetLoopB,
    ClearAbLoop,
    CopyCurrentTrackSpecs,
    ExportViewCatalogHtml,
    ExportViewCatalogMarkdown,
    OpenQuickHelp,
    OpenAbxLab,
    OpenDebugConsole,
    ToggleLanRemote,
}

pub struct PaletteEntry {
    pub category: &'static str,
    pub label: String,
    pub subtitle: String,
    pub action: PaletteCommand,
}

#[derive(Default)]
pub struct CommandPaletteState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    pub just_opened: bool,
}

impl CommandPaletteState {
    pub fn open_fresh(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.just_opened = true;
    }
}

/// Fast keyboard-first `Cmd+K` Command Palette for zero-click navigation & actions.
pub fn show_command_palette(
    context: &egui::Context,
    state: &mut CommandPaletteState,
    entries: &[PaletteEntry],
    palette: &Palette,
) -> Option<PaletteCommand> {
    if !state.open {
        return None;
    }

    let q = state.query.trim().to_lowercase();
    let terms: Vec<&str> = q.split_whitespace().collect();
    let filtered: Vec<&PaletteEntry> = entries
        .iter()
        .filter(|e| {
            if terms.is_empty() {
                return true;
            }
            let hay = format!(
                "{} {} {}",
                e.category.to_lowercase(),
                e.label.to_lowercase(),
                e.subtitle.to_lowercase()
            );
            terms.iter().all(|t| hay.contains(t))
        })
        .take(14)
        .collect();

    if !filtered.is_empty() {
        state.selected = state.selected.min(filtered.len() - 1);
    } else {
        state.selected = 0;
    }

    let mut chosen: Option<PaletteCommand> = None;

    if context.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.open = false;
        return None;
    }
    if context.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !filtered.is_empty() {
        state.selected = (state.selected + 1) % filtered.len();
    }
    if context.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !filtered.is_empty() {
        state.selected = (state.selected + filtered.len() - 1) % filtered.len();
    }
    if context.input(|i| i.key_pressed(egui::Key::Enter))
        && let Some(entry) = filtered.get(state.selected)
    {
        chosen = Some(entry.action.clone());
        state.open = false;
        return chosen;
    }

    let screen = context.content_rect();
    let palette_w = (screen.width() - 32.0).clamp(300.0, 540.0);
    let init_x = (screen.center().x - palette_w * 0.5).max(screen.left() + 8.0);
    let init_y = screen.top() + 48.0;
    egui::Area::new(egui::Id::unique("command_palette_area"))
        .default_pos(pos2(init_x, init_y))
        .movable(true)
        .constrain_to(screen.shrink(6.0))
        .order(egui::Order::Foreground)
        .show(context, |ui| {
            Frame::new()
                .fill(palette.panel)
                .stroke(Stroke::new(1.0, palette.selected_border))
                .corner_radius(8.0)
                .inner_margin(Margin::symmetric(12, 10))
                .shadow(egui::Shadow {
                    offset: [0, 10],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_rgba_unmultiplied(
                        0,
                        0,
                        0,
                        if palette.is_light { 48 } else { 150 },
                    ),
                })
                .show(ui, |ui| {
                    ui.set_width(palette_w);
                    ui.horizontal(|ui| {
                        let (ir, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
                        icons::paint_icon(ui.painter(), ir, Icon::Search, palette.accent);
                        let resp = ui.add_sized(
                            [(palette_w - 64.0).max(160.0), 24.0],
                            egui::TextEdit::singleline(&mut state.query)
                                .frame(Frame::NONE)
                                .hint_text(
                                    RichText::new(
                                        "Jump to track, playlist, theme, visualizer, EQ preset, or action…",
                                    )
                                    .size(12.5)
                                    .color(palette.faint),
                                ),
                        );
                        if state.just_opened {
                            resp.request_focus();
                            state.just_opened = false;
                        }
                        if resp.changed() {
                            state.selected = 0;
                        }
                        ui.label(RichText::new("ESC").size(9.0).monospace().color(palette.faint));
                    });
                    ui.separator();
                    ui.add_space(4.0);

                    if filtered.is_empty() {
                        ui.label(
                            RichText::new("No matching commands or tracks.")
                                .size(11.5)
                                .color(palette.muted),
                        );
                    } else {
                        ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                            ui.style_mut().spacing.item_spacing.y = 2.0;
                            for (idx, entry) in filtered.iter().enumerate() {
                                let sel = idx == state.selected;
                                let (rect, resp) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 30.0),
                                    Sense::click(),
                                );
                                if sel {
                                    ui.painter().rect_filled(rect, 6.0, palette.selected);
                                    ui.painter().rect_stroke(
                                        rect,
                                        6.0,
                                        Stroke::new(1.0, palette.selected_border),
                                        StrokeKind::Inside,
                                    );
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(rect, 6.0, palette.hover);
                                }
                                let badge = Rect::from_min_size(
                                    pos2(rect.left() + 6.0, rect.center().y - 9.0),
                                    vec2(66.0, 18.0),
                                );
                                ui.painter().rect_filled(badge, 4.0, palette.surface);
                                ui.painter().text(
                                    badge.center(),
                                    Align2::CENTER_CENTER,
                                    entry.category,
                                    FontId::proportional(9.0),
                                    palette.accent,
                                );
                                let text_r = Rect::from_min_max(
                                    pos2(badge.right() + 8.0, rect.top()),
                                    pos2(rect.right() - 8.0, rect.bottom()),
                                );
                                let full_line = if entry.subtitle.is_empty() {
                                    entry.label.clone()
                                } else {
                                    format!("{}  ·  {}", entry.label, entry.subtitle)
                                };
                                ui.painter().with_clip_rect(text_r).text(
                                    pos2(text_r.left(), rect.center().y),
                                    Align2::LEFT_CENTER,
                                    full_line,
                                    FontId::proportional(11.5),
                                    if sel { palette.text } else { palette.text_secondary },
                                );
                                if resp.clicked() {
                                    chosen = Some(entry.action.clone());
                                    state.open = false;
                                }
                            }
                        });
                    }
                });
        });

    chosen
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuickHelpTab {
    #[default]
    FirstSteps,
    Shortcuts,
    PowerSearch,
    AudioDsp,
}

/// Built-in F1 Quick Guide, First Steps & Interactive Shortcuts Reference modal.
pub fn show_quick_help_modal(
    context: &egui::Context,
    open: &mut bool,
    tab: &mut QuickHelpTab,
    palette: &Palette,
) {
    if !*open {
        return;
    }
    let mut is_open = *open;
    let ver = format!("v{} · Press F1 anytime", env!("CARGO_PKG_VERSION"));
    themed_modal_window(
        context,
        "quick_help_modal",
        "TxPlayer Quick Guide & Reference",
        Icon::Info,
        Some(&ver),
        [690.0, 515.0],
        Some([540.0, 420.0]),
        true,
        &mut is_open,
        palette,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for (t, lbl) in [
                    (QuickHelpTab::FirstSteps, "5-Min First Steps"),
                    (QuickHelpTab::Shortcuts, "Keyboard Shortcuts"),
                    (QuickHelpTab::PowerSearch, "Power-Search Tokens"),
                    (QuickHelpTab::AudioDsp, "Audio, DoP & Practice"),
                ] {
                    if pill_button(
                        ui,
                        lbl,
                        None,
                        if *tab == t {
                            PillStyle::Accent
                        } else {
                            PillStyle::Secondary
                        },
                        palette,
                    )
                    .clicked()
                    {
                        *tab = t;
                    }
                }
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            ScrollArea::vertical()
                .id_salt("quick_help_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| match *tab {
                    QuickHelpTab::FirstSteps => {
                        let step = |ui: &mut Ui, num: &str, title: &str, body: &str| {
                            Frame::new()
                                .fill(palette.background)
                                .stroke(Stroke::new(1.0, palette.border))
                                .corner_radius(7.0)
                                .inner_margin(Margin::symmetric(10, 7))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(num)
                                                .size(10.0)
                                                .strong()
                                                .monospace()
                                                .color(palette.hires_gold),
                                        );
                                        ui.label(
                                            RichText::new(title)
                                                .size(12.0)
                                                .strong()
                                                .color(palette.text),
                                        );
                                    });
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new(body)
                                            .size(11.0)
                                            .color(palette.text_secondary),
                                    );
                                });
                            ui.add_space(5.0);
                        };
                        step(
                            ui,
                            "STEP 1",
                            "Add Music Folders & Isolated Libraries",
                            "Drag & drop any folder onto TxPlayer or click + Folder. Supports FLAC, ALAC, WAV, AIFF, DSF/DFF (DSD64–512), WavPack (.wv), Monkey's Audio (.ape), Opus, CUE sheets, and SACD .iso images.",
                        );
                        step(
                            ui,
                            "STEP 2",
                            "Unlock Bit-Perfect DAC & Native DSD over PCM (DoP v1.1)",
                            "Click the Signal Path pill (bottom-right) or open Settings → Audio & DSP. Enable Bit-Perfect + Exclusive DAC + 0.0 dBFS Direct Lock for pure hardware playback, or save a Per-Device Profile per DAC/headphone.",
                        );
                        step(
                            ui,
                            "STEP 3",
                            "Sculpt Sound with Interactive EQ, FFT Convolution & DSP A/B",
                            "Drag the 10 glowing frequency nodes directly on the live EQ curve (double-click to reset), load AutoEQ (.txt) or .WAV Convolution IRs, and press D anytime for instant DSP A/B bypass comparison.",
                        );
                        step(
                            ui,
                            "STEP 4",
                            "Explore All 6 Concert Visualizers & Pitch-Locked Practice",
                            "Press V to open the Concert Stage and 1–6 to switch between 88-Key Piano (Pause freezes chords!), Piano Roll, 64-Band RTA, Live Acoustic Spectrogram, Stereo Scope, and Mastering VU + EBU R128 LUFS.",
                        );
                        step(
                            ui,
                            "STEP 5",
                            "Master Your Library, Audits & Portable Catalogs",
                            "Right-click any album to Verify TrueHz/DR or Scan EBU R128 ReplayGain, start 100% Local Similar Radio, run Blind ABX comparisons, or export print-ready HTML & Markdown dossiers from Library Insights.",
                        );
                    }
                    QuickHelpTab::Shortcuts => {
                        let rows: &[(&str, &str)] = &[
                            (&format!("{MOD_SHORT}K"), "Command Palette — fuzzy jump to any track, view, theme, or tool"),
                            ("F1", "Open this Quick Guide & Shortcuts Reference"),
                            (&format!("{MOD_SHORT}F"), "Focus Power-Search bar"),
                            ("Space", "Play / Pause (freezes pressed piano keys & chord readout)"),
                            ("D / E", "Instant DSP A/B Comparison / Toggle Audio DSP"),
                            ("V / 1–6", "Toggle Concert Visualizer Stage / Switch Visualizer Mode (1–6)"),
                            ("[ / ] / \\", "Set A–B Section Loop Point A / Point B / Clear Loop"),
                            (&format!("{MOD_SHORT}B"), "Drop persistent Timestamp Bookmark at current position"),
                            ("L", "Toggle Synced Lyrics (.lrc) drawer"),
                            (&format!("{MOD_SHORT}T / {MOD_SHORT}I"), "Edit Tags & Batch Transforms / Track Info & TrueHz Inspector"),
                            (&format!("{MOD_SHORT}J"), "Jump to currently playing track"),
                            (&format!("{MOD_SHORT}Shift+M"), "Toggle Compact Mini-Player Mode"),
                            ("← → / Cmd+← →", "Seek ±10 s / Previous & Next Track"),
                            ("Cmd+↑ ↓ / M", "Volume ±5% / Mute"),
                            (&format!("{MOD_SHORT},"), "Open Settings"),
                            ("Esc", "Dismiss any modal, popover, or selection"),
                        ];
                        egui::Grid::new("help_shortcuts_grid")
                            .num_columns(2)
                            .spacing([18.0, 6.0])
                            .show(ui, |ui| {
                                for (k, desc) in rows {
                                    ui.label(
                                        RichText::new(*k)
                                            .size(11.0)
                                            .strong()
                                            .monospace()
                                            .color(palette.hires_gold),
                                    );
                                    ui.label(
                                        RichText::new(*desc)
                                            .size(11.0)
                                            .color(palette.text_secondary),
                                    );
                                    ui.end_row();
                                }
                            });
                    }
                    QuickHelpTab::PowerSearch => {
                        ui.label(
                            RichText::new("Combine any tokens right inside the search bar:")
                                .size(11.5)
                                .color(palette.text),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("composer:beethoven dr>=12 rate>=96 codec:flac #reference")
                                .size(11.0)
                                .monospace()
                                .color(palette.accent_bright),
                        );
                        ui.add_space(8.0);
                        let tokens = [
                            ("composer:<name>", "Matches Composer metadata (e.g. composer:chopin)"),
                            ("artist:<name>", "Matches Artist or Album Artist"),
                            ("album:<name>", "Matches Album title"),
                            ("work:<name>", "Matches Work / Opus (e.g. work:bwv or work:op.27)"),
                            ("label:<name>", "Matches Record Label (e.g. label:grammophon)"),
                            ("codec:<name>", "Matches Codec (flac, dsd, alac, wv, ape, opus)"),
                            ("dr>=N / dr<=N / dr=N", "Filter by persisted Dynamic Range score"),
                            ("rate>=N / sr>=N", "Sample rate in kHz (e.g. rate>=96)"),
                            ("bits>=N", "Bit depth (e.g. bits>=24)"),
                            ("rating>=N", "Star rating 1–5 (e.g. rating>=4)"),
                            ("#tag", "Matches custom collector hashtag (e.g. #reference)"),
                        ];
                        egui::Grid::new("help_tokens_grid")
                            .num_columns(2)
                            .spacing([16.0, 5.0])
                            .show(ui, |ui| {
                                for (tok, desc) in tokens {
                                    ui.label(
                                        RichText::new(tok)
                                            .size(10.8)
                                            .monospace()
                                            .color(palette.accent),
                                    );
                                    ui.label(
                                        RichText::new(desc)
                                            .size(10.8)
                                            .color(palette.text_secondary),
                                    );
                                    ui.end_row();
                                }
                            });
                    }
                    QuickHelpTab::AudioDsp => {
                        ui.label(
                            RichText::new("• Bit-Perfect & DoP v1.1: Hard-bypasses all DSP and streams native PCM or 1-bit DSD64/128 directly to your DAC.")
                                .size(11.2)
                                .color(palette.text_secondary),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("• Pitch-Preserving Practice & Tuning: Enable Pitch-Lock beside Speed (0.75x–1.5x) to keep exact musical pitch while practicing piano, transpose ±12 semitones, or switch concert pitch to A=432 Hz or Baroque A=415 Hz.")
                                .size(11.2)
                                .color(palette.text_secondary),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("• Blind ABX Comparison Lab: Load two masters of the same recording (Cmd/Ctrl+K → ABX Lab), auto-match EBU R128 loudness to 0.1 dB, and run blind A/B/X trials with live binomial p-value statistics.")
                                .size(11.2)
                                .color(palette.text_secondary),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("• Local LAN Mobile Remote: Enable in Settings → Plugins to control playback, Signal Path, volume, DSP A/B, and library search from any phone on your Wi-Fi (100% offline LAN).")
                                .size(11.2)
                                .color(palette.text_secondary),
                        );
                    }
                });
        },
    );
    *open = is_open;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbxLabAction {
    PlaySlot(AbxSlot),
    Guess(bool),
    ToggleReveal,
    ResetTrials,
    LoadCurrentAsA,
    LoadCurrentAsB,
    SwapAB,
}

/// Loudness-Matched Blind ABX Comparison Lab modal with exact binomial p-value statistics.
pub fn show_abx_lab_modal(
    context: &egui::Context,
    state: &mut AbxLabState,
    track_a: Option<&Track>,
    track_b: Option<&Track>,
    palette: &Palette,
) -> Option<AbxLabAction> {
    if !state.open {
        return None;
    }
    let mut is_open = state.open;
    let mut action: Option<AbxLabAction> = None;

    themed_modal_window(
        context,
        "abx_comparison_lab",
        "Loudness-Matched Blind ABX Comparison Lab",
        Icon::Sliders,
        Some("Exact Binomial p-value · EBU R128 Trim"),
        [640.0, 495.0],
        Some([520.0, 420.0]),
        true,
        &mut is_open,
        palette,
        |ui| {
            let lufs_a = track_a.and_then(|t| t.integrated_lufs);
            let lufs_b = track_b.and_then(|t| t.integrated_lufs);
            let (trim_a, trim_b) = compute_loudness_match_trims(lufs_a, lufs_b);

            // Track A & Track B Master Cards
            ui.columns(2, |cols| {
                let draw_card = |ui: &mut Ui,
                                 badge: &str,
                                 t: Option<&Track>,
                                 trim_db: f32,
                                 load_lbl: &str|
                 -> bool {
                    let mut load_clicked = false;
                    Frame::new()
                        .fill(palette.background)
                        .stroke(Stroke::new(1.0, palette.border))
                        .corner_radius(7.0)
                        .inner_margin(Margin::symmetric(10, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(badge)
                                        .size(10.0)
                                        .strong()
                                        .monospace()
                                        .color(palette.hires_gold),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if pill_button(ui, load_lbl, None, PillStyle::Secondary, palette)
                                        .clicked()
                                    {
                                        load_clicked = true;
                                    }
                                });
                            });
                            ui.add_space(3.0);
                            if let Some(tr) = t {
                                ui.label(
                                    RichText::new(&tr.title)
                                        .size(12.0)
                                        .strong()
                                        .color(palette.text),
                                );
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {}",
                                        tr.quality_full(),
                                        tr.dr_score
                                            .map(|d| format!("DR{d}"))
                                            .unwrap_or_else(|| "DR —".into())
                                    ))
                                    .size(10.0)
                                    .monospace()
                                    .color(palette.accent),
                                );
                                let lufs_str = tr
                                    .integrated_lufs
                                    .map(|l| format!("{l:.1} LUFS · Match Trim {trim_db:+.1} dB"))
                                    .unwrap_or_else(|| {
                                        "No LUFS yet (run ReplayGain/TrueHz audit)".into()
                                    });
                                ui.label(
                                    RichText::new(lufs_str).size(9.5).color(palette.muted),
                                );
                            } else {
                                ui.label(
                                    RichText::new("No master loaded — right-click a track or click 'Use Playing'")
                                        .size(10.5)
                                        .color(palette.faint),
                                );
                            }
                        });
                    load_clicked
                };

                if draw_card(&mut cols[0], "MASTER A", track_a, trim_a, "Use Playing") {
                    action = Some(AbxLabAction::LoadCurrentAsA);
                }
                if draw_card(&mut cols[1], "MASTER B", track_b, trim_b, "Use Playing") {
                    action = Some(AbxLabAction::LoadCurrentAsB);
                }
            });

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                aligned_inline_checkbox(
                    ui,
                    &mut state.loudness_matched,
                    "Auto EBU R128 Loudness Match",
                    215.0,
                    palette,
                );
                if pill_button(ui, "Swap A ↔ B", None, PillStyle::Secondary, palette).clicked() {
                    action = Some(AbxLabAction::SwapAB);
                }
                if pill_button(ui, "Reset Trials", None, PillStyle::Secondary, palette).clicked() {
                    action = Some(AbxLabAction::ResetTrials);
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            let ready = track_a.is_some() && track_b.is_some();
            ui.label(
                RichText::new("1) AUDITION SEAMLESSLY AT IDENTICAL TIMESTAMP")
                    .size(9.5)
                    .strong()
                    .color(palette.faint),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for (slot, lbl) in [
                    (AbxSlot::A, "Play Master A"),
                    (AbxSlot::B, "Play Master B"),
                    (AbxSlot::X, "Play Blind X"),
                ] {
                    let style = if state.active_slot == Some(slot) {
                        PillStyle::Accent
                    } else if slot == AbxSlot::X {
                        PillStyle::Active
                    } else {
                        PillStyle::Secondary
                    };
                    if ui
                        .add_enabled_ui(ready, |ui| pill_button(ui, lbl, Some(Icon::Play), style, palette))
                        .inner
                        .clicked()
                    {
                        action = Some(AbxLabAction::PlaySlot(slot));
                    }
                }
            });

            ui.add_space(10.0);
            ui.label(
                RichText::new(format!(
                    "2) BLIND TRIAL #{} — IDENTIFY HIDDEN MASTER X",
                    state.history.len() + 1
                ))
                .size(9.5)
                .strong()
                .color(palette.faint),
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled_ui(ready, |ui| {
                        pill_button(ui, "X is Master A", Some(Icon::Check), PillStyle::Active, palette)
                    })
                    .inner
                    .clicked()
                {
                    action = Some(AbxLabAction::Guess(false));
                }
                if ui
                    .add_enabled_ui(ready, |ui| {
                        pill_button(ui, "X is Master B", Some(Icon::Check), PillStyle::Active, palette)
                    })
                    .inner
                    .clicked()
                {
                    action = Some(AbxLabAction::Guess(true));
                }
                if pill_button(
                    ui,
                    if state.revealed { "Hide Score" } else { "Reveal Results" },
                    None,
                    PillStyle::Secondary,
                    palette,
                )
                .clicked()
                {
                    action = Some(AbxLabAction::ToggleReveal);
                }
            });

            ui.add_space(10.0);
            let n = state.history.len();
            let c = state.correct_count();
            let pval = state.p_value();
            Frame::new()
                .fill(palette.background)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(7.0)
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    if n == 0 {
                        ui.label(
                            RichText::new("Complete at least 8–10 blind trials for statistical confidence (p < 0.05).")
                                .size(11.0)
                                .color(palette.muted),
                        );
                    } else if !state.revealed {
                        ui.label(
                            RichText::new(format!(
                                "{n} blind trial(s) recorded — click 'Reveal Results' when finished to avoid bias."
                            ))
                            .size(11.5)
                            .color(palette.accent_bright),
                        );
                    } else {
                        let pct = (c as f64 / n as f64) * 100.0;
                        let sig = n >= 5 && pval < 0.05;
                        ui.label(
                            RichText::new(format!(
                                "Score: {c} / {n} ({pct:.1}%)   ·   Exact Binomial p-value = {pval:.4}"
                            ))
                            .size(12.5)
                            .strong()
                            .monospace()
                            .color(if sig { palette.hires_gold } else { palette.text }),
                        );
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(if sig {
                                "Statistically significant audible difference detected (p < 0.05)!"
                            } else {
                                "Inconclusive (p ≥ 0.05) — indistinguishable from chance so far."
                            })
                            .size(11.0)
                            .color(if sig { palette.accent_bright } else { palette.muted }),
                        );
                    }
                });
        },
    );

    state.open = is_open;
    action
}

