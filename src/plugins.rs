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
    #[serde(default = "default_crossfeed")]
    pub crossfeed_amount: f32,
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

impl Default for DspConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            eq_enabled: true,
            eq_preset: EqPreset::Flat,
            eq_bands_db: [0.0; 10],
            crossfeed_enabled: false,
            crossfeed_amount: default_crossfeed(),
            soft_limiter: true,
            preamp_db: 0.0,
            gapless_enabled: true,
            crossfade_secs: 0.0,
            replay_gain: ReplayGainMode::Off,
            rg_preamp_db: 0.0,
            rg_prevent_clipping: true,
            bit_perfect_mode: false,
            ram_playback: true,
            ram_buffer_mb: default_ram_mb(),
            oversampling: OversamplingMode::Native,
            resampler_quality: ResamplerQuality::LinearPhaseStudio,
            direct_volume_lock: false,
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

    pub fn is_actively_processing(&self) -> bool {
        if !self.enabled {
            return false;
        }
        let eq_active = self.eq_enabled
            && (self.preamp_db.abs() > 0.05
                || self.eq_bands_db.iter().any(|g| g.abs() > 0.05));
        eq_active || (self.crossfeed_enabled && self.crossfeed_amount > 0.01)
    }

    pub fn summary(&self) -> String {
        if !self.enabled {
            return "Bypassed (Bit-Direct)".into();
        }
        let mut parts = Vec::new();
        if self.eq_enabled {
            parts.push(format!("EQ ({})", self.eq_preset.name()));
        }
        if self.crossfeed_enabled {
            parts.push(format!("Crossfeed {:.0}%", self.crossfeed_amount * 100.0));
        }
        if self.soft_limiter {
            parts.push("Limiter".into());
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

/// Top-level pluggable modules configuration persisted in preferences.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginConfig {
    #[serde(default)]
    pub dsp: DspConfig,
    #[serde(default)]
    pub visualizers: VisualizerPluginConfig,
    #[serde(default)]
    pub tag_providers: TagProviderConfig,
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
            ..Default::default()
        };
        assert_eq!(
            vis.next_enabled_mode(VisualizerMode::PianoHarmonic),
            VisualizerMode::StereoScope
        );
    }
}
