use std::path::PathBuf;
use std::thread;

use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};

use crate::audio::{default_output_device_name, list_output_devices};
use crate::icons::{self, Icon};
use crate::library::cover_directory;
use crate::metadata::{Track, format_duration, format_file_size, format_sample_rate};
use crate::platform::{AUDIO_BACKEND_NAME, FILE_MANAGER_NAME, reveal_in_file_manager};
use crate::plugins::{
    DspConfig, EQ_FREQ_LABELS, EqPreset, PianoEngineId, PluginConfig, ReplayGainMode,
    TagProviderConfig, ToolHealth,
};
use crate::tagger::{
    EditableTags, LookupStrategy, TagCandidate, TrackHashes, compute_fast_audio_hash,
    download_cover_artwork, lookup_online, record_hash_cache, write_tags_to_file,
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
    pub tx: Sender<TagWorkerEvent>,
    pub rx: Receiver<TagWorkerEvent>,
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
        .resizable(resizable && !is_tight)
        .frame(frame)
        .constrain_to(screen.shrink(6.0))
        .max_size([max_w, max_h]);

    if is_tight || !resizable {
        win = win.fixed_size([win_w, win_h]);
        if is_tight {
            win = win.fixed_pos(pos2(pos_x, pos_y));
        } else {
            win = win.default_pos(pos2(pos_x, pos_y));
        }
    } else {
        win = win
            .default_pos(pos2(pos_x, pos_y))
            .default_size([win_w, win_h]);
        if let Some(ms) = min_size {
            win = win.min_size([ms[0].min(max_w), ms[1].min(max_h)]);
        }
    }

    win.show(context, |ui| {
        // Slim 28px custom title strip
        let hdr_h = 28.0;
        let avail_w = ui.available_width();
        let (hdr_rect, _) = ui.allocate_exact_size(vec2(avail_w, hdr_h), Sense::hover());
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
                    let prev_bp = plugins.dsp.bit_perfect_mode;
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.dsp.bit_perfect_mode,
                        "Bit-Perfect Direct Output (bypass DSP & ReplayGain, auto-switch native DAC sample rate)",
                        palette,
                    );
                    let prev_gap = plugins.dsp.gapless_enabled;
                    aligned_checkbox_row(
                        ui,
                        &mut plugins.dsp.gapless_enabled,
                        "True Gapless Playback (zero-silence transition between tracks & CUE sheets)",
                        palette,
                    );
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
                    if plugins.dsp.bit_perfect_mode != prev_bp
                        || plugins.dsp.gapless_enabled != prev_gap
                    {
                        actions.dsp_changed = true;
                        actions.rg_changed = true;
                    }

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
) -> Option<Vec<u64>> {
    if !state.open {
        return None;
    }
    let mut save_targets: Option<Vec<u64>> = None;
    let mut is_open = state.open;
    let batch_count = state.target_ids.len();
    let title = if batch_count > 1 {
        format!("Tag Editor · {batch_count} Tracks Selected")
    } else {
        "Tag Editor & Online Metadata Matcher".to_string()
    };

    // Cmd+Enter saves tags immediately without moving mouse
    if context.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) && !state.saving {
        save_targets = Some(state.target_ids.clone());
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

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);

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
                                    save_targets = Some(state.target_ids.clone());
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
                        save_targets = Some(state.target_ids.clone());
                    }
                });
            });
        });

    state.open = is_open;
    save_targets
}

pub fn show_track_info_window(
    context: &egui::Context,
    track: &Track,
    palette: &Palette,
    cover_paint: impl FnOnce(&mut Ui, f32),
    quality_paint: impl FnOnce(&mut Ui),
) -> (bool, Option<TrackInfoAction>) {
    let mut open = true;
    let mut action = None;

    themed_modal_window(
        context,
        "track_inspector_modal",
        "Track Info",
        Icon::Info,
        None,
        [480.0, 390.0],
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
                    if let Some(bytes) = track.file_size_bytes() {
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
                    ui.label(RichText::new("Rating").size(11.0).color(palette.faint));
                    ui.horizontal(|ui| {
                        ui.style_mut().spacing.item_spacing.x = 2.0;
                        for star in 1..=5u8 {
                            let filled = star <= track.rating;
                            if icons::icon_button(
                                ui,
                                Vec2::splat(20.0),
                                if filled {
                                    Icon::StarFilled
                                } else {
                                    Icon::StarOutline
                                },
                                13.0,
                                if filled {
                                    palette.hires_gold
                                } else {
                                    palette.faint
                                },
                                palette.hires_gold,
                                palette.hover,
                                &format!("Set {star}-star rating (click active star to clear)"),
                            )
                            .clicked()
                            {
                                let next = if track.rating == star { 0 } else { star };
                                action = Some(TrackInfoAction::SetRating(next));
                            }
                        }
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

            ui.add_space(12.0);
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

pub enum TrackInfoAction {
    Play,
    EditTags,
    RevealInFinder,
    Remove,
    SetRating(u8),
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
    ui.horizontal(|ui| {
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
        if dsp.crossfeed_enabled {
            let cf_resp = ui.add(
                egui::Slider::new(&mut dsp.crossfeed_amount, 0.05..=1.0)
                    .show_value(false)
                    .trailing_fill(true),
            );
            ui.label(
                RichText::new(format!("{:.0}%", dsp.crossfeed_amount * 100.0))
                    .size(10.0)
                    .monospace()
                    .color(palette.accent),
            );
            if cf_resp.changed() {
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

pub struct SignalPathDisplay<'a> {
    pub codec: &'a str,
    pub source_rate: u32,
    pub output_rate: u32,
    pub channels: u16,
    pub bit_depth: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub variable_bitrate: bool,
    pub device_name: &'a str,
    pub bit_perfect: bool,
    pub rg_gain_db: Option<f32>,
}

pub struct SignalPathActions {
    pub dsp_changed: bool,
    pub open_settings_dsp: bool,
    pub speed_changed: Option<f32>,
    pub set_loop_a: bool,
    pub set_loop_b: bool,
    pub clear_loop: bool,
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

    // Anchor bottom-left of the Area directly at (left_x, bottom_y) using `pivot(Align2::LEFT_BOTTOM)`
    let mut close_clicked = false;
    let resp = egui::Area::new(egui::Id::unique(id))
        .fixed_pos(pos2(left_x, bottom_y))
        .pivot(Align2::LEFT_BOTTOM)
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
                    // Slim 26px popover header
                    let (hdr_rect, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
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
    palette: &Palette,
) -> SignalPathActions {
    let mut out = SignalPathActions {
        dsp_changed: false,
        open_settings_dsp: false,
        speed_changed: None,
        set_loop_a: false,
        set_loop_b: false,
        clear_loop: false,
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
            let stage_row = |ui: &mut Ui, step: &str, title: &str, detail: String, accent: Color32| {
                ui.horizontal(|ui| {
                    let (dot_r, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
                    ui.painter().circle_filled(dot_r.center(), 4.0, accent);
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
                                .size(10.0)
                                .monospace()
                                .color(palette.muted),
                        );
                    });
                });
            };

            if let Some(s) = signal {
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
                );
                ui.add_space(4.0);
                let native = s.source_rate == s.output_rate && (playback_speed - 1.0).abs() < 0.001;
                stage_row(
                    ui,
                    "2",
                    "Sample-Rate & Tempo",
                    if s.bit_perfect {
                        format!("BIT-PERFECT DIRECT ({})", format_sample_rate(s.output_rate))
                    } else if native {
                        format!("Native Bit-Matched ({})", format_sample_rate(s.output_rate))
                    } else {
                        format!(
                            "Rubato Sinc ({} → {}, {:.2}x)",
                            format_sample_rate(s.source_rate),
                            format_sample_rate(s.output_rate),
                            playback_speed
                        )
                    },
                    if s.bit_perfect || native {
                        palette.accent
                    } else {
                        palette.hires_gold
                    },
                );
                ui.add_space(4.0);
                let rg_note = match s.rg_gain_db {
                    Some(db) => format!(" · RG {:+.1} dB", db),
                    None => String::new(),
                };
                stage_row(
                    ui,
                    "3",
                    "DSP & ReplayGain",
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
                );
                ui.add_space(4.0);
                stage_row(
                    ui,
                    "4",
                    &format!("{AUDIO_BACKEND_NAME} Hardware Output"),
                    format!("{} ({})", s.device_name, format_sample_rate(s.output_rate)),
                    palette.accent_bright,
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
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "EQ & DSP…", None, PillStyle::Secondary, palette).clicked() {
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
    SwitchLibrary(u64),
    SetTheme(ThemeId),
    SetVisualizer(VisualizerMode),
    ApplyEqPreset(EqPreset),
    ToggleDsp,
    ToggleVisualizer,
    ToggleCompactMode,
    ToggleLyrics,
    ImportPlaylist,
    ExportActivePlaylist,
    EditCurrentTags,
    OpenSettings(SettingsTab),
    AddFolder,
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
    egui::Area::new(egui::Id::unique("command_palette_area"))
        .anchor(Align2::CENTER_TOP, vec2(0.0, 48.0))
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
