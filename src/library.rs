use std::collections::{HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Sender, unbounded};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::cue::{read_cue_tracks, read_embedded_flac_cue_tracks};
use crate::metadata::{Track, is_cue_file, is_supported, read_track};

#[derive(Debug)]
pub enum ScanEvent {
    Track(Box<Track>),
    Progress { scanned: usize, total: usize },
    Finished { found: usize, failed: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: u64,
    pub name: String,
    pub track_ids: Vec<u64>,
}

/// Persistent chronological play history / scrobble entry stored in the Library Database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayHistoryEntry {
    pub track_id: u64,
    pub played_at: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
}

/// Curated multi-album / multi-track Collection (box sets, themes, audiophile references, eras).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub accent_index: u8,
    #[serde(default)]
    pub track_ids: Vec<u64>,
}

/// Queryable metadata & audio-analysis fields for Custom Saved Views (Smart Database Views).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryField {
    Title,
    Artist,
    Album,
    Composer,
    Genre,
    Label,
    Codec,
    CustomTag,
    SampleRateHz,
    BitDepth,
    DynamicRange,
    Rating,
    PlayCount,
    Year,
    HasReplayGain,
    Authenticity,
}

impl QueryField {
    pub const ALL: &'static [QueryField] = &[
        QueryField::Composer,
        QueryField::Artist,
        QueryField::Album,
        QueryField::Title,
        QueryField::Genre,
        QueryField::Label,
        QueryField::Codec,
        QueryField::CustomTag,
        QueryField::SampleRateHz,
        QueryField::BitDepth,
        QueryField::DynamicRange,
        QueryField::Rating,
        QueryField::PlayCount,
        QueryField::Year,
        QueryField::HasReplayGain,
        QueryField::Authenticity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            QueryField::Title => "Title",
            QueryField::Artist => "Artist",
            QueryField::Album => "Album",
            QueryField::Composer => "Composer",
            QueryField::Genre => "Genre",
            QueryField::Label => "Label",
            QueryField::Codec => "Codec",
            QueryField::CustomTag => "Custom Tag",
            QueryField::SampleRateHz => "Sample Rate (Hz)",
            QueryField::BitDepth => "Bit Depth",
            QueryField::DynamicRange => "DR Score",
            QueryField::Rating => "Rating (0-5)",
            QueryField::PlayCount => "Play Count",
            QueryField::Year => "Year",
            QueryField::HasReplayGain => "Has ReplayGain",
            QueryField::Authenticity => "TrueHz Verdict",
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            QueryField::SampleRateHz
                | QueryField::BitDepth
                | QueryField::DynamicRange
                | QueryField::Rating
                | QueryField::PlayCount
                | QueryField::Year
        )
    }

    pub fn is_bool(self) -> bool {
        matches!(self, QueryField::HasReplayGain)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryOp {
    Contains,
    Equals,
    NotEquals,
    GreaterOrEq,
    LessOrEq,
    IsTrue,
    IsFalse,
}

impl QueryOp {
    pub const ALL: &'static [QueryOp] = &[
        QueryOp::Contains,
        QueryOp::Equals,
        QueryOp::NotEquals,
        QueryOp::GreaterOrEq,
        QueryOp::LessOrEq,
        QueryOp::IsTrue,
        QueryOp::IsFalse,
    ];

    pub fn label(self) -> &'static str {
        match self {
            QueryOp::Contains => "contains",
            QueryOp::Equals => "equals",
            QueryOp::NotEquals => "not equals",
            QueryOp::GreaterOrEq => ">=",
            QueryOp::LessOrEq => "<=",
            QueryOp::IsTrue => "is yes",
            QueryOp::IsFalse => "is no",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRule {
    pub field: QueryField,
    pub op: QueryOp,
    #[serde(default)]
    pub value: String,
}

impl QueryRule {
    pub fn matches(&self, t: &Track) -> bool {
        let needle = self.value.trim().to_lowercase();
        let match_str = |hay: &str| -> bool {
            let h = hay.to_lowercase();
            match self.op {
                QueryOp::Contains => needle.is_empty() || h.contains(&needle),
                QueryOp::Equals => h == needle,
                QueryOp::NotEquals => h != needle,
                QueryOp::GreaterOrEq => h >= needle,
                QueryOp::LessOrEq => h <= needle,
                QueryOp::IsTrue => !h.is_empty(),
                QueryOp::IsFalse => h.is_empty(),
            }
        };
        let match_num = |val: Option<i64>| -> bool {
            let target = needle.parse::<i64>().ok();
            match (val, target, self.op) {
                (Some(v), Some(n), QueryOp::Equals) => v == n,
                (Some(v), Some(n), QueryOp::NotEquals) => v != n,
                (Some(v), Some(n), QueryOp::GreaterOrEq) => v >= n,
                (Some(v), Some(n), QueryOp::LessOrEq) => v <= n,
                (Some(v), Some(n), QueryOp::Contains) => v.to_string().contains(&n.to_string()),
                (Some(v), _, QueryOp::IsTrue) => v > 0,
                (Some(v), _, QueryOp::IsFalse) => v == 0,
                (None, _, QueryOp::IsFalse) => true,
                (None, Some(0), QueryOp::Equals | QueryOp::LessOrEq) => true,
                _ => false,
            }
        };

        match self.field {
            QueryField::Title => match_str(&t.title),
            QueryField::Artist => match_str(&t.artist),
            QueryField::Album => match_str(&t.album),
            QueryField::Composer => match_str(t.composer.as_deref().unwrap_or("")),
            QueryField::Genre => match_str(t.genre.as_deref().unwrap_or("")),
            QueryField::Label => match_str(t.label.as_deref().unwrap_or("")),
            QueryField::Codec => match_str(&t.codec),
            QueryField::CustomTag => {
                if self.op == QueryOp::IsTrue {
                    !t.custom_tags.is_empty()
                } else if self.op == QueryOp::IsFalse {
                    t.custom_tags.is_empty()
                } else if self.op == QueryOp::NotEquals {
                    t.custom_tags.iter().all(|tag| tag.to_lowercase() != needle)
                } else {
                    t.custom_tags.iter().any(|tag| match_str(tag))
                }
            }
            QueryField::SampleRateHz => match_num(t.sample_rate.map(|v| v as i64)),
            QueryField::BitDepth => match_num(t.bit_depth.map(|v| v as i64)),
            QueryField::DynamicRange => match_num(t.dr_score.map(|v| v as i64)),
            QueryField::Rating => {
                // Users enter star values like 4 or 4.5; internal storage is half-stars (0..=10)
                let Some(rhs) = self.value.trim().parse::<f32>().ok() else {
                    return false;
                };
                let lhs = t.rating as f32 / 2.0;
                match self.op {
                    QueryOp::Equals => (lhs - rhs).abs() < 0.01,
                    QueryOp::NotEquals => (lhs - rhs).abs() >= 0.01,
                    QueryOp::GreaterOrEq => lhs + 0.01 >= rhs,
                    QueryOp::LessOrEq => lhs <= rhs + 0.01,
                    _ => false,
                }
            }
            QueryField::PlayCount => match_num(Some(t.play_count as i64)),
            QueryField::Year => {
                let y = t
                    .year
                    .as_deref()
                    .and_then(|s| s.get(..4).unwrap_or(s).parse::<i64>().ok());
                match_num(y)
            }
            QueryField::HasReplayGain => {
                let has = t.rg_track_gain_db.is_some() || t.rg_album_gain_db.is_some();
                match self.op {
                    QueryOp::IsFalse | QueryOp::NotEquals => !has,
                    _ => has,
                }
            }
            QueryField::Authenticity => match_str(t.authenticity.as_deref().unwrap_or("")),
        }
    }
}

/// User-defined persistent database Smart View (dynamic rule-based query).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedView {
    pub id: u64,
    pub name: String,
    #[serde(default = "default_true_lib")]
    pub match_all: bool,
    #[serde(default)]
    pub rules: Vec<QueryRule>,
}

fn default_true_lib() -> bool {
    true
}

impl SavedView {
    pub fn matches(&self, track: &Track) -> bool {
        if self.rules.is_empty() {
            return true;
        }
        if self.match_all {
            self.rules.iter().all(|r| r.matches(track))
        } else {
            self.rules.iter().any(|r| r.matches(track))
        }
    }
}

/// Group of duplicate tracks detected in the library database, with the highest-quality master flagged.
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub key_label: String,
    pub best_track_id: u64,
    pub track_ids: Vec<u64>,
}

/// Aggregate analytics snapshot computed from the active Library database.
#[derive(Debug, Clone, Default)]
pub struct LibraryStats {
    pub total_tracks: usize,
    pub total_albums: usize,
    pub total_artists: usize,
    pub total_composers: usize,
    pub total_genres: usize,
    pub total_labels: usize,
    pub total_duration_secs: f64,
    pub hires_tracks: usize,
    pub lossless_tracks: usize,
    pub dsd_tracks: usize,
    pub rg_scanned_tracks: usize,
    pub dr_analyzed_tracks: usize,
    pub avg_dr: Option<f32>,
    pub verified_hires_tracks: usize,
    pub collections_count: usize,
    pub saved_views_count: usize,
    pub playlists_count: usize,
    pub codec_counts: Vec<(String, usize)>,
    pub rate_buckets: Vec<(String, usize)>,
    pub decade_counts: Vec<(String, usize)>,
    pub dr_buckets: Vec<(String, usize)>,
    pub top_composers: Vec<(String, usize)>,
    pub top_labels: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    #[serde(default = "default_library_id")]
    pub id: u64,
    #[serde(default = "default_library_name")]
    pub name: String,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub folders: Vec<PathBuf>,
    #[serde(default)]
    pub favorites: HashSet<u64>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
    #[serde(default)]
    pub collections: Vec<Collection>,
    #[serde(default)]
    pub saved_views: Vec<SavedView>,
    #[serde(default)]
    pub play_history: Vec<PlayHistoryEntry>,
    #[serde(default)]
    pub album_notes: HashMap<String, String>,
    /// `2` = half-star scale (`0..=10`, i.e. 0.0–5.0 stars in 0.5 steps); `0` = legacy `0..=5` integer stars (auto-migrated on load).
    #[serde(default)]
    pub rating_scale: u8,
    /// O(1) lookup index (`track.id -> index in tracks`); rebuilt automatically on load/mutate.
    #[serde(skip)]
    id_index: HashMap<u64, usize>,
}

fn default_library_id() -> u64 {
    1
}

fn default_library_name() -> String {
    "Main Library".into()
}

impl Default for Library {
    fn default() -> Self {
        Self {
            id: default_library_id(),
            name: default_library_name(),
            tracks: Vec::new(),
            folders: Vec::new(),
            favorites: HashSet::new(),
            playlists: Vec::new(),
            collections: Vec::new(),
            saved_views: Vec::new(),
            play_history: Vec::new(),
            album_notes: HashMap::new(),
            rating_scale: 2,
            id_index: HashMap::new(),
        }
    }
}

impl Library {
    pub fn migrate_ratings_if_needed(&mut self) {
        if self.rating_scale < 2 {
            for tr in &mut self.tracks {
                if tr.rating <= 5 {
                    tr.rating = tr.rating.saturating_mul(2).min(10);
                }
            }
            self.rating_scale = 2;
        }
    }

    pub fn rebuild_index(&mut self) {
        self.id_index.clear();
        self.id_index.reserve(self.tracks.len());
        for (idx, tr) in self.tracks.iter().enumerate() {
            self.id_index.insert(tr.id, idx);
        }
    }

    pub fn get_by_id(&self, id: u64) -> Option<&Track> {
        if let Some(&idx) = self.id_index.get(&id)
            && let Some(tr) = self.tracks.get(idx)
            && tr.id == id
        {
            return Some(tr);
        }
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn get_by_id_mut(&mut self, id: u64) -> Option<&mut Track> {
        if let Some(&idx) = self.id_index.get(&id)
            && self.tracks.get(idx).is_some_and(|t| t.id == id)
        {
            return self.tracks.get_mut(idx);
        }
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn upsert(&mut self, mut track: Track) {
        let existing_idx = self
            .id_index
            .get(&track.id)
            .copied()
            .filter(|&i| self.tracks.get(i).is_some_and(|t| t.id == track.id))
            .or_else(|| self.tracks.iter().position(|item| item.id == track.id));
        if let Some(idx) = existing_idx {
            let existing = &mut self.tracks[idx];
            // Preserve user listening stats, ratings, custom tags, ReplayGain & audio analysis across rescans!
            track.play_count = existing.play_count.max(track.play_count);
            if track.last_played_at.is_none() {
                track.last_played_at = existing.last_played_at;
            }
            if track.rating == 0 {
                track.rating = existing.rating;
            }
            if existing.added_at > 0 {
                track.added_at = existing.added_at;
            }
            if track.custom_tags.is_empty() && !existing.custom_tags.is_empty() {
                track.custom_tags = existing.custom_tags.clone();
            }
            if track.rg_track_gain_db.is_none() {
                track.rg_track_gain_db = existing.rg_track_gain_db;
                track.rg_track_peak = existing.rg_track_peak;
            }
            if track.rg_album_gain_db.is_none() {
                track.rg_album_gain_db = existing.rg_album_gain_db;
                track.rg_album_peak = existing.rg_album_peak;
            }
            if track.dr_score.is_none() {
                track.dr_score = existing.dr_score;
                track.integrated_lufs = existing.integrated_lufs;
                track.true_peak_dbtp = existing.true_peak_dbtp;
                track.effective_bandwidth_hz = existing.effective_bandwidth_hz;
                track.authenticity = existing.authenticity.clone();
            }
            *existing = track;
            self.id_index.insert(self.tracks[idx].id, idx);
        } else {
            let idx = self.tracks.len();
            self.id_index.insert(track.id, idx);
            self.tracks.push(track);
        }
    }

    pub fn increment_play_count(&mut self, id: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = if let Some(t) = self.get_by_id_mut(id) {
            t.play_count = t.play_count.saturating_add(1);
            t.last_played_at = Some(now);
            Some(PlayHistoryEntry {
                track_id: id,
                played_at: now,
                title: t.title.clone(),
                artist: t.artist.clone(),
                album: t.album.clone(),
            })
        } else {
            None
        };
        if let Some(ev) = entry {
            self.play_history.push(ev);
            if self.play_history.len() > 2000 {
                let excess = self.play_history.len() - 2000;
                self.play_history.drain(..excess);
            }
        }
    }

    pub fn set_rating(&mut self, id: u64, rating: u8) {
        if let Some(t) = self.get_by_id_mut(id) {
            t.rating = rating.min(10);
        }
    }

    pub fn sort_default(&mut self) {
        self.tracks.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then(a.album.to_lowercase().cmp(&b.album.to_lowercase()))
                .then(a.disc_number.cmp(&b.disc_number))
                .then(a.track_number.cmp(&b.track_number))
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        self.rebuild_index();
    }

    /// Reorders `self.tracks` to match `ordered_ids` (any unlisted tracks stay at the end).
    pub fn set_tracks_order_by_ids(&mut self, ordered_ids: &[u64]) {
        if ordered_ids.is_empty() || self.tracks.len() < 2 {
            return;
        }
        let mut by_id: HashMap<u64, Track> = self
            .tracks
            .drain(..)
            .map(|t| (t.id, t))
            .collect();
        let mut reordered = Vec::with_capacity(by_id.len());
        for &id in ordered_ids {
            if let Some(t) = by_id.remove(&id) {
                reordered.push(t);
            }
        }
        if !by_id.is_empty() {
            let mut rest: Vec<Track> = by_id.into_values().collect();
            rest.sort_by_key(|t| t.id);
            reordered.extend(rest);
        }
        self.tracks = reordered;
        self.rebuild_index();
    }

    /// Reorders a single track or multi-selected block of indices inside `self.tracks`.
    /// Returns the new contiguous range `(start, len)` if order actually changed.
    pub fn reorder_tracks_indices(
        &mut self,
        moving_indices: &[usize],
        insert_before: usize,
    ) -> Option<(usize, usize)> {
        let res = reorder_vec_indices(&mut self.tracks, moving_indices, insert_before)?;
        self.rebuild_index();
        Some(res)
    }

    pub fn remove_tracks(&mut self, ids: &[u64]) -> Vec<Track> {
        if ids.is_empty() {
            return Vec::new();
        }
        let id_set: HashSet<u64> = ids.iter().copied().collect();
        let mut removed = Vec::new();
        self.tracks.retain(|track| {
            if id_set.contains(&track.id) {
                removed.push(track.clone());
                false
            } else {
                true
            }
        });
        for id in &id_set {
            self.favorites.remove(id);
        }
        for pl in &mut self.playlists {
            pl.track_ids.retain(|tid| !id_set.contains(tid));
        }
        for col in &mut self.collections {
            col.track_ids.retain(|tid| !id_set.contains(tid));
        }
        if !removed.is_empty() {
            self.rebuild_index();
        }
        removed
    }

    pub fn add_folder(&mut self, folder: PathBuf) {
        if !self.folders.contains(&folder) {
            self.folders.push(folder);
            self.folders.sort();
        }
    }

    pub fn remove_folder(&mut self, folder: &Path) -> Vec<u64> {
        self.folders.retain(|item| item != folder);
        let removed_ids: Vec<u64> = self
            .tracks
            .iter()
            .filter(|track| track.path.starts_with(folder))
            .map(|track| track.id)
            .collect();
        self.remove_tracks(&removed_ids);
        removed_ids
    }

    pub fn folder_track_count(&self, folder: &Path) -> usize {
        self.tracks
            .iter()
            .filter(|track| track.path.starts_with(folder))
            .count()
    }

    pub fn prune_missing(&mut self) -> Vec<u64> {
        let missing: Vec<u64> = self
            .tracks
            .iter()
            .filter(|track| !track.path.exists())
            .map(|track| track.id)
            .collect();
        self.remove_tracks(&missing);
        missing
    }

    pub fn toggle_favorite(&mut self, id: u64) {
        if !self.favorites.remove(&id) {
            self.favorites.insert(id);
        }
    }

    pub fn set_favorites(&mut self, ids: &[u64], favorite: bool) {
        for &id in ids {
            if favorite {
                self.favorites.insert(id);
            } else {
                self.favorites.remove(&id);
            }
        }
    }

    pub fn create_playlist(&mut self, name: String) -> u64 {
        let trimmed = name.trim();
        let final_name = if trimmed.is_empty() {
            format!("Playlist {}", self.playlists.len() + 1)
        } else {
            trimmed.to_string()
        };
        let id = unique_id(&final_name, self.playlists.len() as u64);
        self.playlists.push(Playlist {
            id,
            name: final_name,
            track_ids: Vec::new(),
        });
        id
    }

    pub fn playlist(&self, id: u64) -> Option<&Playlist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    pub fn delete_playlist(&mut self, id: u64) -> Option<String> {
        let pos = self.playlists.iter().position(|p| p.id == id)?;
        Some(self.playlists.remove(pos).name)
    }

    pub fn add_tracks_to_playlist(&mut self, playlist_id: u64, track_ids: &[u64]) -> usize {
        let Some(pl) = self.playlists.iter_mut().find(|p| p.id == playlist_id) else {
            return 0;
        };
        let mut added = 0;
        for &tid in track_ids {
            if !pl.track_ids.contains(&tid) {
                pl.track_ids.push(tid);
                added += 1;
            }
        }
        added
    }

    pub fn remove_tracks_from_playlist(&mut self, playlist_id: u64, track_ids: &[u64]) -> usize {
        let Some(pl) = self.playlists.iter_mut().find(|p| p.id == playlist_id) else {
            return 0;
        };
        let id_set: HashSet<u64> = track_ids.iter().copied().collect();
        let before = pl.track_ids.len();
        pl.track_ids.retain(|tid| !id_set.contains(tid));
        before - pl.track_ids.len()
    }

    pub fn move_playlist_track(&mut self, playlist_id: u64, from: usize, to: usize) {
        let Some(pl) = self.playlists.iter_mut().find(|p| p.id == playlist_id) else {
            return;
        };
        if from < pl.track_ids.len() && to < pl.track_ids.len() && from != to {
            pl.track_ids.swap(from, to);
        }
    }

    pub fn rename_playlist(&mut self, playlist_id: u64, new_name: String) -> Option<String> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return None;
        }
        let pl = self.playlists.iter_mut().find(|p| p.id == playlist_id)?;
        pl.name = trimmed.to_string();
        Some(pl.name.clone())
    }

    pub fn duplicate_playlist(&mut self, playlist_id: u64) -> Option<(u64, String)> {
        let orig = self.playlists.iter().find(|p| p.id == playlist_id)?.clone();
        let copy_name = format!("{} (Copy)", orig.name);
        let id = unique_id(&copy_name, self.playlists.len() as u64 + 77);
        self.playlists.push(Playlist {
            id,
            name: copy_name.clone(),
            track_ids: orig.track_ids,
        });
        Some((id, copy_name))
    }

    pub fn clear_playlist(&mut self, playlist_id: u64) -> usize {
        let Some(pl) = self.playlists.iter_mut().find(|p| p.id == playlist_id) else {
            return 0;
        };
        let n = pl.track_ids.len();
        pl.track_ids.clear();
        n
    }

    pub fn set_playlist_tracks(&mut self, playlist_id: u64, track_ids: Vec<u64>) {
        if let Some(pl) = self.playlists.iter_mut().find(|p| p.id == playlist_id) {
            pl.track_ids = track_ids;
        }
    }

    /// Reorders a single track or multi-selected block of indices inside a custom playlist.
    /// Returns the new contiguous range `(start, len)` if order actually changed.
    pub fn reorder_playlist_indices(
        &mut self,
        playlist_id: u64,
        moving_indices: &[usize],
        insert_before: usize,
    ) -> Option<(usize, usize)> {
        let pl = self.playlists.iter_mut().find(|p| p.id == playlist_id)?;
        reorder_vec_indices(&mut pl.track_ids, moving_indices, insert_before)
    }

    /// Removes duplicate entries (by track ID or identical normalized title+artist) from a playlist.
    pub fn deduplicate_playlist(&mut self, playlist_id: u64) -> usize {
        let Some(pl_idx) = self.playlists.iter().position(|p| p.id == playlist_id) else {
            return 0;
        };
        let old_ids = self.playlists[pl_idx].track_ids.clone();
        let mut seen_ids: HashSet<u64> = HashSet::with_capacity(old_ids.len());
        let mut seen_meta: HashSet<(String, String)> = HashSet::with_capacity(old_ids.len());
        let mut kept = Vec::with_capacity(old_ids.len());
        for tid in old_ids {
            if !seen_ids.insert(tid) {
                continue;
            }
            if let Some(tr) = self.get_by_id(tid) {
                let key = (tr.title.trim().to_lowercase(), tr.artist.trim().to_lowercase());
                if !seen_meta.insert(key) {
                    continue;
                }
            }
            kept.push(tid);
        }
        let removed = self.playlists[pl_idx].track_ids.len() - kept.len();
        self.playlists[pl_idx].track_ids = kept;
        removed
    }

    /// Removes entries pointing to deleted/missing files from a playlist.
    pub fn prune_missing_in_playlist(&mut self, playlist_id: u64) -> usize {
        let Some(pl_idx) = self.playlists.iter().position(|p| p.id == playlist_id) else {
            return 0;
        };
        let old_ids = self.playlists[pl_idx].track_ids.clone();
        let mut kept = Vec::with_capacity(old_ids.len());
        for tid in old_ids {
            if self.get_by_id(tid).is_some_and(|t| t.path.exists()) {
                kept.push(tid);
            }
        }
        let removed = self.playlists[pl_idx].track_ids.len() - kept.len();
        self.playlists[pl_idx].track_ids = kept;
        removed
    }

    pub fn reverse_playlist(&mut self, playlist_id: u64) -> bool {
        let Some(pl) = self.playlists.iter_mut().find(|p| p.id == playlist_id) else {
            return false;
        };
        if pl.track_ids.len() < 2 {
            return false;
        }
        pl.track_ids.reverse();
        true
    }

    pub fn randomize_playlist(&mut self, playlist_id: u64) -> bool {
        use rand::seq::SliceRandom;
        let Some(pl) = self.playlists.iter_mut().find(|p| p.id == playlist_id) else {
            return false;
        };
        if pl.track_ids.len() < 2 {
            return false;
        }
        let mut rng = rand::thread_rng();
        pl.track_ids.shuffle(&mut rng);
        true
    }

    // --- Curated Collections ---
    pub fn create_collection(
        &mut self,
        name: String,
        description: String,
        accent_index: u8,
    ) -> u64 {
        let trimmed = name.trim();
        let final_name = if trimmed.is_empty() {
            format!("Collection {}", self.collections.len() + 1)
        } else {
            trimmed.to_string()
        };
        let id = unique_id(&final_name, self.collections.len() as u64 + 500);
        self.collections.push(Collection {
            id,
            name: final_name,
            description: description.trim().to_string(),
            accent_index: accent_index % 6,
            track_ids: Vec::new(),
        });
        id
    }

    pub fn collection(&self, id: u64) -> Option<&Collection> {
        self.collections.iter().find(|c| c.id == id)
    }

    pub fn delete_collection(&mut self, id: u64) -> Option<String> {
        let pos = self.collections.iter().position(|c| c.id == id)?;
        Some(self.collections.remove(pos).name)
    }

    pub fn add_tracks_to_collection(&mut self, collection_id: u64, track_ids: &[u64]) -> usize {
        let Some(col) = self.collections.iter_mut().find(|c| c.id == collection_id) else {
            return 0;
        };
        let mut added = 0;
        for &tid in track_ids {
            if !col.track_ids.contains(&tid) {
                col.track_ids.push(tid);
                added += 1;
            }
        }
        added
    }

    pub fn remove_tracks_from_collection(&mut self, collection_id: u64, track_ids: &[u64]) -> usize {
        let Some(col) = self.collections.iter_mut().find(|c| c.id == collection_id) else {
            return 0;
        };
        let id_set: HashSet<u64> = track_ids.iter().copied().collect();
        let before = col.track_ids.len();
        col.track_ids.retain(|tid| !id_set.contains(tid));
        before - col.track_ids.len()
    }

    // --- Custom Saved Views (Database Queries) ---
    pub fn upsert_saved_view(&mut self, view: SavedView) -> u64 {
        if let Some(existing) = self.saved_views.iter_mut().find(|v| v.id == view.id) {
            let id = existing.id;
            *existing = view;
            return id;
        }
        let id = if view.id == 0 {
            unique_id(&view.name, self.saved_views.len() as u64 + 900)
        } else {
            view.id
        };
        let mut v = view;
        v.id = id;
        self.saved_views.push(v);
        id
    }

    pub fn saved_view(&self, id: u64) -> Option<&SavedView> {
        self.saved_views.iter().find(|v| v.id == id)
    }

    pub fn delete_saved_view(&mut self, id: u64) -> Option<String> {
        let pos = self.saved_views.iter().position(|v| v.id == id)?;
        Some(self.saved_views.remove(pos).name)
    }

    pub fn add_custom_tag(&mut self, ids: &[u64], raw_tag: &str) -> usize {
        let clean = raw_tag.trim().trim_start_matches('#').to_lowercase();
        if clean.is_empty() {
            return 0;
        }
        let mut updated = 0;
        for &id in ids {
            if let Some(t) = self.get_by_id_mut(id)
                && !t.custom_tags.iter().any(|x| x.eq_ignore_ascii_case(&clean))
            {
                t.custom_tags.push(clean.clone());
                updated += 1;
            }
        }
        updated
    }

    pub fn remove_custom_tag(&mut self, id: u64, tag: &str) -> bool {
        let Some(t) = self.get_by_id_mut(id) else {
            return false;
        };
        let before = t.custom_tags.len();
        t.custom_tags.retain(|x| !x.eq_ignore_ascii_case(tag));
        t.custom_tags.len() != before
    }

    pub fn album_note_key(album: &str, artist: &str) -> String {
        format!("{}||{}", album.trim().to_lowercase(), artist.trim().to_lowercase())
    }

    pub fn album_note(&self, album: &str, artist: &str) -> Option<&str> {
        let k = Self::album_note_key(album, artist);
        self.album_notes.get(&k).map(|s| s.as_str())
    }

    pub fn set_album_note(&mut self, album: &str, artist: &str, note: String) {
        let k = Self::album_note_key(album, artist);
        let trimmed = note.trim();
        if trimmed.is_empty() {
            self.album_notes.remove(&k);
        } else {
            self.album_notes.insert(k, trimmed.to_string());
        }
    }

    /// 100% local Roon-Radio-style similarity queue generator seeded from `seed_id`.
    /// Ranks library tracks by shared Composer, Genre, Artist, Label, `#Custom Tags`, Era, Quality Tier & Rating.
    pub fn build_similar_radio(&self, seed_id: u64, limit: usize) -> Vec<u64> {
        let Some(seed) = self.get_by_id(seed_id) else {
            return Vec::new();
        };
        let seed_comp = seed.composer.as_deref().map(|s| s.trim().to_lowercase());
        let seed_genre = seed.genre.as_deref().map(|s| s.trim().to_lowercase());
        let seed_artist = seed.artist.trim().to_lowercase();
        let seed_album = seed.album.trim().to_lowercase();
        let seed_label = seed.label.as_deref().map(|s| s.trim().to_lowercase());
        let seed_decade: Option<u32> = seed
            .year
            .as_deref()
            .and_then(|y| y.trim().get(..4).and_then(|s| s.parse::<u32>().ok()))
            .map(|y| y / 10);
        let seed_tags: HashSet<String> = seed
            .custom_tags
            .iter()
            .map(|t| t.to_lowercase())
            .collect();
        let seed_hires = seed.is_hires();

        let mut scored: Vec<(i32, u64)> = Vec::new();
        for tr in &self.tracks {
            if tr.id == seed_id {
                continue;
            }
            let mut score = 0i32;
            if let (Some(sc), Some(tc)) = (
                seed_comp.as_deref().filter(|s| !s.is_empty()),
                tr.composer.as_deref().map(|s| s.trim().to_lowercase()),
            ) && sc == tc
            {
                score += 50;
            }
            if let (Some(sg), Some(tg)) = (
                seed_genre.as_deref().filter(|s| !s.is_empty()),
                tr.genre.as_deref().map(|s| s.trim().to_lowercase()),
            ) && sg == tg
            {
                score += 25;
            }
            if !seed_artist.is_empty() && tr.artist.trim().to_lowercase() == seed_artist {
                score += 22;
            }
            if let (Some(sl), Some(tl)) = (
                seed_label.as_deref().filter(|s| !s.is_empty()),
                tr.label.as_deref().map(|s| s.trim().to_lowercase()),
            ) && sl == tl
            {
                score += 14;
            }
            if !seed_tags.is_empty() {
                for tag in &tr.custom_tags {
                    if seed_tags.contains(&tag.to_lowercase()) {
                        score += 18;
                    }
                }
            }
            if let Some(sd) = seed_decade
                && let Some(td) = tr
                    .year
                    .as_deref()
                    .and_then(|y| y.trim().get(..4).and_then(|s| s.parse::<u32>().ok()))
                    .map(|y| y / 10)
                && sd == td
            {
                score += 10;
            }
            if tr.is_hires() == seed_hires {
                score += 6;
            }
            score += (tr.rating as i32) * 3;
            // Gentle diversity penalty for same album so radio flows naturally across recordings
            if !seed_album.is_empty() && tr.album.trim().to_lowercase() == seed_album {
                score -= 10;
            }
            if score > 0 {
                scored.push((score, tr.id));
            }
        }
        // Deterministic-tie-broken descending sort with slight rotation so top matches lead
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut out = Vec::with_capacity(limit.min(scored.len()) + 1);
        out.push(seed_id);
        for (_, id) in scored.into_iter().take(limit.saturating_sub(1)) {
            out.push(id);
        }
        out
    }

    /// Detects duplicate tracks by normalized `(title, artist, album)` and ranks copies by audio fidelity
    /// (`DSD / Hi-Res > Lossless > Lossy`, sample rate, bit depth, DR score) so users can keep the master.
    pub fn find_duplicate_groups(&self) -> Vec<DuplicateGroup> {
        let mut groups: HashMap<(String, String, String), Vec<u64>> = HashMap::new();
        for t in &self.tracks {
            let key = (
                t.title.trim().to_lowercase(),
                t.artist.trim().to_lowercase(),
                t.album.trim().to_lowercase(),
            );
            if key.0.is_empty() {
                continue;
            }
            groups.entry(key).or_default().push(t.id);
        }
        let quality_rank = |t: &Track| -> (u8, u32, u32, u8) {
            let tier = if t.codec.starts_with("DSD") {
                3
            } else if t.is_hires() {
                2
            } else if t.is_lossless() {
                1
            } else {
                0
            };
            (
                tier,
                t.sample_rate.unwrap_or(0),
                t.bit_depth.unwrap_or(0),
                t.dr_score.unwrap_or(0),
            )
        };
        let mut out = Vec::new();
        for ids in groups.into_values() {
            if ids.len() < 2 {
                continue;
            }
            let mut sorted = ids;
            sorted.sort_by(|&a, &b| {
                let ra = self.get_by_id(a).map(quality_rank).unwrap_or_default();
                let rb = self.get_by_id(b).map(quality_rank).unwrap_or_default();
                rb.cmp(&ra).then_with(|| a.cmp(&b))
            });
            let best_track_id = sorted[0];
            let key_label = self
                .get_by_id(best_track_id)
                .map(|t| format!("{} — {} ({})", t.title, t.artist, t.album))
                .unwrap_or_else(|| "Duplicate Track".into());
            out.push(DuplicateGroup {
                key_label,
                best_track_id,
                track_ids: sorted,
            });
        }
        out.sort_by_key(|g| g.key_label.to_lowercase());
        out
    }

    /// Returns track IDs whose underlying file path no longer exists on disk.
    pub fn find_missing_track_ids(&self) -> Vec<u64> {
        self.tracks
            .iter()
            .filter(|t| !t.path.exists())
            .map(|t| t.id)
            .collect()
    }

    /// Computes full database analytics (codecs, sample-rate distribution, composers, DR & RG coverage).
    pub fn compute_stats(&self) -> LibraryStats {
        let mut albums = HashSet::new();
        let mut artists = HashSet::new();
        let mut composers = HashSet::new();
        let mut genres = HashSet::new();
        let mut labels = HashSet::new();
        let mut codecs: HashMap<String, usize> = HashMap::new();
        let mut rates: HashMap<&'static str, usize> = HashMap::new();
        let mut decades: HashMap<u32, usize> = HashMap::new();
        let mut composer_tally: HashMap<String, (String, usize)> = HashMap::new();
        let mut label_tally: HashMap<String, (String, usize)> = HashMap::new();
        let mut dr_ref = 0usize;
        let mut dr_dyn = 0usize;
        let mut dr_mod = 0usize;
        let mut dr_loud = 0usize;

        let mut total_duration_secs = 0.0_f64;
        let mut hires_tracks = 0usize;
        let mut lossless_tracks = 0usize;
        let mut dsd_tracks = 0usize;
        let mut rg_scanned_tracks = 0usize;
        let mut dr_analyzed_tracks = 0usize;
        let mut dr_sum = 0.0_f32;
        let mut verified_hires_tracks = 0usize;

        for t in &self.tracks {
            if !t.album.trim().is_empty() {
                albums.insert((t.album.to_lowercase(), t.artist.to_lowercase()));
            }
            if !t.artist.trim().is_empty() {
                artists.insert(t.artist.to_lowercase());
            }
            if let Some(ref c) = t.composer
                && !c.trim().is_empty()
            {
                let trimmed = c.trim();
                let key = trimmed.to_lowercase();
                composers.insert(key.clone());
                let entry = composer_tally
                    .entry(key)
                    .or_insert_with(|| (trimmed.to_string(), 0));
                entry.1 += 1;
            }
            if let Some(ref g) = t.genre
                && !g.trim().is_empty()
            {
                genres.insert(g.trim().to_lowercase());
            }
            if let Some(ref l) = t.label
                && !l.trim().is_empty()
            {
                let trimmed = l.trim();
                let key = trimmed.to_lowercase();
                labels.insert(key.clone());
                let entry = label_tally
                    .entry(key)
                    .or_insert_with(|| (trimmed.to_string(), 0));
                entry.1 += 1;
            }
            if let Some(ref y_str) = t.year
                && let Some(y) = y_str
                    .trim()
                    .get(0..4)
                    .and_then(|s| s.parse::<u32>().ok())
                && (1900..=2100).contains(&y)
            {
                *decades.entry((y / 10) * 10).or_insert(0) += 1;
            }
            if let Some(d) = t.duration {
                total_duration_secs += d;
            }
            if t.is_hires() {
                hires_tracks += 1;
            }
            if t.is_lossless() {
                lossless_tracks += 1;
            }
            if t.codec.starts_with("DSD") {
                dsd_tracks += 1;
            }
            if t.rg_track_gain_db.is_some() || t.rg_album_gain_db.is_some() {
                rg_scanned_tracks += 1;
            }
            if let Some(dr) = t.dr_score {
                dr_analyzed_tracks += 1;
                dr_sum += dr as f32;
                if dr >= 14 {
                    dr_ref += 1;
                } else if dr >= 11 {
                    dr_dyn += 1;
                } else if dr >= 8 {
                    dr_mod += 1;
                } else {
                    dr_loud += 1;
                }
            }
            if t.authenticity.as_deref() == Some("AuthenticHiRes") {
                verified_hires_tracks += 1;
            }
            *codecs.entry(t.codec.clone()).or_insert(0) += 1;
            let bucket = if t.codec.starts_with("DSD") {
                "DSD (1-bit)"
            } else {
                match t.sample_rate.unwrap_or(0) {
                    r if r >= 176_400 => "≥ 176.4 kHz",
                    r if r >= 88_200 => "88.2 – 96 kHz",
                    r if r >= 44_100 => "44.1 – 48 kHz",
                    _ => "Other",
                }
            };
            *rates.entry(bucket).or_insert(0) += 1;
        }

        let mut codec_counts: Vec<(String, usize)> = codecs.into_iter().collect();
        codec_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let bucket_order = ["DSD (1-bit)", "≥ 176.4 kHz", "88.2 – 96 kHz", "44.1 – 48 kHz", "Other"];
        let mut rate_buckets = Vec::new();
        for b in bucket_order {
            if let Some(&cnt) = rates.get(b) {
                rate_buckets.push((b.to_string(), cnt));
            }
        }

        let mut decade_pairs: Vec<(u32, usize)> = decades.into_iter().collect();
        decade_pairs.sort_by_key(|(dec, _)| *dec);
        let decade_counts: Vec<(String, usize)> = decade_pairs
            .into_iter()
            .map(|(dec, cnt)| (format!("{dec}s"), cnt))
            .collect();

        let mut dr_buckets = Vec::new();
        if dr_analyzed_tracks > 0 {
            for (lbl, cnt) in [
                ("DR 14+ Ref", dr_ref),
                ("DR 11–13 Dyn", dr_dyn),
                ("DR 8–10 Mod", dr_mod),
                ("DR ≤7 Loud", dr_loud),
            ] {
                if cnt > 0 {
                    dr_buckets.push((lbl.to_string(), cnt));
                }
            }
        }

        let mut top_composers: Vec<(String, usize)> = composer_tally.into_values().collect();
        top_composers.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        top_composers.truncate(6);

        let mut top_labels: Vec<(String, usize)> = label_tally.into_values().collect();
        top_labels.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        top_labels.truncate(6);

        LibraryStats {
            total_tracks: self.tracks.len(),
            total_albums: albums.len(),
            total_artists: artists.len(),
            total_composers: composers.len(),
            total_genres: genres.len(),
            total_labels: labels.len(),
            total_duration_secs,
            hires_tracks,
            lossless_tracks,
            dsd_tracks,
            rg_scanned_tracks,
            dr_analyzed_tracks,
            avg_dr: if dr_analyzed_tracks > 0 {
                Some(dr_sum / dr_analyzed_tracks as f32)
            } else {
                None
            },
            verified_hires_tracks,
            collections_count: self.collections.len(),
            saved_views_count: self.saved_views.len(),
            playlists_count: self.playlists.len(),
            codec_counts,
            rate_buckets,
            decade_counts,
            dr_buckets,
            top_composers,
            top_labels,
        }
    }

    /// Exports the active library catalog to RFC-4180 CSV for spreadsheets & archival.
    pub fn export_csv(&self, path: &Path) -> std::io::Result<usize> {
        let esc = |s: &str| -> String {
            if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_string()
            }
        };
        let mut out = String::from(
            "Title,Artist,Album,Composer,Conductor,Orchestra,Genre,Year,Label,Disc,Track,Duration_s,Codec,SampleRate_Hz,BitDepth,DR,LUFS,TruePeak_dBTP,Authenticity,RG_Track_dB,RG_Album_dB,Rating,PlayCount,Tags,Path\n",
        );
        for t in &self.tracks {
            let opt_s = |o: &Option<String>| esc(o.as_deref().unwrap_or(""));
            let opt_u = |o: Option<u32>| o.map(|v| v.to_string()).unwrap_or_default();
            let opt_f = |o: Option<f32>| o.map(|v| format!("{v:.2}")).unwrap_or_default();
            let tags_joined = esc(&t.custom_tags.join(";"));
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                esc(&t.title),
                esc(&t.artist),
                esc(&t.album),
                opt_s(&t.composer),
                opt_s(&t.conductor),
                opt_s(&t.orchestra),
                opt_s(&t.genre),
                opt_s(&t.year),
                opt_s(&t.label),
                opt_u(t.disc_number),
                opt_u(t.track_number),
                t.duration.map(|d| format!("{d:.1}")).unwrap_or_default(),
                esc(&t.codec),
                opt_u(t.sample_rate),
                opt_u(t.bit_depth),
                t.dr_score.map(|v| v.to_string()).unwrap_or_default(),
                opt_f(t.integrated_lufs),
                opt_f(t.true_peak_dbtp),
                opt_s(&t.authenticity),
                opt_f(t.rg_track_gain_db),
                opt_f(t.rg_album_gain_db),
                if t.rating == 0 {
                    String::new()
                } else {
                    format!("{:.1}", t.rating as f32 / 2.0)
                },
                t.play_count,
                tags_joined,
                esc(&t.path.to_string_lossy()),
            ));
        }
        std::fs::write(path, out)?;
        Ok(self.tracks.len())
    }

    /// Exports the active library, collection, or smart view to a clean Markdown listening dossier.
    pub fn export_catalog_markdown(
        &self,
        title: &str,
        track_ids: Option<&[u64]>,
        path: &Path,
    ) -> std::io::Result<usize> {
        let selected: Vec<&Track> = match track_ids {
            Some(ids) => ids.iter().filter_map(|id| self.get_by_id(*id)).collect(),
            None => self.tracks.iter().collect(),
        };
        let total_secs: f64 = selected.iter().filter_map(|t| t.duration).sum();
        let hires_n = selected.iter().filter(|t| t.is_hires()).count();
        let dsd_n = selected
            .iter()
            .filter(|t| t.codec.starts_with("DSD"))
            .count();
        let dr_vals: Vec<f32> = selected
            .iter()
            .filter_map(|t| t.dr_score.map(|d| d as f32))
            .collect();
        let avg_dr = if dr_vals.is_empty() {
            "—".to_string()
        } else {
            format!("DR{:.1}", dr_vals.iter().sum::<f32>() / dr_vals.len() as f32)
        };

        // Group tracks by (Album, Artist) preserving display order
        let mut album_order: Vec<(String, String)> = Vec::new();
        let mut album_map: HashMap<(String, String), Vec<&Track>> = HashMap::new();
        for t in &selected {
            let key = (t.album.to_lowercase(), t.artist.to_lowercase());
            if !album_map.contains_key(&key) {
                album_order.push(key.clone());
            }
            album_map.entry(key).or_default().push(*t);
        }

        let mut md = String::with_capacity(selected.len() * 160 + 512);
        md.push_str(&format!("# {title} — Audiophile Listening Catalog\n\n"));
        md.push_str(&format!(
            "> Exported by **TxPlayer v{}** · **{}** tracks · **{}** albums · **{:.1} hrs** · **{}** Hi-Res / **{}** DSD · Avg **{}**\n\n",
            env!("CARGO_PKG_VERSION"),
            selected.len(),
            album_order.len(),
            total_secs / 3600.0,
            hires_n,
            dsd_n,
            avg_dr
        ));

        let esc_md = |s: &str| s.replace('|', "\\|").replace(['\r', '\n'], " ");

        for key in &album_order {
            let Some(tracks) = album_map.get(key) else {
                continue;
            };
            let first = tracks[0];
            let yr = first
                .year
                .as_deref()
                .map(|y| format!(" ({y})"))
                .unwrap_or_default();
            md.push_str(&format!(
                "## {} — {}{}\n\n",
                first.album, first.artist, yr
            ));
            let mut meta_parts = Vec::new();
            if let Some(ref lbl) = first.label
                && !lbl.trim().is_empty()
            {
                meta_parts.push(format!("**Label:** {}", lbl.trim()));
            }
            if let Some(ref cat) = first.catalog_number
                && !cat.trim().is_empty()
            {
                meta_parts.push(format!("**Cat #:** {}", cat.trim()));
            }
            let alb_drs: Vec<f32> = tracks
                .iter()
                .filter_map(|t| t.dr_score.map(|d| d as f32))
                .collect();
            if !alb_drs.is_empty() {
                meta_parts.push(format!(
                    "**Album DR:** DR{:.0}",
                    alb_drs.iter().sum::<f32>() / alb_drs.len() as f32
                ));
            }
            if !meta_parts.is_empty() {
                md.push_str(&meta_parts.join(" · "));
                md.push_str("\n\n");
            }
            if let Some(note) = self.album_note(&first.album, &first.artist)
                && !note.trim().is_empty()
            {
                md.push_str(&format!("> **Collector Liner Notes:** {}\n\n", note.trim()));
            }
            md.push_str("| # | Title | Composer / Work | Format | DR | LUFS | TrueHz | Tags |\n");
            md.push_str("|---:|---|---|---|---:|---:|---|---|\n");
            for (idx, t) in tracks.iter().enumerate() {
                let num = t.track_number.unwrap_or((idx + 1) as u32);
                let comp_work = match (t.composer.as_deref(), t.work.as_deref()) {
                    (Some(c), Some(w)) if !c.is_empty() && !w.is_empty() => format!("{c} — {w}"),
                    (Some(c), _) if !c.is_empty() => c.to_string(),
                    (_, Some(w)) if !w.is_empty() => w.to_string(),
                    _ => "—".into(),
                };
                let dr = t
                    .dr_score
                    .map(|d| format!("DR{d}"))
                    .unwrap_or_else(|| "—".into());
                let lufs = t
                    .integrated_lufs
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "—".into());
                let auth = match t.authenticity.as_deref() {
                    Some("AuthenticHiRes") => "Verified Hi-Res",
                    Some("StandardCd") => "CD Quality",
                    Some("LikelyUpsampled") => "Upsampled",
                    _ => "—",
                };
                let tags = if t.custom_tags.is_empty() {
                    "—".into()
                } else {
                    t.custom_tags
                        .iter()
                        .map(|x| format!("#{x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                    num,
                    esc_md(&t.title),
                    esc_md(&comp_work),
                    esc_md(&t.quality_full()),
                    dr,
                    lufs,
                    auth,
                    esc_md(&tags),
                ));
            }
            md.push('\n');
        }
        std::fs::write(path, md)?;
        Ok(selected.len())
    }

    /// Exports the active library, collection, or smart view to a self-contained print-ready HTML dossier.
    pub fn export_catalog_html(
        &self,
        title: &str,
        track_ids: Option<&[u64]>,
        path: &Path,
    ) -> std::io::Result<usize> {
        let selected: Vec<&Track> = match track_ids {
            Some(ids) => ids.iter().filter_map(|id| self.get_by_id(*id)).collect(),
            None => self.tracks.iter().collect(),
        };
        let total_secs: f64 = selected.iter().filter_map(|t| t.duration).sum();
        let hires_n = selected.iter().filter(|t| t.is_hires()).count();
        let dsd_n = selected
            .iter()
            .filter(|t| t.codec.starts_with("DSD"))
            .count();
        let dr_vals: Vec<f32> = selected
            .iter()
            .filter_map(|t| t.dr_score.map(|d| d as f32))
            .collect();
        let avg_dr = if dr_vals.is_empty() {
            "—".to_string()
        } else {
            format!("DR{:.1}", dr_vals.iter().sum::<f32>() / dr_vals.len() as f32)
        };

        let mut album_order: Vec<(String, String)> = Vec::new();
        let mut album_map: HashMap<(String, String), Vec<&Track>> = HashMap::new();
        for t in &selected {
            let key = (t.album.to_lowercase(), t.artist.to_lowercase());
            if !album_map.contains_key(&key) {
                album_order.push(key.clone());
            }
            album_map.entry(key).or_default().push(*t);
        }

        let esc = |s: &str| -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        };

        let mut html = String::with_capacity(selected.len() * 320 + 2048);
        html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        html.push_str(&format!("<title>{} — TxPlayer Listening Catalog</title>\n", esc(title)));
        html.push_str(
            "<style>\
            :root{--bg:#0f1215;--card:#171c21;--border:#262e36;--fg:#eef2f6;--muted:#8f9ca8;--accent:#4cb8a8;--gold:#e6b85c;--danger:#e06c75;}\
            @media print{:root{--bg:#fff;--card:#fff;--border:#ddd;--fg:#111;--muted:#555;--accent:#1e6f64;--gold:#9c6f12;--danger:#b42318;}}\
            *{box-sizing:border-box}body{margin:0;padding:32px;background:var(--bg);color:var(--fg);font:14px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;max-width:1120px;margin-inline:auto;}\
            h1{margin:0 0 6px;font-size:26px;letter-spacing:-.02em}.sub{color:var(--muted);margin-bottom:20px;font-size:13px}\
            .kpis{display:flex;flex-wrap:wrap;gap:10px;margin-bottom:24px}.kpi{background:var(--card);border:1px solid var(--border);border-radius:8px;padding:10px 14px;min-width:128px}\
            .kpi b{display:block;font-size:18px;font-family:ui-monospace,SFMono-Regular,monospace;color:var(--accent)}.kpi span{font-size:11px;text-transform:uppercase;color:var(--muted);letter-spacing:.05em}\
            .album{background:var(--card);border:1px solid var(--border);border-radius:10px;padding:16px 18px;margin-bottom:18px;break-inside:avoid}\
            .album h2{margin:0 0 4px;font-size:17px}.meta{color:var(--muted);font-size:12px;margin-bottom:8px}\
            .notes{border-left:3px solid var(--gold);padding:6px 12px;margin:8px 0 12px;background:rgba(230,184,92,.08);font-size:12.5px;white-space:pre-wrap}\
            table{width:100%;border-collapse:collapse;font-size:12.5px}th,td{padding:6px 8px;border-bottom:1px solid var(--border);text-align:left}\
            th{color:var(--muted);font-size:11px;text-transform:uppercase;letter-spacing:.04em}\
            .pill{display:inline-block;padding:1px 6px;border-radius:4px;font:11px ui-monospace,SFMono-Regular,monospace;border:1px solid var(--border)}\
            .gold{color:var(--gold);border-color:rgba(230,184,92,.45)}.acc{color:var(--accent);border-color:rgba(76,184,168,.45)}.bad{color:var(--danger);border-color:rgba(224,108,117,.45)}\
            .tag{display:inline-block;margin-right:4px;font-size:11px;color:var(--accent)}\
            </style>\n</head>\n<body>\n",
        );
        html.push_str(&format!("<h1>{}</h1>\n", esc(title)));
        html.push_str(&format!(
            "<div class=\"sub\">Audiophile Listening Catalog · Generated by TxPlayer v{}</div>\n",
            env!("CARGO_PKG_VERSION")
        ));
        html.push_str("<div class=\"kpis\">");
        html.push_str(&format!("<div class=\"kpi\"><span>Tracks</span><b>{}</b></div>", selected.len()));
        html.push_str(&format!("<div class=\"kpi\"><span>Albums</span><b>{}</b></div>", album_order.len()));
        html.push_str(&format!("<div class=\"kpi\"><span>Runtime</span><b>{:.1} hrs</b></div>", total_secs / 3600.0));
        html.push_str(&format!("<div class=\"kpi\"><span>Hi-Res / DSD</span><b style=\"color:var(--gold)\">{} / {}</b></div>", hires_n, dsd_n));
        html.push_str(&format!("<div class=\"kpi\"><span>Average DR</span><b style=\"color:var(--gold)\">{}</b></div>", esc(&avg_dr)));
        html.push_str("</div>\n");

        for key in &album_order {
            let Some(tracks) = album_map.get(key) else {
                continue;
            };
            let first = tracks[0];
            let yr = first
                .year
                .as_deref()
                .map(|y| format!(" ({})", esc(y)))
                .unwrap_or_default();
            html.push_str("<section class=\"album\">\n");
            html.push_str(&format!(
                "<h2>{} — {}{}</h2>\n",
                esc(&first.album),
                esc(&first.artist),
                yr
            ));
            let mut meta_parts = Vec::new();
            if let Some(ref lbl) = first.label
                && !lbl.trim().is_empty()
            {
                meta_parts.push(format!("Label: {}", esc(lbl.trim())));
            }
            if let Some(ref cat) = first.catalog_number
                && !cat.trim().is_empty()
            {
                meta_parts.push(format!("Cat #: {}", esc(cat.trim())));
            }
            if !meta_parts.is_empty() {
                html.push_str(&format!("<div class=\"meta\">{}</div>\n", meta_parts.join(" · ")));
            }
            if let Some(note) = self.album_note(&first.album, &first.artist)
                && !note.trim().is_empty()
            {
                html.push_str(&format!(
                    "<div class=\"notes\"><strong>Collector Liner Notes:</strong> {}</div>\n",
                    esc(note.trim())
                ));
            }
            html.push_str("<table><thead><tr><th>#</th><th>Title</th><th>Composer / Work</th><th>Format</th><th>DR</th><th>LUFS</th><th>Tags</th></tr></thead><tbody>\n");
            for (idx, t) in tracks.iter().enumerate() {
                let num = t.track_number.unwrap_or((idx + 1) as u32);
                let comp_work = match (t.composer.as_deref(), t.work.as_deref()) {
                    (Some(c), Some(w)) if !c.is_empty() && !w.is_empty() => format!("{c} — {w}"),
                    (Some(c), _) if !c.is_empty() => c.to_string(),
                    (_, Some(w)) if !w.is_empty() => w.to_string(),
                    _ => "—".into(),
                };
                let fmt_cls = if t.is_hires() { "pill gold" } else if t.is_lossless() { "pill acc" } else { "pill" };
                let dr_html = match t.dr_score {
                    Some(d) if d >= 11 => format!("<span class=\"pill gold\">DR{d}</span>"),
                    Some(d) if d >= 8 => format!("<span class=\"pill acc\">DR{d}</span>"),
                    Some(d) => format!("<span class=\"pill bad\">DR{d}</span>"),
                    None => "—".into(),
                };
                let lufs = t
                    .integrated_lufs
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "—".into());
                let tags_html = if t.custom_tags.is_empty() {
                    "—".into()
                } else {
                    t.custom_tags
                        .iter()
                        .map(|x| format!("<span class=\"tag\">#{}</span>", esc(x)))
                        .collect::<Vec<_>>()
                        .join("")
                };
                html.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td><span class=\"{}\">{}</span></td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
                    num,
                    esc(&t.title),
                    esc(&comp_work),
                    fmt_cls,
                    esc(&t.quality_full()),
                    dr_html,
                    lufs,
                    tags_html,
                ));
            }
            html.push_str("</tbody></table>\n</section>\n");
        }
        html.push_str("</body>\n</html>\n");
        std::fs::write(path, html)?;
        Ok(selected.len())
    }
}

/// Moves the elements at `moving_indices` (in their existing relative order) to `insert_before`
/// inside `items`. Returns `Some((new_start, count))` if the order changed, or `None` on a no-op.
pub fn reorder_vec_indices<T: Clone>(
    items: &mut Vec<T>,
    moving_indices: &[usize],
    insert_before: usize,
) -> Option<(usize, usize)> {
    let len = items.len();
    if len < 2 || moving_indices.is_empty() {
        return None;
    }
    let mut sorted: Vec<usize> = moving_indices
        .iter()
        .copied()
        .filter(|&i| i < len)
        .collect();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() || sorted.len() == len {
        return None;
    }
    let insert_clamped = insert_before.min(len);
    let before_count = sorted.iter().filter(|&&i| i < insert_clamped).count();
    let dest = insert_clamped.saturating_sub(before_count);

    // Check if the block is already contiguous at `dest` (no-op drop)
    let is_already_in_place =
        sorted.first() == Some(&dest) && sorted.last() == Some(&(dest + sorted.len() - 1));
    if is_already_in_place {
        return None;
    }

    let moving_set: HashSet<usize> = sorted.iter().copied().collect();
    let mut moved = Vec::with_capacity(sorted.len());
    let mut remaining = Vec::with_capacity(len - sorted.len());
    for (idx, item) in items.drain(..).enumerate() {
        if moving_set.contains(&idx) {
            moved.push(item);
        } else {
            remaining.push(item);
        }
    }
    let insert_at = dest.min(remaining.len());
    let moved_len = moved.len();
    let tail = remaining.split_off(insert_at);
    remaining.extend(moved);
    remaining.extend(tail);
    *items = remaining;
    Some((insert_at, moved_len))
}

const DB_SCHEMA_VERSION: u32 = 2;

fn default_schema_version() -> u32 {
    DB_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStore {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub active_id: u64,
    pub libraries: Vec<Library>,
}

impl Default for LibraryStore {
    fn default() -> Self {
        let main = Library::default();
        Self {
            schema_version: DB_SCHEMA_VERSION,
            active_id: main.id,
            libraries: vec![main],
        }
    }
}

impl LibraryStore {
    pub fn load() -> Self {
        let multi_path = data_directory().join("libraries.json");
        if let Ok(data) = std::fs::read(&multi_path)
            && let Ok(mut store) = serde_json::from_slice::<LibraryStore>(&data)
            && !store.libraries.is_empty()
        {
            if !store.libraries.iter().any(|l| l.id == store.active_id) {
                store.active_id = store.libraries[0].id;
            }
            for lib in &mut store.libraries {
                lib.migrate_ratings_if_needed();
                lib.rebuild_index();
            }
            return store;
        }
        // Migrate legacy single library.json transparently
        let legacy_path = data_directory().join("library.json");
        if let Ok(data) = std::fs::read(&legacy_path)
            && let Ok(mut lib) = serde_json::from_slice::<Library>(&data)
        {
            lib.id = default_library_id();
            if lib.name.trim().is_empty() {
                lib.name = default_library_name();
            }
            lib.migrate_ratings_if_needed();
            lib.rebuild_index();
            return Self {
                schema_version: DB_SCHEMA_VERSION,
                active_id: lib.id,
                libraries: vec![lib],
            };
        }
        Self::default()
    }

    pub fn export_backup_json(&self, path: &Path) -> std::io::Result<()> {
        let pretty = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(path, pretty)
    }

    pub fn import_backup_json(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| format!("Reading backup failed: {e}"))?;
        let mut store: LibraryStore =
            serde_json::from_slice(&data).map_err(|e| format!("Invalid backup JSON: {e}"))?;
        if store.libraries.is_empty() {
            return Err("Backup contains no libraries".into());
        }
        if !store.libraries.iter().any(|l| l.id == store.active_id) {
            store.active_id = store.libraries[0].id;
        }
        for lib in &mut store.libraries {
            lib.migrate_ratings_if_needed();
            lib.rebuild_index();
        }
        store.schema_version = DB_SCHEMA_VERSION;
        Ok(store)
    }

    /// Non-blocking save: enqueues snapshot onto a single coalescing background writer thread
    /// so the 60 Hz UI thread never stalls on JSON serialization or disk I/O mid-playback.
    pub fn save(&self) {
        let tx = background_saver();
        let _ = tx.send(self.clone());
    }

    /// Blocking save used during shutdown (`Drop`) to guarantee final state is flushed to disk.
    pub fn save_sync(&self) {
        let path = data_directory().join("libraries.json");
        save_store_to_path(self, &path);
    }

    pub fn active(&self) -> &Library {
        self.libraries
            .iter()
            .find(|l| l.id == self.active_id)
            .unwrap_or(&self.libraries[0])
    }

    pub fn active_mut(&mut self) -> &mut Library {
        let idx = self
            .libraries
            .iter()
            .position(|l| l.id == self.active_id)
            .unwrap_or(0);
        &mut self.libraries[idx]
    }

    pub fn create_library(&mut self, name: String) -> u64 {
        let trimmed = name.trim();
        let final_name = if trimmed.is_empty() {
            format!("Library {}", self.libraries.len() + 1)
        } else {
            trimmed.to_string()
        };
        let id = unique_id(&final_name, self.libraries.len() as u64 + 10);
        self.libraries.push(Library {
            id,
            name: final_name,
            ..Default::default()
        });
        self.active_id = id;
        id
    }

    pub fn rename_library(&mut self, id: u64, new_name: String) -> Option<String> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lib = self.libraries.iter_mut().find(|l| l.id == id)?;
        lib.name = trimmed.to_string();
        Some(lib.name.clone())
    }

    pub fn delete_library(&mut self, id: u64) -> Option<String> {
        if self.libraries.len() <= 1 {
            return None;
        }
        let pos = self.libraries.iter().position(|l| l.id == id)?;
        let name = self.libraries.remove(pos).name;
        if self.active_id == id {
            self.active_id = self.libraries[0].id;
        }
        Some(name)
    }
}

fn background_saver() -> &'static Sender<LibraryStore> {
    static SAVER: OnceLock<Sender<LibraryStore>> = OnceLock::new();
    SAVER.get_or_init(|| {
        let (tx, rx) = unbounded::<LibraryStore>();
        let _ = thread::Builder::new()
            .name("txplayer-save".into())
            .spawn(move || {
                while let Ok(mut latest) = rx.recv() {
                    // Coalesce bursts (e.g. rapid favorite toggles / scan completions)
                    while let Ok(newer) = rx.try_recv() {
                        latest = newer;
                    }
                    let path = data_directory().join("libraries.json");
                    save_store_to_path(&latest, &path);
                }
            });
        tx
    })
}

/// Cross-platform safe atomic replacement with rolling `.bak` database backup.
fn save_store_to_path(store: &LibraryStore, path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(data) = serde_json::to_vec(store) else {
        return;
    };
    if path.exists() {
        let bak = path.with_extension("json.bak");
        let _ = std::fs::copy(path, &bak);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, &data).is_ok() {
        if std::fs::rename(&tmp, path).is_ok() {
            return;
        }
        // Windows: rename refuses to overwrite an existing destination file — remove then rename,
        // falling back to direct write if the file is momentarily held by AV/indexer.
        let _ = std::fs::remove_file(path);
        if std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::write(path, &data);
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn unique_id(label: &str, salt: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    label.hash(&mut hasher);
    salt.hash(&mut hasher);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    nanos.hash(&mut hasher);
    hasher.finish().max(2)
}

pub fn collect_supported_files(path: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut audio_files = Vec::new();
    let mut cue_files = Vec::new();
    if path.is_file() {
        if is_cue_file(path) {
            cue_files.push(path.to_path_buf());
        } else if is_supported(path) {
            audio_files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        for entry in WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file() {
                let p = entry.path();
                if is_cue_file(p) {
                    cue_files.push(entry.into_path());
                } else if is_supported(p) {
                    audio_files.push(entry.into_path());
                }
            }
        }
    }
    audio_files.sort();
    cue_files.sort();
    (audio_files, cue_files)
}

pub fn scan(paths: Vec<PathBuf>, events: Sender<ScanEvent>, cover_directory: PathBuf) {
    let mut files = Vec::new();
    let mut cues = Vec::new();
    for path in paths {
        let (a, c) = collect_supported_files(&path);
        files.extend(a);
        cues.extend(c);
    }
    files.sort();
    files.dedup();
    cues.sort();
    cues.dedup();

    let mut found = 0;
    let mut failed = 0;
    let mut cue_referenced_images: HashSet<PathBuf> = HashSet::new();

    // 1) Parse `.cue` sheets first so monolithic album images are split cleanly without duplicates
    for cue_path in &cues {
        match read_cue_tracks(cue_path, &cover_directory) {
            Ok((cue_tracks, referenced)) => {
                cue_referenced_images.extend(referenced);
                for tr in cue_tracks {
                    found += 1;
                    if events.send(ScanEvent::Track(Box::new(tr))).is_err() {
                        return;
                    }
                }
            }
            Err(_) => failed += 1,
        }
    }

    // 2) Scan standalone audio files (skipping raw monolithic images already expanded by a CUE sheet,
    //    and auto-expanding FLAC files with embedded CUESHEETs into individual album tracks!)
    let standalone: Vec<PathBuf> = files
        .into_iter()
        .filter(|p| !cue_referenced_images.contains(p))
        .collect();
    let total = standalone.len();
    for (index, path) in standalone.iter().enumerate() {
        if let Some(embedded_tracks) = read_embedded_flac_cue_tracks(path, &cover_directory) {
            for tr in embedded_tracks {
                found += 1;
                if events.send(ScanEvent::Track(Box::new(tr))).is_err() {
                    return;
                }
            }
        } else {
            match read_track(path, &cover_directory) {
                Ok(track) => {
                    found += 1;
                    if events.send(ScanEvent::Track(Box::new(track))).is_err() {
                        return;
                    }
                }
                Err(_) => failed += 1,
            }
        }
        let _ = events.send(ScanEvent::Progress {
            scanned: index + 1,
            total,
        });
    }
    let _ = events.send(ScanEvent::Finished { found, failed });
}

/// Determines whether an install directory on Windows should hold self-contained
/// portable data (`<exe_dir>/data`) instead of falling back to `%LOCALAPPDATA%\TxPlayer`.
///
/// Custom install paths (e.g. `D:\TxPlayer`), portable ZIP extractions, and user-writable
/// non-system directories keep ALL data & cover art inside `<exe_dir>\data` so nothing is
/// ever scattered into `AppData\Roaming` or `AppData\Local`.
#[allow(dead_code)]
fn should_use_portable_exe_dir(exe_dir: &Path) -> bool {
    if exe_dir.join("portable.txt").exists() || exe_dir.join("data").is_dir() {
        return true;
    }
    let normalized = exe_dir.to_string_lossy().replace('/', "\\").to_lowercase();
    let is_protected_system_or_build = normalized.contains("\\program files\\")
        || normalized.ends_with("\\program files")
        || normalized.contains("\\program files (x86)\\")
        || normalized.ends_with("\\program files (x86)")
        || normalized.contains("\\windows\\")
        || normalized.ends_with("\\windows")
        || normalized.contains("\\target\\debug")
        || normalized.contains("\\target\\release");
    !is_protected_system_or_build
}

#[allow(dead_code)]
fn try_writable_data_subdir(exe_dir: &Path) -> Option<PathBuf> {
    if !should_use_portable_exe_dir(exe_dir) {
        return None;
    }
    let data_dir = exe_dir.join("data");
    std::fs::create_dir_all(&data_dir).ok()?;
    let probe = data_dir.join(format!(".txplayer_write_probe_{}", std::process::id()));
    std::fs::write(&probe, b"").ok()?;
    let _ = std::fs::remove_file(&probe);
    Some(data_dir)
}

pub fn data_directory() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        use std::sync::OnceLock;
        static RESOLVED: OnceLock<PathBuf> = OnceLock::new();
        RESOLVED
            .get_or_init(|| {
                if let Ok(exe) = std::env::current_exe()
                    && let Some(exe_dir) = exe.parent()
                    && let Some(portable) = try_writable_data_subdir(exe_dir)
                {
                    return portable;
                }
                // Fallback for read-only admin installs (e.g. C:\Program Files\TxPlayer):
                // keep everything in ONE local directory (%LOCALAPPDATA%\TxPlayer) and NEVER
                // touch AppData\Roaming.
                dirs::data_local_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("TxPlayer")
            })
            .clone()
    }
    #[cfg(not(target_os = "windows"))]
    {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("TxPlayer")
    }
}

pub fn cover_directory() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // Keep extracted cover thumbnails inside the same single data directory
        // (e.g. D:\TxPlayer\data\covers) instead of scattering files into AppData\Local.
        data_directory().join("covers")
    }
    #[cfg(not(target_os = "windows"))]
    {
        dirs::cache_dir()
            .unwrap_or_else(data_directory)
            .join("TxPlayer")
            .join("covers")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_track(id: u64, path: &str, title: &str) -> Track {
        Track {
            id,
            path: PathBuf::from(path),
            title: title.into(),
            artist: "Frédéric Chopin".into(),
            album: "Nocturnes".into(),
            album_artist: None,
            track_number: Some(1),
            disc_number: None,
            year: Some("2024".into()),
            genre: Some("Classical".into()),
            duration: Some(260.0),
            sample_rate: Some(96_000),
            bit_depth: Some(24),
            channels: Some(2),
            codec: "FLAC".into(),
            nominal_bitrate: None,
            cover_path: None,
            cue_start: None,
            cue_end: None,
            rg_track_gain_db: None,
            rg_track_peak: None,
            rg_album_gain_db: None,
            rg_album_peak: None,
            lyrics: None,
            composer: Some("Frédéric Chopin".into()),
            conductor: None,
            orchestra: None,
            work: None,
            label: Some("Deutsche Grammophon".into()),
            catalog_number: None,
            comment: None,
            custom_tags: Vec::new(),
            dr_score: Some(13),
            integrated_lufs: Some(-18.4),
            true_peak_dbtp: Some(-1.2),
            effective_bandwidth_hz: Some(38_000.0),
            authenticity: Some("AuthenticHiRes".into()),
            play_count: 0,
            last_played_at: None,
            rating: 0,
            added_at: 0,
        }
    }

    #[test]
    fn collections_saved_views_and_stats_work_end_to_end() {
        let mut lib = Library::default();
        let mut t1 = sample_track(1, "/music/beethoven_op27.flac", "Moonlight Sonata");
        t1.composer = Some("Ludwig van Beethoven".into());
        t1.sample_rate = Some(192_000);
        t1.dr_score = Some(14);
        t1.rating = 10;
        let mut t2 = sample_track(2, "/music/chopin_nocturne.dsf", "Nocturne Op.9 No.2");
        t2.codec = "DSD64".into();
        t2.sample_rate = Some(2_822_400);
        t2.dr_score = Some(11);
        t2.rating = 8;
        lib.upsert(t1);
        lib.upsert(t2);
        assert_eq!(lib.add_custom_tag(&[1], "#reference"), 1);

        let cid = lib.create_collection("Audiophile Piano".into(), "Best piano masters".into(), 2);
        assert_eq!(lib.add_tracks_to_collection(cid, &[1, 2]), 2);
        assert_eq!(lib.collection(cid).unwrap().track_ids.len(), 2);

        let vid = lib.upsert_saved_view(SavedView {
            id: 0,
            name: "High-DR Beethoven".into(),
            match_all: true,
            rules: vec![
                QueryRule {
                    field: QueryField::Composer,
                    op: QueryOp::Contains,
                    value: "Beethoven".into(),
                },
                QueryRule {
                    field: QueryField::DynamicRange,
                    op: QueryOp::GreaterOrEq,
                    value: "12".into(),
                },
                QueryRule {
                    field: QueryField::CustomTag,
                    op: QueryOp::Equals,
                    value: "reference".into(),
                },
            ],
        });
        let sv = lib.saved_view(vid).unwrap();
        let matched: Vec<u64> = lib
            .tracks
            .iter()
            .filter(|t| sv.matches(t))
            .map(|t| t.id)
            .collect();
        assert_eq!(matched, vec![1]);

        let stats = lib.compute_stats();
        assert_eq!(stats.total_tracks, 2);
        assert_eq!(stats.total_composers, 2);
        assert_eq!(stats.dsd_tracks, 1);
        assert_eq!(stats.verified_hires_tracks, 2);
        assert!((stats.avg_dr.unwrap() - 12.5).abs() < 0.01);
    }

    #[test]
    fn duplicate_finder_ranks_hires_master_above_lossy_copy() {
        let mut lib = Library::default();
        let mut mp3 = sample_track(10, "/music/lossy.mp3", "Moonlight Sonata");
        mp3.codec = "MP3".into();
        mp3.sample_rate = Some(44_100);
        mp3.bit_depth = None;
        let mut flac = sample_track(20, "/music/master.flac", "Moonlight Sonata");
        flac.codec = "FLAC".into();
        flac.sample_rate = Some(192_000);
        flac.bit_depth = Some(24);
        lib.upsert(mp3);
        lib.upsert(flac);

        let dups = lib.find_duplicate_groups();
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].best_track_id, 20);
        assert_eq!(dups[0].track_ids, vec![20, 10]);
    }

    #[test]
    fn local_radio_ranks_same_composer_and_tags_highest() {
        let mut lib = Library::default();
        let mut seed = sample_track(1, "/m/chopin1.flac", "Nocturne 1");
        seed.composer = Some("Frédéric Chopin".into());
        seed.custom_tags = vec!["solo-piano".into()];
        let mut best_match = sample_track(2, "/m/chopin2.flac", "Ballade 1");
        best_match.album = "Ballades".into();
        best_match.composer = Some("Frédéric Chopin".into());
        best_match.custom_tags = vec!["solo-piano".into()];
        let mut distant = sample_track(3, "/m/rock.mp3", "Thunder");
        distant.artist = "Rock Band".into();
        distant.album = "Live".into();
        distant.composer = None;
        distant.genre = Some("Rock".into());
        distant.label = None;
        distant.codec = "MP3".into();
        distant.sample_rate = Some(44_100);
        distant.bit_depth = None;
        lib.upsert(seed);
        lib.upsert(best_match);
        lib.upsert(distant);

        let radio = lib.build_similar_radio(1, 10);
        assert_eq!(radio[0], 1);
        assert_eq!(radio[1], 2);
    }

    #[test]
    fn libraries_playlists_and_multi_delete() {
        let mut store = LibraryStore::default();
        let classical_id = store.create_library("Classical & Piano".into());
        assert_eq!(store.active().id, classical_id);

        let lib = store.active_mut();
        lib.upsert(sample_track(1, "/music/nocturne1.flac", "Nocturne Op.9 No.1"));
        lib.upsert(sample_track(2, "/music/nocturne2.flac", "Nocturne Op.9 No.2"));
        lib.upsert(sample_track(3, "/music/ballade1.flac", "Ballade No.1"));

        let pl_id = lib.create_playlist("Late Night Piano".into());
        assert_eq!(lib.add_tracks_to_playlist(pl_id, &[1, 2, 3]), 3);
        // Remove two tracks from playlist without touching library
        assert_eq!(lib.remove_tracks_from_playlist(pl_id, &[1, 3]), 2);
        assert_eq!(lib.playlist(pl_id).unwrap().track_ids, vec![2]);
        assert_eq!(lib.tracks.len(), 3);

        // Removing track 2 from library also cleans playlist
        let removed = lib.remove_tracks(&[2]);
        assert_eq!(removed.len(), 1);
        assert!(lib.playlist(pl_id).unwrap().track_ids.is_empty());
        // O(1) index stays in sync after upsert, sort, and removal
        assert!(lib.get_by_id(1).is_some());
        assert!(lib.get_by_id(2).is_none());
        assert_eq!(lib.get_by_id(3).unwrap().title, "Ballade No.1");
        lib.sort_default();
        assert_eq!(lib.get_by_id(3).unwrap().title, "Ballade No.1");
    }

    #[test]
    fn save_store_overwrites_existing_file_cleanly() {
        let dir = std::env::temp_dir().join(format!(
            "txplayer_save_test_{}",
            std::process::id()
        ));
        let path = dir.join("libraries.json");
        let mut store = LibraryStore::default();
        store
            .active_mut()
            .upsert(sample_track(10, "/a.flac", "First"));
        save_store_to_path(&store, &path);
        // Second save overwrites existing file (regression test for Windows rename failure)
        store
            .active_mut()
            .upsert(sample_track(11, "/b.flac", "Second"));
        save_store_to_path(&store, &path);
        let raw = std::fs::read(&path).unwrap();
        let reloaded: LibraryStore = serde_json::from_slice(&raw).unwrap();
        assert_eq!(reloaded.active().tracks.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playlist_drag_reorder_dedupe_rename_and_duplicate() {
        let mut store = LibraryStore::default();
        let lib = store.active_mut();
        for id in 1..=5u64 {
            lib.upsert(sample_track(id, &format!("/m/{id}.flac"), &format!("Track {id}")));
        }
        // Duplicate track metadata under different ID
        lib.upsert(sample_track(6, "/m/dup.flac", "Track 2"));

        let pl_id = lib.create_playlist("Favorites Mix".into());
        lib.add_tracks_to_playlist(pl_id, &[1, 2, 3, 4, 5]);

        // Drag single track 0 -> before index 4 (after Track 4) => [2, 3, 4, 1, 5]
        assert_eq!(lib.reorder_playlist_indices(pl_id, &[0], 4), Some((3, 1)));
        assert_eq!(lib.playlist(pl_id).unwrap().track_ids, vec![2, 3, 4, 1, 5]);

        // Drag multi-selection [1, 2] (Tracks 3, 4) -> top (index 0) => [3, 4, 2, 1, 5]
        assert_eq!(lib.reorder_playlist_indices(pl_id, &[1, 2], 0), Some((0, 2)));
        assert_eq!(lib.playlist(pl_id).unwrap().track_ids, vec![3, 4, 2, 1, 5]);

        // No-op drag returns None
        assert_eq!(lib.reorder_playlist_indices(pl_id, &[0, 1], 1), None);

        // Dedupe removes Track 6 (same title+artist as Track 2)
        lib.add_tracks_to_playlist(pl_id, &[6]);
        assert_eq!(lib.deduplicate_playlist(pl_id), 1);
        assert_eq!(lib.playlist(pl_id).unwrap().track_ids, vec![3, 4, 2, 1, 5]);

        // Rename & duplicate
        assert_eq!(
            lib.rename_playlist(pl_id, "Evening Chopin".into()).as_deref(),
            Some("Evening Chopin")
        );
        let (dup_id, dup_name) = lib.duplicate_playlist(pl_id).unwrap();
        assert_eq!(dup_name, "Evening Chopin (Copy)");
        assert_eq!(lib.playlist(dup_id).unwrap().track_ids, vec![3, 4, 2, 1, 5]);

        // Reverse
        assert!(lib.reverse_playlist(dup_id));
        assert_eq!(lib.playlist(dup_id).unwrap().track_ids, vec![5, 1, 2, 4, 3]);
    }

    #[test]
    fn library_drag_reorder_keeps_index_consistent() {
        let mut store = LibraryStore::default();
        let lib = store.active_mut();
        for id in 1..=5u64 {
            lib.upsert(sample_track(id, &format!("/m/{id}.flac"), &format!("Song {id}")));
        }

        // Materialize an explicit order first
        lib.set_tracks_order_by_ids(&[5, 4, 3, 2, 1]);
        let ids: Vec<u64> = lib.tracks.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![5, 4, 3, 2, 1]);
        assert_eq!(lib.get_by_id(5).unwrap().title, "Song 5");
        assert_eq!(lib.get_by_id(1).unwrap().title, "Song 1");

        // Drag index 0 (Song 5) before index 4 -> [4, 3, 2, 5, 1]
        assert_eq!(lib.reorder_tracks_indices(&[0], 4), Some((3, 1)));
        let ids: Vec<u64> = lib.tracks.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![4, 3, 2, 5, 1]);
        // O(1) index stays accurate after reorder
        for id in 1..=5u64 {
            assert_eq!(lib.get_by_id(id).unwrap().id, id);
        }
    }

    #[test]
    fn compute_stats_builds_decades_dr_histogram_and_top_composers_labels() {
        let mut store = LibraryStore::default();
        let lib = store.active_mut();
        let mut t1 = sample_track(1, "/m/1.flac", "Sonata 1");
        t1.year = Some("1974".into());
        t1.composer = Some("Ludwig van Beethoven".into());
        t1.label = Some("Deutsche Grammophon".into());
        t1.dr_score = Some(15);

        let mut t2 = sample_track(2, "/m/2.flac", "Sonata 2");
        t2.year = Some("1978-05-12".into());
        t2.composer = Some("Ludwig van Beethoven".into());
        t2.label = Some("Deutsche Grammophon".into());
        t2.dr_score = Some(12);

        let mut t3 = sample_track(3, "/m/3.flac", "Pop Track");
        t3.year = Some("2022".into());
        t3.composer = Some("Frédéric Chopin".into());
        t3.label = Some("Decca".into());
        t3.dr_score = Some(6);

        lib.upsert(t1);
        lib.upsert(t2);
        lib.upsert(t3);

        let stats = lib.compute_stats();
        assert_eq!(
            stats.decade_counts,
            vec![("1970s".to_string(), 2), ("2020s".to_string(), 1)]
        );
        assert_eq!(stats.top_composers[0], ("Ludwig van Beethoven".to_string(), 2));
        assert_eq!(stats.top_labels[0], ("Deutsche Grammophon".to_string(), 2));
        assert_eq!(stats.dr_buckets.len(), 3);
        assert_eq!(stats.dr_buckets[0], ("DR 14+ Ref".to_string(), 1));
        assert_eq!(stats.dr_buckets[1], ("DR 11–13 Dyn".to_string(), 1));
        assert_eq!(stats.dr_buckets[2], ("DR ≤7 Loud".to_string(), 1));
    }

    #[test]
    fn exports_portable_markdown_and_html_listening_catalogs() {
        let mut store = LibraryStore::default();
        let lib = store.active_mut();
        let mut t1 = sample_track(1, "/m/1.flac", "Sonata <No. 14> & Adagio");
        t1.artist = "Maurizio Pollini".into();
        t1.album = "Beethoven Sonatas".into();
        t1.composer = Some("Ludwig van Beethoven".into());
        t1.label = Some("Deutsche Grammophon".into());
        t1.dr_score = Some(14);
        t1.integrated_lufs = Some(-18.2);
        t1.authenticity = Some("AuthenticHiRes".into());
        t1.custom_tags = vec!["reference".into()];
        lib.upsert(t1);
        lib.set_album_note(
            "Beethoven Sonatas",
            "Maurizio Pollini",
            "Original 1977 pressing — breathtaking dynamics.".to_string(),
        );

        let dir = std::env::temp_dir();
        let md_path = dir.join("txplayer_catalog_test.md");
        let html_path = dir.join("txplayer_catalog_test.html");
        let n_md = lib
            .export_catalog_markdown("Reference Classical", None, &md_path)
            .expect("md export");
        let n_html = lib
            .export_catalog_html("Reference Classical", Some(&[1]), &html_path)
            .expect("html export");
        assert_eq!(n_md, 1);
        assert_eq!(n_html, 1);

        let md = std::fs::read_to_string(&md_path).unwrap();
        assert!(md.contains("Reference Classical"));
        assert!(md.contains("Collector Liner Notes:"));
        assert!(md.contains("DR14"));
        assert!(md.contains("#reference"));

        let html = std::fs::read_to_string(&html_path).unwrap();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Sonata &lt;No. 14&gt; &amp; Adagio"));
        assert!(html.contains("DR14"));
        assert!(html.contains("Original 1977 pressing"));

        let _ = std::fs::remove_file(&md_path);
        let _ = std::fs::remove_file(&html_path);
    }

    #[test]
    fn rename_library_and_half_star_migration_work() {
        let mut store = LibraryStore::default();
        let main_id = store.active_id;
        assert_eq!(store.active().name, "Main Library");
        let renamed = store.rename_library(main_id, "  Studio Masters  ".into());
        assert_eq!(renamed.as_deref(), Some("Studio Masters"));
        assert_eq!(store.active().name, "Studio Masters");
        // Empty rename is rejected
        assert_eq!(store.rename_library(main_id, "   ".into()), None);

        // Simulate legacy 0..=5 integer star library and verify automatic ×2 migration
        let mut lib = Library {
            rating_scale: 1,
            ..Library::default()
        };
        let mut t1 = sample_track(1, "/m/a.flac", "A");
        t1.rating = 4; // legacy 4 stars -> 8 half-stars (4.0)
        let mut t2 = sample_track(2, "/m/b.flac", "B");
        t2.rating = 5; // legacy 5 stars -> 10 half-stars (5.0)
        lib.upsert(t1);
        lib.upsert(t2);
        lib.migrate_ratings_if_needed();
        assert_eq!(lib.rating_scale, 2);
        assert_eq!(lib.get_by_id(1).unwrap().rating, 8);
        assert_eq!(lib.get_by_id(2).unwrap().rating, 10);

        // Half-star precision (4.5 stars = 9) + Smart View rule `Rating >= 4.5`
        lib.set_rating(1, 9);
        assert_eq!(lib.get_by_id(1).unwrap().rating, 9);
        let rule = QueryRule {
            field: QueryField::Rating,
            op: QueryOp::GreaterOrEq,
            value: "4.5".into(),
        };
        assert!(rule.matches(lib.get_by_id(1).unwrap()));
        assert!(rule.matches(lib.get_by_id(2).unwrap()));
        lib.set_rating(1, 8);
        assert!(!rule.matches(lib.get_by_id(1).unwrap()));
    }

    #[test]
    fn windows_portable_data_dir_policy_avoids_system_and_build_dirs() {
        assert!(should_use_portable_exe_dir(Path::new(r"D:\TxPlayer")));
        assert!(should_use_portable_exe_dir(Path::new(r"E:\Apps\Audio\TxPlayer")));
        assert!(!should_use_portable_exe_dir(Path::new(
            r"C:\Program Files\TxPlayer"
        )));
        assert!(!should_use_portable_exe_dir(Path::new(
            r"C:\Program Files (x86)\TxPlayer"
        )));
        assert!(!should_use_portable_exe_dir(Path::new(
            r"C:\Windows\System32"
        )));
        assert!(!should_use_portable_exe_dir(Path::new(
            r"C:\dev\txplayer\target\debug\deps"
        )));

        let tmp = std::env::temp_dir().join(format!(
            "txplayer_portable_dir_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let resolved = try_writable_data_subdir(&tmp).expect("writable custom dir");
        assert_eq!(resolved, tmp.join("data"));
        assert!(resolved.is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
