use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, unbounded};
use walkdir::WalkDir;

use crate::metadata::{is_cue_file, is_supported};

#[derive(Debug)]
pub struct WatchDelta {
    pub changed_or_added: Vec<PathBuf>,
    pub removed_count: usize,
}

/// Lightweight, 100% cross-platform background directory watcher (macOS / Windows / Linux / SMB).
/// Diffs `(path -> (mtime_secs, byte_len))` every 4 seconds so newly dropped or deleted audio/CUE
/// files auto-sync into the active library with zero OS-specific watch limits.
pub struct FolderWatcher {
    folders_tx: Sender<Vec<PathBuf>>,
    events_rx: Receiver<WatchDelta>,
    enabled: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    _join: JoinHandle<()>,
}

impl FolderWatcher {
    pub fn spawn(initial_folders: Vec<PathBuf>, enabled: bool) -> Self {
        let (folders_tx, folders_rx) = unbounded::<Vec<PathBuf>>();
        let (events_tx, events_rx) = unbounded::<WatchDelta>();
        let enabled_flag = Arc::new(AtomicBool::new(enabled));
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let en = Arc::clone(&enabled_flag);
        let sd = Arc::clone(&shutdown_flag);

        let join = thread::Builder::new()
            .name("txplayer-watcher".into())
            .spawn(move || {
                let mut watched = initial_folders;
                let mut snapshot = take_snapshot(&watched);
                let mut ticks = 0u32;

                while !sd.load(Ordering::Relaxed) {
                    // Drain any folder list updates quickly
                    let mut list_changed = false;
                    while let Ok(new_list) = folders_rx.try_recv() {
                        if new_list != watched {
                            watched = new_list;
                            list_changed = true;
                        }
                    }
                    if list_changed {
                        snapshot = take_snapshot(&watched);
                        ticks = 0;
                    }

                    thread::sleep(Duration::from_millis(500));
                    ticks += 1;
                    if ticks < 8 || !en.load(Ordering::Relaxed) || watched.is_empty() {
                        continue;
                    }
                    ticks = 0;

                    let next = take_snapshot(&watched);
                    let mut changed_or_added = Vec::new();
                    for (path, sig) in &next {
                        if snapshot.get(path) != Some(sig) {
                            changed_or_added.push(path.clone());
                        }
                    }
                    let removed_count = snapshot
                        .keys()
                        .filter(|p| !next.contains_key(*p))
                        .count();

                    snapshot = next;
                    if !changed_or_added.is_empty() || removed_count > 0 {
                        let _ = events_tx.send(WatchDelta {
                            changed_or_added,
                            removed_count,
                        });
                    }
                }
            })
            .expect("folder watcher thread should start");

        Self {
            folders_tx,
            events_rx,
            enabled: enabled_flag,
            shutdown: shutdown_flag,
            _join: join,
        }
    }

    pub fn set_folders(&self, folders: Vec<PathBuf>) {
        let _ = self.folders_tx.send(folders);
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn try_event(&self) -> Option<WatchDelta> {
        self.events_rx.try_recv().ok()
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

fn take_snapshot(folders: &[PathBuf]) -> HashMap<PathBuf, (u64, u64)> {
    let mut map = HashMap::new();
    for folder in folders {
        if !folder.is_dir() {
            continue;
        }
        for entry in WalkDir::new(folder)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            if !(is_supported(p) || is_cue_file(p)) {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                map.insert(entry.into_path(), (mtime, meta.len()));
            }
        }
    }
    map
}
