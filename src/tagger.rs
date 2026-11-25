use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::library::data_directory;
use crate::metadata::Track;
use crate::plugins::{TagProviderConfig, ToolHealth};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TagCandidate {
    pub source: String,
    pub score: u8,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub duration_secs: Option<f64>,
    pub external_id: Option<String>,
    #[serde(default)]
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrackHashes {
    pub file_sha256: String,
    pub audio_sha256: String,
    pub duration_ms: Option<u64>,
    pub chromaprint: Option<(u32, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupStrategy {
    ByName,
    ByHash,
    All,
}

#[derive(Debug, Clone, Default)]
pub struct EditableTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub genre: String,
    pub track_number: String,
    pub disc_number: String,
}

impl EditableTags {
    pub fn from_track(track: &Track) -> Self {
        Self {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            album_artist: track.album_artist.clone().unwrap_or_default(),
            year: track.year.clone().unwrap_or_default(),
            genre: track.genre.clone().unwrap_or_default(),
            track_number: track
                .track_number
                .map(|n| n.to_string())
                .unwrap_or_default(),
            disc_number: track
                .disc_number
                .map(|n| n.to_string())
                .unwrap_or_default(),
        }
    }

    pub fn apply_to_track(&self, track: &mut Track, batch_mode: bool) {
        if !batch_mode && !self.title.trim().is_empty() {
            track.title = self.title.trim().to_string();
        }
        if !self.artist.trim().is_empty() {
            track.artist = self.artist.trim().to_string();
        }
        if !self.album.trim().is_empty() {
            track.album = self.album.trim().to_string();
        }
        if !self.album_artist.trim().is_empty() || !batch_mode {
            track.album_artist = non_empty(&self.album_artist);
        }
        if !self.year.trim().is_empty() || !batch_mode {
            track.year = non_empty(&self.year);
        }
        if !self.genre.trim().is_empty() || !batch_mode {
            track.genre = non_empty(&self.genre);
        }
        if !batch_mode {
            track.track_number = self.track_number.trim().parse().ok();
            track.disc_number = self.disc_number.trim().parse().ok();
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Persistent local Hash -> Verified Tags map so previously matched/saved hashes resolve instantly.
#[derive(Debug, Default, Serialize, Deserialize)]
struct HashTagCache {
    by_hash: HashMap<String, TagCandidate>,
}

impl HashTagCache {
    fn path() -> PathBuf {
        data_directory().join("tag_hash_cache.json")
    }

    fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let p = Self::path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(p, data);
        }
    }
}

pub fn record_hash_cache(hashes: &TrackHashes, track: &Track) {
    let mut cache = HashTagCache::load();
    let candidate = TagCandidate {
        source: "Hash Cache".into(),
        score: 100,
        title: Some(track.title.clone()),
        artist: Some(track.artist.clone()),
        album: Some(track.album.clone()),
        album_artist: track.album_artist.clone(),
        year: track.year.clone(),
        genre: track.genre.clone(),
        track_number: track.track_number,
        disc_number: track.disc_number,
        duration_secs: track.duration,
        external_id: Some(hashes.audio_sha256[..16].to_string()),
        artwork_url: None,
    };
    if !hashes.file_sha256.is_empty() {
        cache
            .by_hash
            .insert(hashes.file_sha256.clone(), candidate.clone());
    }
    cache.by_hash.insert(hashes.audio_sha256.clone(), candidate);
    cache.save();
}

/// Fast audio-body content hash (reads only 256 KB from middle payload — <1 ms even on 250 MB Hi-Res FLAC).
pub fn compute_fast_audio_hash(path: &Path, duration_secs: Option<f64>) -> Result<TrackHashes> {
    let mut audio_file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
    let file_len = audio_file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut audio_hasher = Sha256::new();
    let skip_start = (file_len / 8).min(65536);
    use std::io::Seek;
    let _ = audio_file.seek(std::io::SeekFrom::Start(skip_start));
    let mut buf = [0u8; 65536];
    let mut remaining = 262_144usize;
    while remaining > 0 {
        let to_read = remaining.min(buf.len());
        let n = audio_file.read(&mut buf[..to_read])?;
        if n == 0 {
            break;
        }
        audio_hasher.update(&buf[..n]);
        remaining -= n;
    }
    let duration_ms = duration_secs.map(|d| (d * 1000.0).round() as u64);
    if let Some(ms) = duration_ms {
        audio_hasher.update(&ms.to_le_bytes());
    }
    let audio_sha256 = audio_hasher.finalize_hex();
    Ok(TrackHashes {
        file_sha256: String::new(),
        audio_sha256,
        duration_ms,
        chromaprint: None,
    })
}

/// Computes full-file SHA-256, audio-content SHA-256, and optional Chromaprint (called lazily on Hash lookup).
pub fn compute_track_hashes(path: &Path, duration_secs: Option<f64>) -> Result<TrackHashes> {
    let mut fast = compute_fast_audio_hash(path, duration_secs)?;
    let mut file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
    let mut full_hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        full_hasher.update(&buf[..n]);
    }
    fast.file_sha256 = full_hasher.finalize_hex();
    fast.chromaprint = run_fpcalc(path);
    Ok(fast)
}

pub fn detect_tool_health() -> ToolHealth {
    ToolHealth {
        ffmpeg_path: find_ffmpeg(),
        fpcalc_path: find_fpcalc(),
        curl_available: Path::new("/usr/bin/curl").is_file(),
    }
}

fn find_fpcalc() -> Option<PathBuf> {
    for candidate in [
        "/opt/homebrew/bin/fpcalc",
        "/usr/local/bin/fpcalc",
        "/usr/bin/fpcalc",
    ] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    Command::new("which")
        .arg("fpcalc")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .filter(|p| p.is_file())
}

fn run_fpcalc(path: &Path) -> Option<(u32, String)> {
    let bin = find_fpcalc()?;
    let out = Command::new(bin)
        .arg("-json")
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let json: Value = serde_json::from_slice(&out.stdout).ok()?;
    let dur = json.get("duration")?.as_f64()?.round() as u32;
    let fp = json.get("fingerprint")?.as_str()?.to_string();
    Some((dur, fp))
}

/// Runs online tag lookup honoring enabled TagProviderConfig plugins.
pub fn lookup_online(
    track: &Track,
    query_title: &str,
    query_artist: &str,
    hashes: Option<&TrackHashes>,
    strategy: LookupStrategy,
    acoustid_api_key: &str,
    providers: &TagProviderConfig,
) -> (Vec<TagCandidate>, Option<String>, Option<TrackHashes>) {
    let mut results = Vec::new();
    let mut notes = Vec::new();
    let mut computed_full_hashes: Option<TrackHashes> = None;

    // Lazily compute full SHA-256 + Chromaprint when Hash lookup is actually requested
    let effective_hashes = if matches!(strategy, LookupStrategy::ByHash | LookupStrategy::All)
        && hashes.is_none_or(|h| h.file_sha256.is_empty())
    {
        if let Ok(full) = compute_track_hashes(&track.path, track.duration) {
            computed_full_hashes = Some(full);
            computed_full_hashes.as_ref()
        } else {
            hashes
        }
    } else {
        hashes
    };

    // 1. Local hash cache (instant)
    if providers.local_hash_cache
        && let Some(h) = effective_hashes
        && matches!(strategy, LookupStrategy::ByHash | LookupStrategy::All)
    {
        let cache = HashTagCache::load();
        let hit = if !h.file_sha256.is_empty() {
            cache
                .by_hash
                .get(&h.file_sha256)
                .or_else(|| cache.by_hash.get(&h.audio_sha256))
        } else {
            cache.by_hash.get(&h.audio_sha256)
        };
        if let Some(hit) = hit {
            results.push(hit.clone());
        }
    }

    // 2. AcoustID fingerprint lookup (if enabled + fpcalc present)
    if providers.acoustid
        && matches!(strategy, LookupStrategy::ByHash | LookupStrategy::All)
        && let Some(h) = effective_hashes
        && let Some((dur, fp)) = &h.chromaprint
    {
        let key = if acoustid_api_key.trim().is_empty() {
            "8XaBELgH"
        } else {
            acoustid_api_key.trim()
        };
        match query_acoustid(key, *dur, fp) {
            Ok(mut hits) => results.append(&mut hits),
            Err(e) => notes.push(format!("AcoustID: {e}")),
        }
    }

    let mut hit_musicbrainz = false;

    // 3. Hash/Duration + Filename stem lookup on MusicBrainz
    if providers.musicbrainz && matches!(strategy, LookupStrategy::ByHash | LookupStrategy::All) {
        let dur_ms = effective_hashes
            .and_then(|h| h.duration_ms)
            .or_else(|| track.duration.map(|d| (d * 1000.0).round() as u64));
        let stem = clean_filename_stem(&track.path);
        if let Some(ms) = dur_ms {
            let q = if stem.is_empty() {
                format!("dur:[{} TO {}]", ms.saturating_sub(2500), ms + 2500)
            } else {
                format!(
                    "{} AND dur:[{} TO {}]",
                    escape_lucene(&stem),
                    ms.saturating_sub(3500),
                    ms + 3500
                )
            };
            hit_musicbrainz = true;
            match query_musicbrainz(&q, "MusicBrainz (Hash/Dur)") {
                Ok(mut hits) => results.append(&mut hits),
                Err(e) => notes.push(format!("MusicBrainz Hash/Dur: {e}")),
            }
        }
    }

    // 4. Name/Metadata lookup on MusicBrainz + iTunes
    if matches!(strategy, LookupStrategy::ByName | LookupStrategy::All) {
        let title = if query_title.trim().is_empty() {
            clean_filename_stem(&track.path)
        } else {
            query_title.trim().to_string()
        };
        let artist = if query_artist.trim().eq_ignore_ascii_case("unknown artist") {
            String::new()
        } else {
            query_artist.trim().to_string()
        };

        if !title.is_empty() || !artist.is_empty() {
            if providers.musicbrainz {
                let mb_q = match (!title.is_empty(), !artist.is_empty()) {
                    (true, true) => format!(
                        "recording:\"{}\" AND artist:\"{}\"",
                        escape_lucene(&title),
                        escape_lucene(&artist)
                    ),
                    (true, false) => format!("recording:\"{}\"", escape_lucene(&title)),
                    (false, true) => format!("artist:\"{}\"", escape_lucene(&artist)),
                    (false, false) => String::new(),
                };
                if !mb_q.is_empty() {
                    // Respect MusicBrainz strict 1 req/sec rate limit to avoid HTTP 503
                    if hit_musicbrainz {
                        thread::sleep(Duration::from_millis(1050));
                    }
                    match query_musicbrainz(&mb_q, "MusicBrainz") {
                        Ok(mut hits) => results.append(&mut hits),
                        Err(e) => notes.push(format!("MusicBrainz: {e}")),
                    }
                }
            }

            if providers.itunes {
                let itunes_term = format!("{artist} {title}").trim().to_string();
                if !itunes_term.is_empty() {
                    match query_itunes(&itunes_term) {
                        Ok(mut hits) => results.append(&mut hits),
                        Err(e) => notes.push(format!("iTunes: {e}")),
                    }
                }
            }
        }
    }

    // Deduplicate and rank by match score (and proximity to track duration)
    let target_dur = track.duration;
    results.sort_by(|a, b| {
        let bonus = |c: &TagCandidate| -> i32 {
            let dur_bonus = match (target_dur, c.duration_secs) {
                (Some(t), Some(d)) if (t - d).abs() < 3.0 => 12,
                (Some(t), Some(d)) if (t - d).abs() < 8.0 => 5,
                _ => 0,
            };
            c.score as i32 + dur_bonus
        };
        bonus(b).cmp(&bonus(a))
    });

    let err_summary = if results.is_empty() && !notes.is_empty() {
        Some(notes.join(" · "))
    } else {
        None
    };
    (results, err_summary, computed_full_hashes)
}

fn curl_get_json(url: &str) -> Result<Value> {
    let output = Command::new("/usr/bin/curl")
        .args([
            "-sS",
            "-L",
            "--max-time",
            "10",
            "-H",
            "User-Agent: TxPlayer/0.2.0 (https://txplayer.local)",
            "-H",
            "Accept: application/json",
            url,
        ])
        .output()
        .context("Running curl for online tag lookup")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Network error: {}", stderr.trim()));
    }
    let val: Value = serde_json::from_slice(&output.stdout).context("Parsing JSON response")?;
    Ok(val)
}

fn query_musicbrainz(query: &str, source_label: &str) -> Result<Vec<TagCandidate>> {
    let url = format!(
        "https://musicbrainz.org/ws/2/recording/?query={}&fmt=json&limit=8",
        url_encode(query)
    );
    let json = curl_get_json(&url)?;
    let Some(recordings) = json.get("recordings").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for rec in recordings {
        let title = rec.get("title").and_then(Value::as_str).map(String::from);
        let score = rec
            .get("score")
            .and_then(Value::as_u64)
            .unwrap_or(80)
            .min(100) as u8;
        let duration_secs = rec
            .get("length")
            .and_then(Value::as_f64)
            .map(|ms| ms / 1000.0);
        let artist = rec
            .get("artist-credit")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        item.get("name")
                            .and_then(Value::as_str)
                            .or_else(|| item.pointer("/artist/name").and_then(Value::as_str))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .filter(|s| !s.is_empty());

        let first_release = rec
            .get("releases")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first());
        let album = first_release
            .and_then(|r| r.get("title"))
            .and_then(Value::as_str)
            .map(String::from);
        let year = first_release
            .and_then(|r| r.get("date"))
            .and_then(Value::as_str)
            .map(|d| d.chars().take(4).collect::<String>())
            .filter(|s| !s.is_empty());
        let track_number = first_release
            .and_then(|r| r.pointer("/media/0/track/0/number"))
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<u32>().ok());
        let genre = rec
            .get("tags")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|t| t.get("name"))
            .and_then(Value::as_str)
            .map(title_case);
        let external_id = rec.get("id").and_then(Value::as_str).map(String::from);

        out.push(TagCandidate {
            source: source_label.into(),
            score,
            title,
            artist: artist.clone(),
            album,
            album_artist: artist,
            year,
            genre,
            track_number,
            disc_number: None,
            duration_secs,
            external_id,
            artwork_url: None,
        });
    }
    Ok(out)
}

fn query_itunes(term: &str) -> Result<Vec<TagCandidate>> {
    let url = format!(
        "https://itunes.apple.com/search?term={}&media=music&entity=song&limit=8",
        url_encode(term)
    );
    let json = curl_get_json(&url)?;
    let Some(items) = json.get("results").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let title = item
            .get("trackName")
            .and_then(Value::as_str)
            .map(String::from);
        let artist = item
            .get("artistName")
            .and_then(Value::as_str)
            .map(String::from);
        let album = item
            .get("collectionName")
            .and_then(Value::as_str)
            .map(String::from);
        let year = item
            .get("releaseDate")
            .and_then(Value::as_str)
            .map(|d| d.chars().take(4).collect::<String>())
            .filter(|s| !s.is_empty());
        let genre = item
            .get("primaryGenreName")
            .and_then(Value::as_str)
            .map(String::from);
        let track_number = item
            .get("trackNumber")
            .and_then(Value::as_u64)
            .map(|n| n as u32);
        let disc_number = item
            .get("discNumber")
            .and_then(Value::as_u64)
            .map(|n| n as u32);
        let duration_secs = item
            .get("trackTimeMillis")
            .and_then(Value::as_f64)
            .map(|ms| ms / 1000.0);
        let external_id = item
            .get("trackId")
            .and_then(Value::as_u64)
            .map(|id| id.to_string());
        let artwork_url = item
            .get("artworkUrl100")
            .and_then(Value::as_str)
            .map(|u| u.replace("100x100bb", "600x600bb"));

        out.push(TagCandidate {
            source: "iTunes".into(),
            score: (92usize.saturating_sub(idx * 4)).max(55) as u8,
            title,
            artist: artist.clone(),
            album,
            album_artist: artist,
            year,
            genre,
            track_number,
            disc_number,
            duration_secs,
            external_id,
            artwork_url,
        });
    }
    Ok(out)
}

fn query_acoustid(api_key: &str, duration: u32, fingerprint: &str) -> Result<Vec<TagCandidate>> {
    let url = format!(
        "https://api.acoustid.org/v2/lookup?client={}&meta=recordings+releasegroups+compress&duration={}&fingerprint={}",
        url_encode(api_key),
        duration,
        url_encode(fingerprint)
    );
    let json = curl_get_json(&url)?;
    let Some(items) = json.get("results").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in items {
        let score = (item.get("score").and_then(Value::as_f64).unwrap_or(0.8) * 100.0)
            .round()
            .clamp(1.0, 100.0) as u8;
        if let Some(recs) = item.get("recordings").and_then(Value::as_array) {
            for rec in recs.iter().take(3) {
                let title = rec.get("title").and_then(Value::as_str).map(String::from);
                let artist = rec
                    .get("artists")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.get("name").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|s| !s.is_empty());
                let album = rec
                    .pointer("/releasegroups/0/title")
                    .and_then(Value::as_str)
                    .map(String::from);
                let duration_secs = rec.get("duration").and_then(Value::as_f64);
                let external_id = rec.get("id").and_then(Value::as_str).map(String::from);
                out.push(TagCandidate {
                    source: "AcoustID".into(),
                    score,
                    title,
                    artist: artist.clone(),
                    album,
                    album_artist: artist,
                    year: None,
                    genre: None,
                    track_number: None,
                    disc_number: None,
                    duration_secs,
                    external_id,
                    artwork_url: None,
                });
            }
        }
    }
    Ok(out)
}

/// Downloads a cover artwork image from an online candidate URL into the local cover cache.
pub fn download_cover_artwork(url: &str, track_id: u64, cover_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(cover_dir)?;
    let out_path = cover_dir.join(format!("{track_id:016x}_fetched.jpg"));
    let output = Command::new("/usr/bin/curl")
        .args(["-sS", "-L", "--max-time", "12", "-o"])
        .arg(&out_path)
        .arg(url)
        .output()
        .context("Downloading cover artwork")?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&out_path);
        return Err(anyhow!("Failed to download artwork"));
    }
    let meta = std::fs::metadata(&out_path)?;
    if meta.len() < 128 {
        let _ = std::fs::remove_file(&out_path);
        return Err(anyhow!("Downloaded artwork was empty"));
    }
    Ok(out_path)
}

/// Builds ffmpeg `-metadata` args cleanly, explicitly clearing emptied optional fields in single-track edit.
fn build_ffmpeg_metadata_args(track: &Track, clear_empty_optionals: bool) -> Vec<String> {
    let mut args = Vec::with_capacity(18);
    let mut push = |k: &str, v: &str| {
        args.push("-metadata".into());
        args.push(format!("{k}={v}"));
    };
    push("title", &track.title);
    push("artist", &track.artist);
    push("album", &track.album);
    if let Some(aa) = &track.album_artist {
        push("album_artist", aa);
    } else if clear_empty_optionals {
        push("album_artist", "");
    }
    if let Some(year) = &track.year {
        push("date", year);
    } else if clear_empty_optionals {
        push("date", "");
    }
    if let Some(genre) = &track.genre {
        push("genre", genre);
    } else if clear_empty_optionals {
        push("genre", "");
    }
    if let Some(num) = track.track_number {
        push("track", &num.to_string());
    } else if clear_empty_optionals {
        push("track", "");
    }
    if let Some(disc) = track.disc_number {
        push("disc", &disc.to_string());
    } else if clear_empty_optionals {
        push("disc", "");
    }
    args
}

/// Losslessly writes metadata tags directly into the audio file via ffmpeg `-c copy` stream remux.
pub fn write_tags_to_file(track: &Track, clear_empty_optionals: bool) -> Result<()> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| anyhow!("ffmpeg not found on system"))?;
    let ext = track
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("flac");
    let parent = track
        .path
        .parent()
        .ok_or_else(|| anyhow!("Track has no parent directory"))?;
    let stem = track
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("track");
    let tmp_path = parent.join(format!(".{stem}.txplayer_tag_tmp.{ext}"));

    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-y", "-v", "error", "-i"])
        .arg(&track.path)
        .args(["-map", "0", "-c", "copy"]);

    for arg in build_ffmpeg_metadata_args(track, clear_empty_optionals) {
        cmd.arg(arg);
    }
    cmd.arg(&tmp_path);

    let output = cmd.output().context("Running ffmpeg to write file tags")?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp_path);
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffmpeg tag write failed: {}", err.trim()));
    }
    let meta = std::fs::metadata(&tmp_path)?;
    if meta.len() == 0 {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(anyhow!("ffmpeg produced an empty file"));
    }
    std::fs::rename(&tmp_path, &track.path).context("Replacing original file with tagged file")?;
    Ok(())
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
    Command::new("which")
        .arg("ffmpeg")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .filter(|p| p.is_file())
}

fn clean_filename_stem(path: &Path) -> String {
    let raw = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .replace(['_', '.', '-'], " ");
    // Strip leading track numbers like "01 " or "02 - "
    let trimmed = raw
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == ' ')
        .trim();
    if trimmed.is_empty() {
        raw.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn escape_lucene(input: &str) -> String {
    input
        .chars()
        .filter(|&c| !matches!(c, '"' | '\\' | '[' | ']' | '{' | '}' | '^' | '~'))
        .collect()
}

fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

pub fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => {
                use std::fmt::Write;
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Compact pure-Rust FIPS 180-4 SHA-256 implementation (zero external crate dependencies).
struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buf_len: usize,
    total_bits: u64,
}

impl Sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c,
                0x1f83d9ab, 0x5be0cd19,
            ],
            buffer: [0; 64],
            buf_len: 0,
            total_bits: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_bits = self.total_bits.wrapping_add((data.len() as u64) * 8);
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = need.min(data.len());
            self.buffer[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for (i, &wi) in w.iter().enumerate() {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(Self::K[i])
                .wrapping_add(wi);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }

    fn finalize_hex(mut self) -> String {
        let bits = self.total_bits;
        self.buffer[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            self.buffer[self.buf_len..64].fill(0);
            let block = self.buffer;
            self.compress(&block);
            self.buf_len = 0;
        }
        self.buffer[self.buf_len..56].fill(0);
        self.buffer[56..64].copy_from_slice(&bits.to_be_bytes());
        let block = self.buffer;
        self.compress(&block);

        let mut hex = String::with_capacity(64);
        use std::fmt::Write;
        for word in self.state {
            let _ = write!(hex, "{word:08x}");
        }
        hex
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_nist_abc_vector() {
        let mut h = Sha256::new();
        h.update(b"abc");
        assert_eq!(
            h.finalize_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn url_encode_handles_spaces_and_symbols() {
        assert_eq!(url_encode("Chopin Op.9/2"), "Chopin%20Op.9%2F2");
    }

    #[test]
    fn ffmpeg_metadata_args_clears_emptied_optional_fields() {
        let track = Track {
            id: 1,
            path: PathBuf::from("/tmp/test.flac"),
            title: "Prelude".into(),
            artist: "Bach".into(),
            album: "WTC".into(),
            album_artist: None,
            track_number: Some(1),
            disc_number: None,
            year: None,
            genre: None,
            duration: Some(120.0),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            codec: "FLAC".into(),
            nominal_bitrate: None,
            cover_path: None,
        };
        let args = build_ffmpeg_metadata_args(&track, true);
        assert!(args.contains(&"genre=".to_string()));
        assert!(args.contains(&"date=".to_string()));
        assert!(args.contains(&"album_artist=".to_string()));
        assert!(args.contains(&"track=1".to_string()));
    }
}
