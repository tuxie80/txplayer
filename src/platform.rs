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
        if path.is_dir() {
            let _ = Command::new("open").arg(path).spawn();
        } else {
            let _ = Command::new("open").arg("-R").arg(path).spawn();
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        // Normalize slashes and strip any verbatim `\\?\` prefix that breaks explorer.exe /select
        let raw = path.to_string_lossy().replace('/', "\\");
        let clean = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
        if path.is_dir() {
            let _ = Command::new("explorer.exe").arg(clean).spawn();
        } else {
            // `raw_arg` passes `/select,"C:\path with spaces\file.flac"` verbatim so
            // explorer.exe selects the exact file instead of misparsing quoted `/select,`.
            let _ = Command::new("explorer.exe")
                .raw_arg(format!("/select,\"{clean}\""))
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

/// Opens a file (such as an album PDF booklet) with the OS default application.
pub fn open_file_with_default_app(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}
