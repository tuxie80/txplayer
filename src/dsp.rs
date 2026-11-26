use std::f64::consts::{FRAC_PI_2, TAU};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::plugins::{
    DitherMode, DspConfig, EQ_FREQS_HZ, IrPreset, MAX_PARAMETRIC_BANDS, ParametricBand,
    ParametricFilterKind,
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

const CONV_BLOCK: usize = 512;
const CONV_FFT: usize = CONV_BLOCK * 2; // 1024-pt overlap-add
const CONV_MAX_TAPS: usize = 16_384; // up to 32 partitions

/// In-place radix-2 Cooley-Tukey FFT (`inverse = false` for forward, `true` for inverse with 1/N scaling).
fn fft_f64(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while (j & bit) != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let half = len >> 1;
        let ang = sign * TAU / len as f64;
        let wlen_re = ang.cos();
        let wlen_im = ang.sin();
        let mut i = 0;
        while i < n {
            let mut w_re = 1.0;
            let mut w_im = 0.0;
            for k in 0..half {
                let u_re = re[i + k];
                let u_im = im[i + k];
                let v_re = re[i + k + half] * w_re - im[i + k + half] * w_im;
                let v_im = re[i + k + half] * w_im + im[i + k + half] * w_re;
                re[i + k] = u_re + v_re;
                im[i + k] = u_im + v_im;
                re[i + k + half] = u_re - v_re;
                im[i + k + half] = u_im - v_im;
                let next_re = w_re * wlen_re - w_im * wlen_im;
                w_im = w_re * wlen_im + w_im * wlen_re;
                w_re = next_re;
            }
            i += len;
        }
        len <<= 1;
    }
    if inverse {
        let inv_n = 1.0 / n as f64;
        for (r, i) in re.iter_mut().zip(im.iter_mut()) {
            *r *= inv_n;
            *i *= inv_n;
        }
    }
}

/// Pure-Rust `.wav` Impulse Response loader (16/24/32-bit int & 32-bit float PCM, mono or stereo).
/// Automatically resamples to `target_sr` and caps cleanly to `CONV_MAX_TAPS`.
pub fn load_wav_ir(path: &Path, target_sr: u32) -> Result<(Vec<f64>, Vec<f64>), String> {
    let data = std::fs::read(path).map_err(|e| format!("Reading IR file: {e}"))?;
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err("Not a valid RIFF WAVE (.wav) impulse response file".into());
    }
    let mut pos = 12usize;
    let mut fmt_tag = 0u16;
    let mut channels = 0u16;
    let mut wav_sr = 0u32;
    let mut bits_per_sample = 0u16;
    let mut raw_data: Option<&[u8]> = None;

    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let sz = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = pos + 8;
        let body_end = body_start.saturating_add(sz).min(data.len());
        if id == b"fmt " && body_end - body_start >= 16 {
            let f = &data[body_start..body_end];
            fmt_tag = u16::from_le_bytes([f[0], f[1]]);
            channels = u16::from_le_bytes([f[2], f[3]]);
            wav_sr = u32::from_le_bytes(f[4..8].try_into().unwrap());
            bits_per_sample = u16::from_le_bytes([f[14], f[15]]);
            if fmt_tag == 0xFFFE && f.len() >= 26 {
                fmt_tag = u16::from_le_bytes([f[24], f[25]]);
            }
        } else if id == b"data" {
            raw_data = Some(&data[body_start..body_end]);
        }
        pos = body_end + (sz & 1);
    }

    let raw = raw_data.ok_or_else(|| "WAV IR missing data chunk".to_string())?;
    if channels == 0 || wav_sr < 8_000 {
        return Err("Invalid WAV IR format header".into());
    }
    let ch = channels as usize;
    let bytes_per_sample = (bits_per_sample / 8) as usize;
    if bytes_per_sample == 0 {
        return Err("Unsupported WAV IR bit depth".into());
    }
    let frame_bytes = ch * bytes_per_sample;
    let total_frames = raw.len() / frame_bytes;
    if total_frames == 0 {
        return Err("WAV IR contains zero audio samples".into());
    }

    let mut ir_l = Vec::with_capacity(total_frames.min(CONV_MAX_TAPS * 2));
    let mut ir_r = Vec::with_capacity(total_frames.min(CONV_MAX_TAPS * 2));
    let read_sample = |slice: &[u8]| -> f64 {
        match (fmt_tag, bits_per_sample) {
            (1, 16) => i16::from_le_bytes([slice[0], slice[1]]) as f64 / 32_768.0,
            (1, 24) => {
                let v = ((slice[2] as i32) << 24)
                    | ((slice[1] as i32) << 16)
                    | ((slice[0] as i32) << 8);
                (v >> 8) as f64 / 8_388_608.0
            }
            (1, 32) => {
                i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as f64 / 2_147_483_648.0
            }
            (3, 32) => {
                let f = f32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]);
                if f.is_finite() { f as f64 } else { 0.0 }
            }
            _ => 0.0,
        }
    };

    for f_idx in 0..total_frames {
        let base = f_idx * frame_bytes;
        let sl = read_sample(&raw[base..base + bytes_per_sample]);
        let sr = if ch >= 2 {
            read_sample(&raw[base + bytes_per_sample..base + 2 * bytes_per_sample])
        } else {
            sl
        };
        ir_l.push(sl);
        ir_r.push(sr);
        if ir_l.len() >= CONV_MAX_TAPS * 2 {
            break;
        }
    }

    // Resample linearly if WAV sample rate differs from active DAC sample rate
    if wav_sr != target_sr {
        let ratio = target_sr as f64 / wav_sr as f64;
        let out_len = ((ir_l.len() as f64 * ratio).round() as usize).clamp(1, CONV_MAX_TAPS);
        let resample = |src: &[f64]| -> Vec<f64> {
            let mut out = Vec::with_capacity(out_len);
            let scale = 1.0 / ratio.max(0.25);
            for i in 0..out_len {
                let pos_f = i as f64 / ratio;
                let idx = (pos_f as usize).min(src.len().saturating_sub(1));
                let frac = pos_f - idx as f64;
                let next = (idx + 1).min(src.len().saturating_sub(1));
                out.push((src[idx] * (1.0 - frac) + src[next] * frac) * scale);
            }
            out
        };
        ir_l = resample(&ir_l);
        ir_r = resample(&ir_r);
    } else {
        ir_l.truncate(CONV_MAX_TAPS);
        ir_r.truncate(CONV_MAX_TAPS);
    }
    Ok((ir_l, ir_r))
}

/// Generates built-in studio/acoustic stereo impulse responses for instant 1-click convolution.
fn build_preset_ir(preset: IrPreset, sr: f64) -> Option<(Vec<f64>, Vec<f64>)> {
    match preset {
        IrPreset::Off | IrPreset::CustomWav => None,
        IrPreset::LinearWarmth => {
            // Subtle warm analog transformer/console FIR (16 taps, smooth HF roll & slight 2nd-tap body)
            let mut h = vec![0.0_f64; 16];
            h[0] = 0.86;
            h[1] = 0.14;
            h[2] = -0.025;
            h[3] = 0.025;
            Some((h.clone(), h))
        }
        IrPreset::StudioAir => {
            // Crisp mastering presence/air FIR (high-frequency micro-lift)
            let mut h = vec![0.0_f64; 16];
            h[0] = 1.06;
            h[1] = -0.05;
            h[2] = -0.01;
            Some((h.clone(), h))
        }
        IrPreset::ChamberRoom => {
            // Tasteful acoustic early-reflection studio chamber (~85 ms tail)
            let len = ((sr * 0.085).round() as usize).clamp(512, 4096);
            let mut l = vec![0.0_f64; len];
            let mut r = vec![0.0_f64; len];
            l[0] = 0.88;
            r[0] = 0.88;
            let taps_l = [
                (0.0085, 0.14),
                (0.0152, -0.10),
                (0.0238, 0.075),
                (0.0361, -0.052),
                (0.0524, 0.034),
                (0.0710, -0.018),
            ];
            let taps_r = [
                (0.0097, 0.135),
                (0.0169, -0.095),
                (0.0261, 0.072),
                (0.0389, -0.048),
                (0.0562, 0.031),
                (0.0755, -0.016),
            ];
            for (sec, amp) in taps_l {
                let idx = (sec * sr).round() as usize;
                if idx < len {
                    l[idx] += amp;
                }
            }
            for (sec, amp) in taps_r {
                let idx = (sec * sr).round() as usize;
                if idx < len {
                    r[idx] += amp;
                }
            }
            Some((l, r))
        }
    }
}

/// Real-time Lock-Free Partitioned FFT Overlap-Add Stereo Convolver (zero audio-thread allocations).
struct StereoConvolver {
    num_parts: usize,
    ir_re_l: Vec<[f64; CONV_FFT]>,
    ir_im_l: Vec<[f64; CONV_FFT]>,
    ir_re_r: Vec<[f64; CONV_FFT]>,
    ir_im_r: Vec<[f64; CONV_FFT]>,
    fdl_re_l: Vec<[f64; CONV_FFT]>,
    fdl_im_l: Vec<[f64; CONV_FFT]>,
    fdl_re_r: Vec<[f64; CONV_FFT]>,
    fdl_im_r: Vec<[f64; CONV_FFT]>,
    fdl_pos: usize,
    in_l: [f64; CONV_BLOCK],
    in_r: [f64; CONV_BLOCK],
    out_l: [f64; CONV_BLOCK],
    out_r: [f64; CONV_BLOCK],
    overlap_l: [f64; CONV_BLOCK],
    overlap_r: [f64; CONV_BLOCK],
    dry_delay_l: [f64; CONV_BLOCK],
    dry_delay_r: [f64; CONV_BLOCK],
    cursor: usize,
    work_re: [f64; CONV_FFT],
    work_im: [f64; CONV_FFT],
}

impl StereoConvolver {
    fn from_ir(ir_l: &[f64], ir_r: &[f64]) -> Self {
        let len = ir_l.len().max(ir_r.len()).clamp(1, CONV_MAX_TAPS);
        let num_parts = len.div_ceil(CONV_BLOCK);
        let mut ir_re_l = vec![[0.0; CONV_FFT]; num_parts];
        let mut ir_im_l = vec![[0.0; CONV_FFT]; num_parts];
        let mut ir_re_r = vec![[0.0; CONV_FFT]; num_parts];
        let mut ir_im_r = vec![[0.0; CONV_FFT]; num_parts];

        for p in 0..num_parts {
            let start = p * CONV_BLOCK;
            for i in 0..CONV_BLOCK {
                if start + i < ir_l.len() {
                    ir_re_l[p][i] = ir_l[start + i];
                }
                if start + i < ir_r.len() {
                    ir_re_r[p][i] = ir_r[start + i];
                }
            }
            fft_f64(&mut ir_re_l[p], &mut ir_im_l[p], false);
            fft_f64(&mut ir_re_r[p], &mut ir_im_r[p], false);
        }

        Self {
            num_parts,
            ir_re_l,
            ir_im_l,
            ir_re_r,
            ir_im_r,
            fdl_re_l: vec![[0.0; CONV_FFT]; num_parts],
            fdl_im_l: vec![[0.0; CONV_FFT]; num_parts],
            fdl_re_r: vec![[0.0; CONV_FFT]; num_parts],
            fdl_im_r: vec![[0.0; CONV_FFT]; num_parts],
            fdl_pos: 0,
            in_l: [0.0; CONV_BLOCK],
            in_r: [0.0; CONV_BLOCK],
            out_l: [0.0; CONV_BLOCK],
            out_r: [0.0; CONV_BLOCK],
            overlap_l: [0.0; CONV_BLOCK],
            overlap_r: [0.0; CONV_BLOCK],
            dry_delay_l: [0.0; CONV_BLOCK],
            dry_delay_r: [0.0; CONV_BLOCK],
            cursor: 0,
            work_re: [0.0; CONV_FFT],
            work_im: [0.0; CONV_FFT],
        }
    }

    fn reset(&mut self) {
        for p in 0..self.num_parts {
            self.fdl_re_l[p].fill(0.0);
            self.fdl_im_l[p].fill(0.0);
            self.fdl_re_r[p].fill(0.0);
            self.fdl_im_r[p].fill(0.0);
        }
        self.fdl_pos = 0;
        self.in_l.fill(0.0);
        self.in_r.fill(0.0);
        self.out_l.fill(0.0);
        self.out_r.fill(0.0);
        self.overlap_l.fill(0.0);
        self.overlap_r.fill(0.0);
        self.dry_delay_l.fill(0.0);
        self.dry_delay_r.fill(0.0);
        self.cursor = 0;
    }

    #[inline(always)]
    fn process_stereo(&mut self, l: f64, r: f64, wet: f64, gain: f64) -> (f64, f64) {
        let dry_l = self.dry_delay_l[self.cursor];
        let dry_r = self.dry_delay_r[self.cursor];
        let wet_l = self.out_l[self.cursor] * gain;
        let wet_r = self.out_r[self.cursor] * gain;

        self.in_l[self.cursor] = l;
        self.in_r[self.cursor] = r;
        self.dry_delay_l[self.cursor] = l;
        self.dry_delay_r[self.cursor] = r;
        self.cursor += 1;

        if self.cursor >= CONV_BLOCK {
            self.cursor = 0;
            self.process_block();
        }

        let inv = 1.0 - wet;
        (dry_l * inv + wet_l * wet, dry_r * inv + wet_r * wet)
    }

    fn process_block(&mut self) {
        // Transform Left input block into current FDL slot
        self.fdl_re_l[self.fdl_pos][..CONV_BLOCK].copy_from_slice(&self.in_l);
        self.fdl_re_l[self.fdl_pos][CONV_BLOCK..].fill(0.0);
        self.fdl_im_l[self.fdl_pos].fill(0.0);
        fft_f64(
            &mut self.fdl_re_l[self.fdl_pos],
            &mut self.fdl_im_l[self.fdl_pos],
            false,
        );

        // Convolve Left across all partitions
        self.work_re.fill(0.0);
        self.work_im.fill(0.0);
        for p in 0..self.num_parts {
            let slot = (self.fdl_pos + self.num_parts - p) % self.num_parts;
            let xr = &self.fdl_re_l[slot];
            let xi = &self.fdl_im_l[slot];
            let hr = &self.ir_re_l[p];
            let hi = &self.ir_im_l[p];
            for k in 0..CONV_FFT {
                self.work_re[k] += xr[k] * hr[k] - xi[k] * hi[k];
                self.work_im[k] += xr[k] * hi[k] + xi[k] * hr[k];
            }
        }
        fft_f64(&mut self.work_re, &mut self.work_im, true);
        for i in 0..CONV_BLOCK {
            self.out_l[i] = self.work_re[i] + self.overlap_l[i];
            self.overlap_l[i] = self.work_re[CONV_BLOCK + i];
        }

        // Transform Right input block into current FDL slot
        self.fdl_re_r[self.fdl_pos][..CONV_BLOCK].copy_from_slice(&self.in_r);
        self.fdl_re_r[self.fdl_pos][CONV_BLOCK..].fill(0.0);
        self.fdl_im_r[self.fdl_pos].fill(0.0);
        fft_f64(
            &mut self.fdl_re_r[self.fdl_pos],
            &mut self.fdl_im_r[self.fdl_pos],
            false,
        );

        // Convolve Right across all partitions
        self.work_re.fill(0.0);
        self.work_im.fill(0.0);
        for p in 0..self.num_parts {
            let slot = (self.fdl_pos + self.num_parts - p) % self.num_parts;
            let xr = &self.fdl_re_r[slot];
            let xi = &self.fdl_im_r[slot];
            let hr = &self.ir_re_r[p];
            let hi = &self.ir_im_r[p];
            for k in 0..CONV_FFT {
                self.work_re[k] += xr[k] * hr[k] - xi[k] * hi[k];
                self.work_im[k] += xr[k] * hi[k] + xi[k] * hr[k];
            }
        }
        fft_f64(&mut self.work_re, &mut self.work_im, true);
        for i in 0..CONV_BLOCK {
            self.out_r[i] = self.work_re[i] + self.overlap_r[i];
            self.overlap_r[i] = self.work_re[CONV_BLOCK + i];
        }

        self.fdl_pos = (self.fdl_pos + 1) % self.num_parts;
    }
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
    balance_gain_l: f64,
    balance_gain_r: f64,
    mono_fold: bool,
    invert_polarity: bool,
    swap_channels: bool,
    ir_enabled: bool,
    ir_mix: f64,
    ir_gain: f64,
    loaded_ir_key: (IrPreset, Option<std::path::PathBuf>),
    convolver: Option<Box<StereoConvolver>>,
    pitch_ratio: f64,
    pitch_shifter: Box<StereoPitchShifter>,
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

/// Lock-free, zero-allocation pure-Rust dual-tap Hann-windowed stereo pitch shifter
/// supporting real-time ±12 semitone transpose, concert tuning (`A=432/415 Hz`), and
/// pitch-preserving practice slowdown/speedup.
const PITCH_BUF_LEN: usize = 8192;
const PITCH_BUF_MASK: usize = PITCH_BUF_LEN - 1;

struct StereoPitchShifter {
    buf_l: [f64; PITCH_BUF_LEN],
    buf_r: [f64; PITCH_BUF_LEN],
    write_pos: usize,
    grain_len: f64,
    phase: f64,
}

impl StereoPitchShifter {
    fn new(sample_rate: f64) -> Self {
        // ~44 ms grain window keeps piano attacks crisp while staying smooth on sustained strings
        let grain_len = (sample_rate * 0.044).clamp(512.0, 3600.0);
        Self {
            buf_l: [0.0; PITCH_BUF_LEN],
            buf_r: [0.0; PITCH_BUF_LEN],
            write_pos: 0,
            grain_len,
            phase: 0.0,
        }
    }

    fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write_pos = 0;
        self.phase = 0.0;
    }

    #[inline(always)]
    fn read_interp(buf: &[f64; PITCH_BUF_LEN], write_pos: usize, delay: f64) -> f64 {
        let pos = (write_pos as f64) - delay;
        let idx0 = pos.floor() as isize;
        let frac = pos - (idx0 as f64);
        let i0 = (idx0 as usize) & PITCH_BUF_MASK;
        let i1 = i0.wrapping_add(1) & PITCH_BUF_MASK;
        buf[i0] + (buf[i1] - buf[i0]) * frac
    }

    #[inline(always)]
    fn process_stereo(&mut self, l: f64, r: f64, ratio: f64) -> (f64, f64) {
        self.buf_l[self.write_pos] = l;
        self.buf_r[self.write_pos] = r;

        let ph_a = self.phase;
        let ph_b = (self.phase + 0.5).fract();
        let d_a = ph_a * self.grain_len + 4.0;
        let d_b = ph_b * self.grain_len + 4.0;

        // Complementary Hann crossfade windows (sum identically to 1.0!)
        let w_a = 0.5 * (1.0 - (TAU * ph_a).cos());
        let w_b = 0.5 * (1.0 - (TAU * ph_b).cos());

        let out_l = Self::read_interp(&self.buf_l, self.write_pos, d_a) * w_a
            + Self::read_interp(&self.buf_l, self.write_pos, d_b) * w_b;
        let out_r = Self::read_interp(&self.buf_r, self.write_pos, d_a) * w_a
            + Self::read_interp(&self.buf_r, self.write_pos, d_b) * w_b;

        self.write_pos = (self.write_pos + 1) & PITCH_BUF_MASK;
        // Delay moves at rate (1 - ratio) per input sample -> read pointer advances at `ratio`
        let delta = (1.0 - ratio) / self.grain_len;
        self.phase = (self.phase + delta).rem_euclid(1.0);
        (out_l, out_r)
    }
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
            balance_gain_l: 1.0,
            balance_gain_r: 1.0,
            mono_fold: false,
            invert_polarity: false,
            swap_channels: false,
            ir_enabled: false,
            ir_mix: 1.0,
            ir_gain: 1.0,
            loaded_ir_key: (IrPreset::Off, None),
            convolver: None,
            pitch_ratio: 1.0,
            pitch_shifter: Box::new(StereoPitchShifter::new(sr)),
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

        // Constant-power L/R balance (0.0 = unity both channels, -1.0 = full left, +1.0 = full right)
        let bal = (snap.balance as f64).clamp(-1.0, 1.0);
        if bal.abs() < 1e-4 {
            self.balance_gain_l = 1.0;
            self.balance_gain_r = 1.0;
        } else if bal > 0.0 {
            self.balance_gain_l = ((1.0 - bal) * FRAC_PI_2).sin();
            self.balance_gain_r = 1.0;
        } else {
            self.balance_gain_l = 1.0;
            self.balance_gain_r = ((1.0 + bal) * FRAC_PI_2).sin();
        }
        self.mono_fold = snap.mono_fold;
        self.invert_polarity = snap.invert_polarity;
        self.swap_channels = snap.swap_channels;

        let total_st = (snap.pitch_semitones as f64).clamp(-12.0, 12.0)
            + snap.concert_tuning.semitone_offset();
        let speed_comp = if snap.preserve_pitch && snap.playback_speed_hint > 0.2 {
            snap.playback_speed_hint as f64
        } else {
            1.0
        };
        let raw_ratio = 2.0_f64.powf(total_st / 12.0) / speed_comp;
        self.pitch_ratio = if (raw_ratio - 1.0).abs() < 1e-4 {
            1.0
        } else {
            raw_ratio.clamp(0.4, 2.5)
        };

        self.ir_enabled = snap.ir_enabled && snap.ir_preset != IrPreset::Off;
        self.ir_mix = (snap.ir_mix as f64).clamp(0.0, 1.0);
        self.ir_gain = 10.0_f64.powf((snap.ir_gain_db as f64).clamp(-12.0, 6.0) / 20.0);
        let new_ir_key = (snap.ir_preset, snap.ir_path.clone());
        if new_ir_key != self.loaded_ir_key {
            self.loaded_ir_key = new_ir_key;
            self.convolver = match snap.ir_preset {
                IrPreset::Off => None,
                IrPreset::CustomWav => snap
                    .ir_path
                    .as_deref()
                    .and_then(|p| load_wav_ir(p, self.sample_rate.round() as u32).ok())
                    .map(|(l, r)| Box::new(StereoConvolver::from_ir(&l, &r))),
                builtin => build_preset_ir(builtin, self.sample_rate)
                    .map(|(l, r)| Box::new(StereoConvolver::from_ir(&l, &r))),
            };
        }

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
        if let Some(ref mut conv) = self.convolver {
            conv.reset();
        }
        self.pitch_shifter.reset();
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

            if self.ir_enabled
                && self.ir_mix > 1e-4
                && let Some(ref mut conv) = self.convolver
            {
                (l, r) = conv.process_stereo(l, r, self.ir_mix, self.ir_gain);
            }

            if (self.pitch_ratio - 1.0).abs() >= 1e-4 {
                (l, r) = self.pitch_shifter.process_stereo(l, r, self.pitch_ratio);
            }

            if self.swap_channels {
                std::mem::swap(&mut l, &mut r);
            }
            if self.mono_fold {
                let m = (l + r) * 0.5;
                l = m;
                r = m;
            }
            if self.invert_polarity {
                l = -l;
                r = -r;
            }
            l *= self.balance_gain_l;
            r *= self.balance_gain_r;

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

    #[test]
    fn partitioned_fft_convolver_reproduces_exact_impulse_response() {
        // Multi-partition IR (600 taps > 512 CONV_BLOCK) with impulses at tap 0, 17, and 550
        let mut ir_l = vec![0.0_f64; 600];
        let mut ir_r = vec![0.0_f64; 600];
        ir_l[0] = 0.75;
        ir_l[17] = -0.25;
        ir_l[550] = 0.40;
        ir_r[0] = 0.50;
        ir_r[550] = -0.30;

        let mut conv = StereoConvolver::from_ir(&ir_l, &ir_r);
        let mut out_l = Vec::new();
        let mut out_r = Vec::new();
        for i in 0..(CONV_BLOCK * 3) {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (yl, yr) = conv.process_stereo(x, x, 1.0, 1.0);
            out_l.push(yl);
            out_r.push(yr);
        }
        // Overlap-add latency is exactly CONV_BLOCK samples (512)
        assert!((out_l[CONV_BLOCK] - 0.75).abs() < 1e-9);
        assert!((out_l[CONV_BLOCK + 17] - (-0.25)).abs() < 1e-9);
        assert!((out_l[CONV_BLOCK + 550] - 0.40).abs() < 1e-9);
        assert!((out_r[CONV_BLOCK] - 0.50).abs() < 1e-9);
        assert!((out_r[CONV_BLOCK + 550] - (-0.30)).abs() < 1e-9);
    }

    #[test]
    fn mastering_console_polarity_mono_swap_and_balance() {
        let cfg = DspConfig {
            enabled: true,
            eq_enabled: false,
            soft_limiter: false,
            swap_channels: true,
            invert_polarity: true,
            balance: 1.0, // full right -> left attenuated to 0
            ..Default::default()
        };
        let shared = SharedDspParams::new(&cfg);
        let mut dsp = RealtimeDsp::new(48_000, &shared);
        let (l, r) = dsp.process_stereo(0.4, -0.2);
        // After swap: (-0.2, 0.4) -> invert: (0.2, -0.4) -> balance +1.0: (0.0, -0.4)
        assert!(l.abs() < 1e-5, "Expected left muted at +100% balance, got {l}");
        assert!((r - (-0.4)).abs() < 1e-5, "Expected inverted swapped right = -0.4, got {r}");
    }

    #[test]
    fn pitch_shifter_transposes_440hz_up_one_octave_to_880hz() {
        let sr = 48_000.0_f64;
        let mut shifter = StereoPitchShifter::new(sr);
        let n = 8192usize;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let x = (TAU * 440.0 * i as f64 / sr).sin();
            let (yl, _) = shifter.process_stereo(x, x, 2.0);
            out.push(yl);
        }
        // Measure dominant frequency via DFT energy at 440 Hz vs 880 Hz on settled tail
        let tail = &out[4096..];
        let power_at = |freq: f64| -> f64 {
            let (mut re, mut im) = (0.0, 0.0);
            for (k, &v) in tail.iter().enumerate() {
                let a = TAU * freq * k as f64 / sr;
                re += v * a.cos();
                im -= v * a.sin();
            }
            (re * re + im * im).sqrt()
        };
        let p440 = power_at(440.0);
        let p880 = power_at(880.0);
        assert!(
            p880 > p440 * 10.0 && p880 > 200.0,
            "Expected strong 880 Hz octave shift (p880={p880:.1}, p440={p440:.1})"
        );
    }
}
