use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::metadata::{Track, cue_track_id, read_track};

#[derive(Debug, Clone)]
pub struct CueEntry {
    pub file: String,
    pub track_number: u32,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub start_secs: f64,
    pub end_secs: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ParsedCue {
    pub album: Option<String>,
    pub album_performer: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub entries: Vec<CueEntry>,
}

/// Parses a standard `.cue` sheet (supporting UTF-8 / BOM, quoted fields, REM DATE/GENRE,
/// multiple `FILE` blocks, and `INDEX 01 MM:SS:FF` at 75 CD frames/sec).
pub fn parse_cue_text(text: &str) -> ParsedCue {
    let cleaned = text.trim_start_matches('\u{feff}');
    let mut album: Option<String> = None;
    let mut album_performer: Option<String> = None;
    let mut year: Option<String> = None;
    let mut genre: Option<String> = None;

    let mut current_file: Option<String> = None;
    let mut entries: Vec<CueEntry> = Vec::new();
    let mut cur_track: Option<CueEntry> = None;

    let flush_track = |cur: &mut Option<CueEntry>, list: &mut Vec<CueEntry>| {
        if let Some(entry) = cur.take() {
            list.push(entry);
        }
    };

    for raw_line in cleaned.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let upper = line.to_ascii_uppercase();

        if upper.starts_with("REM ") {
            let rest = line[4..].trim();
            let rest_up = rest.to_ascii_uppercase();
            if rest_up.starts_with("DATE ") {
                let val = unquote(rest[5..].trim());
                if !val.is_empty() {
                    year = Some(val);
                }
            } else if rest_up.starts_with("GENRE ") {
                let val = unquote(rest[6..].trim());
                if !val.is_empty() {
                    genre = Some(val);
                }
            }
            continue;
        }

        if upper.starts_with("FILE ") {
            flush_track(&mut cur_track, &mut entries);
            if let Some(fname) = extract_file_name(&line[5..]) {
                current_file = Some(fname);
            }
            continue;
        }

        if upper.starts_with("TRACK ") {
            flush_track(&mut cur_track, &mut entries);
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[2].eq_ignore_ascii_case("AUDIO") {
                let num = parts[1].parse::<u32>().unwrap_or(entries.len() as u32 + 1);
                if let Some(ref file) = current_file {
                    cur_track = Some(CueEntry {
                        file: file.clone(),
                        track_number: num,
                        title: None,
                        performer: None,
                        start_secs: 0.0,
                        end_secs: None,
                    });
                }
            }
            continue;
        }

        if upper.starts_with("TITLE ") {
            let val = unquote(line[6..].trim());
            if !val.is_empty() {
                if let Some(ref mut tr) = cur_track {
                    tr.title = Some(val);
                } else {
                    album = Some(val);
                }
            }
            continue;
        }

        if upper.starts_with("PERFORMER ") {
            let val = unquote(line[10..].trim());
            if !val.is_empty() {
                if let Some(ref mut tr) = cur_track {
                    tr.performer = Some(val);
                } else {
                    album_performer = Some(val);
                }
            }
            continue;
        }

        if upper.starts_with("INDEX ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3
                && (parts[1] == "01" || parts[1] == "1")
                && let Some(secs) = parse_cue_timestamp(parts[2])
                && let Some(ref mut tr) = cur_track
            {
                tr.start_secs = secs;
            }
        }
    }
    flush_track(&mut cur_track, &mut entries);

    // Fill end_secs for each entry pointing to the same underlying file
    for i in 0..entries.len() {
        if i + 1 < entries.len() && entries[i].file == entries[i + 1].file {
            let next_start = entries[i + 1].start_secs;
            if next_start > entries[i].start_secs {
                entries[i].end_secs = Some(next_start);
            }
        }
    }

    ParsedCue {
        album,
        album_performer,
        year,
        genre,
        entries,
    }
}

/// Expands a `.cue` sheet on disk into virtual `Track` records pointing at the underlying audio file(s).
/// Also returns the set of canonicalized underlying audio paths referenced by this CUE sheet so the
/// directory scanner can skip adding the monolithic disc image as a duplicate track.
pub fn read_cue_tracks(
    cue_path: &Path,
    cover_dir: &Path,
) -> Result<(Vec<Track>, HashSet<PathBuf>)> {
    let raw = std::fs::read(cue_path)
        .with_context(|| format!("Reading CUE sheet {}", cue_path.display()))?;
    let text = String::from_utf8_lossy(&raw);
    let parsed = parse_cue_text(&text);
    let cue_dir = cue_path.parent().unwrap_or_else(|| Path::new("."));

    let mut tracks = Vec::new();
    let mut referenced_files = HashSet::new();
    let mut base_cache: std::collections::HashMap<PathBuf, Track> =
        std::collections::HashMap::new();

    for entry in &parsed.entries {
        let Some(audio_path) = resolve_cue_audio_file(cue_dir, &entry.file) else {
            continue;
        };
        referenced_files.insert(audio_path.clone());

        let base = if let Some(cached) = base_cache.get(&audio_path) {
            cached.clone()
        } else {
            let Ok(read) = read_track(&audio_path, cover_dir) else {
                continue;
            };
            base_cache.insert(audio_path.clone(), read.clone());
            read
        };

        let total_dur = base.duration;
        let end = entry.end_secs.or(total_dur);
        let track_dur = end.map(|e| (e - entry.start_secs).max(0.0));

        let mut tr = base;
        tr.id = cue_track_id(&audio_path, entry.track_number, entry.start_secs);
        tr.title = entry
            .title
            .clone()
            .unwrap_or_else(|| format!("Track {:02}", entry.track_number));
        tr.artist = entry
            .performer
            .clone()
            .or_else(|| parsed.album_performer.clone())
            .unwrap_or(tr.artist);
        if let Some(ref alb) = parsed.album {
            tr.album = alb.clone();
        }
        if parsed.album_performer.is_some() {
            tr.album_artist = parsed.album_performer.clone();
        }
        tr.track_number = Some(entry.track_number);
        if parsed.year.is_some() {
            tr.year = parsed.year.clone();
        }
        if parsed.genre.is_some() {
            tr.genre = parsed.genre.clone();
        }
        tr.duration = track_dur;
        tr.cue_start = Some(entry.start_secs);
        tr.cue_end = entry.end_secs;
        tracks.push(tr);
    }

    Ok((tracks, referenced_files))
}

fn resolve_cue_audio_file(cue_dir: &Path, filename: &str) -> Option<PathBuf> {
    let direct = cue_dir.join(filename);
    if direct.is_file() {
        return Some(direct);
    }
    // Common real-world case: CUE references `Album.wav` while the user transcoded to `Album.flac`
    let stem = Path::new(filename).file_stem()?.to_string_lossy();
    for ext in ["flac", "wv", "wav", "m4a", "aiff", "aif", "mp3", "ogg"] {
        let candidate = cue_dir.join(format!("{stem}.{ext}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn extract_file_name(rest: &str) -> Option<String> {
    let trimmed = rest.trim();
    if let Some(stripped) = trimmed.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        trimmed.split_whitespace().next().map(str::to_string)
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].trim().to_string()
    } else {
        s.to_string()
    }
}

/// Parses `MM:SS:FF` (75 frames per second) into seconds.
pub fn parse_cue_timestamp(ts: &str) -> Option<f64> {
    let mut parts = ts.trim().split(':');
    let mins: f64 = parts.next()?.parse().ok()?;
    let secs: f64 = parts.next()?.parse().ok()?;
    let frames: f64 = parts.next().unwrap_or("0").parse().ok()?;
    Some(mins * 60.0 + secs + frames / 75.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_classical_cue_sheet() {
        let cue = r#"
REM GENRE Classical
REM DATE 1996
PERFORMER "Marc-André Hamelin"
TITLE "Scriabin: Complete Piano Sonatas"
FILE "CDImage.flac" WAVE
  TRACK 01 AUDIO
    TITLE "Sonata No. 1 - I. Allegro con fuoco"
    INDEX 01 00:00:00
  TRACK 02 AUDIO
    TITLE "Sonata No. 1 - II. Adagio"
    INDEX 01 08:07:37
  TRACK 03 AUDIO
    TITLE "Sonata No. 1 - III. Presto"
    INDEX 01 13:03:00
"#;
        let parsed = parse_cue_text(cue);
        assert_eq!(
            parsed.album.as_deref(),
            Some("Scriabin: Complete Piano Sonatas")
        );
        assert_eq!(parsed.album_performer.as_deref(), Some("Marc-André Hamelin"));
        assert_eq!(parsed.year.as_deref(), Some("1996"));
        assert_eq!(parsed.genre.as_deref(), Some("Classical"));
        assert_eq!(parsed.entries.len(), 3);
        assert!((parsed.entries[0].start_secs - 0.0).abs() < 1e-3);
        assert!((parsed.entries[0].end_secs.unwrap() - (8.0 * 60.0 + 7.0 + 37.0 / 75.0)).abs() < 1e-3);
        assert!((parsed.entries[1].end_secs.unwrap() - (13.0 * 60.0 + 3.0)).abs() < 1e-3);
        assert!(parsed.entries[2].end_secs.is_none());
    }
}
