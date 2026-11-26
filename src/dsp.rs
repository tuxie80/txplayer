use std::f64::consts::TAU;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::plugins::{
    DitherMode, DspConfig, EQ_FREQS_HZ, MAX_PARAMETRIC_BANDS, ParametricBand, ParametricFilterKind,
};

/// Atomic-generation guarded DSP parameter bridge so UI EQ / AutoEQ Parametric / Crossfeed
/// tweaks reach the real-time audio callback in <10 ms with zero allocations on the audio thread.
pub struct SharedDspParams {
    generation: AtomicU64,
    config: Mutex<DspConfig>,
}

impl SharedDspParams {
    pub fn new(cfg: &DspConfig) -> Self {
        Self {
            generation: AtomicU64::new(1),
            config: Mutex::new(cfg.clone()),
        }
    }

    pub fn update(&self, cfg: &DspConfig) {
        if let Ok(mut guard) = self.config.lock() {
            *guard = cfg.clone();
        }
        self.generation.fetch_add(1, Ordering::Release);
    }

    fn try_snapshot(&self) -> Option<(u64, DspConfig)> {
        let generation = self.generation.load(Ordering::Acquire);
        let guard = self.config.try_lock().ok()?;
        Some((generation, guard.clone()))
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

/// Direct Form II Transposed Biquad filter (numerically clean in f64 state for 24-bit Hi-Res audio).
#[derive(Clone, Copy)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
    bypass: bool,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
            bypass: true,
        }
    }
}

impl Biquad {
    pub fn set_peaking(&mut self, sample_rate: f64, freq_hz: f64, q: f64, gain_db: f64) {
        if gain_db.abs() < 0.02 || freq_hz >= sample_rate * 0.48 || freq_hz <= 10.0 {
            self.bypass = true;
            return;
        }
        let a = 10.0_f64.powf(gain_db / 40.0);
        let w0 = TAU * freq_hz / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q.max(0.1));

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;
        self.commit_coeffs(b0, b1, b2, a0, a1, a2);
    }

    pub fn set_parametric(&mut self, sample_rate: f64, band: &ParametricBand) {
        let freq_hz = band.freq_hz as f64;
        let q = (band.q as f64).clamp(0.15, 18.0);
        let gain_db = band.gain_db as f64;
        if !band.enabled || freq_hz >= sample_rate * 0.48 || freq_hz <= 10.0 {
            self.bypass = true;
            return;
        }
        match band.kind {
            ParametricFilterKind::Peaking => self.set_peaking(sample_rate, freq_hz, q, gain_db),
            ParametricFilterKind::LowShelf => {
                if gain_db.abs() < 0.02 {
                    self.bypass = true;
                    return;
                }
                let a = 10.0_f64.powf(gain_db / 40.0);
                let w0 = TAU * freq_hz / sample_rate;
                let cos_w0 = w0.cos();
                let sin_w0 = w0.sin();
                let alpha = sin_w0 / (2.0 * q);
                let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

                let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
                let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
                let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
                let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
                let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
                let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
                self.commit_coeffs(b0, b1, b2, a0, a1, a2);
            }
            ParametricFilterKind::HighShelf => {
                if gain_db.abs() < 0.02 {
                    self.bypass = true;
                    return;
                }
                let a = 10.0_f64.powf(gain_db / 40.0);
                let w0 = TAU * freq_hz / sample_rate;
                let cos_w0 = w0.cos();
                let sin_w0 = w0.sin();
                let alpha = sin_w0 / (2.0 * q);
                let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

                let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha);
                let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
                let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha);
                let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
                let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
                let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha;
                self.commit_coeffs(b0, b1, b2, a0, a1, a2);
            }
            ParametricFilterKind::LowPass => {
                let w0 = TAU * freq_hz / sample_rate;
                let cos_w0 = w0.cos();
                let alpha = w0.sin() / (2.0 * q);
                let b0 = (1.0 - cos_w0) * 0.5;
                let b1 = 1.0 - cos_w0;
                let b2 = (1.0 - cos_w0) * 0.5;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                self.commit_coeffs(b0, b1, b2, a0, a1, a2);
            }
            ParametricFilterKind::HighPass => {
                let w0 = TAU * freq_hz / sample_rate;
                let cos_w0 = w0.cos();
                let alpha = w0.sin() / (2.0 * q);
                let b0 = (1.0 + cos_w0) * 0.5;
                let b1 = -(1.0 + cos_w0);
                let b2 = (1.0 + cos_w0) * 0.5;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                self.commit_coeffs(b0, b1, b2, a0, a1, a2);
            }
        }
    }

    fn commit_coeffs(&mut self, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
        let inv_a0 = 1.0 / a0;
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
        self.bypass = false;
    }

    /// Exact analytical magnitude response `|H(e^{jω})|` in dB at `freq_hz`.
    pub fn magnitude_db(&self, freq_hz: f64, sample_rate: f64) -> f64 {
        if self.bypass {
            return 0.0;
        }
        let w = TAU * freq_hz / sample_rate;
        let c1 = w.cos();
        let s1 = w.sin();
        let c2 = (2.0 * w).cos();
        let s2 = (2.0 * w).sin();
        let num_r = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_i = -(self.b1 * s1 + self.b2 * s2);
        let den_r = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_i = -(self.a1 * s1 + self.a2 * s2);
        let mag2 = (num_r * num_r + num_i * num_i) / (den_r * den_r + den_i * den_i).max(1e-18);
        10.0 * mag2.max(1e-12).log10()
    }

    #[inline(always)]
    fn process(&mut self, x: f64) -> f64 {
        if self.bypass {
            return x;
        }
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        if !y.is_finite() {
            self.z1 = 0.0;
            self.z2 = 0.0;
            return 0.0;
        }
        y
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Evaluates the combined Graphic + Parametric EQ frequency response curve (in dB) across `points`
/// logarithmically spaced frequencies from 20 Hz to 20 kHz for UI visualization.
pub fn compute_eq_response_curve(cfg: &DspConfig, points: usize) -> Vec<f32> {
    let sr = 48_000.0_f64;
    let mut filters = Vec::new();
    let mut base_db = 0.0_f64;
    if cfg.eq_enabled {
        base_db += cfg.preamp_db as f64;
        for (i, &freq) in EQ_FREQS_HZ.iter().enumerate() {
            let mut bq = Biquad::default();
            bq.set_peaking(sr, freq as f64, 1.414, cfg.eq_bands_db[i] as f64);
            if !bq.bypass {
                filters.push(bq);
            }
        }
    }
    if cfg.peq_enabled {
        base_db += cfg.peq_preamp_db as f64;
        for band in cfg.peq_bands.iter().take(MAX_PARAMETRIC_BANDS) {
            let mut bq = Biquad::default();
            bq.set_parametric(sr, band);
            if !bq.bypass {
                filters.push(bq);
            }
        }
    }
    let n = points.max(16);
    let mut out = Vec::with_capacity(n);
    let log_min = 20.0_f64.ln();
    let log_max = 20_000.0_f64.ln();
    for i in 0..n {
        let t = i as f64 / (n - 1) as f64;
        let f = (log_min + t * (log_max - log_min)).exp();
        let mut db = base_db;
        for bq in &filters {
            db += bq.magnitude_db(f, sr);
        }
        out.push(db.clamp(-18.0, 18.0) as f32);
    }
    out
}

/// Real-time stereo audiophile DSP chain executed inside the audio callback (64-bit f64 internal).
pub struct RealtimeDsp {
    sample_rate: f64,
    last_gen: u64,
    enabled: bool,
    eq_enabled: bool,
    peq_enabled: bool,
    peq_active_count: usize,
    crossfeed_enabled: bool,
    soft_limiter: bool,
    dither: DitherMode,
    preamp_gain: f64,
    peq_preamp_gain: f64,
    crossfeed_mix: f64,
    cf_lp_alpha: f64,
    cf_lp_l: f64,
    cf_lp_r: f64,
    rng_state: u64,
    ns_e1_l: f64,
    ns_e2_l: f64,
    ns_e1_r: f64,
    ns_e2_r: f64,
    eq_l: [Biquad; 10],
    eq_r: [Biquad; 10],
    peq_l: [Biquad; MAX_PARAMETRIC_BANDS],
    peq_r: [Biquad; MAX_PARAMETRIC_BANDS],
}

impl RealtimeDsp {
    pub fn new(sample_rate: u32, shared: &SharedDspParams) -> Self {
        let sr = (sample_rate as f64).max(8_000.0);
        // 1-pole lowpass at ~700 Hz for natural Bauer-style acoustic head-shadow crossfeed
        let cf_lp_alpha = 1.0 - (-TAU * 700.0 / sr).exp();
        let mut dsp = Self {
            sample_rate: sr,
            last_gen: 0,
            enabled: false,
            eq_enabled: false,
            peq_enabled: false,
            peq_active_count: 0,
            crossfeed_enabled: false,
            soft_limiter: true,
            dither: DitherMode::Off,
            preamp_gain: 1.0,
            peq_preamp_gain: 1.0,
            crossfeed_mix: 0.0,
            cf_lp_alpha,
            cf_lp_l: 0.0,
            cf_lp_r: 0.0,
            rng_state: 0x9E37_79B9_7F4A_7C15,
            ns_e1_l: 0.0,
            ns_e2_l: 0.0,
            ns_e1_r: 0.0,
            ns_e2_r: 0.0,
            eq_l: [Biquad::default(); 10],
            eq_r: [Biquad::default(); 10],
            peq_l: [Biquad::default(); MAX_PARAMETRIC_BANDS],
            peq_r: [Biquad::default(); MAX_PARAMETRIC_BANDS],
        };
        dsp.sync(shared);
        dsp
    }

    pub fn sync(&mut self, shared: &SharedDspParams) {
        if shared.generation() == self.last_gen {
            return;
        }
        let Some((generation, snap)) = shared.try_snapshot() else {
            return;
        };
        self.last_gen = generation;
        self.enabled = snap.enabled;
        self.eq_enabled = snap.eq_enabled;
        self.peq_enabled = snap.peq_enabled;
        self.crossfeed_enabled = snap.crossfeed_enabled;
        self.soft_limiter = snap.soft_limiter;
        self.dither = snap.dither;
        self.preamp_gain = 10.0_f64.powf(snap.preamp_db.clamp(-12.0, 12.0) as f64 / 20.0);
        self.peq_preamp_gain = 10.0_f64.powf(snap.peq_preamp_db.clamp(-18.0, 12.0) as f64 / 20.0);
        self.crossfeed_mix = (snap.crossfeed_amount as f64).clamp(0.0, 1.0) * 0.34;
        let cf_cutoff = (snap.crossfeed_cutoff_hz as f64).clamp(300.0, 2_000.0);
        self.cf_lp_alpha = 1.0 - (-TAU * cf_cutoff / self.sample_rate).exp();

        // Constant-Q octave graphic EQ spacing
        for (i, &freq_hz) in EQ_FREQS_HZ.iter().enumerate() {
            let freq = freq_hz as f64;
            let gain = snap.eq_bands_db[i].clamp(-12.0, 12.0) as f64;
            self.eq_l[i].set_peaking(self.sample_rate, freq, 1.414, gain);
            self.eq_r[i].set_peaking(self.sample_rate, freq, 1.414, gain);
        }

        // Parametric EQ / AutoEQ headphone correction bank (up to 16 bands)
        let count = snap.peq_bands.len().min(MAX_PARAMETRIC_BANDS);
        self.peq_active_count = count;
        for i in 0..MAX_PARAMETRIC_BANDS {
            if i < count {
                let band = &snap.peq_bands[i];
                self.peq_l[i].set_parametric(self.sample_rate, band);
                self.peq_r[i].set_parametric(self.sample_rate, band);
            } else {
                self.peq_l[i].bypass = true;
                self.peq_r[i].bypass = true;
            }
        }
    }

    pub fn reset_state(&mut self) {
        for f in &mut self.eq_l {
            f.reset();
        }
        for f in &mut self.eq_r {
            f.reset();
        }
        for f in &mut self.peq_l {
            f.reset();
        }
        for f in &mut self.peq_r {
            f.reset();
        }
        self.cf_lp_l = 0.0;
        self.cf_lp_r = 0.0;
        self.ns_e1_l = 0.0;
        self.ns_e2_l = 0.0;
        self.ns_e1_r = 0.0;
        self.ns_e2_r = 0.0;
    }

    /// Lock-free xorshift64 uniform in [-0.5, 0.5).
    #[inline(always)]
    fn next_uniform(&mut self) -> f64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        // Top 53 bits -> [0, 1) -> [-0.5, 0.5)
        ((x >> 11) as f64) * (1.0 / ((1u64 << 53) as f64)) - 0.5
    }

    /// Exact triangular probability density function (TPDF) noise in [-1.0, 1.0].
    #[inline(always)]
    fn next_tpdf(&mut self) -> f64 {
        self.next_uniform() + self.next_uniform()
    }

    #[inline(always)]
    pub fn process_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        if !self.enabled && self.dither == DitherMode::Off {
            return (left, right);
        }
        let mut l = left as f64;
        let mut r = right as f64;

        if self.enabled {
            if self.peq_enabled && self.peq_active_count > 0 {
                l *= self.peq_preamp_gain;
                r *= self.peq_preamp_gain;
                for i in 0..self.peq_active_count {
                    l = self.peq_l[i].process(l);
                    r = self.peq_r[i].process(r);
                }
            }

            if self.eq_enabled {
                l *= self.preamp_gain;
                r *= self.preamp_gain;
                for i in 0..10 {
                    l = self.eq_l[i].process(l);
                    r = self.eq_r[i].process(r);
                }
            }

            if self.crossfeed_enabled && self.crossfeed_mix > 1e-4 {
                // Head-shadow low-passed opposite-channel bleed with energy-preserving attenuation
                self.cf_lp_l += self.cf_lp_alpha * (l - self.cf_lp_l);
                self.cf_lp_r += self.cf_lp_alpha * (r - self.cf_lp_r);
                let m = self.crossfeed_mix;
                let norm = 1.0 / (1.0 + m * 0.55);
                let out_l = (l + self.cf_lp_r * m) * norm;
                let out_r = (r + self.cf_lp_l * m) * norm;
                l = out_l;
                r = out_r;
            }

            if self.soft_limiter {
                l = soft_limit(l);
                r = soft_limit(r);
            }
        }

        if self.dither != DitherMode::Off {
            let dl = self.next_tpdf();
            let dr = self.next_tpdf();
            l = apply_dither(l, self.dither, dl, &mut self.ns_e1_l, &mut self.ns_e2_l);
            r = apply_dither(r, self.dither, dr, &mut self.ns_e1_r, &mut self.ns_e2_r);
        }

        (l as f32, r as f32)
    }
}

/// Applies 24-bit TPDF, 16-bit TPDF, or 16-bit 2nd-order psychoacoustic high-pass noise-shaped
/// dither to eliminate quantization distortion before integer DAC output.
#[inline(always)]
fn apply_dither(x: f64, mode: DitherMode, tpdf: f64, e1: &mut f64, e2: &mut f64) -> f64 {
    match mode {
        DitherMode::Off => x,
        DitherMode::Tpdf24 => {
            const SCALE: f64 = 8_388_607.0; // 2^23 - 1
            ((x * SCALE + tpdf).round().clamp(-SCALE, SCALE)) / SCALE
        }
        DitherMode::Tpdf16 => {
            const SCALE: f64 = 32_767.0; // 2^15 - 1
            ((x * SCALE + tpdf).round().clamp(-SCALE, SCALE)) / SCALE
        }
        DitherMode::NoiseShaped16 => {
            const SCALE: f64 = 32_767.0;
            // 2nd-order high-pass error-feedback noise shaper: pushes quantization noise > 14 kHz
            let fb = 1.5 * (*e1) - 0.55 * (*e2);
            let shaped = x * SCALE - fb;
            let q = (shaped + tpdf).round().clamp(-SCALE, SCALE);
            let err = q - shaped;
            *e2 = *e1;
            *e1 = err.clamp(-2.5, 2.5);
            q / SCALE
        }
    }
}

/// Transparent soft-knee limiter above -0.7 dBFS (0.92) to prevent inter-sample EQ clipping.
#[inline(always)]
fn soft_limit(x: f64) -> f64 {
    const KNEE: f64 = 0.92;
    const HEADROOM: f64 = 1.0 - KNEE;
    let ax = x.abs();
    if ax <= KNEE {
        x
    } else {
        let excess = (ax - KNEE) / HEADROOM;
        let shaped = KNEE + HEADROOM * excess.tanh();
        x.signum() * shaped.min(0.998)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{EqPreset, HeadphonePreset};

    #[test]
    fn dsp_bypass_is_bit_identical() {
        let cfg = DspConfig::default();
        let shared = SharedDspParams::new(&cfg);
        let mut dsp = RealtimeDsp::new(48_000, &shared);
        let (l, r) = dsp.process_stereo(0.42, -0.73);
        assert_eq!(l, 0.42);
        assert_eq!(r, -0.73);
    }

    #[test]
    fn dsp_eq_boost_and_limiter_prevent_clipping() {
        let mut cfg = DspConfig {
            enabled: true,
            preamp_db: 6.0,
            crossfeed_enabled: true,
            crossfeed_amount: 0.5,
            ..Default::default()
        };
        cfg.apply_preset(EqPreset::ConcertHall);
        let shared = SharedDspParams::new(&cfg);
        let mut dsp = RealtimeDsp::new(48_000, &shared);
        let mut max_out = 0.0_f32;
        for i in 0..4096 {
            let s = (std::f32::consts::TAU * 100.0 * i as f32 / 48_000.0).sin() * 0.95;
            let (l, r) = dsp.process_stereo(s, s * 0.5);
            assert!(l.is_finite() && r.is_finite());
            max_out = max_out.max(l.abs()).max(r.abs());
        }
        assert!(max_out <= 1.0, "soft limiter should bound peak <= 1.0, got {max_out}");
    }

    #[test]
    fn parametric_autoeq_response_matches_shelf_boost() {
        let mut cfg = DspConfig::default();
        cfg.apply_headphone_preset(HeadphonePreset::SennheiserHd650);
        let curve = compute_eq_response_curve(&cfg, 64);
        assert_eq!(curve.len(), 64);
        // HD650 has +5.5 dB low shelf at 105 Hz & -2.5 dB cut at 200 Hz -> sub-bass (bin 0, 20 Hz) > mid-bass
        assert!(
            curve[0] > curve[20] + 2.0,
            "Expected sub-bass shelf boost relative to 200 Hz (20Hz={}, mid={})",
            curve[0],
            curve[20]
        );
    }

    #[test]
    fn tpdf_and_noise_shaped_dither_are_bounded_and_finite() {
        for mode in [DitherMode::Tpdf24, DitherMode::Tpdf16, DitherMode::NoiseShaped16] {
            let cfg = DspConfig {
                enabled: false,
                dither: mode,
                ..Default::default()
            };
            let shared = SharedDspParams::new(&cfg);
            let mut dsp = RealtimeDsp::new(48_000, &shared);
            let mut max_err = 0.0_f32;
            let mut non_zero_diff = 0usize;
            for i in 0..2048 {
                let s = (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin() * 0.25;
                let (l, r) = dsp.process_stereo(s, -s);
                assert!(l.is_finite() && r.is_finite());
                assert!(l.abs() <= 1.0 && r.abs() <= 1.0);
                let err = (l - s).abs();
                max_err = max_err.max(err);
                if err > 0.0 {
                    non_zero_diff += 1;
                }
            }
            assert!(non_zero_diff > 1000, "{mode:?} should actively dither signal");
            let tol = match mode {
                DitherMode::Tpdf24 => 5e-6,
                DitherMode::Tpdf16 => 1e-4,
                DitherMode::NoiseShaped16 => 3e-4,
                DitherMode::Off => 0.0,
            };
            assert!(
                max_err <= tol,
                "{mode:?} quantization error {max_err} exceeded tolerance {tol}"
            );
        }
    }

    #[test]
    fn crossfeed_presets_produce_distinct_head_shadow_bleed() {
        use crate::plugins::CrossfeedPreset;
        let measure_bleed = |preset: CrossfeedPreset| -> f32 {
            let mut cfg = DspConfig::default();
            cfg.apply_crossfeed_preset(preset);
            let shared = SharedDspParams::new(&cfg);
            let mut dsp = RealtimeDsp::new(48_000, &shared);
            let mut rms_r = 0.0_f64;
            for i in 0..2048 {
                // Pure left-channel 250 Hz tone -> right channel receives head-shadow bleed only
                let s = (std::f32::consts::TAU * 250.0 * i as f32 / 48_000.0).sin() * 0.5;
                let (_l, r) = dsp.process_stereo(s, 0.0);
                if i >= 512 {
                    rms_r += (r as f64) * (r as f64);
                }
            }
            (rms_r / 1536.0).sqrt() as f32
        };
        let gentle = measure_bleed(CrossfeedPreset::Gentle);
        let cmoy = measure_bleed(CrossfeedPreset::ChuMoy);
        let meier = measure_bleed(CrossfeedPreset::JanMeier);
        let bauer = measure_bleed(CrossfeedPreset::BauerStudio);
        assert!(
            gentle > 0.01 && cmoy > gentle && meier > cmoy && bauer > meier,
            "Expected monotonic crossfeed bleed: gentle={gentle}, cmoy={cmoy}, meier={meier}, bauer={bauer}"
        );
    }
}
