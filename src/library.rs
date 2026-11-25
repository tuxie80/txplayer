use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::metadata::{Track, is_supported, read_track};

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
        }
    }
}

impl Library {
    pub fn upsert(&mut self, track: Track) {
        if let Some(existing) = self.tracks.iter_mut().find(|item| item.id == track.id) {
            *existing = track;
        } else {
            self.tracks.push(track);
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
            return Self {
                active_id: lib.id,
                libraries: vec![lib],
            };
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = data_directory().join("libraries.json");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_vec(self) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, data).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
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

pub fn collect_supported_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if path.is_file() {
        if is_supported(path) {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        for entry in WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file() && is_supported(entry.path()) {
                files.push(entry.into_path());
            }
        }
    }
    files.sort();
    files
}

pub fn scan(paths: Vec<PathBuf>, events: Sender<ScanEvent>, cover_directory: PathBuf) {
    let mut files = Vec::new();
    for path in paths {
        files.extend(collect_supported_files(&path));
    }
    files.sort();
    files.dedup();

    let total = files.len();
    let mut found = 0;
    let mut failed = 0;
    for (index, path) in files.iter().enumerate() {
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
    }
}
