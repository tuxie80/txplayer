use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::time::Instant;

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2, pos2, vec2,
};
use serde::{Deserialize, Serialize};

use crate::audio::{VISUAL_BUFFER_LEN, VisualTap};
use crate::plugins::{PianoEngineId, VisualizerPluginConfig};
use crate::theme::Palette;

const FFT_SIZE: usize = 4096;
pub const PIANO_KEYS: usize = 88; // MIDI 21 (A0 = 27.5 Hz) .. MIDI 108 (C8 = 4186.0 Hz)
const CQT_SEMITONES: usize = 108; // MIDI 21 .. MIDI 128 (extra 20 semitones for upper harmonic support)
pub const SPECTRUM_BANDS: usize = 64;
const SCOPE_SAMPLES: usize = 512;
const GONIO_POINTS: usize = 280;
const PIANO_ROLL_HISTORY: usize = 92;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VisualizerMode {
    #[default]
    PianoHarmonic,
    PianoRoll,
    ConcertSpectrum,
    StereoScope,
    MasteringVu,
}

impl VisualizerMode {
    pub const ALL: &'static [VisualizerMode] = &[
        VisualizerMode::PianoHarmonic,
        VisualizerMode::PianoRoll,
        VisualizerMode::ConcertSpectrum,
        VisualizerMode::StereoScope,
        VisualizerMode::MasteringVu,
    ];

    pub fn name(self) -> &'static str {
        match self {
            VisualizerMode::PianoHarmonic => "Piano & Harmonic Ribbon",
            VisualizerMode::PianoRoll => "88-Key Concert Piano Roll",
            VisualizerMode::ConcertSpectrum => "64-Band Concert Spectrum",
            VisualizerMode::StereoScope => "Stereo Scope & Phase",
            VisualizerMode::MasteringVu => "Mastering VU & Correlation",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            VisualizerMode::PianoHarmonic => "Piano",
            VisualizerMode::PianoRoll => "Roll",
            VisualizerMode::ConcertSpectrum => "Spectrum",
            VisualizerMode::StereoScope => "Phase",
            VisualizerMode::MasteringVu => "VU",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            VisualizerMode::PianoHarmonic => {
                "88-key harmonic envelope spline with live depressed keybed and chord readout."
            }
            VisualizerMode::PianoRoll => {
                "Scrolling real-time waterfall of detected piano notes rising from the 88-key keybed."
            }
            VisualizerMode::ConcertSpectrum => {
                "64-band logarithmic frequency analyzer (27.5 Hz – 18 kHz) with mirror reflection."
            }
            VisualizerMode::StereoScope => {
                "Zero-crossing triggered dual-channel oscilloscope paired with an M/S phase goniometer."
            }
            VisualizerMode::MasteringVu => {
                "Broadcast-style stereo RMS/Peak VU meters with dBFS and phase correlation bar."
            }
        }
    }
}

/// Precomputed windowed cosine/sine basis for one exact piano semitone in a decimated stage.
struct SemitoneKernel {
    stage: u8, // 0 = 1x, 1 = /4, 2 = /16
    len: usize,
    cos_w: Vec<f32>,
    sin_w: Vec<f32>,
    inv_norm: f32,
}

/// Multi-rate Constant-Q Semitone Analyzer with Piano Inharmonicity & Iterative Harmonic Cancellation.
struct PrecisionPianoAnalyzer {
    built_sample_rate: u32,
    kernels: Vec<SemitoneKernel>,
    raw_mono: Vec<f32>,
    stage1: Vec<f32>, // decimated /4 (or /8 for hi-res >=88.2kHz)
    stage2: Vec<f32>, // decimated /16 (or /32 for hi-res >=88.2kHz)
    semitone_mag: [f32; CQT_SEMITONES],
    prev_detected: [f32; PIANO_KEYS],
}

impl PrecisionPianoAnalyzer {
    fn new() -> Self {
        let mut s = Self {
            built_sample_rate: 0,
            kernels: Vec::with_capacity(CQT_SEMITONES),
            raw_mono: vec![0.0; VISUAL_BUFFER_LEN],
            stage1: vec![0.0; VISUAL_BUFFER_LEN / 4],
            stage2: vec![0.0; VISUAL_BUFFER_LEN / 16],
            semitone_mag: [0.0; CQT_SEMITONES],
            prev_detected: [0.0; PIANO_KEYS],
        };
        s.rebuild_kernels(44_100);
        s
    }

    fn rebuild_kernels(&mut self, sample_rate: u32) {
        let sr = sample_rate.max(8_000);
        if self.built_sample_rate == sr && self.kernels.len() == CQT_SEMITONES {
            return;
        }
        self.built_sample_rate = sr;
        self.kernels.clear();

        // For 88.2 / 96 / 192 kHz Hi-Res streams, double decimation factor so time windows stay musical
        let hires_mult = if sr >= 160_000 {
            4usize
        } else if sr >= 80_000 {
            2usize
        } else {
            1usize
        };
        let s0_sr = sr as f32;
        let s1_factor = 4 * hires_mult;
        let s2_factor = 16 * hires_mult;
        let s1_sr = sr as f32 / s1_factor as f32;
        let s2_sr = sr as f32 / s2_factor as f32;

        let s0_max = 2048usize.min(VISUAL_BUFFER_LEN);
        let s1_max = (VISUAL_BUFFER_LEN / s1_factor).max(256);
        let s2_max = (VISUAL_BUFFER_LEN / s2_factor).max(256);

        for semi in 0..CQT_SEMITONES {
            let midi = 21.0 + semi as f32;
            // Subtle Railsback piano stretch: bass slightly flat, extreme treble slightly sharp
            let base_freq = 440.0 * 2.0_f32.powf((midi - 69.0) / 12.0);
            let stretch_cents = ((midi - 60.0) / 48.0).powi(3) * 14.0;
            let freq = base_freq * 2.0_f32.powf(stretch_cents / 1200.0);

            let (stage, stage_sr, max_len) = if freq < 280.0 {
                (2u8, s2_sr, s2_max)
            } else if freq < 1150.0 {
                (1u8, s1_sr, s1_max)
            } else {
                (0u8, s0_sr, s0_max)
            };

            // Q ~ 28 with Blackman-Harris window yields ~1 semitone main-lobe selectivity and -92 dB sidelobes
            let desired_len = (2.35 * stage_sr / (freq * 0.059_463)).round() as usize;
            let len = desired_len.clamp(96, max_len);

            let mut cos_w = Vec::with_capacity(len);
            let mut sin_w = Vec::with_capacity(len);
            let denom = (len.saturating_sub(1)).max(1) as f32;
            let omega = TAU * freq / stage_sr;
            let mut win_sum = 0.0_f32;
            for i in 0..len {
                let t = i as f32 / denom;
                let w = 0.35875
                    - 0.48829 * (TAU * t).cos()
                    + 0.14128 * (2.0 * TAU * t).cos()
                    - 0.01168 * (3.0 * TAU * t).cos();
                win_sum += w;
                let phase = omega * i as f32;
                cos_w.push(w * phase.cos());
                sin_w.push(w * phase.sin());
            }
            let inv_norm = 2.0 / win_sum.max(1.0);
            self.kernels.push(SemitoneKernel {
                stage,
                len,
                cos_w,
                sin_w,
                inv_norm,
            });
        }
    }

    /// Analyzes `mono_samples` (length `VISUAL_BUFFER_LEN`, newest samples at end) and writes
    /// `(note_strength[88], hammer_onset[88])`.
    fn analyze(
        &mut self,
        sample_rate: u32,
        harmonic_cancellation: bool,
        sensitivity: f32,
        out_levels: &mut [f32; PIANO_KEYS],
        out_onsets: &mut [f32; PIANO_KEYS],
    ) {
        self.rebuild_kernels(sample_rate);
        let sr = sample_rate.max(8_000);
        let hires_mult = if sr >= 160_000 {
            4usize
        } else if sr >= 80_000 {
            2usize
        } else {
            1usize
        };
        let s1_factor = 4 * hires_mult;
        let s2_factor = 16 * hires_mult;
        let s1_len = VISUAL_BUFFER_LEN / s1_factor;
        let s2_len = VISUAL_BUFFER_LEN / s2_factor;

        // Multi-rate anti-aliased decimation (triangular/boxcar cascade)
        self.stage1.resize(s1_len, 0.0);
        let inv1 = 1.0 / s1_factor as f32;
        for (i, slot) in self.stage1.iter_mut().enumerate() {
            let base = i * s1_factor;
            let mut sum = 0.0_f32;
            for j in 0..s1_factor {
                sum += self.raw_mono[base + j];
            }
            *slot = sum * inv1;
        }

        self.stage2.resize(s2_len, 0.0);
        let ratio_2_1 = s2_factor / s1_factor; // 4
        let inv2 = 1.0 / ratio_2_1 as f32;
        for (i, slot) in self.stage2.iter_mut().enumerate() {
            let base = i * ratio_2_1;
            let mut sum = 0.0_f32;
            for j in 0..ratio_2_1 {
                sum += self.stage1[base + j];
            }
            *slot = sum * inv2;
        }

        // 1) Compute exact windowed Constant-Q magnitude for all 108 semitones
        for (semi, kernel) in self.kernels.iter().enumerate() {
            let buf: &[f32] = match kernel.stage {
                0 => &self.raw_mono[VISUAL_BUFFER_LEN - kernel.len..],
                1 => &self.stage1[s1_len - kernel.len..],
                _ => &self.stage2[s2_len - kernel.len..],
            };
            let mut re = 0.0_f32;
            let mut im = 0.0_f32;
            for ((&x, &cw), &sw) in buf
                .iter()
                .zip(kernel.cos_w.iter())
                .zip(kernel.sin_w.iter())
                .take(kernel.len)
            {
                re += x * cw;
                im += x * sw;
            }
            self.semitone_mag[semi] = (re * re + im * im).sqrt() * kernel.inv_norm;
        }

        // 2) Local spectral noise-floor suppression (separates tonal piano peaks from broadband wash/reverb)
        let mut clean = [0.0_f32; CQT_SEMITONES];
        for (i, slot) in clean.iter_mut().enumerate() {
            let lo = i.saturating_sub(6);
            let hi = (i + 6).min(CQT_SEMITONES - 1);
            let mut vals = [0.0_f32; 13];
            let mut count = 0usize;
            for &v in &self.semitone_mag[lo..=hi] {
                vals[count] = v;
                count += 1;
            }
            vals[..count].sort_unstable_by(|a, b| a.total_cmp(b));
            let median = vals[count / 2];
            *slot = (self.semitone_mag[i] - median * 0.85).max(0.0);
        }

        // 3) Harmonic Comb + Iterative Harmonic Cancellation (eliminates false octave/5th ghost keys)
        let mut raw_note_amp = [0.0_f32; PIANO_KEYS];
        if harmonic_cancellation {
            // Semitone offsets for piano partials h = 1..=6 (fundamental, +12, +19, +24, +28, +31)
            const HARM_OFFSETS: [usize; 6] = [0, 12, 19, 24, 28, 31];
            const HARM_WEIGHTS_MID: [f32; 6] = [1.0, 0.58, 0.36, 0.22, 0.14, 0.09];
            // Acoustic bass strings (A0..C2) often radiate stronger 2nd & 3rd partials than fundamental
            const HARM_WEIGHTS_BASS: [f32; 6] = [0.75, 0.88, 0.55, 0.34, 0.20, 0.12];

            let mut residual = clean;
            let global_peak = residual.iter().copied().fold(0.0_f32, f32::max);
            let stop_floor = (global_peak * 0.08).max(0.0008);

            // Detect up to 10 simultaneous polyphonic voices (10 fingers!)
            for _voice in 0..10 {
                let mut best_key = None;
                let mut best_score = 0.0_f32;

                for k in 0..PIANO_KEYS {
                    let f0 = residual[k];
                    if f0 < stop_floor * 0.45 && k >= 18 {
                        continue;
                    }
                    let weights = if k < 18 {
                        &HARM_WEIGHTS_BASS
                    } else {
                        &HARM_WEIGHTS_MID
                    };

                    let mut salience = 0.0_f32;
                    let mut weight_sum = 0.0_f32;
                    let mut active_harmonics = 0u32;

                    for (hi, &off) in HARM_OFFSETS.iter().enumerate() {
                        let idx = k + off;
                        if idx < CQT_SEMITONES {
                            let mag = residual[idx];
                            // Local peak check: prefer true spectral peaks over shoulders of neighbor semitones
                            let left_m = if idx > 0 { residual[idx - 1] } else { 0.0 };
                            let right_m = if idx + 1 < CQT_SEMITONES {
                                residual[idx + 1]
                            } else {
                                0.0
                            };
                            let peak_bonus = if mag >= left_m * 0.92 && mag >= right_m * 0.92 {
                                1.0
                            } else {
                                0.45
                            };
                            salience += mag * weights[hi] * peak_bonus;
                            weight_sum += weights[hi];
                            if mag > stop_floor * 0.65 {
                                active_harmonics += 1;
                            }
                        }
                    }

                    if weight_sum > 0.0 {
                        let mut score = salience / weight_sum;
                        // Require fundamental presence for mid/upper notes, and harmonic corroboration below C6
                        if k >= 18 && f0 < stop_floor * 0.5 {
                            score *= 0.25;
                        }
                        if k < 60 && active_harmonics < 2 {
                            score *= 0.55;
                        }
                        // Mild upper-register perceptual lift so treble melody cuts through left-hand chords
                        let reg_tilt = 0.88 + (k as f32 / (PIANO_KEYS - 1) as f32) * 0.95;
                        score *= reg_tilt;

                        if score > best_score {
                            best_score = score;
                            best_key = Some(k);
                        }
                    }
                }

                let Some(k) = best_key else {
                    break;
                };
                if best_score < stop_floor {
                    break;
                }

                // Store detected fundamental amplitude (normalized back from tilt)
                let reg_tilt = 0.88 + (k as f32 / (PIANO_KEYS - 1) as f32) * 0.95;
                let true_amp = best_score / reg_tilt;
                raw_note_amp[k] = raw_note_amp[k].max(true_amp * 1.45);

                // Subtract this note's harmonic template from the residual spectrum
                let weights = if k < 18 {
                    &HARM_WEIGHTS_BASS
                } else {
                    &HARM_WEIGHTS_MID
                };
                let fund_energy = residual[k].max(true_amp);
                for (hi, &off) in HARM_OFFSETS.iter().enumerate() {
                    let idx = k + off;
                    if idx < CQT_SEMITONES {
                        let sub = fund_energy * weights[hi] * 1.15;
                        residual[idx] = (residual[idx] - sub).max(0.0);
                        // Also damp immediate semitone bleed around cancelled overtones
                        if hi > 0 {
                            if idx > 0 {
                                residual[idx - 1] = (residual[idx - 1] - sub * 0.35).max(0.0);
                            }
                            if idx + 1 < CQT_SEMITONES {
                                residual[idx + 1] = (residual[idx + 1] - sub * 0.35).max(0.0);
                            }
                        }
                    }
                }
            }
        } else {
            for k in 0..PIANO_KEYS {
                raw_note_amp[k] = clean[k] * 1.3;
            }
        }

        // 4) Convert to perceptual meter (0..1) + compute hammer-strike positive onset flux
        for k in 0..PIANO_KEYS {
            let key_frac = k as f32 / (PIANO_KEYS - 1) as f32;
            let tilt = 0.92 + key_frac * 1.15;
            let level = amp_to_meter(raw_note_amp[k] * tilt, sensitivity * 1.25);
            let flux = (level - self.prev_detected[k]).max(0.0);
            self.prev_detected[k] = level;
            out_levels[k] = level;
            out_onsets[k] = if flux > 0.14 {
                (flux * 2.2).clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
    }
}

pub struct VisualizerState {
    pub mode: VisualizerMode,
    pub show_piano_keys: bool,
    pub sensitivity: f32,
    pub last_compute_ms: f32,
    paused_freeze: bool,
    mono_buf: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    window: Vec<f32>,
    fft_twiddle_re: Vec<f32>,
    fft_twiddle_im: Vec<f32>,
    mag_db: Vec<f32>,
    precision_piano: PrecisionPianoAnalyzer,
    /// 88-key concert piano energies (0.0..1.0)
    piano_levels: [f32; PIANO_KEYS],
    piano_peaks: [f32; PIANO_KEYS],
    /// Hammer-strike onset flash per key (0.0..1.0, decays swiftly)
    piano_onsets: [f32; PIANO_KEYS],
    /// Rolling history for 88-key Piano Roll waterfall
    piano_roll: VecDeque<[u8; PIANO_KEYS]>,
    roll_accum_dt: f32,
    /// Human-readable active chord + top notes (e.g. "Cmaj7  ·  C3 E3 G3 B4")
    detected_harmony: String,
    /// 64-band high-definition logarithmic spectrum (27.5 Hz .. 18 kHz)
    spectrum_bands: [f32; SPECTRUM_BANDS],
    spectrum_peaks: [f32; SPECTRUM_BANDS],
    /// Stereo waveform buffers
    scope_l: Vec<f32>,
    scope_r: Vec<f32>,
    /// Stereo mastering VU + phase correlation
    vu_left: f32,
    vu_right: f32,
    vu_peak_left: f32,
    vu_peak_right: f32,
    rms_db_l: f32,
    rms_db_r: f32,
    correlation: f32,
}

impl VisualizerState {
    pub fn new(mode: VisualizerMode, show_piano_keys: bool, sensitivity: f32) -> Self {
        let mut window = vec![0.0_f32; FFT_SIZE];
        let denom = (FFT_SIZE - 1) as f32;
        for (i, w) in window.iter_mut().enumerate() {
            let t = i as f32 / denom;
            *w = 0.35875
                - 0.48829 * (TAU * t).cos()
                + 0.14128 * (2.0 * TAU * t).cos()
                - 0.01168 * (3.0 * TAU * t).cos();
        }
        // Precompute FFT twiddle tables once (eliminates per-frame trig calls!)
        let mut fft_twiddle_re = Vec::with_capacity(FFT_SIZE / 2);
        let mut fft_twiddle_im = Vec::with_capacity(FFT_SIZE / 2);
        for k in 0..(FFT_SIZE / 2) {
            let angle = -TAU * k as f32 / FFT_SIZE as f32;
            fft_twiddle_re.push(angle.cos());
            fft_twiddle_im.push(angle.sin());
        }

        Self {
            mode,
            show_piano_keys,
            sensitivity: sensitivity.clamp(0.5, 2.0),
            last_compute_ms: 0.0,
            paused_freeze: false,
            mono_buf: vec![0.0; FFT_SIZE],
            re: vec![0.0; FFT_SIZE],
            im: vec![0.0; FFT_SIZE],
            window,
            fft_twiddle_re,
            fft_twiddle_im,
            mag_db: vec![0.0; FFT_SIZE / 2],
            precision_piano: PrecisionPianoAnalyzer::new(),
            piano_levels: [0.0; PIANO_KEYS],
            piano_peaks: [0.0; PIANO_KEYS],
            piano_onsets: [0.0; PIANO_KEYS],
            piano_roll: VecDeque::with_capacity(PIANO_ROLL_HISTORY + 2),
            roll_accum_dt: 0.0,
            detected_harmony: String::new(),
            spectrum_bands: [0.0; SPECTRUM_BANDS],
            spectrum_peaks: [0.0; SPECTRUM_BANDS],
            scope_l: vec![0.0; SCOPE_SAMPLES],
            scope_r: vec![0.0; SCOPE_SAMPLES],
            vu_left: 0.0,
            vu_right: 0.0,
            vu_peak_left: 0.0,
            vu_peak_right: 0.0,
            rms_db_l: -60.0,
            rms_db_r: -60.0,
            correlation: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.paused_freeze = false;
        self.piano_levels = [0.0; PIANO_KEYS];
        self.piano_peaks = [0.0; PIANO_KEYS];
        self.piano_onsets = [0.0; PIANO_KEYS];
        self.piano_roll.clear();
        self.detected_harmony.clear();
        self.spectrum_bands = [0.0; SPECTRUM_BANDS];
        self.spectrum_peaks = [0.0; SPECTRUM_BANDS];
        self.scope_l.fill(0.0);
        self.scope_r.fill(0.0);
        self.vu_left = 0.0;
        self.vu_right = 0.0;
        self.vu_peak_left = 0.0;
        self.vu_peak_right = 0.0;
    }

    pub fn update(
        &mut self,
        tap: &VisualTap,
        dt: f32,
        plugin_cfg: &VisualizerPluginConfig,
        paused: bool,
    ) {
        let t_start = Instant::now();
        let dt = dt.clamp(0.004, 0.1);
        let decay = (-dt * 7.2).exp();
        let piano_decay = (-dt * 5.6).exp();
        let onset_decay = (-dt * 12.5).exp();
        let peak_drop = dt * 0.28;
        let attack = 1.0 - (-dt * 34.0).exp();

        if paused {
            // Freeze pressed piano keys, chord badge, piano roll & spectrum so users can inspect exact notes while paused!
            self.paused_freeze = true;
            return;
        }
        self.paused_freeze = false;

        if !tap.is_active() {
            for i in 0..PIANO_KEYS {
                self.piano_levels[i] *= piano_decay;
                self.piano_onsets[i] *= onset_decay;
                self.piano_peaks[i] = (self.piano_peaks[i] - peak_drop).max(self.piano_levels[i]);
            }
            for i in 0..SPECTRUM_BANDS {
                self.spectrum_bands[i] *= decay;
                self.spectrum_peaks[i] =
                    (self.spectrum_peaks[i] - peak_drop).max(self.spectrum_bands[i]);
            }
            self.vu_left *= decay;
            self.vu_right *= decay;
            self.vu_peak_left = (self.vu_peak_left - peak_drop).max(self.vu_left);
            self.vu_peak_right = (self.vu_peak_right - peak_drop).max(self.vu_right);
            self.rms_db_l = -60.0;
            self.rms_db_r = -60.0;
            self.correlation *= decay;
            self.scope_l.fill(0.0);
            self.scope_r.fill(0.0);
            self.detected_harmony.clear();
            self.last_compute_ms *= 0.9;
            return;
        }

        // Only compute the DSP needed for the active visualizer mode!
        let needs_piano = matches!(
            self.mode,
            VisualizerMode::PianoHarmonic | VisualizerMode::PianoRoll
        );
        let needs_spectrum = self.mode == VisualizerMode::ConcertSpectrum;
        let needs_scope = self.mode == VisualizerMode::StereoScope;
        let needs_vu = self.mode == VisualizerMode::MasteringVu;

        if needs_vu {
            let (rms_l, rms_r, peak_l, peak_r, corr) = tap.levels();
            self.rms_db_l = if rms_l > 1e-4 {
                (20.0 * rms_l.log10()).max(-60.0)
            } else {
                -60.0
            };
            self.rms_db_r = if rms_r > 1e-4 {
                (20.0 * rms_r.log10()).max(-60.0)
            } else {
                -60.0
            };
            let target_l = db_to_meter(self.rms_db_l + 3.0);
            let target_r = db_to_meter(self.rms_db_r + 3.0);
            let target_pk_l = amp_to_meter(peak_l, self.sensitivity);
            let target_pk_r = amp_to_meter(peak_r, self.sensitivity);

            self.vu_left = smooth(self.vu_left, target_l, attack, decay);
            self.vu_right = smooth(self.vu_right, target_r, attack, decay);
            self.vu_peak_left = (self.vu_peak_left - peak_drop * 0.65)
                .max(target_pk_l)
                .max(self.vu_left);
            self.vu_peak_right = (self.vu_peak_right - peak_drop * 0.65)
                .max(target_pk_r)
                .max(self.vu_right);
            self.correlation += (corr - self.correlation) * (1.0 - (-dt * 12.0).exp());
        }

        if needs_scope {
            tap.read_latest_stereo(&mut self.scope_l, &mut self.scope_r);
        }

        let sample_rate = tap.sample_rate();

        if needs_piano {
            match plugin_cfg.piano_engine {
                PianoEngineId::PrecisionCqt => {
                    tap.read_latest_mono(&mut self.precision_piano.raw_mono);
                    let mut target_levels = [0.0_f32; PIANO_KEYS];
                    let mut target_onsets = [0.0_f32; PIANO_KEYS];
                    self.precision_piano.analyze(
                        sample_rate,
                        plugin_cfg.harmonic_cancellation,
                        self.sensitivity,
                        &mut target_levels,
                        &mut target_onsets,
                    );
                    for key in 0..PIANO_KEYS {
                        self.piano_levels[key] = smooth(
                            self.piano_levels[key],
                            target_levels[key],
                            attack,
                            piano_decay,
                        );
                        self.piano_onsets[key] =
                            (self.piano_onsets[key] * onset_decay).max(target_onsets[key]);
                        if self.piano_levels[key] >= self.piano_peaks[key] {
                            self.piano_peaks[key] = self.piano_levels[key];
                        } else {
                            self.piano_peaks[key] =
                                (self.piano_peaks[key] - peak_drop).max(self.piano_levels[key]);
                        }
                    }
                }
                PianoEngineId::ClassicFft => {
                    self.run_fft(tap);
                    let bin_hz = sample_rate as f32 / FFT_SIZE as f32;
                    for key in 0..PIANO_KEYS {
                        let midi = 21.0 + key as f32;
                        let freq = 440.0 * 2.0_f32.powf((midi - 69.0) / 12.0);
                        let f_lo = freq * 2.0_f32.powf(-0.48 / 12.0);
                        let f_hi = freq * 2.0_f32.powf(0.48 / 12.0);

                        let exact_bin = freq / bin_hz;
                        let b_lo = ((f_lo / bin_hz).floor() as usize).clamp(1, FFT_SIZE / 2 - 2);
                        let b_hi = ((f_hi / bin_hz).ceil() as usize).clamp(b_lo, FFT_SIZE / 2 - 2);

                        let raw = if b_lo == b_hi {
                            let idx = (exact_bin.floor() as usize).clamp(1, FFT_SIZE / 2 - 2);
                            let frac = (exact_bin - idx as f32).clamp(0.0, 1.0);
                            let m0 = self.mag_db[idx];
                            let m1 = self.mag_db[idx + 1];
                            m0 + (m1 - m0) * frac
                        } else {
                            let mut peak = 0.0_f32;
                            let mut sum = 0.0_f32;
                            for k in b_lo..=b_hi {
                                let m = self.mag_db[k];
                                peak = peak.max(m);
                                sum += m;
                            }
                            peak * 0.78 + (sum / (b_hi - b_lo + 1) as f32) * 0.22
                        };

                        let key_frac = key as f32 / (PIANO_KEYS - 1) as f32;
                        let tilt = 0.85 + key_frac * 1.45;
                        let level = amp_to_meter(raw * tilt, self.sensitivity);
                        let prev = self.piano_levels[key];
                        self.piano_levels[key] = smooth(prev, level, attack, piano_decay);
                        self.piano_onsets[key] =
                            (self.piano_onsets[key] * onset_decay).max((level - prev).max(0.0) * 1.5);
                        if self.piano_levels[key] >= self.piano_peaks[key] {
                            self.piano_peaks[key] = self.piano_levels[key];
                        } else {
                            self.piano_peaks[key] =
                                (self.piano_peaks[key] - peak_drop).max(self.piano_levels[key]);
                        }
                    }
                }
            }

            if plugin_cfg.show_chord_readout {
                self.detected_harmony = detect_chord_and_notes(&self.piano_levels);
            } else {
                self.detected_harmony.clear();
            }

            // Push waterfall frame at ~35 Hz for smooth, readable Piano Roll motion
            self.roll_accum_dt += dt;
            if self.roll_accum_dt >= 0.028 {
                self.roll_accum_dt = 0.0;
                let mut slice = [0u8; PIANO_KEYS];
                for (k, slot) in slice.iter_mut().enumerate() {
                    let v = (self.piano_levels[k] * 0.8 + self.piano_onsets[k] * 0.35).clamp(0.0, 1.0);
                    *slot = (v * 255.0).round() as u8;
                }
                if self.piano_roll.len() >= PIANO_ROLL_HISTORY {
                    self.piano_roll.pop_front();
                }
                self.piano_roll.push_back(slice);
            }
        }

        if needs_spectrum {
            self.run_fft(tap);
            let sr_f = sample_rate as f32;
            let bin_hz = sr_f / FFT_SIZE as f32;
            let min_f = 27.5_f32;
            let max_f = (sr_f * 0.48).min(18_000.0);
            let ratio = (max_f / min_f).powf(1.0 / SPECTRUM_BANDS as f32);
            for band in 0..SPECTRUM_BANDS {
                let f_lo = min_f * ratio.powi(band as i32);
                let f_hi = min_f * ratio.powi((band + 1) as i32);
                let b_lo = ((f_lo / bin_hz).floor() as usize).clamp(1, FFT_SIZE / 2 - 2);
                let b_hi = ((f_hi / bin_hz).ceil() as usize).clamp(b_lo + 1, FFT_SIZE / 2 - 1);

                let mut peak = 0.0_f32;
                let mut sum = 0.0_f32;
                for k in b_lo..b_hi {
                    let m = self.mag_db[k];
                    peak = peak.max(m);
                    sum += m;
                }
                let avg = sum / (b_hi - b_lo).max(1) as f32;
                let tilt = 0.85 + (band as f32 / SPECTRUM_BANDS as f32) * 1.65;
                let level = amp_to_meter((peak * 0.72 + avg * 0.28) * tilt, self.sensitivity);
                self.spectrum_bands[band] = smooth(self.spectrum_bands[band], level, attack, decay);
                if self.spectrum_bands[band] >= self.spectrum_peaks[band] {
                    self.spectrum_peaks[band] = self.spectrum_bands[band];
                } else {
                    self.spectrum_peaks[band] =
                        (self.spectrum_peaks[band] - peak_drop).max(self.spectrum_bands[band]);
                }
            }
        }

        let elapsed_ms = t_start.elapsed().as_secs_f32() * 1000.0;
        self.last_compute_ms = self.last_compute_ms * 0.85 + elapsed_ms * 0.15;
    }

    fn run_fft(&mut self, tap: &VisualTap) {
        tap.read_latest_mono(&mut self.mono_buf);
        for i in 0..FFT_SIZE {
            self.re[i] = self.mono_buf[i] * self.window[i];
            self.im[i] = 0.0;
        }
        fft_radix2_precomputed(
            &mut self.re,
            &mut self.im,
            &self.fft_twiddle_re,
            &self.fft_twiddle_im,
        );
        let inv_norm = 1.0 / (FFT_SIZE as f32 * 0.18);
        for k in 0..(FFT_SIZE / 2) {
            self.mag_db[k] = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * inv_norm;
        }
    }

    /// Compact crisp visualizer widget inside the bottom transport deck.
    pub fn ui_compact(
        &mut self,
        ui: &mut egui::Ui,
        size: Vec2,
        palette: &Palette,
        plugin_cfg: &VisualizerPluginConfig,
    ) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        if response.clicked() {
            self.mode = plugin_cfg.next_enabled_mode(self.mode);
        }
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            6.0,
            if response.hovered() {
                palette.surface
            } else {
                palette.background
            },
        );
        painter.rect_stroke(
            rect,
            6.0,
            Stroke::new(
                1.0,
                if response.hovered() {
                    palette.border_strong
                } else {
                    palette.border
                },
            ),
            StrokeKind::Inside,
        );
        let inner = rect.shrink2(Vec2::new(6.0, 4.0));
        match self.mode {
            VisualizerMode::PianoHarmonic | VisualizerMode::PianoRoll => {
                self.paint_piano_ribbon(painter, inner, palette, false, false)
            }
            VisualizerMode::ConcertSpectrum => {
                self.paint_concert_spectrum(painter, inner, palette, false)
            }
            VisualizerMode::StereoScope => self.paint_oscilloscope(painter, inner, palette),
            VisualizerMode::MasteringVu => self.paint_vu_compact(painter, inner, palette),
        }
        response.on_hover_text(format!(
            "{} · Click to switch visualizer mode",
            self.mode.name()
        ))
    }

    /// Full-width panoramic Concert Visualizer stage designed for classical & piano listening.
    pub fn ui_concert_stage(
        &mut self,
        ui: &mut egui::Ui,
        height: f32,
        palette: &Palette,
        plugin_cfg: &VisualizerPluginConfig,
    ) {
        let avail_w = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(avail_w, height), Sense::click());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter();
        painter.rect_filled(rect, 10.0, palette.background);
        painter.rect_stroke(
            rect,
            10.0,
            Stroke::new(1.0, palette.border),
            StrokeKind::Inside,
        );

        let inner = rect.shrink2(Vec2::new(12.0, 10.0));
        match self.mode {
            VisualizerMode::PianoHarmonic => {
                self.paint_piano_ribbon(
                    painter,
                    inner,
                    palette,
                    self.show_piano_keys,
                    plugin_cfg.show_chord_readout,
                );
            }
            VisualizerMode::PianoRoll => {
                self.paint_piano_roll(painter, inner, palette, plugin_cfg.show_chord_readout);
            }
            VisualizerMode::ConcertSpectrum => {
                self.paint_concert_spectrum(painter, inner, palette, true);
            }
            VisualizerMode::StereoScope => {
                let gonio_w = (inner.height() * 1.15).clamp(100.0, inner.width() * 0.34);
                let scope_rect = Rect::from_min_max(
                    inner.min,
                    pos2(inner.right() - gonio_w - 12.0, inner.bottom()),
                );
                let gonio_rect = Rect::from_min_max(
                    pos2(scope_rect.right() + 12.0, inner.top()),
                    inner.max,
                );
                self.paint_oscilloscope(painter, scope_rect, palette);
                self.paint_goniometer(painter, gonio_rect, palette);
            }
            VisualizerMode::MasteringVu => {
                self.paint_vu_mastering(painter, inner, palette);
            }
        }

        if response.double_clicked() {
            self.mode = plugin_cfg.next_enabled_mode(self.mode);
        }
    }

    fn paint_harmony_badge(&self, painter: &egui::Painter, rect: Rect, palette: &Palette) {
        let text = if self.paused_freeze {
            if !self.detected_harmony.is_empty() {
                format!("⏸  {}", self.detected_harmony)
            } else if self.piano_levels.iter().any(|&v| v > 0.18) {
                "⏸  KEYS HELD".to_string()
            } else {
                return;
            }
        } else if !self.detected_harmony.is_empty() {
            self.detected_harmony.clone()
        } else {
            return;
        };
        let font = FontId::monospace(10.0);
        let fg = if self.paused_freeze {
            palette.hires_gold
        } else {
            palette.accent_bright
        };
        let border_col = if self.paused_freeze {
            palette.hires_border
        } else {
            palette.selected_border
        };
        let galley = painter.layout_no_wrap(text.clone(), font.clone(), fg);
        let pad_x = 8.0;
        let pill_w = galley.size().x + pad_x * 2.0;
        let pill = Rect::from_min_size(
            pos2(rect.right() - pill_w - 2.0, rect.top() + 1.0),
            vec2(pill_w, 18.0),
        );
        painter.rect_filled(
            pill,
            5.0,
            Color32::from_rgba_unmultiplied(
                palette.surface_elevated.r(),
                palette.surface_elevated.g(),
                palette.surface_elevated.b(),
                230,
            ),
        );
        painter.rect_stroke(
            pill,
            5.0,
            Stroke::new(0.9, border_col),
            StrokeKind::Inside,
        );
        painter.text(pill.center(), Align2::CENTER_CENTER, text, font, fg);
    }

    fn paint_piano_roll(
        &self,
        painter: &egui::Painter,
        inner: Rect,
        palette: &Palette,
        show_chord: bool,
    ) {
        let keybed_h = if inner.height() >= 70.0 { 26.0 } else { 0.0 };
        let roll_rect = Rect::from_min_max(
            inner.min,
            pos2(
                inner.right(),
                inner.bottom() - keybed_h - if keybed_h > 0.0 { 4.0 } else { 0.0 },
            ),
        );

        // Octave gridlines (C1..C7) aligned with true piano key centers
        for octave in 1..=7 {
            let midi = 12 * (octave + 1);
            let x = key_center_x(midi - 21, roll_rect);
            painter.line_segment(
                [pos2(x, roll_rect.top()), pos2(x, roll_rect.bottom())],
                Stroke::new(0.7, palette.border.gamma_multiply(0.55)),
            );
            painter.text(
                pos2(x + 3.0, roll_rect.top() + 2.0),
                Align2::LEFT_TOP,
                format!("C{octave}"),
                FontId::monospace(8.5),
                palette.faint,
            );
        }

        // Render scrolling note waterfall rising upward from the keybed
        let rows = self.piano_roll.len();
        if rows > 0 {
            let row_h = (roll_rect.height() / PIANO_ROLL_HISTORY as f32).max(1.2);
            let white_w = roll_rect.width() / 52.0;
            for (age_idx, frame) in self.piano_roll.iter().rev().enumerate() {
                let y_bot = roll_rect.bottom() - age_idx as f32 * row_h;
                let y_top = (y_bot - row_h - 0.6).max(roll_rect.top());
                if y_bot <= roll_rect.top() {
                    break;
                }
                let fade = 1.0 - (age_idx as f32 / PIANO_ROLL_HISTORY as f32) * 0.68;
                for (k, &raw) in frame.iter().enumerate() {
                    if raw < 46 {
                        continue;
                    }
                    let energy = raw as f32 / 255.0;
                    let cx = key_center_x(k, roll_rect);
                    let w = if is_black_key(21 + k) {
                        (white_w * 0.56).max(2.0)
                    } else {
                        (white_w * 0.78).max(2.6)
                    };
                    let col = harmonic_register_color(k as f32 / (PIANO_KEYS - 1) as f32, energy, palette);
                    let alpha = ((energy * fade * 245.0).round() as u8).clamp(35, 250);
                    let bar = Rect::from_min_max(pos2(cx - w * 0.5, y_top), pos2(cx + w * 0.5, y_bot));
                    painter.rect_filled(
                        bar,
                        1.0,
                        Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha),
                    );
                }
            }
        }

        if show_chord {
            self.paint_harmony_badge(painter, roll_rect, palette);
        }

        if keybed_h > 0.0 {
            let kb_rect = Rect::from_min_max(
                pos2(inner.left(), inner.bottom() - keybed_h),
                inner.max,
            );
            self.paint_keybed(painter, kb_rect, palette);
        }
    }

    fn paint_piano_ribbon(
        &self,
        painter: &egui::Painter,
        inner: Rect,
        palette: &Palette,
        with_keyboard: bool,
        show_chord: bool,
    ) {
        let keybed_h = if with_keyboard && inner.height() >= 70.0 {
            26.0
        } else {
            0.0
        };
        let ribbon_rect = Rect::from_min_max(
            inner.min,
            pos2(
                inner.right(),
                inner.bottom() - keybed_h - if keybed_h > 0.0 { 4.0 } else { 0.0 },
            ),
        );

        // Octave gridlines (C1..C7) aligned precisely to each C key on the 88-key keybed!
        if with_keyboard {
            for octave in 1..=7 {
                let midi = 12 * (octave + 1);
                let x = key_center_x(midi - 21, ribbon_rect);
                painter.line_segment(
                    [pos2(x, ribbon_rect.top()), pos2(x, ribbon_rect.bottom())],
                    Stroke::new(0.7, palette.border.gamma_multiply(0.55)),
                );
                painter.text(
                    pos2(x + 3.0, ribbon_rect.top() + 2.0),
                    Align2::LEFT_TOP,
                    format!("C{octave}"),
                    FontId::monospace(8.5),
                    palette.faint,
                );
            }
            for f in [0.33_f32, 0.66] {
                let y = ribbon_rect.bottom() - ribbon_rect.height() * f;
                painter.line_segment(
                    [pos2(ribbon_rect.left(), y), pos2(ribbon_rect.right(), y)],
                    Stroke::new(0.6, palette.border.gamma_multiply(0.35)),
                );
            }
        }

        let n = PIANO_KEYS;
        let white_w = ribbon_rect.width() / 52.0;
        let mut pts = Vec::with_capacity(n);
        for i in 0..n {
            // Preserve crisp single-key peaks (much lighter neighbor smoothing than before!)
            let curr = (self.piano_levels[i] + self.piano_onsets[i] * 0.22).clamp(0.0, 1.0);
            let prev = self.piano_levels[i.saturating_sub(1)];
            let next = self.piano_levels[(i + 1).min(n - 1)];
            let val = (curr * 0.88 + prev * 0.06 + next * 0.06).clamp(0.0, 1.0);
            let x = if with_keyboard {
                key_center_x(i, ribbon_rect)
            } else {
                ribbon_rect.left() + (i as f32 / (n - 1) as f32) * ribbon_rect.width()
            };
            let y = ribbon_rect.bottom() - val * (ribbon_rect.height() - 2.0);
            pts.push((pos2(x, y), val));
        }

        let col_w = if with_keyboard {
            (white_w * 0.48).max(1.8)
        } else {
            (ribbon_rect.width() / n as f32 * 0.72).max(1.2)
        };
        for (i, (pt, val)) in pts.iter().enumerate() {
            if *val > 0.02 {
                let frac = i as f32 / (n - 1) as f32;
                let onset = self.piano_onsets[i];
                let base_col = harmonic_register_color(frac, (*val + onset * 0.25).min(1.0), palette);
                let stem = Rect::from_min_max(
                    pos2(pt.x - col_w * 0.5, pt.y),
                    pos2(pt.x + col_w * 0.5, ribbon_rect.bottom()),
                );
                let fill_alpha = if onset > 0.2 {
                    125
                } else if palette.is_light {
                    82
                } else {
                    64
                };
                painter.rect_filled(
                    stem,
                    0.8,
                    Color32::from_rgba_unmultiplied(
                        base_col.r(),
                        base_col.g(),
                        base_col.b(),
                        fill_alpha,
                    ),
                );
                let tip_h = (ribbon_rect.bottom() - pt.y).min(5.5);
                let tip = Rect::from_min_max(
                    pos2(pt.x - col_w * 0.5, pt.y),
                    pos2(pt.x + col_w * 0.5, pt.y + tip_h),
                );
                painter.rect_filled(tip, 0.8, base_col);
            }

            let pk = self.piano_peaks[i];
            if pk > 0.08 && with_keyboard {
                let py = ribbon_rect.bottom() - pk * (ribbon_rect.height() - 2.0);
                let dot_col = harmonic_register_color(i as f32 / (n - 1) as f32, pk, palette);
                painter.circle_filled(pos2(pt.x, py), 1.45, dot_col);
            }
        }

        let line_pts: Vec<Pos2> = pts.iter().map(|(p, _)| *p).collect();
        painter.add(Shape::line(
            line_pts.clone(),
            Stroke::new(
                if with_keyboard { 3.0 } else { 2.0 },
                Color32::from_rgba_unmultiplied(
                    palette.accent.r(),
                    palette.accent.g(),
                    palette.accent.b(),
                    if palette.is_light { 46 } else { 60 },
                ),
            ),
        ));
        painter.add(Shape::line(
            line_pts,
            Stroke::new(if with_keyboard { 1.35 } else { 1.1 }, palette.accent_bright),
        ));

        if with_keyboard && show_chord {
            self.paint_harmony_badge(painter, ribbon_rect, palette);
        }

        if with_keyboard && keybed_h > 0.0 {
            let kb_rect = Rect::from_min_max(
                pos2(inner.left(), inner.bottom() - keybed_h),
                inner.max,
            );
            self.paint_keybed(painter, kb_rect, palette);
        }
    }

    fn paint_keybed(&self, painter: &egui::Painter, kb_rect: Rect, palette: &Palette) {
        let white_count = 52;
        let white_w = kb_rect.width() / white_count as f32;

        // Pass 1: 52 white keys with hammer-strike depression & glow
        let mut w_idx = 0usize;
        for k in 0..PIANO_KEYS {
            let midi = 21 + k;
            if !is_black_key(midi) {
                let x0 = kb_rect.left() + w_idx as f32 * white_w;
                let energy = self.piano_levels[k];
                let onset = self.piano_onsets[k];
                let pressed_drop = if energy > 0.25 {
                    ((energy - 0.25) * 2.4).min(2.0)
                } else {
                    0.0
                };
                let key_r = Rect::from_min_max(
                    pos2(x0 + 0.5, kb_rect.top() + pressed_drop),
                    pos2(x0 + white_w - 0.5, kb_rect.bottom()),
                );
                let base = if palette.is_light {
                    Color32::from_rgb(248, 244, 234)
                } else {
                    Color32::from_rgb(38, 43, 47)
                };
                let lit = harmonic_register_color(
                    k as f32 / (PIANO_KEYS - 1) as f32,
                    (energy + onset * 0.35).min(1.0),
                    palette,
                );
                let mix_t = (energy * 1.15 + onset * 0.55).clamp(0.0, 1.0);
                let fill = lerp_color(base, lit, mix_t);
                painter.rect_filled(key_r, 1.5, fill);
                painter.rect_stroke(
                    key_r,
                    1.5,
                    Stroke::new(
                        if onset > 0.25 { 1.0 } else { 0.6 },
                        if onset > 0.25 {
                            palette.hires_gold
                        } else {
                            palette.border
                        },
                    ),
                    StrokeKind::Inside,
                );
                w_idx += 1;
            }
        }

        // Pass 2: 36 black keys nestled between white keys
        let mut prev_white = 0usize;
        for k in 0..PIANO_KEYS {
            let midi = 21 + k;
            if is_black_key(midi) {
                let cx = kb_rect.left() + prev_white as f32 * white_w;
                let bw = (white_w * 0.62).max(2.2);
                let energy = self.piano_levels[k];
                let onset = self.piano_onsets[k];
                let pressed_drop = if energy > 0.25 {
                    ((energy - 0.25) * 2.0).min(1.6)
                } else {
                    0.0
                };
                let bh = kb_rect.height() * 0.60;
                let key_r = Rect::from_min_max(
                    pos2(cx - bw * 0.5, kb_rect.top() + pressed_drop),
                    pos2(cx + bw * 0.5, kb_rect.top() + bh + pressed_drop * 0.5),
                );
                let base = if palette.is_light {
                    Color32::from_rgb(44, 52, 56)
                } else {
                    Color32::from_rgb(13, 15, 17)
                };
                let lit = harmonic_register_color(
                    k as f32 / (PIANO_KEYS - 1) as f32,
                    (energy + onset * 0.4).min(1.0),
                    palette,
                );
                let mix_t = (energy * 1.25 + onset * 0.6).clamp(0.0, 1.0);
                let fill = lerp_color(base, lit, mix_t);
                painter.rect_filled(key_r, 1.0, fill);
                if onset > 0.25 {
                    painter.rect_stroke(
                        key_r,
                        1.0,
                        Stroke::new(0.8, palette.hires_gold),
                        StrokeKind::Inside,
                    );
                }
            } else {
                prev_white += 1;
            }
        }
    }

    fn paint_concert_spectrum(
        &self,
        painter: &egui::Painter,
        inner: Rect,
        palette: &Palette,
        detailed: bool,
    ) {
        let horizon_y = if detailed {
            inner.top() + inner.height() * 0.78
        } else {
            inner.bottom()
        };
        let top_h = (horizon_y - inner.top()).max(10.0);
        let refl_h = (inner.bottom() - horizon_y).max(0.0);

        if detailed {
            painter.line_segment(
                [pos2(inner.left(), horizon_y), pos2(inner.right(), horizon_y)],
                Stroke::new(1.0, palette.border),
            );
            for (label, frac) in [
                ("40 Hz", 0.05_f32),
                ("150 Hz", 0.25),
                ("500 Hz", 0.45),
                ("2 kHz", 0.66),
                ("8 kHz", 0.87),
            ] {
                let x = inner.left() + inner.width() * frac;
                painter.text(
                    pos2(x, inner.bottom() - 1.0),
                    Align2::CENTER_BOTTOM,
                    label,
                    FontId::monospace(8.5),
                    palette.faint,
                );
            }
        }

        let bands = if detailed { SPECTRUM_BANDS } else { 32 };
        let gap = if detailed { 2.0 } else { 1.5 };
        let bar_w = ((inner.width() - gap * (bands - 1) as f32) / bands as f32).max(1.5);

        for i in 0..bands {
            let src_idx = if detailed { i } else { i * 2 };
            let val = self.spectrum_bands[src_idx].clamp(0.0, 1.0);
            let peak = self.spectrum_peaks[src_idx].clamp(0.0, 1.0);
            let x = inner.left() + i as f32 * (bar_w + gap);
            let frac = i as f32 / (bands - 1) as f32;
            let col = harmonic_register_color(frac, val, palette);

            if val > 0.01 {
                let bh = (top_h * val).max(1.5);
                let bar = Rect::from_min_max(pos2(x, horizon_y - bh), pos2(x + bar_w, horizon_y - 0.5));
                painter.rect_filled(bar, 1.0, col);

                if detailed && refl_h > 4.0 {
                    let rh = (refl_h * val * 0.72).min(refl_h - 10.0).max(0.0);
                    if rh > 1.0 {
                        let rbar = Rect::from_min_max(
                            pos2(x, horizon_y + 1.5),
                            pos2(x + bar_w, horizon_y + 1.5 + rh),
                        );
                        painter.rect_filled(
                            rbar,
                            1.0,
                            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 44),
                        );
                    }
                }
            }

            if peak > 0.05 {
                let py = (horizon_y - top_h * peak).clamp(inner.top(), horizon_y - 2.0);
                painter.rect_filled(
                    Rect::from_min_max(pos2(x, py), pos2(x + bar_w, py + 1.5)),
                    0.5,
                    palette.accent_bright,
                );
            }
        }
    }

    fn paint_oscilloscope(&self, painter: &egui::Painter, rect: Rect, palette: &Palette) {
        let cy = rect.center().y;
        painter.line_segment(
            [pos2(rect.left(), cy), pos2(rect.right(), cy)],
            Stroke::new(0.8, palette.border),
        );

        let mut trigger = 0usize;
        let search_limit = self.scope_l.len() / 2;
        for i in 1..search_limit {
            if self.scope_l[i - 1] <= 0.0 && self.scope_l[i] > 0.002 {
                trigger = i;
                break;
            }
        }
        let draw_len = (self.scope_l.len() - trigger).min(240);
        if draw_len < 4 {
            return;
        }
        let amp_h = rect.height() * 0.44;
        let dx = rect.width() / (draw_len - 1) as f32;

        let mut pts_l = Vec::with_capacity(draw_len);
        let mut pts_r = Vec::with_capacity(draw_len);
        for i in 0..draw_len {
            let x = rect.left() + i as f32 * dx;
            let sl = (self.scope_l[trigger + i] * self.sensitivity * 1.15).clamp(-1.0, 1.0);
            let sr = (self.scope_r[trigger + i] * self.sensitivity * 1.15).clamp(-1.0, 1.0);
            pts_l.push(pos2(x, cy - sl * amp_h));
            pts_r.push(pos2(x, cy - sr * amp_h));
        }

        painter.add(Shape::line(
            pts_r,
            Stroke::new(
                1.2,
                Color32::from_rgba_unmultiplied(
                    palette.accent_secondary.r(),
                    palette.accent_secondary.g(),
                    palette.accent_secondary.b(),
                    175,
                ),
            ),
        ));
        painter.add(Shape::line(pts_l, Stroke::new(1.5, palette.accent_bright)));
    }

    fn paint_goniometer(&self, painter: &egui::Painter, rect: Rect, palette: &Palette) {
        let c = rect.center();
        let r = rect.width().min(rect.height()) * 0.46;

        painter.circle_stroke(c, r, Stroke::new(0.9, palette.border));
        painter.circle_stroke(c, r * 0.5, Stroke::new(0.6, palette.border.gamma_multiply(0.6)));
        let diag = r * std::f32::consts::FRAC_1_SQRT_2;
        painter.line_segment(
            [pos2(c.x - diag, c.y - diag), pos2(c.x + diag, c.y + diag)],
            Stroke::new(0.6, palette.border.gamma_multiply(0.6)),
        );
        painter.line_segment(
            [pos2(c.x - diag, c.y + diag), pos2(c.x + diag, c.y - diag)],
            Stroke::new(0.6, palette.border.gamma_multiply(0.6)),
        );
        painter.text(
            pos2(c.x - diag - 4.0, c.y - diag - 2.0),
            Align2::RIGHT_BOTTOM,
            "L",
            FontId::monospace(8.5),
            palette.faint,
        );
        painter.text(
            pos2(c.x + diag + 4.0, c.y - diag - 2.0),
            Align2::LEFT_BOTTOM,
            "R",
            FontId::monospace(8.5),
            palette.faint,
        );

        let n = self.scope_l.len().min(GONIO_POINTS);
        let start = self.scope_l.len().saturating_sub(n);
        let mut pts = Vec::with_capacity(n);
        for i in 0..n {
            let l = self.scope_l[start + i] * self.sensitivity * 1.3;
            let r_s = self.scope_r[start + i] * self.sensitivity * 1.3;
            let x = ((r_s - l) * std::f32::consts::FRAC_1_SQRT_2).clamp(-1.0, 1.0);
            let y = ((l + r_s) * std::f32::consts::FRAC_1_SQRT_2).clamp(-1.0, 1.0);
            pts.push(pos2(c.x + x * r, c.y - y * r));
        }
        if pts.len() >= 2 {
            painter.add(Shape::line(
                pts,
                Stroke::new(
                    1.15,
                    Color32::from_rgba_unmultiplied(
                        palette.accent.r(),
                        palette.accent.g(),
                        palette.accent.b(),
                        190,
                    ),
                ),
            ));
        }
    }

    fn paint_vu_compact(&self, painter: &egui::Painter, inner: Rect, palette: &Palette) {
        let row_h = (inner.height() - 4.0) * 0.5;
        let rows = [
            ("L", self.vu_left, self.vu_peak_left, inner.top()),
            ("R", self.vu_right, self.vu_peak_right, inner.top() + row_h + 4.0),
        ];
        self.paint_vu_bars(painter, inner.left(), inner.right(), row_h, &rows, 22, palette);
    }

    fn paint_vu_mastering(&self, painter: &egui::Painter, inner: Rect, palette: &Palette) {
        let meter_right = inner.right() - 62.0;
        let row_h = ((inner.height() - 34.0) * 0.5).clamp(14.0, 26.0);
        let rows = [
            ("L", self.vu_left, self.vu_peak_left, inner.top() + 2.0),
            ("R", self.vu_right, self.vu_peak_right, inner.top() + row_h + 8.0),
        ];
        self.paint_vu_bars(painter, inner.left(), meter_right, row_h, &rows, 42, palette);

        painter.text(
            pos2(inner.right() - 2.0, rows[0].3 + row_h * 0.5),
            Align2::RIGHT_CENTER,
            format!("{:5.1} dB", self.rms_db_l),
            FontId::monospace(10.0),
            palette.muted,
        );
        painter.text(
            pos2(inner.right() - 2.0, rows[1].3 + row_h * 0.5),
            Align2::RIGHT_CENTER,
            format!("{:5.1} dB", self.rms_db_r),
            FontId::monospace(10.0),
            palette.muted,
        );

        let corr_y = inner.bottom() - 14.0;
        painter.text(
            pos2(inner.left(), corr_y + 6.0),
            Align2::LEFT_CENTER,
            "PHASE",
            FontId::monospace(8.5),
            palette.faint,
        );
        let bar_l = inner.left() + 44.0;
        let bar_r = meter_right;
        let bar_rect = Rect::from_min_max(pos2(bar_l, corr_y + 2.0), pos2(bar_r, corr_y + 10.0));
        painter.rect_filled(bar_rect, 3.0, palette.surface);
        let mid_x = bar_rect.center().x;
        painter.line_segment(
            [pos2(mid_x, bar_rect.top()), pos2(mid_x, bar_rect.bottom())],
            Stroke::new(1.0, palette.border_strong),
        );
        let corr_x = bar_rect.left() + ((self.correlation + 1.0) * 0.5).clamp(0.0, 1.0) * bar_rect.width();
        let corr_col = if self.correlation >= 0.0 {
            palette.accent
        } else {
            palette.danger
        };
        let fill_rect = Rect::from_min_max(
            pos2(mid_x.min(corr_x), bar_rect.top() + 1.0),
            pos2(mid_x.max(corr_x), bar_rect.bottom() - 1.0),
        );
        painter.rect_filled(fill_rect, 2.0, corr_col);
        painter.text(
            pos2(inner.right() - 2.0, corr_y + 6.0),
            Align2::RIGHT_CENTER,
            format!("{:+.2}", self.correlation),
            FontId::monospace(9.5),
            corr_col,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_vu_bars(
        &self,
        painter: &egui::Painter,
        left: f32,
        right: f32,
        row_h: f32,
        rows: &[(&str, f32, f32, f32); 2],
        segments: usize,
        palette: &Palette,
    ) {
        let label_w = 14.0;
        for (label, level, peak, top) in rows {
            let row_rect = Rect::from_min_max(pos2(left, *top), pos2(right, *top + row_h));
            painter.text(
                pos2(row_rect.left() + 5.0, row_rect.center().y),
                Align2::CENTER_CENTER,
                *label,
                FontId::monospace(9.5),
                palette.faint,
            );
            let meter_left = row_rect.left() + label_w;
            let meter_w = (row_rect.right() - meter_left).max(16.0);
            let seg_gap = 1.6;
            let seg_w = ((meter_w - seg_gap * (segments - 1) as f32) / segments as f32).max(1.5);
            let lit_count = (level.clamp(0.0, 1.0) * segments as f32).round() as usize;
            let peak_idx = ((peak.clamp(0.0, 1.0) * segments as f32).floor() as usize)
                .min(segments.saturating_sub(1));

            for s in 0..segments {
                let sx = meter_left + s as f32 * (seg_w + seg_gap);
                let seg_rect = Rect::from_min_max(
                    pos2(sx, row_rect.top() + 1.0),
                    pos2(sx + seg_w, row_rect.bottom() - 1.0),
                );
                let frac = s as f32 / segments as f32;
                let active_color = if frac >= 0.86 {
                    palette.danger
                } else if frac >= 0.68 {
                    palette.hires_gold
                } else {
                    palette.accent
                };
                let color = if s < lit_count || (*peak > 0.04 && s == peak_idx) {
                    active_color
                } else {
                    palette.surface
                };
                painter.rect_filled(seg_rect, 1.0, color);
            }
        }
    }
}

/// Exact horizontal center of piano key `k` (0=A0 .. 87=C8) aligned to a 52-white-key keybed.
fn key_center_x(key_idx: usize, rect: Rect) -> f32 {
    let white_w = rect.width() / 52.0;
    let mut whites_before = 0usize;
    for k in 0..key_idx {
        if !is_black_key(21 + k) {
            whites_before += 1;
        }
    }
    if is_black_key(21 + key_idx) {
        rect.left() + whites_before as f32 * white_w
    } else {
        rect.left() + (whites_before as f32 + 0.5) * white_w
    }
}

fn is_black_key(midi: usize) -> bool {
    matches!(midi % 12, 1 | 3 | 6 | 8 | 10)
}

fn midi_note_name(midi: usize) -> String {
    let pc = midi % 12;
    let octave = (midi as isize / 12) - 1;
    format!("{}{octave}", NOTE_NAMES[pc])
}

/// Infers active chord name + top sounding piano notes from 88-key energies.
fn detect_chord_and_notes(levels: &[f32; PIANO_KEYS]) -> String {
    let mut active: Vec<(usize, f32)> = levels
        .iter()
        .enumerate()
        .filter(|&(_, &v)| v > 0.30)
        .map(|(k, &v)| (k, v))
        .collect();
    if active.is_empty() {
        return String::new();
    }
    active.sort_by(|a, b| b.1.total_cmp(&a.1));
    active.truncate(6);

    // Build pitch-class bitmask weighted by energy
    let mut pc_mask = 0u16;
    let mut pc_energy = [0.0_f32; 12];
    for &(k, v) in &active {
        let pc = (21 + k) % 12;
        pc_mask |= 1u16 << pc;
        pc_energy[pc] += v;
    }

    // Sort active notes low-to-high for crisp musical display
    let mut display_notes = active.clone();
    display_notes.sort_by_key(|&(k, _)| k);
    let notes_str = display_notes
        .iter()
        .take(5)
        .map(|&(k, _)| midi_note_name(21 + k))
        .collect::<Vec<_>>()
        .join(" ");

    if pc_mask.count_ones() < 2 {
        return notes_str;
    }

    // Match common harmonic templates rotated over all 12 roots
    const TEMPLATES: &[(&str, u16, f32)] = &[
        ("maj7", (1 << 0) | (1 << 4) | (1 << 7) | (1 << 11), 1.12),
        ("m7", (1 << 0) | (1 << 3) | (1 << 7) | (1 << 10), 1.12),
        ("7", (1 << 0) | (1 << 4) | (1 << 7) | (1 << 10), 1.10),
        ("dim7", (1 << 0) | (1 << 3) | (1 << 6) | (1 << 9), 1.08),
        ("", (1 << 0) | (1 << 4) | (1 << 7), 1.05),
        ("m", (1 << 0) | (1 << 3) | (1 << 7), 1.05),
        ("dim", (1 << 0) | (1 << 3) | (1 << 6), 0.98),
        ("aug", (1 << 0) | (1 << 4) | (1 << 8), 0.96),
        ("sus4", (1 << 0) | (1 << 5) | (1 << 7), 0.94),
        ("sus2", (1 << 0) | (1 << 2) | (1 << 7), 0.94),
    ];

    let bass_pc = (21 + display_notes[0].0) % 12;
    let mut best_chord: Option<(f32, String)> = None;

    for (root, root_name) in NOTE_NAMES.iter().enumerate() {
        for &(suffix, pattern, weight) in TEMPLATES {
            let mut matched = 0.0_f32;
            let mut template_hits = 0u32;
            let mut extra_penalty = 0.0_f32;
            for (pc, &energy) in pc_energy.iter().enumerate() {
                let rel = (pc + 12 - root) % 12;
                let in_tpl = (pattern & (1 << rel)) != 0;
                if in_tpl && energy > 0.0 {
                    matched += energy;
                    template_hits += 1;
                } else if !in_tpl && energy > 0.0 {
                    extra_penalty += energy * 0.55;
                }
            }
            let needed = pattern.count_ones();
            if template_hits < needed {
                continue;
            }
            let bass_bonus = if root == bass_pc { 1.18 } else { 1.0 };
            let score = (matched - extra_penalty) * weight * bass_bonus;
            if score > 0.45 && best_chord.as_ref().is_none_or(|(b, _)| score > *b) {
                best_chord = Some((score, format!("{root_name}{suffix}")));
            }
        }
    }

    match best_chord {
        Some((_, chord)) => format!("{chord}  ·  {notes_str}"),
        None => notes_str,
    }
}

fn harmonic_register_color(pos: f32, intensity: f32, palette: &Palette) -> Color32 {
    let base = if pos < 0.35 {
        lerp_color(palette.hires_gold, palette.accent, pos / 0.35)
    } else if pos < 0.75 {
        lerp_color(palette.accent, palette.accent_bright, (pos - 0.35) / 0.40)
    } else {
        lerp_color(
            palette.accent_bright,
            palette.accent_secondary,
            (pos - 0.75) / 0.25,
        )
    };
    if intensity > 0.85 {
        lerp_color(base, palette.hires_gold, (intensity - 0.85) / 0.15)
    } else {
        base
    }
}

fn smooth(curr: f32, target: f32, attack: f32, decay: f32) -> f32 {
    if target > curr {
        curr + (target - curr) * attack
    } else {
        curr * decay + target * (1.0 - decay)
    }
}

fn db_to_meter(db: f32) -> f32 {
    ((db + 54.0) / 52.0).clamp(0.0, 1.0)
}

fn amp_to_meter(amp: f32, sensitivity: f32) -> f32 {
    let scaled = amp * sensitivity;
    if scaled <= 1e-4 {
        return 0.0;
    }
    let db = 20.0 * scaled.log10();
    ((db + 54.0) / 50.0).clamp(0.0, 1.0)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// In-place radix-2 Cooley-Tukey FFT using precomputed twiddle tables (zero per-frame trig!).
fn fft_radix2_precomputed(re: &mut [f32], im: &mut [f32], tw_re: &[f32], tw_im: &[f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && n == im.len() && tw_re.len() == n / 2);

    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let step = n / len;
        let mut i = 0;
        while i < n {
            for k in 0..half {
                let tw_idx = k * step;
                let w_re = tw_re[tw_idx];
                let w_im = tw_im[tw_idx];
                let u_re = re[i + k];
                let u_im = im[i + k];
                let v_re = re[i + k + half] * w_re - im[i + k + half] * w_im;
                let v_im = re[i + k + half] * w_im + im[i + k + half] * w_re;
                re[i + k] = u_re + v_re;
                im[i + k] = u_im + v_im;
                re[i + k + half] = u_re - v_re;
                im[i + k + half] = u_im - v_im;
            }
            i += len;
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_detects_concert_a4_440hz() {
        let n = FFT_SIZE;
        let sr = 44_100.0_f32;
        let freq = 440.0_f32;
        let mut re = vec![0.0_f32; n];
        let mut im = vec![0.0_f32; n];
        let mut tw_re = Vec::with_capacity(n / 2);
        let mut tw_im = Vec::with_capacity(n / 2);
        for k in 0..(n / 2) {
            let a = -TAU * k as f32 / n as f32;
            tw_re.push(a.cos());
            tw_im.push(a.sin());
        }
        for (i, slot) in re.iter_mut().enumerate() {
            *slot = (TAU * freq * i as f32 / sr).sin();
        }
        fft_radix2_precomputed(&mut re, &mut im, &tw_re, &tw_im);
        let peak_bin = (1..n / 2)
            .max_by(|&a, &b| {
                let ma = re[a] * re[a] + im[a] * im[a];
                let mb = re[b] * re[b] + im[b] * im[b];
                ma.partial_cmp(&mb).unwrap()
            })
            .unwrap();
        let detected_hz = peak_bin as f32 * sr / n as f32;
        assert!((detected_hz - 440.0).abs() < 12.0);
    }

    #[test]
    fn precision_piano_separates_bass_and_cancels_overtone_ghosts() {
        let mut analyzer = PrecisionPianoAnalyzer::new();
        let sr = 48_000_u32;
        // Synthesize a realistic C3 (MIDI 48, key index 27, 130.81 Hz) with strong 2nd & 3rd piano overtones
        let f0 = 130.813_f32;
        for (i, slot) in analyzer.raw_mono.iter_mut().enumerate() {
            let t = i as f32 / sr as f32;
            *slot = 0.55 * (TAU * f0 * t).sin()
                + 0.35 * (TAU * 2.0 * f0 * t).sin()
                + 0.22 * (TAU * 3.0 * f0 * t).sin()
                + 0.12 * (TAU * 4.0 * f0 * t).sin();
        }
        let mut levels = [0.0_f32; PIANO_KEYS];
        let mut onsets = [0.0_f32; PIANO_KEYS];
        analyzer.analyze(sr, true, 1.15, &mut levels, &mut onsets);

        let c3_key = 48 - 21; // 27
        let c4_ghost = 60 - 21; // 39
        let g4_ghost = 67 - 21; // 46
        assert!(
            levels[c3_key] > 0.55,
            "C3 fundamental should be strongly detected, got {}",
            levels[c3_key]
        );
        assert!(
            levels[c3_key] > levels[c4_ghost] + 0.25,
            "Harmonic cancellation should suppress C4 octave ghost (C3={}, C4={})",
            levels[c3_key],
            levels[c4_ghost]
        );
        assert!(
            levels[c3_key] > levels[g4_ghost] + 0.3,
            "Harmonic cancellation should suppress G4 3rd-harmonic ghost (C3={}, G4={})",
            levels[c3_key],
            levels[g4_ghost]
        );
    }

    #[test]
    fn chord_detector_identifies_c_major_triad() {
        let mut levels = [0.0_f32; PIANO_KEYS];
        levels[60 - 21] = 0.85; // C4
        levels[64 - 21] = 0.80; // E4
        levels[67 - 21] = 0.78; // G4
        let out = detect_chord_and_notes(&levels);
        assert!(out.starts_with("C  ·"), "expected C major chord, got: {out}");
    }

    #[test]
    fn paused_playback_freezes_piano_keys_and_chord() {
        let mut vis = VisualizerState::new(VisualizerMode::PianoHarmonic, true, 1.15);
        vis.piano_levels[60 - 21] = 0.85;
        vis.piano_levels[64 - 21] = 0.80;
        vis.detected_harmony = "C  ·  C4 E4".into();
        let tap = VisualTap::new();
        tap.set_active(false);
        let cfg = VisualizerPluginConfig::default();

        // While paused, keys must remain completely frozen instead of decaying to 0!
        for _ in 0..30 {
            vis.update(&tap, 0.016, &cfg, true);
        }
        assert_eq!(vis.piano_levels[60 - 21], 0.85);
        assert_eq!(vis.detected_harmony, "C  ·  C4 E4");

        // When unpaused & stopped, keys decay cleanly
        vis.update(&tap, 0.05, &cfg, false);
        assert!(vis.piano_levels[60 - 21] < 0.85);
    }
}
