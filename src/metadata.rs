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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityTier {
    HiRes,
    Lossless,
    Standard,
}

impl Track {
    pub fn is_lossless(&self) -> bool {
        matches!(self.codec.as_str(), "FLAC" | "ALAC" | "PCM")
    }

    pub fn is_hires(&self) -> bool {
        self.is_lossless()
            && (self.sample_rate.unwrap_or(0) > 48_000 || self.bit_depth.unwrap_or(0) >= 24)
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
        format!(
            "{} {} {} {} {}",
            self.title.to_lowercase(),
            self.artist.to_lowercase(),
            self.album.to_lowercase(),
            self.genre.as_deref().unwrap_or_default().to_lowercase(),
            self.codec.to_lowercase()
        )
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
}

pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "flac", "m4a", "mp4", "aac", "mp3", "mp2", "mp1", "wav", "wave", "aif", "aiff", "aifc", "caf",
    "ogg", "oga", "webm", "mka", "mkv",
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

pub fn read_track(path: &Path, cover_directory: &Path) -> Result<Track> {
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
    })
}

fn find_sidecar_cover(audio_path: &Path) -> Option<PathBuf> {
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
            _ => {}
        }
    }
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

fn save_cover(directory: &Path, id: u64, data: &[u8], media_type: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(directory)?;
    let extension = if media_type.contains("png") {
        "png"
    } else if media_type.contains("webp") {
        "webp"
    } else {
        "jpg"
    };
    let path = directory.join(format!("{id:016x}.{extension}"));
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

pub fn format_sample_rate(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
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
