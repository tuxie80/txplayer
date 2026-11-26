use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, anyhow};

use crate::metadata::parse_gain_db;

#[derive(Debug, Clone)]
pub struct ArchiveInfo {
    pub codec: &'static str,
    pub sample_rate: u32,
    pub bit_depth: Option<u32>,
    pub channels: u16,
    pub duration_secs: Option<f64>,
    pub nominal_bitrate: Option<u32>,
}

#[derive(Debug, Default, Clone)]
pub struct ArchiveTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub composer: Option<String>,
    pub conductor: Option<String>,
    pub orchestra: Option<String>,
    pub work: Option<String>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub comment: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub rg_track_gain_db: Option<f32>,
    pub rg_track_peak: Option<f32>,
    pub rg_album_gain_db: Option<f32>,
    pub rg_album_peak: Option<f32>,
    pub artwork: Option<(Vec<u8>, String)>,
}

/// Returns true for audiophile archive codecs handled by TxPlayer's extended container engine:
/// WavPack (`.wv`), Monkey's Audio (`.ape`), and Ogg Opus (`.opus`).
pub fn is_archive_codec_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            e.eq_ignore_ascii_case("wv")
                || e.eq_ignore_ascii_case("ape")
                || e.eq_ignore_ascii_case("opus")
        })
}

/// Pure-Rust container & tag prober for `.wv` (WavPack), `.ape` (Monkey's Audio), and `.opus` (Ogg Opus).
pub fn probe_archive(path: &Path) -> Result<(ArchiveInfo, ArchiveTags)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let mut file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);

    match ext.as_str() {
        "wv" => probe_wavpack(&mut file, file_len),
        "ape" => probe_monkeys_audio(&mut file, file_len),
        "opus" => probe_opus(&mut file, file_len),
        _ => Err(anyhow!("Unsupported archive codec extension: .{ext}")),
    }
}

/// Pure-Rust WavPack (`wvpk`) block header + APEv2 tag parser.
fn probe_wavpack(file: &mut File, file_len: u64) -> Result<(ArchiveInfo, ArchiveTags)> {
    const WV_RATES: [u32; 15] = [
        6_000, 8_000, 9_600, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
        64_000, 88_200, 96_000, 192_000,
    ];
    let mut hdr = [0u8; 32];
    file.read_exact(&mut hdr).context("Reading WavPack header")?;
    if &hdr[0..4] != b"wvpk" {
        return Err(anyhow!("Invalid WavPack header (missing wvpk magic)"));
    }
    let total_samples = u32::from_le_bytes(hdr[12..16].try_into().unwrap());
    let flags = u32::from_le_bytes(hdr[24..28].try_into().unwrap());

    let bytes_per_sample = (flags & 0x03) + 1; // 1..4 bytes -> 8..32 bit
    let bit_depth = Some(bytes_per_sample * 8);
    let is_mono = (flags & (1 << 2)) != 0;
    let channels = if is_mono { 1 } else { 2 };
    let rate_idx = ((flags >> 23) & 0x0F) as usize;
    let sample_rate = WV_RATES.get(rate_idx).copied().unwrap_or(44_100);
    let duration_secs = if total_samples != u32::MAX && total_samples > 0 {
        Some(total_samples as f64 / sample_rate as f64)
    } else {
        None
    };
    let nominal_bitrate = duration_secs
        .filter(|&d| d > 0.5)
        .map(|d| ((file_len as f64 * 8.0) / (d * 1000.0)).round() as u32);

    let mut tags = ArchiveTags::default();
    let _ = parse_apev2_trailer(file, file_len, &mut tags);

    Ok((
        ArchiveInfo {
            codec: "WavPack",
            sample_rate,
            bit_depth,
            channels,
            duration_secs,
            nominal_bitrate,
        },
        tags,
    ))
}

/// Pure-Rust Monkey's Audio (`MAC `) header + APEv2 tag parser.
fn probe_monkeys_audio(file: &mut File, file_len: u64) -> Result<(ArchiveInfo, ArchiveTags)> {
    let mut lead = [0u8; 76];
    let n = file.read(&mut lead).context("Reading Monkey's Audio header")?;
    if n < 32 || &lead[0..4] != b"MAC " {
        return Err(anyhow!("Invalid Monkey's Audio header (missing 'MAC ' magic)"));
    }
    let version = u16::from_le_bytes([lead[4], lead[5]]);
    let (sample_rate, bit_depth, channels, total_samples) = if version >= 3980 && n >= 76 {
        // v3.98+ descriptor (52 bytes) + header (24 bytes)
        let desc_bytes = u32::from_le_bytes(lead[8..12].try_into().unwrap()) as usize;
        let hdr_off = desc_bytes.max(52);
        let mut h = [0u8; 24];
        if hdr_off + 24 <= n {
            h.copy_from_slice(&lead[hdr_off..hdr_off + 24]);
        } else {
            file.seek(SeekFrom::Start(hdr_off as u64))?;
            file.read_exact(&mut h)?;
        }
        let blocks_per_frame = u32::from_le_bytes(h[4..8].try_into().unwrap()) as u64;
        let final_frame_blocks = u32::from_le_bytes(h[8..12].try_into().unwrap()) as u64;
        let total_frames = u32::from_le_bytes(h[12..16].try_into().unwrap()) as u64;
        let bits = u16::from_le_bytes([h[16], h[17]]) as u32;
        let ch = u16::from_le_bytes([h[18], h[19]]).clamp(1, 8);
        let sr = u32::from_le_bytes(h[20..24].try_into().unwrap()).max(8_000);
        let total = if total_frames > 0 {
            (total_frames - 1) * blocks_per_frame + final_frame_blocks
        } else {
            0
        };
        (sr, if bits > 0 { Some(bits) } else { Some(16) }, ch, total)
    } else {
        // Legacy v3.97 header
        let flags = u16::from_le_bytes([lead[8], lead[9]]);
        let ch = u16::from_le_bytes([lead[10], lead[11]]).clamp(1, 8);
        let sr = u32::from_le_bytes(lead[12..16].try_into().unwrap()).max(8_000);
        let total_frames = u32::from_le_bytes(lead[24..28].try_into().unwrap()) as u64;
        let final_blocks = u32::from_le_bytes(lead[28..32].try_into().unwrap()) as u64;
        let bits = if (flags & 0x01) != 0 {
            8
        } else if (flags & 0x08) != 0 {
            24
        } else {
            16
        };
        let blocks_per_frame: u64 = if version >= 3950 { 73_728 * 4 } else { 73_728 };
        let total = if total_frames > 0 {
            (total_frames - 1) * blocks_per_frame + final_blocks
        } else {
            0
        };
        (sr, Some(bits), ch, total)
    };

    let duration_secs = if total_samples > 0 {
        Some(total_samples as f64 / sample_rate as f64)
    } else {
        None
    };
    let nominal_bitrate = duration_secs
        .filter(|&d| d > 0.5)
        .map(|d| ((file_len as f64 * 8.0) / (d * 1000.0)).round() as u32);

    let mut tags = ArchiveTags::default();
    let _ = parse_apev2_trailer(file, file_len, &mut tags);

    Ok((
        ArchiveInfo {
            codec: "APE",
            sample_rate,
            bit_depth,
            channels,
            duration_secs,
            nominal_bitrate,
        },
        tags,
    ))
}

/// Pure-Rust Ogg Opus (`OpusHead` + `OpusTags` + final OggS granule duration) parser.
fn probe_opus(file: &mut File, file_len: u64) -> Result<(ArchiveInfo, ArchiveTags)> {
    let head_len = file_len.min(65_536) as usize;
    let mut buf = vec![0u8; head_len];
    file.read_exact(&mut buf).context("Reading Ogg Opus header")?;

    let head_pos = find_subslice(&buf, b"OpusHead")
        .ok_or_else(|| anyhow!("Not a valid Ogg Opus file (missing OpusHead)"))?;
    if head_pos + 19 > buf.len() {
        return Err(anyhow!("Truncated OpusHead packet"));
    }
    let channels = (buf[head_pos + 9] as u16).clamp(1, 8);
    let pre_skip = u16::from_le_bytes([buf[head_pos + 10], buf[head_pos + 11]]) as u64;

    let mut tags = ArchiveTags::default();
    if let Some(tags_pos) = find_subslice(&buf[head_pos..], b"OpusTags").map(|p| head_pos + p) {
        parse_vorbis_comment_block(&buf[tags_pos + 8..], &mut tags);
    }

    // Read tail (up to 64 KB) to find last OggS page granule position for exact 48 kHz duration
    let tail_start = file_len.saturating_sub(65_536);
    file.seek(SeekFrom::Start(tail_start))?;
    let mut tail = vec![0u8; (file_len - tail_start) as usize];
    let _ = file.read_exact(&mut tail);
    let mut max_granule: u64 = 0;
    let mut i = 0usize;
    while i + 27 <= tail.len() {
        if &tail[i..i + 4] == b"OggS" {
            let g = u64::from_le_bytes(tail[i + 6..i + 14].try_into().unwrap());
            if g != u64::MAX && g > max_granule {
                max_granule = g;
            }
            i += 27;
        } else {
            i += 1;
        }
    }
    let duration_secs = if max_granule > pre_skip {
        Some((max_granule - pre_skip) as f64 / 48_000.0)
    } else {
        None
    };
    let nominal_bitrate = duration_secs
        .filter(|&d| d > 0.5)
        .map(|d| ((file_len as f64 * 8.0) / (d * 1000.0)).round() as u32);

    Ok((
        ArchiveInfo {
            codec: "Opus",
            sample_rate: 48_000,
            bit_depth: None,
            channels,
            duration_secs,
            nominal_bitrate,
        },
        tags,
    ))
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn parse_vorbis_comment_block(data: &[u8], tags: &mut ArchiveTags) {
    if data.len() < 8 {
        return;
    }
    let vendor_len = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let mut pos = 4usize.saturating_add(vendor_len);
    if pos + 4 > data.len() {
        return;
    }
    let count = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
    pos += 4;
    for _ in 0..count.min(256) {
        if pos + 4 > data.len() {
            break;
        }
        let len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if pos + len > data.len() {
            break;
        }
        if let Ok(entry) = std::str::from_utf8(&data[pos..pos + len])
            && let Some((k, v)) = entry.split_once('=')
        {
            apply_archive_kv(k, v, tags);
        }
        pos += len;
    }
}

/// Pure-Rust APEv2 trailer parser (`APETAGEX` footer at EOF or before 128-byte ID3v1).
fn parse_apev2_trailer(file: &mut File, file_len: u64, tags: &mut ArchiveTags) -> Result<()> {
    let mut footer_off = None;
    for back in [32u64, 160u64] {
        if file_len >= back {
            file.seek(SeekFrom::Start(file_len - back))?;
            let mut ftr = [0u8; 32];
            if file.read_exact(&mut ftr).is_ok() && &ftr[0..8] == b"APETAGEX" {
                footer_off = Some((file_len - back, ftr));
                break;
            }
        }
    }
    let Some((f_off, ftr)) = footer_off else {
        return Ok(());
    };
    let tag_size = u32::from_le_bytes(ftr[12..16].try_into().unwrap()) as u64;
    let item_count = u32::from_le_bytes(ftr[16..20].try_into().unwrap()) as usize;
    if !(32..=16 * 1024 * 1024).contains(&tag_size) {
        return Ok(());
    }
    let body_size = (tag_size - 32) as usize;
    let body_start = f_off.saturating_sub(body_size as u64);
    file.seek(SeekFrom::Start(body_start))?;
    let mut body = vec![0u8; body_size];
    file.read_exact(&mut body)?;

    let mut pos = 0usize;
    for _ in 0..item_count.min(256) {
        if pos + 8 > body.len() {
            break;
        }
        let val_len = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(body[pos + 4..pos + 8].try_into().unwrap());
        pos += 8;
        let Some(nul) = body[pos..].iter().position(|&b| b == 0) else {
            break;
        };
        let key = String::from_utf8_lossy(&body[pos..pos + nul]).to_string();
        pos += nul + 1;
        if pos + val_len > body.len() {
            break;
        }
        let raw_val = &body[pos..pos + val_len];
        pos += val_len;
        let is_binary = ((flags >> 1) & 0x03) == 1;
        if is_binary {
            let ku = key.to_ascii_uppercase();
            if ku.contains("COVER ART") && tags.artwork.is_none() {
                // APEv2 Cover Art payload: `<filename>\0<image bytes>`
                if let Some(img_nul) = raw_val.iter().position(|&b| b == 0) {
                    let img_bytes = &raw_val[img_nul + 1..];
                    if img_bytes.len() > 32 {
                        let mime = if img_bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
                            "image/png"
                        } else {
                            "image/jpeg"
                        };
                        tags.artwork = Some((img_bytes.to_vec(), mime.into()));
                    }
                }
            }
        } else if let Ok(val) = std::str::from_utf8(raw_val) {
            apply_archive_kv(&key, val, tags);
        }
    }
    Ok(())
}

fn apply_archive_kv(raw_key: &str, raw_val: &str, tags: &mut ArchiveTags) {
    let val = raw_val.trim();
    if val.is_empty() {
        return;
    }
    match raw_key.trim().to_ascii_uppercase().as_str() {
        "TITLE" => tags.title = Some(val.to_string()),
        "ARTIST" => tags.artist = Some(val.to_string()),
        "ALBUM" => tags.album = Some(val.to_string()),
        "ALBUMARTIST" | "ALBUM ARTIST" => tags.album_artist = Some(val.to_string()),
        "COMPOSER" => tags.composer = Some(val.to_string()),
        "CONDUCTOR" => tags.conductor = Some(val.to_string()),
        "ORCHESTRA" | "ENSEMBLE" => tags.orchestra = Some(val.to_string()),
        "WORK" | "GROUPING" => tags.work = Some(val.to_string()),
        "LABEL" | "PUBLISHER" | "ORGANIZATION" => tags.label = Some(val.to_string()),
        "CATALOGNUMBER" | "CATALOG" => tags.catalog_number = Some(val.to_string()),
        "COMMENT" => tags.comment = Some(val.to_string()),
        "YEAR" | "DATE" => tags.year = Some(val.to_string()),
        "GENRE" => tags.genre = Some(val.to_string()),
        "TRACK" | "TRACKNUMBER" => {
            tags.track_number = val.split('/').next().and_then(|s| s.trim().parse().ok());
        }
        "DISC" | "DISCNUMBER" => {
            tags.disc_number = val.split('/').next().and_then(|s| s.trim().parse().ok());
        }
        "REPLAYGAIN_TRACK_GAIN" => tags.rg_track_gain_db = parse_gain_db(val),
        "REPLAYGAIN_TRACK_PEAK" => tags.rg_track_peak = val.parse::<f32>().ok(),
        "REPLAYGAIN_ALBUM_GAIN" => tags.rg_album_gain_db = parse_gain_db(val),
        "REPLAYGAIN_ALBUM_PEAK" => tags.rg_album_peak = val.parse::<f32>().ok(),
        "R128_TRACK_GAIN" => {
            if let Ok(q) = val.parse::<f32>() {
                tags.rg_track_gain_db = Some(q / 256.0 + 5.0);
            }
        }
        "R128_ALBUM_GAIN" => {
            if let Ok(q) = val.parse::<f32>() {
                tags.rg_album_gain_db = Some(q / 256.0 + 5.0);
            }
        }
        _ => {}
    }
}

/// Transparent lossless PCM streaming decoder for `.wv`, `.ape`, and `.opus` via system `ffmpeg`.
pub struct ArchivePcmDecoder {
    path: PathBuf,
    pub info: ArchiveInfo,
    child: Option<Child>,
    reader: Option<BufReader<ChildStdout>>,
}

impl ArchivePcmDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let (info, _) = probe_archive(path)?;
        let mut dec = Self {
            path: path.to_path_buf(),
            info,
            child: None,
            reader: None,
        };
        dec.spawn_at(0.0)?;
        Ok(dec)
    }

    fn spawn_at(&mut self, start_secs: f64) -> Result<()> {
        self.kill_child();
        let ffmpeg = find_ffmpeg().ok_or_else(|| {
            anyhow!(
                "{} playback requires ffmpeg on PATH (or /opt/homebrew/bin/ffmpeg)",
                self.info.codec
            )
        })?;
        let mut cmd = Command::new(ffmpeg);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.args(["-v", "error", "-nostdin"]);
        if start_secs > 0.01 {
            cmd.args(["-ss", &format!("{start_secs:.3}")]);
        }
        cmd.arg("-i")
            .arg(&self.path)
            .args([
                "-vn",
                "-f",
                "f32le",
                "-ac",
                &self.info.channels.to_string(),
                "-ar",
                &self.info.sample_rate.to_string(),
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().context("Spawning ffmpeg archive decoder")?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture ffmpeg PCM stdout"))?;
        self.reader = Some(BufReader::with_capacity(65_536, stdout));
        self.child = Some(child);
        Ok(())
    }

    fn kill_child(&mut self) {
        self.reader = None;
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        self.spawn_at(seconds.max(0.0))
    }

    /// Reads up to `max_frames` interleaved `f32` PCM frames into `out`.
    pub fn read_pcm_frames(&mut self, max_frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        out.clear();
        let Some(ref mut reader) = self.reader else {
            return Ok(0);
        };
        let ch = self.info.channels.max(1) as usize;
        let frame_bytes = ch * 4;
        let want_bytes = max_frames * frame_bytes;
        let mut raw = vec![0u8; want_bytes];
        let mut filled = 0usize;
        while filled < want_bytes {
            match reader.read(&mut raw[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        let frames = filled / frame_bytes;
        out.reserve(frames * ch);
        for &chunk in raw[..frames * frame_bytes].as_chunks::<4>().0 {
            let v = f32::from_le_bytes(chunk);
            out.push(if v.is_finite() { v } else { 0.0 });
        }
        Ok(frames)
    }
}

impl Drop for ArchivePcmDecoder {
    fn drop(&mut self) {
        self.kill_child();
    }
}

fn find_ffmpeg() -> Option<PathBuf> {
    for candidate in [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let cand = dir.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_apev2_trailer(items: &[(&str, &str)]) -> Vec<u8> {
        let mut body = Vec::new();
        for &(k, v) in items {
            body.extend_from_slice(&(v.len() as u32).to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes()); // UTF-8 text flags
            body.extend_from_slice(k.as_bytes());
            body.push(0);
            body.extend_from_slice(v.as_bytes());
        }
        let tag_size = (body.len() + 32) as u32;
        let mut out = body;
        out.extend_from_slice(b"APETAGEX");
        out.extend_from_slice(&2000u32.to_le_bytes());
        out.extend_from_slice(&tag_size.to_le_bytes());
        out.extend_from_slice(&(items.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        out
    }

    #[test]
    fn probes_wavpack_header_and_apev2_tags() {
        let dir = std::env::temp_dir().join("txplayer_wv_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.wv");

        let mut bytes = vec![0u8; 32];
        bytes[0..4].copy_from_slice(b"wvpk");
        // 96,000 Hz = index 13, 24-bit = bytes_per_sample 3 -> bits 0..1 = 2, stereo -> bit 2 = 0
        let total_samples: u32 = 96_000 * 10; // 10 seconds
        bytes[12..16].copy_from_slice(&total_samples.to_le_bytes());
        let flags: u32 = 2 | (13 << 23);
        bytes[24..28].copy_from_slice(&flags.to_le_bytes());
        bytes.extend_from_slice(&build_apev2_trailer(&[
            ("Title", "Nocturne Op. 27"),
            ("Artist", "Artur Rubinstein"),
            ("Composer", "Frédéric Chopin"),
            ("Label", "RCA Red Seal"),
        ]));
        std::fs::write(&path, &bytes).unwrap();

        let (info, tags) = probe_archive(&path).unwrap();
        assert_eq!(info.codec, "WavPack");
        assert_eq!(info.sample_rate, 96_000);
        assert_eq!(info.bit_depth, Some(24));
        assert_eq!(info.channels, 2);
        assert!((info.duration_secs.unwrap() - 10.0).abs() < 0.01);
        assert_eq!(tags.title.as_deref(), Some("Nocturne Op. 27"));
        assert_eq!(tags.composer.as_deref(), Some("Frédéric Chopin"));
        assert_eq!(tags.label.as_deref(), Some("RCA Red Seal"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probes_monkeys_audio_header_and_apev2_tags() {
        let dir = std::env::temp_dir().join("txplayer_ape_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.ape");

        let mut bytes = vec![0u8; 76];
        bytes[0..4].copy_from_slice(b"MAC ");
        bytes[4..6].copy_from_slice(&3990u16.to_le_bytes());
        bytes[8..12].copy_from_slice(&52u32.to_le_bytes()); // descriptor bytes
        // Header at offset 52
        bytes[56..60].copy_from_slice(&44_100u32.to_le_bytes()); // blocks_per_frame
        bytes[60..64].copy_from_slice(&44_100u32.to_le_bytes()); // final_frame_blocks
        bytes[64..68].copy_from_slice(&5u32.to_le_bytes()); // total_frames = 5s
        bytes[68..70].copy_from_slice(&16u16.to_le_bytes()); // bits
        bytes[70..72].copy_from_slice(&2u16.to_le_bytes()); // channels
        bytes[72..76].copy_from_slice(&44_100u32.to_le_bytes()); // sample rate
        bytes.extend_from_slice(&build_apev2_trailer(&[
            ("Title", "Partita No. 2"),
            ("Composer", "J.S. Bach"),
        ]));
        std::fs::write(&path, &bytes).unwrap();

        let (info, tags) = probe_archive(&path).unwrap();
        assert_eq!(info.codec, "APE");
        assert_eq!(info.sample_rate, 44_100);
        assert_eq!(info.bit_depth, Some(16));
        assert_eq!(info.channels, 2);
        assert!((info.duration_secs.unwrap() - 5.0).abs() < 0.01);
        assert_eq!(tags.title.as_deref(), Some("Partita No. 2"));
        assert_eq!(tags.composer.as_deref(), Some("J.S. Bach"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probes_ogg_opus_head_tags_and_duration() {
        let dir = std::env::temp_dir().join("txplayer_opus_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.opus");

        let mut bytes = Vec::new();
        // Ogg page 1 with OpusHead
        bytes.extend_from_slice(b"OggS");
        bytes.extend_from_slice(&[0; 23]);
        bytes.extend_from_slice(b"OpusHead");
        bytes.push(1); // version
        bytes.push(2); // channels
        bytes.extend_from_slice(&312u16.to_le_bytes()); // pre_skip
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&0i16.to_le_bytes());
        bytes.push(0);

        // OpusTags block
        bytes.extend_from_slice(b"OpusTags");
        let vendor = b"txplayer";
        bytes.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        bytes.extend_from_slice(vendor);
        let comments = ["TITLE=Adagio", "COMPOSER=Albinoni", "R128_TRACK_GAIN=-512"];
        bytes.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for c in comments {
            bytes.extend_from_slice(&(c.len() as u32).to_le_bytes());
            bytes.extend_from_slice(c.as_bytes());
        }

        // Final OggS page with granule = 48000*12 + 312 (12.0 seconds)
        bytes.extend_from_slice(b"OggS");
        bytes.extend_from_slice(&[0, 4]);
        let gran: u64 = 48_000 * 12 + 312;
        bytes.extend_from_slice(&gran.to_le_bytes());
        bytes.extend_from_slice(&[0; 13]);
        std::fs::write(&path, &bytes).unwrap();

        let (info, tags) = probe_archive(&path).unwrap();
        assert_eq!(info.codec, "Opus");
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.channels, 2);
        assert!((info.duration_secs.unwrap() - 12.0).abs() < 0.01);
        assert_eq!(tags.title.as_deref(), Some("Adagio"));
        assert_eq!(tags.composer.as_deref(), Some("Albinoni"));
        assert!((tags.rg_track_gain_db.unwrap() - 3.0).abs() < 0.05);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
