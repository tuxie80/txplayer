use std::path::Path;
use std::time::Duration;

use crossbeam_channel::{Receiver, unbounded};
#[cfg(target_os = "windows")]
use raw_window_handle::HasWindowHandle;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

#[derive(Debug, Clone)]
pub enum OsMediaCommand {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    SeekAbs(f64),
    SeekRel(f64),
}

/// Cross-platform OS Media Controls & Now Playing bridge:
/// - macOS: Control Center / Lock Screen / headphone keys (`MediaPlayer.framework`)
/// - Windows: System Media Transport Controls (SMTC)
/// - Linux: MPRIS2 over pure-Rust D-Bus (`zbus`)
pub struct OsMediaBridge {
    controls: Option<MediaControls>,
    rx: Receiver<OsMediaCommand>,
    last_track_key: Option<(u64, String, String)>,
    last_state: Option<(bool, bool)>, // (loaded, playing)
}

impl OsMediaBridge {
    pub fn new(creation_ctx: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = unbounded::<OsMediaCommand>();

        #[cfg(target_os = "windows")]
        let hwnd = creation_ctx.window_handle().ok().and_then(|wh| {
            if let raw_window_handle::RawWindowHandle::Win32(handle) = wh.as_raw() {
                Some(handle.hwnd.get() as *mut std::ffi::c_void)
            } else {
                None
            }
        });
        #[cfg(not(target_os = "windows"))]
        let hwnd = {
            let _ = creation_ctx;
            None
        };

        let config = PlatformConfig {
            dbus_name: "com.txplayer.player",
            display_name: "TxPlayer",
            hwnd,
        };

        let controls = MediaControls::new(config).ok().and_then(|mut mc| {
            let sender = tx.clone();
            if mc
                .attach(move |event: MediaControlEvent| {
                    let mapped = match event {
                        MediaControlEvent::Play => Some(OsMediaCommand::Play),
                        MediaControlEvent::Pause => Some(OsMediaCommand::Pause),
                        MediaControlEvent::Toggle => Some(OsMediaCommand::Toggle),
                        MediaControlEvent::Next => Some(OsMediaCommand::Next),
                        MediaControlEvent::Previous => Some(OsMediaCommand::Previous),
                        MediaControlEvent::Stop => Some(OsMediaCommand::Stop),
                        MediaControlEvent::SetPosition(MediaPosition(pos)) => {
                            Some(OsMediaCommand::SeekAbs(pos.as_secs_f64()))
                        }
                        MediaControlEvent::SeekBy(dir, delta) => {
                            let secs = delta.as_secs_f64();
                            Some(OsMediaCommand::SeekRel(match dir {
                                SeekDirection::Forward => secs,
                                SeekDirection::Backward => -secs,
                            }))
                        }
                        _ => None,
                    };
                    if let Some(cmd) = mapped {
                        let _ = sender.send(cmd);
                    }
                })
                .is_ok()
            {
                Some(mc)
            } else {
                None
            }
        });

        Self {
            controls,
            rx,
            last_track_key: None,
            last_state: None,
        }
    }

    pub fn try_event(&self) -> Option<OsMediaCommand> {
        self.rx.try_recv().ok()
    }

    pub fn sync_metadata(
        &mut self,
        track_id: u64,
        title: &str,
        artist: &str,
        album: &str,
        duration: Option<f64>,
        cover_path: Option<&Path>,
    ) {
        let key = (track_id, title.to_string(), artist.to_string());
        if self.last_track_key.as_ref() == Some(&key) {
            return;
        }
        self.last_track_key = Some(key);
        let Some(mc) = &mut self.controls else {
            return;
        };
        let cover_url = cover_path.map(|p| format!("file://{}", p.display()));
        let _ = mc.set_metadata(MediaMetadata {
            title: Some(title),
            artist: Some(artist),
            album: Some(album),
            duration: duration.filter(|d| *d > 0.0).map(Duration::from_secs_f64),
            cover_url: cover_url.as_deref(),
        });
    }

    pub fn sync_playback(&mut self, loaded: bool, playing: bool, position_secs: f64) {
        let state = (loaded, playing);
        if self.last_state == Some(state) {
            return;
        }
        self.last_state = Some(state);
        if !loaded {
            self.last_track_key = None;
        }
        let Some(mc) = &mut self.controls else {
            return;
        };
        let progress = Some(MediaPosition(Duration::from_secs_f64(
            position_secs.max(0.0),
        )));
        let pb = if !loaded {
            MediaPlayback::Stopped
        } else if playing {
            MediaPlayback::Playing { progress }
        } else {
            MediaPlayback::Paused { progress }
        };
        let _ = mc.set_playback(pb);
    }
}
