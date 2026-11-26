use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use symphonia::core::codecs::{CodecParameters, CodecType};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::{MetadataOptions, StandardTagKey, Value};
use symphonia::core::probe::Hint;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub duration: Option<f64>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u16>,
    pub codec: String,
    pub nominal_bitrate: Option<u32>,
    pub cover_path: Option<PathBuf>,
    // CUE sheet virtual track boundaries (None for normal standalone files)
    #[serde(default)]
    pub cue_start: Option<f64>,
    #[serde(default)]
    pub cue_end: Option<f64>,
    // ReplayGain / EBU R128 loudness metadata
    #[serde(default)]
    pub rg_track_gain_db: Option<f32>,
    #[serde(default)]
    pub rg_track_peak: Option<f32>,
    #[serde(default)]
    pub rg_album_gain_db: Option<f32>,
    #[serde(default)]
    pub rg_album_peak: Option<f32>,
    // Embedded synced/unsynced lyrics (sidecar .lrc also supported dynamically)
    #[serde(default)]
    pub lyrics: Option<String>,
    // Classical & collector database metadata
    #[serde(default)]
    pub composer: Option<String>,
    #[serde(default)]
    pub conductor: Option<String>,
    #[serde(default)]
    pub orchestra: Option<String>,
    #[serde(default)]
    pub work: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub catalog_number: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub custom_tags: Vec<String>,
    // Persisted Audio Quality & TrueHz Authenticity results (saved permanently in DB)
    #[serde(default)]
    pub dr_score: Option<u8>,
    #[serde(default)]
    pub integrated_lufs: Option<f32>,
    #[serde(default)]
    pub true_peak_dbtp: Option<f32>,
    #[serde(default)]
    pub effective_bandwidth_hz: Option<f32>,
    #[serde(default)]
    pub authenticity: Option<String>,
    // Listening stats & user rating (0..=10 half-stars, representing 0.0..=5.0 stars)
    #[serde(default)]
    pub play_count: u32,
    #[serde(default)]
    pub last_played_at: Option<u64>,
    #[serde(default)]
    pub rating: u8,
    #[serde(default)]
    pub added_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityTier {
    HiRes,
    Lossless,
    Standard,
}

impl Track {
    pub fn is_lossless(&self) -> bool {
        matches!(
            self.codec.as_str(),
            "FLAC"
                | "ALAC"
                | "PCM"
                | "CD-BE"
                | "WavPack"
                | "APE"
                | "DSD64"
                | "DSD128"
                | "DSD256"
                | "DSD512"
        )
    }

    pub fn is_hires(&self) -> bool {
        self.codec.starts_with("DSD")
            || (self.is_lossless()
                && (self.sample_rate.unwrap_or(0) > 48_000 || self.bit_depth.unwrap_or(0) >= 24))
    }

    pub fn quality_tier(&self) -> QualityTier {
        if self.is_hires() {
            QualityTier::HiRes
        } else if self.is_lossless() {
            QualityTier::Lossless
        } else {
            QualityTier::Standard
        }
    }

    pub fn quality(&self) -> String {
        let mut parts = Vec::new();
        if let Some(bits) = self.bit_depth {
            parts.push(format!("{bits}b"));
        }
        if let Some(rate) = self.sample_rate {
            parts.push(format_sample_rate(rate));
        }
        if parts.is_empty() {
            self.codec.clone()
        } else {
            parts.join(" · ")
        }
    }

    pub fn quality_full(&self) -> String {
        let mut parts = vec![self.codec.clone()];
        if let Some(bits) = self.bit_depth {
            parts.push(format!("{bits}-bit"));
        }
        if let Some(rate) = self.sample_rate {
            parts.push(format_sample_rate(rate));
        }
        parts.join(" · ")
    }

    pub fn file_size_bytes(&self) -> Option<u64> {
        std::fs::metadata(&self.path).ok().map(|m| m.len())
    }

    pub fn search_text(&self) -> String {
        let mut s = format!(
            "{} {} {} {} {} {} {} {} {} {}",
            self.title.to_lowercase(),
            self.artist.to_lowercase(),
            self.album.to_lowercase(),
            self.genre.as_deref().unwrap_or_default().to_lowercase(),
            self.codec.to_lowercase(),
            self.composer.as_deref().unwrap_or_default().to_lowercase(),
            self.conductor.as_deref().unwrap_or_default().to_lowercase(),
            self.orchestra.as_deref().unwrap_or_default().to_lowercase(),
            self.work.as_deref().unwrap_or_default().to_lowercase(),
            self.label.as_deref().unwrap_or_default().to_lowercase(),
        );
        if !self.custom_tags.is_empty() {
            for tag in &self.custom_tags {
                s.push(' ');
                s.push_str(&tag.to_lowercase());
            }
        }
        s
    }

    /// Evaluates TxPlayer's audiophile power-search syntax (all whitespace tokens must match):
    /// - `#tag` -> custom tag
    /// - `dr>=12`, `dr<=8`, `dr=14` -> persisted Dynamic Range score
    /// - `rate>=96` (or `96000`), `bits>=24`, `rating>=4` -> numeric audio/user filters
    /// - `codec:dsd`, `composer:beethoven`, `artist:...`, `album:...`, `label:...`, `genre:...`, `year:...`, `work:...`
    /// - Any plain token -> case-insensitive match across title/artist/album/composer/orchestra/label/tags
    pub fn matches_query(&self, raw_query: &str) -> bool {
        let trimmed = raw_query.trim();
        if trimmed.is_empty() {
            return true;
        }
        let haystack = self.search_text();
        for tok in trimmed.split_whitespace() {
            let low = tok.to_lowercase();
            if let Some(tag) = low.strip_prefix('#') {
                if !tag.is_empty()
                    && !self
                        .custom_tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(tag))
                {
                    return false;
                }
                continue;
            }
            if let Some(rest) = low.strip_prefix("dr>=")
                && let Ok(n) = rest.parse::<u8>()
            {
                if self.dr_score.unwrap_or(0) < n {
                    return false;
                }
                continue;
            }
            if let Some(rest) = low.strip_prefix("dr<=")
                && let Ok(n) = rest.parse::<u8>()
            {
                if !self.dr_score.is_some_and(|d| d <= n) {
                    return false;
                }
                continue;
            }
            if let Some(rest) = low.strip_prefix("dr=")
                && let Ok(n) = rest.parse::<u8>()
            {
                if self.dr_score != Some(n) {
                    return false;
                }
                continue;
            }
            if let Some(rest) = low.strip_prefix("rate>=").or_else(|| low.strip_prefix("sr>="))
                && let Ok(v) = rest.parse::<u32>()
            {
                let hz = if v <= 1000 { v * 1000 } else { v };
                if self.sample_rate.unwrap_or(0) < hz {
                    return false;
                }
                continue;
            }
            if let Some(rest) = low.strip_prefix("bits>=")
                && let Ok(b) = rest.parse::<u32>()
            {
                if self.bit_depth.unwrap_or(0) < b {
                    return false;
                }
                continue;
            }
            if let Some(rest) = low.strip_prefix("rating>=")
                && let Ok(r) = rest.parse::<f32>()
            {
                // Half-star storage (`0..=10`): `rating>=4` or `rating>=4.5` maps to `8` or `9`
                let min_half = (r * 2.0).round().clamp(0.0, 10.0) as u8;
                if self.rating < min_half {
                    return false;
                }
                continue;
            }
            if let Some((k, v)) = low.split_once(':')
                && !v.is_empty()
            {
                let matched = match k {
                    "codec" => self.codec.to_lowercase().contains(v),
                    "composer" => self
                        .composer
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(v),
                    "artist" => self.artist.to_lowercase().contains(v),
                    "album" => self.album.to_lowercase().contains(v),
                    "label" => self
                        .label
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(v),
                    "genre" => self
                        .genre
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(v),
                    "year" => self
                        .year
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(v),
                    "work" => self
                        .work
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(v),
                    _ => haystack.contains(&low),
                };
                if !matched {
                    return false;
                }
                continue;
            }
            if !haystack.contains(&low) {
                return false;
            }
        }
        true
    }
}

#[derive(Default)]
struct Tags {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    track_number: Option<u32>,
    disc_number: Option<u32>,
    year: Option<String>,
    genre: Option<String>,
    composer: Option<String>,
    conductor: Option<String>,
    orchestra: Option<String>,
    work: Option<String>,
    label: Option<String>,
    catalog_number: Option<String>,
    comment: Option<String>,
    rg_track_gain_db: Option<f32>,
    rg_track_peak: Option<f32>,
    rg_album_gain_db: Option<f32>,
    rg_album_peak: Option<f32>,
    lyrics: Option<String>,
}

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "flac", "dsf", "dff", "wv", "ape", "opus", "m4a", "mp4", "aac", "mp3", "mp2", "mp1", "wav",
    "wave", "aif", "aiff", "aifc", "caf", "ogg", "oga", "webm", "mka", "mkv",
];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            let lower = extension.to_ascii_lowercase();
            SUPPORTED_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

pub fn is_cue_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cue"))
}

pub fn read_track(path: &Path, cover_directory: &Path) -> Result<Track> {
    if crate::archive::is_archive_codec_path(path) {
        let (info, tags) = crate::archive::probe_archive(path)?;
        let id = track_id(path);
        let title = tags.title.unwrap_or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Unknown title")
                .replace('_', " ")
        });
        let cover_path = tags
            .artwork
            .and_then(|(data, media_type)| save_cover(cover_directory, id, &data, &media_type).ok())
            .or_else(|| find_sidecar_cover(path));
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        return Ok(Track {
            id,
            path: path.to_path_buf(),
            title,
            artist: tags.artist.unwrap_or_else(|| "Unknown Artist".into()),
            album: tags.album.unwrap_or_else(|| "Unknown Album".into()),
            album_artist: tags.album_artist,
            track_number: tags.track_number,
            disc_number: tags.disc_number,
            year: tags.year,
            genre: tags.genre,
            duration: info.duration_secs,
            sample_rate: Some(info.sample_rate),
            bit_depth: info.bit_depth,
            channels: Some(info.channels),
            codec: info.codec.into(),
            nominal_bitrate: info.nominal_bitrate,
            cover_path,
            cue_start: None,
            cue_end: None,
            rg_track_gain_db: tags.rg_track_gain_db,
            rg_track_peak: tags.rg_track_peak,
            rg_album_gain_db: tags.rg_album_gain_db,
            rg_album_peak: tags.rg_album_peak,
            lyrics: None,
            composer: tags.composer,
            conductor: tags.conductor,
            orchestra: tags.orchestra,
            work: tags.work,
            label: tags.label,
            catalog_number: tags.catalog_number,
            comment: tags.comment,
            custom_tags: Vec::new(),
            dr_score: None,
            integrated_lufs: None,
            true_peak_dbtp: None,
            effective_bandwidth_hz: None,
            authenticity: None,
            play_count: 0,
            last_played_at: None,
            rating: 0,
            added_at: now_unix,
        });
    }
    if crate::dsd::is_dsd_path(path) {
        let (info, tags) = crate::dsd::probe_dsd(path)?;
        let id = track_id(path);
        let title = tags.title.unwrap_or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Unknown title")
                .replace('_', " ")
        });
        let cover_path = tags
            .artwork
            .and_then(|(data, media_type)| save_cover(cover_directory, id, &data, &media_type).ok())
            .or_else(|| find_sidecar_cover(path));
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let nominal_kbps = ((info.sample_rate_hz as u64 * info.channels as u64) / 1000) as u32;
        return Ok(Track {
            id,
            path: path.to_path_buf(),
            title,
            artist: tags.artist.unwrap_or_else(|| "Unknown Artist".into()),
            album: tags.album.unwrap_or_else(|| "Unknown Album".into()),
            album_artist: tags.album_artist,
            track_number: tags.track_number,
            disc_number: tags.disc_number,
            year: tags.year,
            genre: tags.genre,
            duration: Some(info.duration_secs),
            sample_rate: Some(info.sample_rate_hz),
            bit_depth: Some(1),
            channels: Some(info.channels.min(u16::MAX as usize) as u16),
            codec: info.dsd_label.into(),
            nominal_bitrate: Some(nominal_kbps),
            cover_path,
            cue_start: None,
            cue_end: None,
            rg_track_gain_db: None,
            rg_track_peak: None,
            rg_album_gain_db: None,
            rg_album_peak: None,
            lyrics: None,
            composer: None,
            conductor: None,
            orchestra: None,
            work: None,
            label: None,
            catalog_number: None,
            comment: None,
            custom_tags: Vec::new(),
            dr_score: None,
            integrated_lufs: None,
            true_peak_dbtp: None,
            effective_bandwidth_hz: None,
            authenticity: None,
            play_count: 0,
            last_played_at: None,
            rating: 0,
            added_at: now_unix,
        });
    }

    let file = File::open(path).with_context(|| format!("Opening {}", path.display()))?;
    let source = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }
    let mut probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("Reading audio metadata")?;
    let mut tags = Tags::default();
    let mut artwork = None;
    if let Some(mut metadata) = probed.metadata.get()
        && let Some(revision) = metadata.skip_to_latest()
    {
        read_tags(revision.tags(), &mut tags);
        if let Some(visual) = revision.visuals().first() {
            artwork = Some((visual.data.to_vec(), visual.media_type.clone()));
        }
    }
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("No default audio track"))?;
    let parameters = track.codec_params.clone();
    let id = track_id(path);

    let mut metadata = format.metadata();
    if let Some(revision) = metadata.skip_to_latest() {
        read_tags(revision.tags(), &mut tags);
        if let Some(visual) = revision.visuals().first() {
            artwork = Some((visual.data.to_vec(), visual.media_type.clone()));
        }
    }

    let title = tags.title.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("Unknown title")
            .replace('_', " ")
    });
    let cover_path = artwork
        .and_then(|(data, media_type)| save_cover(cover_directory, id, &data, &media_type).ok())
        .or_else(|| find_sidecar_cover(path));

    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    Ok(Track {
        id,
        path: path.to_path_buf(),
        title,
        artist: tags.artist.unwrap_or_else(|| "Unknown Artist".into()),
        album: tags.album.unwrap_or_else(|| "Unknown Album".into()),
        album_artist: tags.album_artist,
        track_number: tags.track_number,
        disc_number: tags.disc_number,
        year: tags.year,
        genre: tags.genre,
        duration: duration(&parameters),
        sample_rate: parameters.sample_rate,
        bit_depth: parameters.bits_per_sample,
        channels: parameters.channels.map(|channels| channels.count() as u16),
        codec: codec_name(parameters.codec).into(),
        nominal_bitrate: None,
        cover_path,
        cue_start: None,
        cue_end: None,
        rg_track_gain_db: tags.rg_track_gain_db,
        rg_track_peak: tags.rg_track_peak,
        rg_album_gain_db: tags.rg_album_gain_db,
        rg_album_peak: tags.rg_album_peak,
        lyrics: tags.lyrics,
        composer: tags.composer,
        conductor: tags.conductor,
        orchestra: tags.orchestra,
        work: tags.work,
        label: tags.label,
        catalog_number: tags.catalog_number,
        comment: tags.comment,
        custom_tags: Vec::new(),
        dr_score: None,
        integrated_lufs: None,
        true_peak_dbtp: None,
        effective_bandwidth_hz: None,
        authenticity: None,
        play_count: 0,
        last_played_at: None,
        rating: 0,
        added_at: now_unix,
    })
}

pub(crate) fn find_sidecar_cover(audio_path: &Path) -> Option<PathBuf> {
    let dir = audio_path.parent()?;
    const CANDIDATES: &[&str] = &[
        "cover.jpg",
        "cover.jpeg",
        "cover.png",
        "cover.webp",
        "folder.jpg",
        "folder.png",
        "front.jpg",
        "front.png",
        "artwork.jpg",
        "artwork.png",
        "Cover.jpg",
        "Cover.png",
        "Folder.jpg",
    ];
    for candidate in CANDIDATES {
        let p = dir.join(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn read_tags(tags: &[symphonia::core::meta::Tag], output: &mut Tags) {
    for tag in tags {
        let value = tag_value(&tag.value).trim().to_string();
        if value.is_empty() {
            continue;
        }
        match tag.std_key {
            Some(StandardTagKey::TrackTitle) => output.title = Some(value),
            Some(StandardTagKey::Artist) => output.artist = Some(value),
            Some(StandardTagKey::Album) => output.album = Some(value),
            Some(StandardTagKey::AlbumArtist) => output.album_artist = Some(value),
            Some(StandardTagKey::TrackNumber) => output.track_number = parse_number(&value),
            Some(StandardTagKey::DiscNumber) => output.disc_number = parse_number(&value),
            Some(StandardTagKey::Date | StandardTagKey::OriginalDate) => output.year = Some(value),
            Some(StandardTagKey::Genre) => output.genre = Some(value),
            Some(StandardTagKey::Composer) => output.composer = Some(value),
            Some(StandardTagKey::Conductor) => output.conductor = Some(value),
            Some(StandardTagKey::Ensemble) => output.orchestra = Some(value),
            Some(StandardTagKey::ContentGroup) => output.work = Some(value),
            Some(StandardTagKey::Label) => output.label = Some(value),
            Some(StandardTagKey::IdentCatalogNumber) => output.catalog_number = Some(value),
            Some(StandardTagKey::Comment) => output.comment = Some(value),
            Some(StandardTagKey::ReplayGainTrackGain) => {
                output.rg_track_gain_db = parse_gain_db(&value);
            }
            Some(StandardTagKey::ReplayGainTrackPeak) => {
                output.rg_track_peak = value.parse::<f32>().ok();
            }
            Some(StandardTagKey::ReplayGainAlbumGain) => {
                output.rg_album_gain_db = parse_gain_db(&value);
            }
            Some(StandardTagKey::ReplayGainAlbumPeak) => {
                output.rg_album_peak = value.parse::<f32>().ok();
            }
            Some(StandardTagKey::Lyrics) => output.lyrics = Some(value),
            _ => {
                // Handle custom Vorbis/ID3/MP4 ReplayGain, R128 & Lyrics keys
                let k = tag.key.to_ascii_uppercase();
                match k.as_str() {
                    "REPLAYGAIN_TRACK_GAIN" => output.rg_track_gain_db = parse_gain_db(&value),
                    "REPLAYGAIN_TRACK_PEAK" => output.rg_track_peak = value.parse::<f32>().ok(),
                    "REPLAYGAIN_ALBUM_GAIN" => output.rg_album_gain_db = parse_gain_db(&value),
                    "REPLAYGAIN_ALBUM_PEAK" => output.rg_album_peak = value.parse::<f32>().ok(),
                    "R128_TRACK_GAIN" => {
                        // Opus R128 gain is Q7.8 fixed-point (1/256 dB) relative to -23 LUFS (+5 dB to -18 LUFS RG ref)
                        if let Ok(q) = value.parse::<f32>() {
                            output.rg_track_gain_db = Some(q / 256.0 + 5.0);
                        }
                    }
                    "R128_ALBUM_GAIN" => {
                        if let Ok(q) = value.parse::<f32>() {
                            output.rg_album_gain_db = Some(q / 256.0 + 5.0);
                        }
                    }
                    "LYRICS" | "UNSYNCEDLYRICS" | "USLT" | "SYLT" if output.lyrics.is_none() => {
                        output.lyrics = Some(value);
                    }
                    "COMPOSER" | "TCOM" if output.composer.is_none() => {
                        output.composer = Some(value);
                    }
                    "CONDUCTOR" | "TPE3" if output.conductor.is_none() => {
                        output.conductor = Some(value);
                    }
                    "ORCHESTRA" | "ENSEMBLE" | "PERFORMER" if output.orchestra.is_none() => {
                        output.orchestra = Some(value);
                    }
                    "WORK" | "GROUPING" | "TIT1" if output.work.is_none() => {
                        output.work = Some(value);
                    }
                    "LABEL" | "ORGANIZATION" | "PUBLISHER" | "TPUB" if output.label.is_none() => {
                        output.label = Some(value);
                    }
                    "CATALOGNUMBER" | "CATALOG" if output.catalog_number.is_none() => {
                        output.catalog_number = Some(value);
                    }
                    "COMMENT" | "DESCRIPTION" if output.comment.is_none() => {
                        output.comment = Some(value);
                    }
                    _ => {}
                }
            }
        }
    }
}

pub fn parse_gain_db(value: &str) -> Option<f32> {
    let cleaned = value
        .trim()
        .trim_end_matches("dB")
        .trim_end_matches("db")
        .trim_end_matches("DB")
        .trim();
    cleaned.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn tag_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::UnsignedInt(value) => value.to_string(),
        Value::SignedInt(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Boolean(value) => value.to_string(),
        Value::Binary(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        _ => String::new(),
    }
}

fn parse_number(value: &str) -> Option<u32> {
    value.split('/').next()?.trim().parse().ok()
}

fn duration(parameters: &CodecParameters) -> Option<f64> {
    let time_base = parameters.time_base?;
    let frames = parameters.n_frames?;
    let time = time_base.calc_time(frames);
    Some(time.seconds as f64 + time.frac)
}

fn codec_name(codec: CodecType) -> &'static str {
    use symphonia::core::codecs::*;
    match codec {
        CODEC_TYPE_FLAC => "FLAC",
        CODEC_TYPE_ALAC => "ALAC",
        CODEC_TYPE_MP1 => "MP1",
        CODEC_TYPE_MP2 => "MP2",
        CODEC_TYPE_MP3 => "MP3",
        CODEC_TYPE_AAC => "AAC",
        CODEC_TYPE_VORBIS => "Vorbis",
        CODEC_TYPE_ADPCM_IMA_WAV | CODEC_TYPE_ADPCM_MS => "ADPCM",
        CODEC_TYPE_PCM_S16LE | CODEC_TYPE_PCM_S16BE | CODEC_TYPE_PCM_S24LE
        | CODEC_TYPE_PCM_S24BE | CODEC_TYPE_PCM_S32LE | CODEC_TYPE_PCM_S32BE
        | CODEC_TYPE_PCM_U8 | CODEC_TYPE_PCM_S8 | CODEC_TYPE_PCM_F32LE | CODEC_TYPE_PCM_F32BE
        | CODEC_TYPE_PCM_F64LE | CODEC_TYPE_PCM_F64BE | CODEC_TYPE_PCM_ALAW
        | CODEC_TYPE_PCM_MULAW => "PCM",
        _ => "Audio",
    }
}

fn save_cover(directory: &Path, _id: u64, data: &[u8], media_type: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let extension = if media_type.contains("png") {
        "png"
    } else if media_type.contains("webp") {
        "webp"
    } else {
        "jpg"
    };
    // Deduplicate embedded album artwork by content hash so an album of 20 tracks
    // writes 1 shared thumbnail file instead of 20 identical copies.
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    let content_hash = hasher.finish();
    let path = directory.join(format!("art_{content_hash:016x}.{extension}"));
    if !path.exists() {
        std::fs::write(&path, data)?;
    }
    Ok(path)
}

pub fn track_id(path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    hasher.finish()
}

pub fn cue_track_id(audio_path: &Path, track_number: u32, start_secs: f64) -> u64 {
    let mut hasher = DefaultHasher::new();
    audio_path.to_string_lossy().hash(&mut hasher);
    "cue".hash(&mut hasher);
    track_number.hash(&mut hasher);
    (start_secs * 100.0).round().to_bits().hash(&mut hasher);
    hasher.finish()
}

pub fn format_sample_rate(rate: u32) -> String {
    if rate >= 1_000_000 {
        format!("{:.2} MHz", rate as f64 / 1_000_000.0)
    } else if rate.is_multiple_of(1000) {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", rate as f64 / 1000.0)
    }
}

pub fn format_duration(seconds: Option<f64>) -> String {
    let Some(seconds) = seconds else {
        return "--:--".into();
    };
    let total = seconds.max(0.0).round() as u64;
    if total >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            total / 3600,
            (total % 3600) / 60,
            total % 60
        )
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

pub fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_replaygain_db_strings() {
        assert!((parse_gain_db("-6.42 dB").unwrap() - (-6.42)).abs() < 1e-4);
        assert!((parse_gain_db("+2.50dB").unwrap() - 2.50).abs() < 1e-4);
        assert!((parse_gain_db("-0.15 DB").unwrap() - (-0.15)).abs() < 1e-4);
        assert!(parse_gain_db("invalid").is_none());
    }

    #[test]
    fn power_search_evaluates_dr_rate_codec_composer_and_hashtags() {
        let t = Track {
            id: 1,
            path: PathBuf::from("/music/beethoven.flac"),
            title: "Piano Sonata No. 14".into(),
            artist: "Maurizio Pollini".into(),
            album: "Beethoven Sonatas".into(),
            album_artist: None,
            track_number: Some(1),
            disc_number: Some(1),
            year: Some("1991".into()),
            genre: Some("Classical".into()),
            duration: Some(360.0),
            sample_rate: Some(96_000),
            bit_depth: Some(24),
            channels: Some(2),
            codec: "FLAC".into(),
            nominal_bitrate: None,
            cover_path: None,
            cue_start: None,
            cue_end: None,
            rg_track_gain_db: None,
            rg_track_peak: None,
            rg_album_gain_db: None,
            rg_album_peak: None,
            lyrics: None,
            composer: Some("Ludwig van Beethoven".into()),
            conductor: None,
            orchestra: None,
            work: Some("Op. 27 No. 2".into()),
            label: Some("Deutsche Grammophon".into()),
            catalog_number: None,
            comment: None,
            custom_tags: vec!["reference".into(), "solo-piano".into()],
            dr_score: Some(14),
            integrated_lufs: Some(-20.1),
            true_peak_dbtp: Some(-1.5),
            effective_bandwidth_hz: Some(41_000.0),
            authenticity: Some("AuthenticHiRes".into()),
            play_count: 3,
            last_played_at: Some(100),
            rating: 9, // 4.5 stars in half-star scale
            added_at: 10,
        };
        assert!(t.matches_query("composer:beethoven dr>=12 rate>=96 #reference"));
        assert!(t.matches_query("codec:flac label:grammophon rating>=4.5 work:27"));
        assert!(!t.matches_query("rating>=5"));
        assert!(!t.matches_query("dr>=16"));
        assert!(!t.matches_query("rate>=192"));
        assert!(!t.matches_query("#vinyl"));
    }

    #[test]
    fn save_cover_deduplicates_identical_album_artwork() {
        let dir = std::env::temp_dir().join(format!(
            "txplayer_cover_dedupe_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let art = b"\xff\xd8\xff\xe0FAKE_JPEG_ALBUM_ART_BYTES";
        let p1 = save_cover(&dir, 1001, art, "image/jpeg").unwrap();
        let p2 = save_cover(&dir, 1002, art, "image/jpeg").unwrap();
        assert_eq!(p1, p2);
        let count = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
