use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::metadata::Track;

/// Exports a list of tracks to an extended `.m3u8` (UTF-8) or `.pls` playlist file.
pub fn export_playlist(dest: &Path, tracks: &[&Track]) -> Result<()> {
    let ext = dest
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("m3u8")
        .to_ascii_lowercase();

    let content = if ext == "pls" {
        format_pls(tracks)
    } else {
        format_m3u8(tracks)
    };
    std::fs::write(dest, content)
        .with_context(|| format!("Writing playlist to {}", dest.display()))
}

/// Parses an `.m3u`, `.m3u8`, or `.pls` file into resolved audio file paths.
pub fn import_playlist_paths(src: &Path) -> Result<Vec<PathBuf>> {
    let raw = std::fs::read(src).with_context(|| format!("Reading playlist {}", src.display()))?;
    let text = String::from_utf8_lossy(&raw);
    let base_dir = src.parent().unwrap_or_else(|| Path::new("."));
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let raw_entries = if ext == "pls" || text.trim_start().starts_with("[playlist]") {
        parse_pls_entries(&text)
    } else {
        parse_m3u_entries(&text)
    };

    let mut out = Vec::with_capacity(raw_entries.len());
    for entry in raw_entries {
        let trimmed = entry.trim();
        if trimmed.is_empty() || trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            continue;
        }
        let cleaned = trimmed.strip_prefix("file://").unwrap_or(trimmed);
        let candidate = PathBuf::from(cleaned);
        let resolved = if candidate.is_absolute() {
            candidate
        } else {
            base_dir.join(candidate)
        };
        if resolved.exists() {
            out.push(resolved);
        }
    }
    Ok(out)
}

pub fn format_m3u8(tracks: &[&Track]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for t in tracks {
        let secs = t.duration.map(|d| d.round() as i64).unwrap_or(-1);
        out.push_str(&format!("#EXTINF:{secs},{} - {}\n", t.artist, t.title));
        out.push_str(&format!("{}\n", t.path.display()));
    }
    out
}

pub fn format_pls(tracks: &[&Track]) -> String {
    let mut out = String::from("[playlist]\n");
    for (idx, t) in tracks.iter().enumerate() {
        let n = idx + 1;
        let secs = t.duration.map(|d| d.round() as i64).unwrap_or(-1);
        out.push_str(&format!("File{n}={}\n", t.path.display()));
        out.push_str(&format!("Title{n}={} - {}\n", t.artist, t.title));
        out.push_str(&format!("Length{n}={secs}\n"));
    }
    out.push_str(&format!("NumberOfEntries={}\nVersion=2\n", tracks.len()));
    out
}

pub fn parse_m3u_entries(text: &str) -> Vec<String> {
    text.trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

pub fn parse_pls_entries(text: &str) -> Vec<String> {
    let mut numbered: Vec<(usize, String)> = Vec::new();
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim();
        if let Some((k, v)) = line.split_once('=') {
            let kl = k.trim().to_ascii_lowercase();
            if let Some(num_str) = kl.strip_prefix("file")
                && let Ok(idx) = num_str.parse::<usize>()
            {
                numbered.push((idx, v.trim().to_string()));
            }
        }
    }
    numbered.sort_by_key(|(idx, _)| *idx);
    numbered.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_m3u_and_pls_formats() {
        let m3u = "#EXTM3U\n#EXTINF:240,Chopin - Nocturne\n/music/nocturne.flac\n\nrelative/prelude.mp3\n";
        assert_eq!(
            parse_m3u_entries(m3u),
            vec!["/music/nocturne.flac", "relative/prelude.mp3"]
        );

        let pls = "[playlist]\nFile2=/b.flac\nTitle2=B\nFile1=/a.flac\nTitle1=A\nNumberOfEntries=2\nVersion=2\n";
        assert_eq!(parse_pls_entries(pls), vec!["/a.flac", "/b.flac"]);
    }
}
