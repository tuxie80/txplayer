//! CLI file-association launch parsing & single-instance loopback IPC.
//!
//! When a user double-clicks one or more associated audio files in Windows Explorer
//! (or opens files via Linux `.desktop` / CLI), the OS passes those file paths as
//! positional arguments to `txplayer`.
//!
//! - If another TxPlayer instance is already running, the new process forwards the
//!   paths over localhost loopback (`127.0.0.1`) to the running instance and exits
//!   immediately so no duplicate windows spawn.
//! - Windows Explorer multi-file open spawns one process per selected file in rapid
//!   succession; forwarding + burst coalescing in `TxPlayerApp` collects every
//!   selected file into the active queue while starting playback on track #1 right away.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, unbounded};
use eframe::egui;

const HANDSHAKE_PREFIX: &str = "TXPLAYER_OPEN_V1 ";
const END_MARKER: &str = "END";

/// Returns the default single-instance discovery file path (`<data_dir>/instance.port`).
pub fn instance_port_file() -> PathBuf {
    crate::library::data_directory().join("instance.port")
}

/// Extracts audio / playlist / folder paths passed on the command line (both positional
/// OS file-association arguments and explicit `--play <path>`), ignoring known flags.
pub fn extract_cli_launch_paths(args: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut i = 1usize;
    let mut after_double_dash = false;

    while i < args.len() {
        let arg = &args[i];
        if !after_double_dash {
            if arg == "--" {
                after_double_dash = true;
                i += 1;
                continue;
            }
            if arg == "--play" {
                if let Some(val) = args.get(i + 1)
                    && let Some(p) = normalize_cli_path(val)
                {
                    out.push(p);
                }
                i += 2;
                continue;
            }
            if matches!(
                arg.as_str(),
                "--theme" | "--vis-mode" | "--open-settings" | "--screenshot"
            ) {
                i += 2;
                continue;
            }
            if matches!(
                arg.as_str(),
                "--compact"
                    | "--full"
                    | "--demo-piano"
                    | "--demo-spectrogram"
                    | "--open-insights"
                    | "--open-help"
                    | "--open-abx"
            ) || arg.starts_with("-psn_")
            {
                i += 1;
                continue;
            }
            if arg.starts_with("--") {
                i += 1;
                continue;
            }
        }

        if let Some(p) = normalize_cli_path(arg) {
            out.push(p);
        }
        i += 1;
    }

    out
}

fn normalize_cli_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("file://") {
        let stripped = rest.strip_prefix("localhost").unwrap_or(rest);
        let decoded = percent_decode(stripped);
        // Strip leading slash before Windows drive letters like `/C:/Music/track.flac`
        let bytes = decoded.as_bytes();
        let cleaned = if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
            &decoded[1..]
        } else {
            decoded.as_str()
        };
        if cleaned.is_empty() {
            None
        } else {
            Some(PathBuf::from(cleaned))
        }
    } else {
        Some(PathBuf::from(trimmed))
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex, 16)
        {
            out.push(val);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encode_ipc_request(token: &str, paths: &[PathBuf]) -> String {
    let mut payload = format!("{HANDSHAKE_PREFIX}{token}\n");
    for path in paths {
        // Resolve relative paths against this child process's CWD before forwarding
        let resolved = if path.is_relative() {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.clone())
        } else {
            path.clone()
        };
        let s = resolved.to_string_lossy().replace(['\r', '\n'], "");
        if !s.is_empty() {
            payload.push_str(&s);
            payload.push('\n');
        }
    }
    payload.push_str(END_MARKER);
    payload.push('\n');
    payload
}

fn process_ipc_request<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    expected_header: &str,
) -> Option<Vec<PathBuf>> {
    let mut header = String::new();
    if reader.read_line(&mut header).is_err() || header.trim() != expected_header {
        return None;
    }
    let mut paths = Vec::new();
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap_or(0) > 0 {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == END_MARKER {
            break;
        }
        if !trimmed.is_empty() {
            paths.push(PathBuf::from(trimmed));
        }
        line.clear();
    }
    let _ = writer.write_all(b"OK\n");
    let _ = writer.flush();
    Some(paths)
}

/// Attempts to forward `paths` (or a window-focus ping if empty) to an already-running
/// TxPlayer instance. Returns `true` if the running instance acknowledged receipt.
pub fn try_forward_to_running_instance(port_file: &Path, paths: &[PathBuf]) -> bool {
    let Ok(text) = fs::read_to_string(port_file) else {
        return false;
    };
    let mut lines = text.lines();
    let Some(port) = lines.next().and_then(|s| s.trim().parse::<u16>().ok()) else {
        return false;
    };
    let Some(token) = lines.next().map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(250)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(400)));

    let payload = encode_ipc_request(token, paths);
    if stream.write_all(payload.as_bytes()).is_err() || stream.flush().is_err() {
        return false;
    }

    let mut resp = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut resp).is_ok() && resp.trim() == "OK"
}

/// Loopback IPC server owned by the primary TxPlayer instance.
pub struct SingleInstanceServer {
    pub rx: Receiver<Vec<PathBuf>>,
    port_file: PathBuf,
    token: String,
    egui_ctx: Arc<Mutex<Option<egui::Context>>>,
}

impl SingleInstanceServer {
    pub fn start(port_file: PathBuf) -> Option<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        let token = format!("{:016x}", rand::random::<u64>());

        if let Some(parent) = port_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let content = format!("{port}\n{token}\n");
        let tmp = port_file.with_extension("port.tmp");
        if fs::write(&tmp, &content).is_ok() && fs::rename(&tmp, &port_file).is_err() {
            let _ = fs::write(&port_file, &content);
            let _ = fs::remove_file(&tmp);
        } else if !port_file.exists() {
            let _ = fs::write(&port_file, &content);
        }

        let (tx, rx) = unbounded::<Vec<PathBuf>>();
        let egui_ctx: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
        let thread_ctx = Arc::clone(&egui_ctx);
        let expected_header = format!("{HANDSHAKE_PREFIX}{token}");

        thread::Builder::new()
            .name("txplayer-ipc".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
                    let Ok(mut writer) = stream.try_clone() else {
                        continue;
                    };
                    let mut reader = BufReader::new(stream);
                    let Some(paths) =
                        process_ipc_request(&mut reader, &mut writer, &expected_header)
                    else {
                        continue;
                    };

                    if tx.send(paths).is_err() {
                        break;
                    }
                    if let Ok(guard) = thread_ctx.lock()
                        && let Some(ctx) = guard.as_ref()
                    {
                        ctx.request_repaint();
                    }
                }
            })
            .ok()?;

        Some(Self {
            rx,
            port_file,
            token,
            egui_ctx,
        })
    }

    pub fn attach_egui_ctx(&self, ctx: egui::Context) {
        if let Ok(mut guard) = self.egui_ctx.lock() {
            *guard = Some(ctx);
        }
    }
}

impl Drop for SingleInstanceServer {
    fn drop(&mut self) {
        if let Ok(text) = fs::read_to_string(&self.port_file)
            && text.contains(&self.token)
        {
            let _ = fs::remove_file(&self.port_file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn extracts_positional_files_and_skips_known_flags() {
        let args = vec![
            "txplayer.exe".into(),
            "--theme".into(),
            "solarized-dark".into(),
            "--compact".into(),
            r"C:\Music\01 Nocturne.flac".into(),
            "file:///C:/Music/02%20Etude.flac".into(),
            "--play".into(),
            r"C:\Music\03 Prelude.mp3".into(),
        ];
        let paths = extract_cli_launch_paths(&args);
        assert_eq!(
            paths,
            vec![
                PathBuf::from(r"C:\Music\01 Nocturne.flac"),
                PathBuf::from("C:/Music/02 Etude.flac"),
                PathBuf::from(r"C:\Music\03 Prelude.mp3"),
            ]
        );
    }

    #[test]
    fn ipc_protocol_roundtrips_multiple_paths_and_rejects_wrong_token() {
        let token = "deadbeef12345678";
        let expected_header = format!("{HANDSHAKE_PREFIX}{token}");
        let sent = vec![
            PathBuf::from("/tmp/01 track.flac"),
            PathBuf::from("/tmp/02 track.flac"),
        ];
        let payload = encode_ipc_request(token, &sent);

        let mut reader = Cursor::new(payload.as_bytes());
        let mut resp = Vec::new();
        let parsed = process_ipc_request(&mut reader, &mut resp, &expected_header)
            .expect("valid token should succeed");
        assert_eq!(parsed, sent);
        assert_eq!(String::from_utf8_lossy(&resp), "OK\n");

        // Wrong token must be rejected
        let mut bad_reader = Cursor::new(payload.as_bytes());
        let mut bad_resp = Vec::new();
        assert!(
            process_ipc_request(&mut bad_reader, &mut bad_resp, "TXPLAYER_OPEN_V1 wrong").is_none()
        );
        assert!(bad_resp.is_empty());
    }
}
