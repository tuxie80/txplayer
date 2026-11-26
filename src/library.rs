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
            QueryField::Rating => match_num(Some(t.rating as i64)),
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
            id_index: HashMap::new(),
        }
    }
}

impl Library {
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
            t.rating = rating.min(5);
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
                composers.insert(c.trim().to_lowercase());
            }
            if let Some(ref g) = t.genre
                && !g.trim().is_empty()
            {
                genres.insert(g.trim().to_lowercase());
            }
            if let Some(ref l) = t.label
                && !l.trim().is_empty()
            {
                labels.insert(l.trim().to_lowercase());
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
                t.rating,
                t.play_count,
                tags_joined,
                esc(&t.path.to_string_lossy()),
            ));
        }
        std::fs::write(path, out)?;
        Ok(self.tracks.len())
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

pub fn data_directory() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("TxPlayer")
}

pub fn cover_directory() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(data_directory)
        .join("TxPlayer")
        .join("covers")
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
        t1.rating = 5;
        let mut t2 = sample_track(2, "/music/chopin_nocturne.dsf", "Nocturne Op.9 No.2");
        t2.codec = "DSD64".into();
        t2.sample_rate = Some(2_822_400);
        t2.dr_score = Some(11);
        t2.rating = 4;
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
}
