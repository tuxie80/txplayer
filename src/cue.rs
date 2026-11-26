use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::dsd::is_raw_cd_bin;
use crate::metadata::{Track, cue_track_id, read_track};

#[derive(Debug, Clone)]
pub struct CueEntry {
    pub file: String,
    pub track_number: u32,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub start_secs: f64,
    pub end_secs: Option<f64>,
    pub big_endian_bin: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedCue {
    pub album: Option<String>,
    pub album_performer: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub disc_number: Option<u32>,
    pub entries: Vec<CueEntry>,
}

/// Decodes `.cue` raw bytes with full support for UTF-8 (with/without BOM), UTF-16 LE/BE,
/// Windows-1250 (Central European: Czech, Slovak, Polish, Hungarian), Windows-1251 (Cyrillic),
/// and Windows-1252 (Western European) — zero external dependencies!
pub fn decode_cue_bytes(raw: &[u8]) -> String {
    if raw.starts_with(&[0xEF, 0xBB, 0xBF])
        && let Ok(s) = std::str::from_utf8(&raw[3..])
    {
        return s.to_string();
    }
    if raw.starts_with(&[0xFF, 0xFE]) {
        return decode_utf16_bytes(&raw[2..], false);
    }
    if raw.starts_with(&[0xFE, 0xFF]) {
        return decode_utf16_bytes(&raw[2..], true);
    }
    if let Ok(s) = std::str::from_utf8(raw) {
        return s.to_string();
    }

    // Heuristic between CP1251 (Cyrillic) vs CP1250 (Central European) vs CP1252 (Western)
    let mut cyr_score = 0usize;
    let mut ce_score = 0usize;
    let mut non_ascii = 0usize;
    for &b in raw {
        if b >= 0x80 {
            non_ascii += 1;
            // Cyrillic lowercase/uppercase concentrates heavily in 0xC0..=0xFF (especially 0xE0..=0xFF)
            if matches!(b, 0xE0..=0xFF | 0xB8 | 0xA8) {
                cyr_score += 1;
            }
            // Signature Windows-1250 Central European bytes (ě, š, č, ř, ž, ů, ň, ť, ď, ł, ś, ć)
            if matches!(
                b,
                0x8A | 0x8C | 0x8D | 0x8E | 0x8F | 0x9A | 0x9C | 0x9D | 0x9E | 0x9F | 0xB3 | 0xB9
                    | 0xBC | 0xBE | 0xE8 | 0xEC | 0xF2 | 0xF8 | 0xF9
            ) {
                ce_score += 2;
            }
        }
    }

    let table: &[u16; 128] = if non_ascii >= 4 && cyr_score * 100 / non_ascii.max(1) >= 72 && ce_score < cyr_score / 2 {
        &CP1251_HIGH
    } else if ce_score > 0 {
        &CP1250_HIGH
    } else {
        &CP1252_HIGH
    };

    let mut out = String::with_capacity(raw.len());
    for &b in raw {
        if b < 0x80 {
            out.push(b as char);
        } else {
            let cp = table[(b - 0x80) as usize];
            out.push(char::from_u32(cp as u32).unwrap_or('\u{FFFD}'));
        }
    }
    out
}

fn decode_utf16_bytes(bytes: &[u8], be: bool) -> String {
    let mut u16s = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let code = if be {
            u16::from_be_bytes([pair[0], pair[1]])
        } else {
            u16::from_le_bytes([pair[0], pair[1]])
        };
        u16s.push(code);
    }
    String::from_utf16_lossy(&u16s)
}

/// Parses a standard `.cue` sheet (supporting UTF-8 / BOM, quoted fields, REM DATE/GENRE/DISCNUMBER,
/// multiple `FILE` blocks including `BINARY`/`MOTOROLA`, and `INDEX 01 MM:SS:FF` at 75 CD frames/sec).
pub fn parse_cue_text(text: &str) -> ParsedCue {
    let cleaned = text.trim_start_matches('\u{feff}');
    let mut album: Option<String> = None;
    let mut album_performer: Option<String> = None;
    let mut year: Option<String> = None;
    let mut genre: Option<String> = None;
    let mut disc_number: Option<u32> = None;

    let mut current_file: Option<String> = None;
    let mut current_big_endian = false;
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
            } else if rest_up.starts_with("DISCNUMBER ") {
                let val = unquote(rest[11..].trim());
                if let Some(num) = val.split('/').next().and_then(|s| s.trim().parse::<u32>().ok()) {
                    disc_number = Some(num);
                }
            } else if rest_up.starts_with("COMPOSER ") && album_performer.is_none() {
                let val = unquote(rest[9..].trim());
                if !val.is_empty() {
                    album_performer = Some(val);
                }
            }
            continue;
        }

        if upper.starts_with("FILE ") {
            flush_track(&mut cur_track, &mut entries);
            if let Some(fname) = extract_file_name(&line[5..]) {
                current_big_endian = upper.ends_with(" MOTOROLA");
                current_file = Some(fname);
            }
            continue;
        }

        if upper.starts_with("TRACK ") {
            flush_track(&mut cur_track, &mut entries);
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[2].eq_ignore_ascii_case("AUDIO") {
                let num = parts[1].parse::<u32>().unwrap_or(entries.len() as u32 + 1);
                let file = current_file
                    .clone()
                    .unwrap_or_else(|| "embedded".to_string());
                cur_track = Some(CueEntry {
                    file,
                    track_number: num,
                    title: None,
                    performer: None,
                    start_secs: 0.0,
                    end_secs: None,
                    big_endian_bin: current_big_endian,
                });
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

        if upper.starts_with("SONGWRITER ") && cur_track.is_none() && album_performer.is_none() {
            let val = unquote(line[11..].trim());
            if !val.is_empty() {
                album_performer = Some(val);
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
        disc_number,
        entries,
    }
}

/// Expands a `.cue` sheet on disk into virtual `Track` records pointing at the underlying audio file(s).
/// Also returns the set of underlying audio paths referenced by this CUE sheet so the directory scanner
/// skips adding the monolithic disc image as a duplicate track.
pub fn read_cue_tracks(
    cue_path: &Path,
    cover_dir: &Path,
) -> Result<(Vec<Track>, HashSet<PathBuf>)> {
    let raw = std::fs::read(cue_path)
        .with_context(|| format!("Reading CUE sheet {}", cue_path.display()))?;
    let text = decode_cue_bytes(&raw);
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
            let read = if is_raw_cd_bin(&audio_path) {
                read_raw_cd_bin_base(&audio_path, &parsed)
            } else {
                read_track(&audio_path, cover_dir)
            };
            let Ok(read) = read else {
                continue;
            };
            base_cache.insert(audio_path.clone(), read.clone());
            read
        };

        tracks.push(build_virtual_cue_track(base, &audio_path, &parsed, entry));
    }

    Ok((tracks, referenced_files))
}

/// Checks whether a standalone `.flac` file contains an embedded CUESHEET (either a Vorbis `CUESHEET`
/// comment tag or a binary FLAC `METADATA_BLOCK_CUESHEET` block 5) with 2+ tracks, and expands it cleanly.
pub fn read_embedded_flac_cue_tracks(flac_path: &Path, cover_dir: &Path) -> Option<Vec<Track>> {
    if !flac_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("flac"))
    {
        return None;
    }
    let parsed = extract_embedded_flac_cue(flac_path)?;
    if parsed.entries.len() < 2 {
        return None;
    }
    let base = read_track(flac_path, cover_dir).ok()?;
    let tracks = parsed
        .entries
        .iter()
        .map(|entry| build_virtual_cue_track(base.clone(), flac_path, &parsed, entry))
        .collect();
    Some(tracks)
}

fn build_virtual_cue_track(
    base: Track,
    audio_path: &Path,
    parsed: &ParsedCue,
    entry: &CueEntry,
) -> Track {
    let total_dur = base.duration;
    let end = entry.end_secs.or(total_dur);
    let track_dur = end.map(|e| (e - entry.start_secs).max(0.0));

    let mut tr = base;
    tr.id = cue_track_id(audio_path, entry.track_number, entry.start_secs);
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
    if parsed.disc_number.is_some() {
        tr.disc_number = parsed.disc_number;
    }
    if parsed.year.is_some() {
        tr.year = parsed.year.clone();
    }
    if parsed.genre.is_some() {
        tr.genre = parsed.genre.clone();
    }
    tr.duration = track_dur;
    tr.cue_start = Some(entry.start_secs);
    tr.cue_end = entry.end_secs;
    if entry.big_endian_bin && is_raw_cd_bin(audio_path) {
        tr.codec = "CD-BE".into();
    }
    tr
}

fn read_raw_cd_bin_base(path: &Path, parsed: &ParsedCue) -> Result<Track> {
    let bytes = std::fs::metadata(path)
        .with_context(|| format!("Reading raw CD image {}", path.display()))?
        .len();
    let dur = (bytes / 4) as f64 / 44_100.0;
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("CD Image")
        .to_string();
    let cover_path = crate::metadata::find_sidecar_cover(path);
    Ok(Track {
        id: cue_track_id(path, 0, 0.0),
        path: path.to_path_buf(),
        title: stem.clone(),
        artist: parsed
            .album_performer
            .clone()
            .unwrap_or_else(|| "Unknown Artist".into()),
        album: parsed.album.clone().unwrap_or(stem),
        album_artist: parsed.album_performer.clone(),
        track_number: None,
        disc_number: parsed.disc_number,
        year: parsed.year.clone(),
        genre: parsed.genre.clone(),
        duration: Some(dur),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
        channels: Some(2),
        codec: "PCM".into(),
        nominal_bitrate: Some(1411),
        cover_path,
        cue_start: None,
        cue_end: None,
        rg_track_gain_db: None,
        rg_track_peak: None,
        rg_album_gain_db: None,
        rg_album_peak: None,
        lyrics: None,
        play_count: 0,
        rating: 0,
        added_at: now_unix,
    })
}

/// Fast pure-Rust FLAC metadata block scanner for Vorbis `CUESHEET` tag or binary `METADATA_BLOCK_CUESHEET`.
fn extract_embedded_flac_cue(path: &Path) -> Option<ParsedCue> {
    let mut file = File::open(path).ok()?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;
    if &magic != b"fLaC" {
        return None;
    }

    let mut sample_rate = 44_100_u32;
    let mut vorbis_cue: Option<String> = None;
    let mut binary_cue: Option<Vec<(u32, f64)>> = None;

    loop {
        let mut hdr = [0u8; 4];
        file.read_exact(&mut hdr).ok()?;
        let is_last = (hdr[0] & 0x80) != 0;
        let block_type = hdr[0] & 0x7F;
        let block_len = ((hdr[1] as usize) << 16) | ((hdr[2] as usize) << 8) | (hdr[3] as usize);

        match block_type {
            // STREAMINFO (block 0): extract sample rate
            0 if block_len >= 18 => {
                let mut buf = vec![0u8; block_len];
                file.read_exact(&mut buf).ok()?;
                sample_rate = (((buf[10] as u32) << 12)
                    | ((buf[11] as u32) << 4)
                    | ((buf[12] as u32) >> 4))
                    .max(8_000);
            }
            // VORBIS_COMMENT (block 4): look for embedded `CUESHEET=` tag
            4 if block_len <= 4 * 1024 * 1024 => {
                let mut buf = vec![0u8; block_len];
                file.read_exact(&mut buf).ok()?;
                if let Some(cs) = find_vorbis_cuesheet_tag(&buf) {
                    vorbis_cue = Some(cs);
                }
            }
            // CUESHEET binary block (block 5)
            5 if (396..=1024 * 1024).contains(&block_len) => {
                let mut buf = vec![0u8; block_len];
                file.read_exact(&mut buf).ok()?;
                binary_cue = parse_flac_binary_cuesheet(&buf, sample_rate);
            }
            _ => {
                file.seek(SeekFrom::Current(block_len as i64)).ok()?;
            }
        }
        if is_last {
            break;
        }
    }

    if let Some(cue_text) = vorbis_cue {
        let parsed = parse_cue_text(&cue_text);
        if parsed.entries.len() >= 2 {
            return Some(parsed);
        }
    }

    if let Some(tracks) = binary_cue
        && tracks.len() >= 2
    {
        let mut entries: Vec<CueEntry> = tracks
            .into_iter()
            .map(|(num, start)| CueEntry {
                file: "embedded".into(),
                track_number: num,
                title: None,
                performer: None,
                start_secs: start,
                end_secs: None,
                big_endian_bin: false,
            })
            .collect();
        for i in 0..entries.len() {
            if i + 1 < entries.len() && entries[i + 1].start_secs > entries[i].start_secs {
                entries[i].end_secs = Some(entries[i + 1].start_secs);
            }
        }
        return Some(ParsedCue {
            album: None,
            album_performer: None,
            year: None,
            genre: None,
            disc_number: None,
            entries,
        });
    }

    None
}

fn find_vorbis_cuesheet_tag(buf: &[u8]) -> Option<String> {
    if buf.len() < 8 {
        return None;
    }
    let vendor_len = u32::from_le_bytes(buf[0..4].try_into().ok()?) as usize;
    let mut pos = 4usize.checked_add(vendor_len)?;
    if pos + 4 > buf.len() {
        return None;
    }
    let count = u32::from_le_bytes(buf[pos..pos + 4].try_into().ok()?) as usize;
    pos += 4;
    for _ in 0..count.min(1024) {
        if pos + 4 > buf.len() {
            break;
        }
        let clen = u32::from_le_bytes(buf[pos..pos + 4].try_into().ok()?) as usize;
        pos += 4;
        if pos + clen > buf.len() {
            break;
        }
        let raw = &buf[pos..pos + clen];
        pos += clen;
        if let Some(eq) = raw.iter().position(|&b| b == b'=') {
            let key = &raw[..eq];
            if key.eq_ignore_ascii_case(b"CUESHEET") {
                return Some(decode_cue_bytes(&raw[eq + 1..]));
            }
        }
    }
    None
}

fn parse_flac_binary_cuesheet(buf: &[u8], sample_rate: u32) -> Option<Vec<(u32, f64)>> {
    // 128 catalog + 8 lead-in + 1 flags + 258 reserved = 395 bytes, then 1 byte num_tracks
    if buf.len() < 396 {
        return None;
    }
    let num_tracks = buf[395] as usize;
    let mut pos = 396usize;
    let mut out = Vec::new();
    for _ in 0..num_tracks {
        if pos + 36 > buf.len() {
            break;
        }
        let offset = u64::from_be_bytes(buf[pos..pos + 8].try_into().ok()?);
        let track_num = buf[pos + 8];
        let flags = buf[pos + 21];
        let is_audio = (flags & 0x80) == 0;
        let num_indices = buf[pos + 35] as usize;
        pos += 36;
        let mut idx01_offset = 0u64;
        for _ in 0..num_indices {
            if pos + 12 > buf.len() {
                break;
            }
            let rel_off = u64::from_be_bytes(buf[pos..pos + 8].try_into().ok()?);
            let idx_num = buf[pos + 8];
            if idx_num == 1 {
                idx01_offset = rel_off;
            }
            pos += 12;
        }
        // Lead-out track is numbered 170 (CD) or 255
        if is_audio && track_num > 0 && track_num < 100 {
            let secs = (offset + idx01_offset) as f64 / sample_rate.max(1) as f64;
            out.push((track_num as u32, secs));
        }
    }
    (!out.is_empty()).then_some(out)
}

fn resolve_cue_audio_file(cue_dir: &Path, filename: &str) -> Option<PathBuf> {
    let direct = cue_dir.join(filename);
    if direct.is_file() {
        return Some(direct);
    }
    // Case-insensitive exact match inside cue_dir (common for Windows-created CUE sheets on Linux/macOS)
    let lower_target = filename.to_ascii_lowercase();
    if let Ok(entries) = std::fs::read_dir(cue_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.to_ascii_lowercase() == lower_target)
            {
                return Some(p);
            }
        }
    }
    // Common real-world case: CUE references `Album.wav` while the user transcoded to `Album.flac` or `.dsf`
    let stem = Path::new(filename).file_stem()?.to_string_lossy();
    for ext in [
        "flac", "dsf", "dff", "wav", "bin", "m4a", "aiff", "aif", "mp3", "ogg",
    ] {
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

// Windows-1250 (Central European: Czech, Slovak, Polish, Hungarian, German) 0x80..=0xFF -> Unicode
#[rustfmt::skip]
const CP1250_HIGH: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0083, 0x201E, 0x2026, 0x2020, 0x2021,
    0x0088, 0x2030, 0x0160, 0x2039, 0x015A, 0x0164, 0x017D, 0x0179,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0098, 0x2122, 0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A,
    0x00A0, 0x02C7, 0x02D8, 0x0141, 0x00A4, 0x0104, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B,
    0x00B0, 0x00B1, 0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x0105, 0x015F, 0x00BB, 0x013D, 0x02DD, 0x013E, 0x017C,
    0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E,
    0x0110, 0x0143, 0x0147, 0x00D3, 0x00D4, 0x0150, 0x00D6, 0x00D7,
    0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7,
    0x010D, 0x00E9, 0x0119, 0x00EB, 0x011B, 0x00ED, 0x00EE, 0x010F,
    0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7,
    0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

// Windows-1251 (Cyrillic) 0x80..=0xFF -> Unicode
#[rustfmt::skip]
const CP1251_HIGH: [u16; 128] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021,
    0x20AC, 0x2030, 0x0409, 0x2039, 0x040A, 0x040C, 0x040B, 0x040F,
    0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x0098, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F,
    0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7,
    0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7,
    0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457,
    0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417,
    0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F,
    0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427,
    0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437,
    0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447,
    0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F,
];

// Windows-1252 (Western European) 0x80..=0xFF -> Unicode
#[rustfmt::skip]
const CP1252_HIGH: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7,
    0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7,
    0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x00D0, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7,
    0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7,
    0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_classical_cue_sheet() {
        let cue = r#"
REM GENRE Classical
REM DATE 1996
REM DISCNUMBER 2
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
        assert_eq!(parsed.disc_number, Some(2));
        assert_eq!(parsed.entries.len(), 3);
        assert!((parsed.entries[0].start_secs - 0.0).abs() < 1e-3);
        assert!((parsed.entries[0].end_secs.unwrap() - (8.0 * 60.0 + 7.0 + 37.0 / 75.0)).abs() < 1e-3);
    }

    #[test]
    fn decodes_legacy_cp1250_czech_cue_sheet() {
        // "Dvořák: Novosvětská" in Windows-1250 (ř = 0xF8, á = 0xE1, ě = 0xEC)
        let bytes: &[u8] = b"TITLE \"Dvo\xF8\xE1k: Novosv\xECtsk\xE1\"";
        let decoded = decode_cue_bytes(bytes);
        assert_eq!(decoded, "TITLE \"Dvořák: Novosvětská\"");
    }
}
