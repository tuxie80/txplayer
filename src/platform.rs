use std::path::Path;
use std::process::Command;

#[cfg(target_os = "macos")]
pub const AUDIO_BACKEND_NAME: &str = "CoreAudio";
#[cfg(target_os = "windows")]
pub const AUDIO_BACKEND_NAME: &str = "WASAPI";
#[cfg(all(unix, not(target_os = "macos")))]
pub const AUDIO_BACKEND_NAME: &str = "ALSA / PipeWire";

#[cfg(target_os = "macos")]
pub const FILE_MANAGER_NAME: &str = "Finder";
#[cfg(target_os = "windows")]
pub const FILE_MANAGER_NAME: &str = "Explorer";
#[cfg(all(unix, not(target_os = "macos")))]
pub const FILE_MANAGER_NAME: &str = "Files";

#[cfg(target_os = "macos")]
pub const MOD_SHORT: &str = "⌘";
#[cfg(not(target_os = "macos"))]
pub const MOD_SHORT: &str = "Ctrl+";

/// Reveals a file or directory in the OS native file manager (Finder / Explorer / xdg-open).
pub fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        if path.is_dir() {
            let _ = Command::new("explorer").arg(path).spawn();
        } else {
            let _ = Command::new("explorer")
                .arg(format!("/select,{}", path.display()))
                .spawn();
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        let _ = Command::new("xdg-open").arg(target).spawn();
    }
}
