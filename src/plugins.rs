use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::visualizer::VisualizerMode;

/// Selectable real-time piano pitch/note detection engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PianoEngineId {
    #[default]
    PrecisionCqt,
    ClassicFft,
}

impl PianoEngineId {
    pub const ALL: &'static [PianoEngineId] = &[
        PianoEngineId::PrecisionCqt,
        PianoEngineId::ClassicFft,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PianoEngineId::PrecisionCqt => "Precision Piano CQT + Harmonic Cancel",
            PianoEngineId::ClassicFft => "Classic 4096-pt FFT",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            PianoEngineId::PrecisionCqt => "Precision CQT",
            PianoEngineId::ClassicFft => "Classic FFT",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            PianoEngineId::PrecisionCqt => {
                "Multi-rate semitone filterbank with <1.5 Hz bass resolution, piano inharmonicity model, iterative overtone cancellation, and hammer-strike onset detection."
            }
            PianoEngineId::ClassicFft => {
                "Lightweight single-pass 4096-point Blackman-Harris FFT mapped directly to 88 keys."
            }
        }
    }
}

pub const EQ_FREQS_HZ: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

pub const EQ_FREQ_LABELS: [&str; 10] = [
    "31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EqPreset {
    #[default]
    Flat,
    ConcertHall,
    PianoWarmth,
    VocalPresence,
    LateNight,
    StudioBright,
    Custom,
}

impl EqPreset {
    pub const SELECTABLE: &'static [EqPreset] = &[
        EqPreset::Flat,
        EqPreset::ConcertHall,
        EqPreset::PianoWarmth,
        EqPreset::VocalPresence,
        EqPreset::LateNight,
        EqPreset::StudioBright,
    ];

    pub fn name(self) -> &'static str {
        match self {
            EqPreset::Flat => "Flat (Reference)",
            EqPreset::ConcertHall => "Concert Hall",
            EqPreset::PianoWarmth => "Grand Piano Warmth",
            EqPreset::VocalPresence => "Vocal & Chamber",
            EqPreset::LateNight => "Late Night",
            EqPreset::StudioBright => "Studio Air",
            EqPreset::Custom => "Custom",
        }
    }

    pub fn gains_db(self) -> [f32; 10] {
        match self {
            EqPreset::Flat | EqPreset::Custom => [0.0; 10],
            EqPreset::ConcertHall => [1.5, 2.0, 1.0, -0.5, -0.5, 0.0, 0.8, 1.4, 1.8, 1.5],
            EqPreset::PianoWarmth => [1.0, 1.8, 1.5, 0.6, 0.0, -0.4, 0.5, 1.2, 1.0, 0.6],
            EqPreset::VocalPresence => [-1.0, -0.5, 0.0, 0.8, 1.5, 2.0, 2.2, 1.2, 0.5, 0.0],
            EqPreset::LateNight => [3.0, 2.5, 1.2, 0.0, -0.8, -0.8, 0.4, 1.2, 1.8, 1.5],
            EqPreset::StudioBright => [0.0, 0.0, -0.3, -0.5, 0.0, 0.5, 1.2, 1.8, 2.2, 2.5],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReplayGainMode {
    #[default]
    Off,
    Track,
    Album,
}

impl ReplayGainMode {
    pub const ALL: &'static [ReplayGainMode] = &[
        ReplayGainMode::Off,
        ReplayGainMode::Track,
        ReplayGainMode::Album,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ReplayGainMode::Off => "Off",
            ReplayGainMode::Track => "Track",
            ReplayGainMode::Album => "Album",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OversamplingMode {
    #[default]
    Native,
    PowerOf2,
    MaxDeviceRate,
}

impl OversamplingMode {
    pub const ALL: &'static [OversamplingMode] = &[
        OversamplingMode::Native,
        OversamplingMode::PowerOf2,
        OversamplingMode::MaxDeviceRate,
    ];

    pub fn name(self) -> &'static str {
        match self {
            OversamplingMode::Native => "Native (1:1)",
            OversamplingMode::PowerOf2 => "Power of 2",
            OversamplingMode::MaxDeviceRate => "Max DAC Rate",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            OversamplingMode::Native => "Keep native source sample rate (bit-matched when DAC supports it)",
            OversamplingMode::PowerOf2 => {
                "Synchronous integer upsampling (44.1→176.4 kHz, 48→192 kHz) up to DAC limit"
            }
            OversamplingMode::MaxDeviceRate => "Upsample every track to the highest rate supported by the DAC",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ResamplerQuality {
    #[default]
    LinearPhaseStudio,
    SharpPrecision,
    GentleAnalog,
}

impl ResamplerQuality {
    pub const ALL: &'static [ResamplerQuality] = &[
        ResamplerQuality::LinearPhaseStudio,
        ResamplerQuality::SharpPrecision,
        ResamplerQuality::GentleAnalog,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ResamplerQuality::LinearPhaseStudio => "Studio Linear",
            ResamplerQuality::SharpPrecision => "Sharp Precision",
            ResamplerQuality::GentleAnalog => "Gentle Analog",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            ResamplerQuality::LinearPhaseStudio => "256-tap Blackman-Harris2 · 0.95 cutoff",
            ResamplerQuality::SharpPrecision => "256-tap Hann2 · 0.97 steep cutoff",
            ResamplerQuality::GentleAnalog => "128-tap Blackman · 0.91 soft rolloff",
        }
    }
}

/// Output quantization dither mode (automatically bypassed in Bit-Perfect mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DitherMode {
    #[default]
    Off,
    Tpdf24,
    Tpdf16,
    NoiseShaped16,
}

impl DitherMode {
    pub const ALL: &'static [DitherMode] = &[
        DitherMode::Off,
        DitherMode::Tpdf24,
        DitherMode::Tpdf16,
        DitherMode::NoiseShaped16,
    ];

    pub fn name(self) -> &'static str {
        match self {
            DitherMode::Off => "Off",
            DitherMode::Tpdf24 => "24-bit TPDF",
            DitherMode::Tpdf16 => "16-bit TPDF",
            DitherMode::NoiseShaped16 => "16-bit Shaped",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            DitherMode::Off => "No output dither (pure 32-bit float to DAC / bit-direct)",
            DitherMode::Tpdf24 => {
                "Triangular PDF dither at 24-bit LSB (-141 dBFS) for post-DSP purity on 24-bit DACs"
            }
            DitherMode::Tpdf16 => {
                "Classic Red Book 16-bit TPDF dither — eliminates quantization harmonics on 16-bit DACs"
            }
            DitherMode::NoiseShaped16 => {
                "16-bit TPDF with 2nd-order psychoacoustic high-pass noise shaping (>14 kHz)"
            }
        }
    }
}

/// Classic audiophile headphone crossfeed topologies (head-shadow low-pass + opposite-ear feed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CrossfeedPreset {
    #[default]
    Gentle,
    ChuMoy,
    JanMeier,
    BauerStudio,
    Custom,
}

impl CrossfeedPreset {
    pub const SELECTABLE: &'static [CrossfeedPreset] = &[
        CrossfeedPreset::Gentle,
        CrossfeedPreset::ChuMoy,
        CrossfeedPreset::JanMeier,
        CrossfeedPreset::BauerStudio,
    ];

    pub fn name(self) -> &'static str {
        match self {
            CrossfeedPreset::Gentle => "Gentle",
            CrossfeedPreset::ChuMoy => "Chu Moy",
            CrossfeedPreset::JanMeier => "Jan Meier",
            CrossfeedPreset::BauerStudio => "Bauer Studio",
            CrossfeedPreset::Custom => "Custom",
        }
    }

    /// Returns `(lowpass_cutoff_hz, amount_0_to_1)` for this classic crossfeed profile.
    pub fn params(self) -> (f32, f32) {
        match self {
            CrossfeedPreset::Gentle => (700.0, 0.25),
            CrossfeedPreset::ChuMoy => (700.0, 0.42),
            CrossfeedPreset::JanMeier => (650.0, 0.58),
            CrossfeedPreset::BauerStudio => (700.0, 0.72),
            CrossfeedPreset::Custom => (700.0, 0.35),
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            CrossfeedPreset::Gentle => "700 Hz · 25% — subtle speaker-like width without narrowing",
            CrossfeedPreset::ChuMoy => "700 Hz · 42% — classic CMoy balanced headphone crossfeed",
            CrossfeedPreset::JanMeier => "650 Hz · 58% — natural Meier Corda acoustic soundstage",
            CrossfeedPreset::BauerStudio => "700 Hz · 72% — BS2B studio nearfield monitor simulation",
            CrossfeedPreset::Custom => "Custom crossfeed blend",
        }
    }
}

pub const MAX_PARAMETRIC_BANDS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ParametricFilterKind {
    #[default]
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
}

impl ParametricFilterKind {
    pub fn short_label(self) -> &'static str {
        match self {
            ParametricFilterKind::Peaking => "PK",
            ParametricFilterKind::LowShelf => "LS",
            ParametricFilterKind::HighShelf => "HS",
            ParametricFilterKind::LowPass => "LP",
            ParametricFilterKind::HighPass => "HP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ParametricBand {
    pub enabled: bool,
    pub kind: ParametricFilterKind,
    pub freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

impl Default for ParametricBand {
    fn default() -> Self {
        Self {
            enabled: true,
            kind: ParametricFilterKind::Peaking,
            freq_hz: 1000.0,
            gain_db: 0.0,
            q: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadphonePreset {
    HarmanOverEar,
    SennheiserHd650,
    SennheiserHd800S,
    HifimanSundara,
    WarmStudioTilt,
}

impl HeadphonePreset {
    pub const ALL: &'static [HeadphonePreset] = &[
        HeadphonePreset::HarmanOverEar,
        HeadphonePreset::SennheiserHd650,
        HeadphonePreset::SennheiserHd800S,
        HeadphonePreset::HifimanSundara,
        HeadphonePreset::WarmStudioTilt,
    ];

    pub fn name(self) -> &'static str {
        match self {
            HeadphonePreset::HarmanOverEar => "Harman Over-Ear",
            HeadphonePreset::SennheiserHd650 => "HD 650 (Oratory)",
            HeadphonePreset::SennheiserHd800S => "HD 800 S (Oratory)",
            HeadphonePreset::HifimanSundara => "Sundara (AutoEQ)",
            HeadphonePreset::WarmStudioTilt => "Warm Studio Tilt",
        }
    }

    pub fn profile(self) -> (String, f32, Vec<ParametricBand>) {
        let mk = |kind, freq_hz, gain_db, q| ParametricBand {
            enabled: true,
            kind,
            freq_hz,
            gain_db,
            q,
        };
        use ParametricFilterKind::*;
        match self {
            HeadphonePreset::HarmanOverEar => (
                "Harman Over-Ear Target".into(),
                -4.5,
                vec![
                    mk(LowShelf, 105.0, 4.5, 0.71),
                    mk(Peaking, 220.0, -1.5, 0.90),
                    mk(Peaking, 3000.0, 2.2, 1.40),
                    mk(HighShelf, 10000.0, -1.5, 0.71),
                ],
            ),
            HeadphonePreset::SennheiserHd650 => (
                "Sennheiser HD 650 (Oratory1990)".into(),
                -6.2,
                vec![
                    mk(LowShelf, 105.0, 5.5, 0.71),
                    mk(Peaking, 200.0, -2.5, 0.80),
                    mk(Peaking, 1400.0, -1.2, 2.20),
                    mk(Peaking, 2700.0, -1.4, 2.50),
                    mk(Peaking, 4400.0, 2.6, 2.20),
                    mk(HighShelf, 11000.0, 2.0, 0.71),
                ],
            ),
            HeadphonePreset::SennheiserHd800S => (
                "Sennheiser HD 800 S (Oratory1990)".into(),
                -6.6,
                vec![
                    mk(LowShelf, 105.0, 6.2, 0.71),
                    mk(Peaking, 210.0, -1.8, 1.10),
                    mk(Peaking, 2000.0, 2.4, 1.80),
                    mk(Peaking, 5800.0, -4.8, 3.20),
                    mk(Peaking, 10500.0, -2.2, 2.50),
                ],
            ),
            HeadphonePreset::HifimanSundara => (
                "Hifiman Sundara (AutoEQ)".into(),
                -5.8,
                vec![
                    mk(LowShelf, 105.0, 5.5, 0.71),
                    mk(Peaking, 2000.0, 2.8, 1.80),
                    mk(Peaking, 3600.0, -1.8, 2.60),
                    mk(Peaking, 8200.0, -3.2, 3.50),
                ],
            ),
            HeadphonePreset::WarmStudioTilt => (
                "Warm Studio Tilt".into(),
                -2.0,
                vec![
                    mk(LowShelf, 120.0, 2.0, 0.70),
                    mk(HighShelf, 6500.0, -2.2, 0.65),
                ],
            ),
        }
    }
}

/// Parses an EqualizerAPO / AutoEQ / Oratory1990 `.txt` parametric EQ file:
/// ```text
/// Preamp: -6.2 dB
/// Filter 1: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.71
/// Filter 2: ON PK Fc 200 Hz Gain -2.5 dB Q 0.80
/// ```
pub fn parse_autoeq_txt(text: &str, name_hint: &str) -> Result<(String, f32, Vec<ParametricBand>), String> {
    let mut preamp_db = 0.0_f32;
    let mut bands = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if let Some(rest) = upper.strip_prefix("PREAMP:") {
            let num = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<f32>().ok());
            if let Some(db) = num {
                preamp_db = db.clamp(-18.0, 12.0);
            }
            continue;
        }
        if upper.starts_with("FILTER")
            && let Some(colon) = line.find(':')
        {
            let tokens: Vec<&str> = line[colon + 1..].split_whitespace().collect();
            if tokens.len() < 4 {
                continue;
            }
            let mut idx = 0usize;
            let enabled = if tokens[idx].eq_ignore_ascii_case("ON") {
                idx += 1;
                true
            } else if tokens[idx].eq_ignore_ascii_case("OFF") {
                idx += 1;
                false
            } else {
                true
            };
            if idx >= tokens.len() {
                continue;
            }
            let kind = match tokens[idx].to_ascii_uppercase().as_str() {
                "PK" | "PEQ" | "PEAKING" => ParametricFilterKind::Peaking,
                "LS" | "LSC" | "LSQ" | "LOWSHELF" => ParametricFilterKind::LowShelf,
                "HS" | "HSC" | "HSQ" | "HIGHSHELF" => ParametricFilterKind::HighShelf,
                "LP" | "LPQ" | "LOWPASS" => ParametricFilterKind::LowPass,
                "HP" | "HPQ" | "HIGHPASS" => ParametricFilterKind::HighPass,
                _ => continue,
            };
            idx += 1;

            let mut freq_hz: Option<f32> = None;
            let mut gain_db: f32 = 0.0;
            let mut q: f32 = 0.707;
            while idx < tokens.len() {
                let tok = tokens[idx].to_ascii_uppercase();
                if tok == "FC" && idx + 1 < tokens.len() {
                    if let Ok(v) = tokens[idx + 1].parse::<f32>() {
                        freq_hz = Some(v.clamp(16.0, 22_000.0));
                    }
                    idx += 2;
                } else if tok == "GAIN" && idx + 1 < tokens.len() {
                    if let Ok(v) = tokens[idx + 1].parse::<f32>() {
                        gain_db = v.clamp(-18.0, 18.0);
                    }
                    idx += 2;
                } else if tok == "Q" && idx + 1 < tokens.len() {
                    if let Ok(v) = tokens[idx + 1].parse::<f32>() {
                        q = v.clamp(0.15, 18.0);
                    }
                    idx += 2;
                } else {
                    idx += 1;
                }
            }

            if let Some(fc) = freq_hz
                && bands.len() < MAX_PARAMETRIC_BANDS
            {
                bands.push(ParametricBand {
                    enabled,
                    kind,
                    freq_hz: fc,
                    gain_db,
                    q,
                });
            }
        }
    }

    if bands.is_empty() {
        return Err("No valid AutoEQ / EqualizerAPO Filter lines found in file".into());
    }
    let name = if name_hint.trim().is_empty() {
        "Imported AutoEQ".to_string()
    } else {
        name_hint.trim().to_string()
    };
    Ok((name, preamp_db, bands))
}

/// Per-output device saved audio profile (automatically applied when switching DAC / Headphones / Speakers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceProfile {
    #[serde(default)]
    pub exclusive_mode: bool,
    #[serde(default)]
    pub bit_perfect_mode: bool,
    #[serde(default)]
    pub direct_volume_lock: bool,
    #[serde(default)]
    pub oversampling: OversamplingMode,
    #[serde(default)]
    pub resampler_quality: ResamplerQuality,
    #[serde(default)]
    pub dsp_enabled: bool,
    #[serde(default)]
    pub crossfeed_enabled: bool,
    #[serde(default)]
    pub crossfeed_preset: CrossfeedPreset,
    #[serde(default)]
    pub dither: DitherMode,
    #[serde(default)]
    pub peq_enabled: bool,
    #[serde(default)]
    pub peq_profile_name: String,
    #[serde(default)]
    pub peq_preamp_db: f32,
    #[serde(default)]
    pub peq_bands: Vec<ParametricBand>,
}

impl DeviceProfile {
    pub fn capture_from(dsp: &DspConfig) -> Self {
        Self {
            exclusive_mode: dsp.exclusive_mode,
            bit_perfect_mode: dsp.bit_perfect_mode,
            direct_volume_lock: dsp.direct_volume_lock,
            oversampling: dsp.oversampling,
            resampler_quality: dsp.resampler_quality,
            dsp_enabled: dsp.enabled,
            crossfeed_enabled: dsp.crossfeed_enabled,
            crossfeed_preset: dsp.crossfeed_preset,
            dither: dsp.dither,
            peq_enabled: dsp.peq_enabled,
            peq_profile_name: dsp.peq_profile_name.clone(),
            peq_preamp_db: dsp.peq_preamp_db,
            peq_bands: dsp.peq_bands.clone(),
        }
    }

    pub fn apply_to(&self, dsp: &mut DspConfig) {
        dsp.exclusive_mode = self.exclusive_mode;
        dsp.bit_perfect_mode = self.bit_perfect_mode;
        dsp.direct_volume_lock = self.direct_volume_lock;
        dsp.oversampling = self.oversampling;
        dsp.resampler_quality = self.resampler_quality;
        dsp.enabled = self.dsp_enabled;
        dsp.crossfeed_enabled = self.crossfeed_enabled;
        if self.crossfeed_preset != CrossfeedPreset::Custom {
            let (fc, amt) = self.crossfeed_preset.params();
            dsp.crossfeed_preset = self.crossfeed_preset;
            dsp.crossfeed_cutoff_hz = fc;
            dsp.crossfeed_amount = amt;
        }
        dsp.dither = self.dither;
        dsp.peq_enabled = self.peq_enabled;
        dsp.peq_profile_name = self.peq_profile_name.clone();
        dsp.peq_preamp_db = self.peq_preamp_db;
        dsp.peq_bands = self.peq_bands.clone();
    }

    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.exclusive_mode {
            parts.push("Exclusive".to_string());
        }
        if self.bit_perfect_mode {
            parts.push("Bit-Perfect".to_string());
        }
        if self.direct_volume_lock {
            parts.push("0 dB Lock".to_string());
        }
        if self.peq_enabled && !self.peq_profile_name.is_empty() {
            parts.push(self.peq_profile_name.clone());
        } else if self.dsp_enabled && self.crossfeed_enabled {
            parts.push("Crossfeed".to_string());
        }
        if parts.is_empty() {
            "Standard".into()
        } else {
            parts.join(" · ")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DspConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub eq_enabled: bool,
    #[serde(default)]
    pub eq_preset: EqPreset,
    #[serde(default = "default_eq_bands")]
    pub eq_bands_db: [f32; 10],
    #[serde(default)]
    pub crossfeed_enabled: bool,
    #[serde(default)]
    pub crossfeed_preset: CrossfeedPreset,
    #[serde(default = "default_crossfeed")]
    pub crossfeed_amount: f32,
    #[serde(default = "default_crossfeed_cutoff")]
    pub crossfeed_cutoff_hz: f32,
    #[serde(default)]
    pub dither: DitherMode,
    #[serde(default = "default_true")]
    pub soft_limiter: bool,
    #[serde(default)]
    pub preamp_db: f32,
    // Gapless, Crossfade, ReplayGain, and Bit-Perfect / Audirvāna-Class Engine settings
    #[serde(default = "default_true")]
    pub gapless_enabled: bool,
    #[serde(default)]
    pub crossfade_secs: f32,
    #[serde(default)]
    pub replay_gain: ReplayGainMode,
    #[serde(default)]
    pub rg_preamp_db: f32,
    #[serde(default = "default_true")]
    pub rg_prevent_clipping: bool,
    #[serde(default)]
    pub bit_perfect_mode: bool,
    #[serde(default)]
    pub exclusive_mode: bool,
    #[serde(default = "default_true")]
    pub ram_playback: bool,
    #[serde(default = "default_ram_mb")]
    pub ram_buffer_mb: u32,
    #[serde(default)]
    pub oversampling: OversamplingMode,
    #[serde(default)]
    pub resampler_quality: ResamplerQuality,
    #[serde(default)]
    pub direct_volume_lock: bool,
    #[serde(default)]
    pub dsd_dop: bool,
    // Mastering Console Controls (64-bit f64)
    #[serde(default)]
    pub balance: f32, // -1.0 (Full Left) .. 0.0 (Center) .. +1.0 (Full Right)
    #[serde(default)]
    pub mono_fold: bool,
    #[serde(default)]
    pub invert_polarity: bool,
    #[serde(default)]
    pub swap_channels: bool,
    // Real-Time Partitioned FFT Convolution Engine (Room / Headphone / Studio .wav IR)
    #[serde(default)]
    pub ir_enabled: bool,
    #[serde(default)]
    pub ir_preset: IrPreset,
    #[serde(default)]
    pub ir_path: Option<PathBuf>,
    #[serde(default)]
    pub ir_name: String,
    #[serde(default = "default_ir_mix")]
    pub ir_mix: f32,
    #[serde(default)]
    pub ir_gain_db: f32,
    // Parametric EQ / AutoEQ Headphone Correction Engine
    #[serde(default)]
    pub peq_enabled: bool,
    #[serde(default)]
    pub peq_profile_name: String,
    #[serde(default)]
    pub peq_preamp_db: f32,
    #[serde(default)]
    pub peq_bands: Vec<ParametricBand>,
    // Pitch-Preserving Practice Mode & Musical Tuning Reference
    #[serde(default = "default_true")]
    pub preserve_pitch: bool,
    #[serde(default)]
    pub pitch_semitones: f32,
    #[serde(default)]
    pub concert_tuning: ConcertTuning,
    /// Runtime playback speed hint synced from transport (used to compensate pitch when `preserve_pitch` is active)
    #[serde(skip, default = "default_one_f32")]
    pub playback_speed_hint: f32,
}

fn default_one_f32() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConcertTuning {
    #[default]
    Standard440,
    Verdi432,
    Baroque415,
}

impl ConcertTuning {
    pub const ALL: &'static [ConcertTuning] = &[
        ConcertTuning::Standard440,
        ConcertTuning::Verdi432,
        ConcertTuning::Baroque415,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ConcertTuning::Standard440 => "A=440 Hz",
            ConcertTuning::Verdi432 => "A=432 Hz",
            ConcertTuning::Baroque415 => "A=415 Hz",
        }
    }

    /// Semitone offset relative to A=440 Hz (`12 * log2(f / 440)`).
    pub fn semitone_offset(self) -> f64 {
        match self {
            ConcertTuning::Standard440 => 0.0,
            ConcertTuning::Verdi432 => 12.0 * (432.0_f64 / 440.0).log2(),
            ConcertTuning::Baroque415 => 12.0 * (415.0_f64 / 440.0).log2(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_ram_mb() -> u32 {
    512
}

fn default_eq_bands() -> [f32; 10] {
    [0.0; 10]
}

fn default_crossfeed() -> f32 {
    0.35
}

/// Built-in & custom `.wav` Impulse Response convolution presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum IrPreset {
    #[default]
    Off,
    LinearWarmth,
    ChamberRoom,
    StudioAir,
    CustomWav,
}

impl IrPreset {
    pub const BUILTIN: &'static [IrPreset] = &[
        IrPreset::Off,
        IrPreset::LinearWarmth,
        IrPreset::ChamberRoom,
        IrPreset::StudioAir,
    ];

    pub fn name(self) -> &'static str {
        match self {
            IrPreset::Off => "Off",
            IrPreset::LinearWarmth => "Analog Warmth",
            IrPreset::ChamberRoom => "Intimate Chamber",
            IrPreset::StudioAir => "Mastering Air",
            IrPreset::CustomWav => "Custom .WAV IR",
        }
    }
}

fn default_ir_mix() -> f32 {
    1.0
}

fn default_crossfeed_cutoff() -> f32 {
    700.0
}

impl Default for DspConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            eq_enabled: true,
            eq_preset: EqPreset::Flat,
            eq_bands_db: [0.0; 10],
            crossfeed_enabled: false,
            crossfeed_preset: CrossfeedPreset::Gentle,
            crossfeed_amount: default_crossfeed(),
            crossfeed_cutoff_hz: default_crossfeed_cutoff(),
            dither: DitherMode::Off,
            soft_limiter: true,
            preamp_db: 0.0,
            gapless_enabled: true,
            crossfade_secs: 0.0,
            replay_gain: ReplayGainMode::Off,
            rg_preamp_db: 0.0,
            rg_prevent_clipping: true,
            bit_perfect_mode: false,
            exclusive_mode: false,
            ram_playback: true,
            ram_buffer_mb: default_ram_mb(),
            oversampling: OversamplingMode::Native,
            resampler_quality: ResamplerQuality::LinearPhaseStudio,
            direct_volume_lock: false,
            dsd_dop: false,
            balance: 0.0,
            mono_fold: false,
            invert_polarity: false,
            swap_channels: false,
            ir_enabled: false,
            ir_preset: IrPreset::Off,
            ir_path: None,
            ir_name: String::new(),
            ir_mix: default_ir_mix(),
            ir_gain_db: 0.0,
            peq_enabled: false,
            peq_profile_name: String::new(),
            peq_preamp_db: 0.0,
            peq_bands: Vec::new(),
            preserve_pitch: true,
            pitch_semitones: 0.0,
            concert_tuning: ConcertTuning::Standard440,
            playback_speed_hint: 1.0,
        }
    }
}

impl DspConfig {
    pub fn apply_preset(&mut self, preset: EqPreset) {
        self.eq_preset = preset;
        if preset != EqPreset::Custom {
            self.eq_bands_db = preset.gains_db();
        }
    }

    pub fn apply_crossfeed_preset(&mut self, preset: CrossfeedPreset) {
        self.crossfeed_preset = preset;
        if preset != CrossfeedPreset::Custom {
            let (cutoff_hz, amt) = preset.params();
            self.crossfeed_cutoff_hz = cutoff_hz;
            self.crossfeed_amount = amt;
        }
        self.crossfeed_enabled = true;
        self.enabled = true;
        self.bit_perfect_mode = false;
    }

    pub fn apply_headphone_preset(&mut self, preset: HeadphonePreset) {
        let (name, preamp, bands) = preset.profile();
        self.peq_profile_name = name;
        self.peq_preamp_db = preamp;
        self.peq_bands = bands;
        self.peq_enabled = true;
        self.enabled = true;
        self.bit_perfect_mode = false;
    }

    pub fn is_actively_processing(&self) -> bool {
        if self.dither != DitherMode::Off {
            return true;
        }
        if !self.enabled {
            return false;
        }
        let eq_active = self.eq_enabled
            && (self.preamp_db.abs() > 0.05
                || self.eq_bands_db.iter().any(|g| g.abs() > 0.05));
        let peq_active = self.peq_enabled && self.peq_bands.iter().any(|b| b.enabled);
        let console_active = self.balance.abs() > 0.01
            || self.mono_fold
            || self.invert_polarity
            || self.swap_channels;
        let ir_active = self.ir_enabled && self.ir_preset != IrPreset::Off && self.ir_mix > 0.01;
        let pitch_active = self.pitch_semitones.abs() > 0.05
            || self.concert_tuning != ConcertTuning::Standard440
            || (self.preserve_pitch && (self.playback_speed_hint - 1.0).abs() > 0.01);
        eq_active
            || peq_active
            || (self.crossfeed_enabled && self.crossfeed_amount > 0.01)
            || console_active
            || ir_active
            || pitch_active
    }

    pub fn summary(&self) -> String {
        if !self.enabled && self.dither == DitherMode::Off {
            return "Bypassed (Bit-Direct)".into();
        }
        let mut parts = Vec::new();
        if self.enabled {
            if self.peq_enabled && !self.peq_bands.is_empty() {
                let lbl = if self.peq_profile_name.is_empty() {
                    format!("PEQ ({} bands)", self.peq_bands.len())
                } else {
                    self.peq_profile_name.clone()
                };
                parts.push(lbl);
            }
            if self.ir_enabled && self.ir_preset != IrPreset::Off && self.ir_mix > 0.01 {
                let ir_lbl = if self.ir_preset == IrPreset::CustomWav && !self.ir_name.is_empty() {
                    format!("IR ({})", self.ir_name)
                } else {
                    format!("IR ({})", self.ir_preset.name())
                };
                parts.push(ir_lbl);
            }
            if self.eq_enabled
                && (self.preamp_db.abs() > 0.05
                    || self.eq_bands_db.iter().any(|g| g.abs() > 0.05))
            {
                parts.push(format!("EQ ({})", self.eq_preset.name()));
            }
            if self.crossfeed_enabled {
                if self.crossfeed_preset == CrossfeedPreset::Custom {
                    parts.push(format!("Crossfeed {:.0}%", self.crossfeed_amount * 100.0));
                } else {
                    parts.push(format!(
                        "Crossfeed {} ({:.0}%)",
                        self.crossfeed_preset.name(),
                        self.crossfeed_amount * 100.0
                    ));
                }
            }
            if self.mono_fold {
                parts.push("Mono".into());
            }
            if self.invert_polarity {
                parts.push("Ø Invert".into());
            }
            if self.swap_channels {
                parts.push("L↔R".into());
            }
            if self.balance.abs() > 0.02 {
                parts.push(format!("Bal {:+.0}%", self.balance * 100.0));
            }
            if self.pitch_semitones.abs() > 0.05 {
                parts.push(format!("Pitch {:+.1} st", self.pitch_semitones));
            }
            if self.concert_tuning != ConcertTuning::Standard440 {
                parts.push(self.concert_tuning.name().into());
            }
            if self.preserve_pitch && (self.playback_speed_hint - 1.0).abs() > 0.01 {
                parts.push("Pitch-Lock".into());
            }
            if self.soft_limiter {
                parts.push("Limiter".into());
            }
        }
        if self.dither != DitherMode::Off {
            parts.push(self.dither.name().into());
        }
        if parts.is_empty() {
            "Active (Flat)".into()
        } else {
            parts.join(" · ")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizerPluginConfig {
    #[serde(default = "default_true")]
    pub enable_piano_harmonic: bool,
    #[serde(default = "default_true")]
    pub enable_piano_roll: bool,
    #[serde(default = "default_true")]
    pub enable_concert_spectrum: bool,
    #[serde(default = "default_true")]
    pub enable_spectrogram: bool,
    #[serde(default = "default_true")]
    pub enable_stereo_scope: bool,
    #[serde(default = "default_true")]
    pub enable_mastering_vu: bool,
    #[serde(default)]
    pub piano_engine: PianoEngineId,
    #[serde(default = "default_true")]
    pub show_chord_readout: bool,
    #[serde(default = "default_true")]
    pub harmonic_cancellation: bool,
}

impl Default for VisualizerPluginConfig {
    fn default() -> Self {
        Self {
            enable_piano_harmonic: true,
            enable_piano_roll: true,
            enable_concert_spectrum: true,
            enable_spectrogram: true,
            enable_stereo_scope: true,
            enable_mastering_vu: true,
            piano_engine: PianoEngineId::PrecisionCqt,
            show_chord_readout: true,
            harmonic_cancellation: true,
        }
    }
}

impl VisualizerPluginConfig {
    pub fn is_mode_enabled(&self, mode: VisualizerMode) -> bool {
        match mode {
            VisualizerMode::PianoHarmonic => self.enable_piano_harmonic,
            VisualizerMode::PianoRoll => self.enable_piano_roll,
            VisualizerMode::ConcertSpectrum => self.enable_concert_spectrum,
            VisualizerMode::Spectrogram => self.enable_spectrogram,
            VisualizerMode::StereoScope => self.enable_stereo_scope,
            VisualizerMode::MasteringVu => self.enable_mastering_vu,
        }
    }

    pub fn enabled_modes(&self) -> Vec<VisualizerMode> {
        let mut out: Vec<VisualizerMode> = VisualizerMode::ALL
            .iter()
            .copied()
            .filter(|m| self.is_mode_enabled(*m))
            .collect();
        if out.is_empty() {
            out.push(VisualizerMode::PianoHarmonic);
        }
        out
    }

    pub fn next_enabled_mode(&self, current: VisualizerMode) -> VisualizerMode {
        let modes = self.enabled_modes();
        let idx = modes.iter().position(|&m| m == current).unwrap_or(0);
        modes[(idx + 1) % modes.len()]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagProviderConfig {
    #[serde(default = "default_true")]
    pub local_hash_cache: bool,
    #[serde(default = "default_true")]
    pub musicbrainz: bool,
    #[serde(default = "default_true")]
    pub itunes: bool,
    #[serde(default = "default_true")]
    pub acoustid: bool,
}

impl Default for TagProviderConfig {
    fn default() -> Self {
        Self {
            local_hash_cache: true,
            musicbrainz: true,
            itunes: true,
            acoustid: true,
        }
    }
}

/// Timestamp bookmark inside a track (ideal for symphonies, concertos, opera acts & podcasts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackBookmark {
    pub position_secs: f64,
    pub label: String,
}

/// Top-level pluggable modules configuration persisted in preferences.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub dsp: DspConfig,
    #[serde(default)]
    pub visualizers: VisualizerPluginConfig,
    #[serde(default)]
    pub tag_providers: TagProviderConfig,
    #[serde(default)]
    pub device_profiles: BTreeMap<String, DeviceProfile>,
    #[serde(default)]
    pub bookmarks: BTreeMap<String, Vec<TrackBookmark>>,
    #[serde(default)]
    pub lan_remote_enabled: bool,
    #[serde(default = "default_remote_port")]
    pub lan_remote_port: u16,
}

fn default_remote_port() -> u16 {
    7878
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            dsp: DspConfig::default(),
            visualizers: VisualizerPluginConfig::default(),
            tag_providers: TagProviderConfig::default(),
            device_profiles: BTreeMap::new(),
            bookmarks: BTreeMap::new(),
            lan_remote_enabled: false,
            lan_remote_port: default_remote_port(),
        }
    }
}

impl PluginConfig {
    pub fn bookmark_key(path: &std::path::Path, cue_start: Option<f64>) -> String {
        match cue_start {
            Some(s) => format!("{}@{:.1}", path.to_string_lossy(), s),
            None => path.to_string_lossy().into_owned(),
        }
    }

    pub fn bookmarks_for(&self, key: &str) -> &[TrackBookmark] {
        self.bookmarks.get(key).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn add_bookmark(&mut self, key: String, position_secs: f64, label: String) {
        let pos = position_secs.max(0.0);
        let list = self.bookmarks.entry(key).or_default();
        // Replace existing bookmark if within 1.5s to prevent accidental double-tap duplicates
        if let Some(existing) = list.iter_mut().find(|b| (b.position_secs - pos).abs() < 1.5) {
            existing.position_secs = pos;
            existing.label = label;
        } else {
            list.push(TrackBookmark {
                position_secs: pos,
                label,
            });
            list.sort_by(|a, b| a.position_secs.total_cmp(&b.position_secs));
        }
    }

    pub fn remove_bookmark(&mut self, key: &str, idx: usize) {
        if let Some(list) = self.bookmarks.get_mut(key) {
            if idx < list.len() {
                list.remove(idx);
            }
            if list.is_empty() {
                self.bookmarks.remove(key);
            }
        }
    }
}

/// Runtime external tool health detected at startup for Settings -> Plugins & Modules.
#[derive(Debug, Clone, Default)]
pub struct ToolHealth {
    pub ffmpeg_path: Option<PathBuf>,
    pub fpcalc_path: Option<PathBuf>,
    pub curl_available: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_config_deserializes_empty_json_with_defaults() {
        let cfg: PluginConfig = serde_json::from_str("{}").expect("default deserialization");
        assert_eq!(cfg.visualizers.piano_engine, PianoEngineId::PrecisionCqt);
        assert!(cfg.visualizers.harmonic_cancellation);
        assert!(cfg.tag_providers.musicbrainz);
        assert!(!cfg.dsp.enabled);
        assert_eq!(cfg.visualizers.enabled_modes().len(), VisualizerMode::ALL.len());
    }

    #[test]
    fn visualizer_next_enabled_mode_skips_disabled() {
        let vis = VisualizerPluginConfig {
            enable_piano_roll: false,
            enable_concert_spectrum: false,
            enable_spectrogram: false,
            ..Default::default()
        };
        assert_eq!(
            vis.next_enabled_mode(VisualizerMode::PianoHarmonic),
            VisualizerMode::StereoScope
        );
    }

    #[test]
    fn parses_autoeq_oratory1990_parametric_profile() {
        let sample = r#"
Preamp: -6.4 dB
Filter 1: ON LSC Fc 105 Hz Gain 5.5 dB Q 0.71
Filter 2: ON PK Fc 195 Hz Gain -2.4 dB Q 0.90
Filter 3: OFF PK Fc 1200 Hz Gain 1.0 dB Q 2.0
Filter 4: ON HSC Fc 10000 Hz Gain -3.0 dB Q 0.71
"#;
        let (name, preamp, bands) = parse_autoeq_txt(sample, "HD 650").unwrap();
        assert_eq!(name, "HD 650");
        assert!((preamp - (-6.4)).abs() < 1e-3);
        assert_eq!(bands.len(), 4);
        assert_eq!(bands[0].kind, ParametricFilterKind::LowShelf);
        assert_eq!(bands[1].kind, ParametricFilterKind::Peaking);
        assert!(!bands[2].enabled);
        assert_eq!(bands[3].kind, ParametricFilterKind::HighShelf);
    }

    #[test]
    fn bookmarks_sort_dedupe_and_remove_cleanly() {
        let mut cfg = PluginConfig::default();
        let key = PluginConfig::bookmark_key(std::path::Path::new("/music/mahler9.flac"), None);
        cfg.add_bookmark(key.clone(), 420.0, "Climax".into());
        cfg.add_bookmark(key.clone(), 95.0, "Adagio theme".into());
        cfg.add_bookmark(key.clone(), 95.5, "Adagio reprise".into());
        let list = cfg.bookmarks_for(&key);
        assert_eq!(list.len(), 2);
        assert!((list[0].position_secs - 95.5).abs() < 1e-6);
        assert_eq!(list[0].label, "Adagio reprise");
        assert!((list[1].position_secs - 420.0).abs() < 1e-6);
        cfg.remove_bookmark(&key, 0);
        assert_eq!(cfg.bookmarks_for(&key).len(), 1);
    }
}
