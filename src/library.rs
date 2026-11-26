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

use crate::cue::read_cue_tracks;
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
            // Preserve user listening stats, ratings, and original added_at when rescanning!
            track.play_count = existing.play_count.max(track.play_count);
            if track.rating == 0 {
                track.rating = existing.rating;
            }
            if existing.added_at > 0 {
                track.added_at = existing.added_at;
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
        if let Some(t) = self.get_by_id_mut(id) {
            t.play_count = t.play_count.saturating_add(1);
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStore {
    pub active_id: u64,
    pub libraries: Vec<Library>,
}

impl Default for LibraryStore {
    fn default() -> Self {
        let main = Library::default();
        Self {
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
                active_id: lib.id,
                libraries: vec![lib],
            };
        }
        Self::default()
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

/// Cross-platform safe atomic replacement (fixes Windows `std::fs::rename` failing with
/// `ERROR_ALREADY_EXISTS` when destination `libraries.json` already exists!).
fn save_store_to_path(store: &LibraryStore, path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(data) = serde_json::to_vec(store) else {
        return;
    };
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

    // 2) Scan standalone audio files (skipping raw monolithic images already expanded by a CUE sheet)
    let standalone: Vec<PathBuf> = files
        .into_iter()
        .filter(|p| !cue_referenced_images.contains(p))
        .collect();
    let total = standalone.len();
    for (index, path) in standalone.iter().enumerate() {
        match read_track(path, &cover_directory) {
            Ok(track) => {
                found += 1;
                if events.send(ScanEvent::Track(Box::new(track))).is_err() {
                    return;
                }
            }
            Err(_) => failed += 1,
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
            play_count: 0,
            rating: 0,
            added_at: 0,
        }
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
}
