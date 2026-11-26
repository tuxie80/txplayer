use std::f64::consts::PI;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

const FFT_SIZE: usize = 2048;
const SPECTRUM_BINS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HiResVerdict {
    AuthenticHiRes,
    LosslessCdQuality,
    SuspectedUpsample,
    LossyTranscode,
}

impl HiResVerdict {
    pub fn badge(self) -> &'static str {
        match self {
            HiResVerdict::AuthenticHiRes => "AUTHENTIC HI-RES",
            HiResVerdict::LosslessCdQuality => "TRUE LOSSLESS CD",
            HiResVerdict::SuspectedUpsample => "SUSPECTED UPSCALE",
            HiResVerdict::LossyTranscode => "LOSSY CUTOFF",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioAnalysis {
    pub sample_rate: u32,
    pub dr_score: u8,
    pub dr_precise: f32,
    pub integrated_lufs: f32,
    pub sample_peak_db: f32,
    pub true_peak_dbtp: f32,
    pub clipped_samples: u64,
    pub effective_bandwidth_hz: f32,
    pub verdict: HiResVerdict,
    pub verdict_detail: String,
    /// 64 linearly spaced average spectral magnitude bins from 0 Hz to Nyquist, in dBFS (-96..0).
    pub spectrum_db: Vec<f32>,
}

/// Decodes a local audio file (respecting optional CUE boundaries) and computes
/// Dynamic Range (DR), EBU R128 Integrated Loudness (LUFS), 4x True Peak (dBTP),
/// and Hi-Res spectral bandwidth authenticity.
pub fn analyze_file(
    path: &Path,
    cue_start: Option<f64>,
    cue_end: Option<f64>,
) -> Result<AudioAnalysis> {
    if crate::archive::is_archive_codec_path(path) {
        let mut dec = crate::archive::ArchivePcmDecoder::open(path)?;
        let sr = dec.info.sample_rate;
        let ch = dec.info.channels.max(1) as usize;
        let bit_depth = dec.info.bit_depth;
        let start_secs = cue_start.unwrap_or(0.0).max(0.0);
        if start_secs > 0.05 {
            let _ = dec.seek(start_secs);
        }
        let max_frames = cue_end
            .map(|e| ((e - start_secs).max(0.0) * sr as f64).round() as u64)
            .unwrap_or(u64::MAX);
        let mut acc = StreamAnalyzer::new(sr, ch);
        let mut pcm = Vec::with_capacity(4096 * ch);
        let mut total_frames = 0u64;
        while total_frames < max_frames {
            let want = 4096.min((max_frames - total_frames) as usize);
            let n = dec.read_pcm_frames(want, &mut pcm)?;
            if n == 0 {
                break;
            }
            acc.push_interleaved(&pcm, ch);
            total_frames += n as u64;
        }
        if total_frames == 0 {
            return Err(anyhow!("Archive track contained no decodable audio frames"));
        }
        return Ok(acc.finish(bit_depth));
    }
    if crate::dsd::is_dsd_path(path) {
        let mut dec = crate::dsd::DsdPcmDecoder::open(path)?;
        let sr = dec.info.pcm_sample_rate;
        let ch = dec.info.channels;
        let start_secs = cue_start.unwrap_or(0.0).max(0.0);
        if start_secs > 0.05 {
            let _ = dec.seek(start_secs);
        }
        let max_frames = cue_end
            .map(|e| ((e - start_secs).max(0.0) * sr as f64).round() as u64)
            .unwrap_or(u64::MAX);
        let mut acc = StreamAnalyzer::new(sr, ch);
        let mut pcm = Vec::with_capacity(4096 * ch);
        let mut total_frames = 0u64;
        while total_frames < max_frames {
            let want = 4096.min((max_frames - total_frames) as usize);
            let n = dec.read_pcm_frames(want, &mut pcm)?;
            if n == 0 {
                break;
            }
            acc.push_interleaved(&pcm, ch);
            total_frames += n as u64;
        }
        if total_frames == 0 {
            return Err(anyhow!("DSD track contained no decodable audio frames"));
        }
        return Ok(acc.finish(Some(24)));
    }
    if crate::dsd::is_raw_cd_bin(path) {
        let mut dec = crate::dsd::RawCdBinDecoder::open(path, false)?;
        let sr = 44_100_u32;
        let start_secs = cue_start.unwrap_or(0.0).max(0.0);
        if start_secs > 0.05 {
            let _ = dec.seek(start_secs);
        }
        let max_frames = cue_end
            .map(|e| ((e - start_secs).max(0.0) * sr as f64).round() as u64)
            .unwrap_or(u64::MAX);
        let mut acc = StreamAnalyzer::new(sr, 2);
        let mut pcm = Vec::with_capacity(4096 * 2);
        let mut total_frames = 0u64;
        while total_frames < max_frames {
            let want = 4096.min((max_frames - total_frames) as usize);
            let n = dec.read_pcm_frames(want, &mut pcm)?;
            if n == 0 {
                break;
            }
            acc.push_interleaved(&pcm, 2);
            total_frames += n as u64;
        }
        if total_frames == 0 {
            return Err(anyhow!("CD image contained no decodable audio frames"));
        }
        return Ok(acc.finish(Some(16)));
    }

    let file = std::fs::File::open(path).with_context(|| format!("Opening {}", path.display()))?;
    let source = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("Probing audio format")?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("No audio track found"))?;
    let tid = track.id;
    let params = track.codec_params.clone();
    if params.codec == CODEC_TYPE_NULL {
        return Err(anyhow!("Unsupported codec"));
    }
    let sample_rate = params.sample_rate.unwrap_or(44_100).max(8_000);
    let channels = params.channels.map(|c| c.count()).unwrap_or(2).max(1);
    let bit_depth = params.bits_per_sample.or(params.bits_per_coded_sample);
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .context("Creating decoder")?;

    let start_secs = cue_start.unwrap_or(0.0).max(0.0);
    if start_secs > 0.05 {
        let time = Time::new(start_secs.trunc() as u64, start_secs.fract());
        let _ = format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time,
                track_id: Some(tid),
            },
        );
        decoder.reset();
    }
    let max_frames = cue_end
        .map(|e| ((e - start_secs).max(0.0) * sample_rate as f64).round() as u64)
        .unwrap_or(u64::MAX);

    let mut acc = StreamAnalyzer::new(sample_rate, channels);
    let mut sample_buf: Option<(usize, SampleBuffer<f32>)> = None;
    let mut total_frames: u64 = 0;

    loop {
        if total_frames >= max_frames {
            break;
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != tid {
            continue;
        }
        let Ok(audio) = decoder.decode(&packet) else {
            continue;
        };
        let spec = *audio.spec();
        let pkt_ch = spec.channels.count().max(1);
        let cap = audio.capacity();
        let buf = match &mut sample_buf {
            Some((cur_cap, sb)) if *cur_cap >= cap => sb,
            slot => {
                *slot = Some((cap, SampleBuffer::<f32>::new(cap as u64, spec)));
                &mut slot.as_mut().unwrap().1
            }
        };
        buf.copy_interleaved_ref(audio);
        let data = buf.samples();
        let frames_in_pkt = data.len() / pkt_ch;
        let take_frames = frames_in_pkt.min((max_frames - total_frames) as usize);
        if take_frames > 0 {
            acc.push_interleaved(&data[..take_frames * pkt_ch], pkt_ch);
            total_frames += take_frames as u64;
        }
    }

    if total_frames == 0 {
        return Err(anyhow!("Track contained no decodable audio frames"));
    }

    Ok(acc.finish(bit_depth))
}

/// Simple second-order biquad filter for EBU R128 K-weighting.
#[derive(Clone, Copy)]
struct Biquad64 {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad64 {
    /// Stage 1: +4 dB high-shelf above ~1.68 kHz (acoustic head modeling).
    fn r128_high_shelf(sample_rate: f64) -> Self {
        let f0 = 1681.974450955533;
        let g_db = 3.999843853973347;
        let q = 0.7071752369554196;
        let k = (PI * f0 / sample_rate).tan();
        let vh = 10.0_f64.powf(g_db / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        Self {
            b0: (vh + vb * k / q + k * k) / a0,
            b1: 2.0 * (k * k - vh) / a0,
            b2: (vh - vb * k / q + k * k) / a0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Stage 2: 38 Hz 2nd-order Butterworth high-pass filter.
    fn r128_high_pass(sample_rate: f64) -> Self {
        let f0 = 38.13547087602444;
        let q = 0.5003270373238773;
        let k = (PI * f0 / sample_rate).tan();
        let a0 = 1.0 + k / q + k * k;
        Self {
            b0: 1.0 / a0,
            b1: -2.0 / a0,
            b2: 1.0 / a0,
            a1: 2.0 * (k * k - 1.0) / a0,
            a2: (1.0 - k / q + k * k) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline(always)]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

struct StreamAnalyzer {
    sample_rate: u32,
    channels: usize,
    // Peak & True Peak (cubic Catmull-Rom 4x oversampled interpolation)
    sample_peak: f64,
    true_peak: f64,
    clipped_samples: u64,
    hist_l: [f64; 4],
    hist_r: [f64; 4],
    hist_count: usize,
    // Dynamic Range (3-second blocks, per channel)
    dr_block_frames: usize,
    dr_cur_frames: usize,
    dr_sum_sq_l: f64,
    dr_sum_sq_r: f64,
    dr_peak_l: f64,
    dr_peak_r: f64,
    dr_rms_blocks_l: Vec<f64>,
    dr_rms_blocks_r: Vec<f64>,
    dr_peak_blocks_l: Vec<f64>,
    dr_peak_blocks_r: Vec<f64>,
    // EBU R128 Integrated Loudness (400 ms blocks, 75% overlap -> 100 ms hops)
    k1_l: Biquad64,
    k2_l: Biquad64,
    k1_r: Biquad64,
    k2_r: Biquad64,
    hop_frames: usize,
    hop_cur_frames: usize,
    hop_sum_sq: f64,
    hop_energies: Vec<f64>,
    // Average spectral distribution (sample up to ~120 FFT windows across the track)
    fft_window: Vec<f64>,
    fft_ring: Vec<f64>,
    fft_accum: Vec<f64>,
    fft_count: usize,
    fft_stride_frames: usize,
    fft_skip_counter: usize,
}

impl StreamAnalyzer {
    fn new(sample_rate: u32, channels: usize) -> Self {
        let sr = sample_rate as f64;
        let dr_block_frames = (sample_rate as usize * 3).max(1024);
        let hop_frames = (sample_rate as usize / 10).max(256); // 100 ms
        let mut fft_window = Vec::with_capacity(FFT_SIZE);
        for i in 0..FFT_SIZE {
            // Hann window normalized for coherent amplitude
            let w = 0.5 * (1.0 - (2.0 * PI * i as f64 / FFT_SIZE as f64).cos());
            fft_window.push(w);
        }
        // Take ~1 FFT every 0.75s so long tracks analyze in well under 1 second
        let fft_stride_frames = ((sample_rate as f64 * 0.75) as usize).max(FFT_SIZE);
        Self {
            sample_rate,
            channels,
            sample_peak: 0.0,
            true_peak: 0.0,
            clipped_samples: 0,
            hist_l: [0.0; 4],
            hist_r: [0.0; 4],
            hist_count: 0,
            dr_block_frames,
            dr_cur_frames: 0,
            dr_sum_sq_l: 0.0,
            dr_sum_sq_r: 0.0,
            dr_peak_l: 0.0,
            dr_peak_r: 0.0,
            dr_rms_blocks_l: Vec::new(),
            dr_rms_blocks_r: Vec::new(),
            dr_peak_blocks_l: Vec::new(),
            dr_peak_blocks_r: Vec::new(),
            k1_l: Biquad64::r128_high_shelf(sr),
            k2_l: Biquad64::r128_high_pass(sr),
            k1_r: Biquad64::r128_high_shelf(sr),
            k2_r: Biquad64::r128_high_pass(sr),
            hop_frames,
            hop_cur_frames: 0,
            hop_sum_sq: 0.0,
            hop_energies: Vec::new(),
            fft_window,
            fft_ring: Vec::with_capacity(FFT_SIZE),
            fft_accum: vec![0.0; FFT_SIZE / 2],
            fft_count: 0,
            fft_stride_frames,
            fft_skip_counter: 0,
        }
    }

    fn push_interleaved(&mut self, samples: &[f32], pkt_ch: usize) {
        let ch = pkt_ch.max(1);
        for frame in samples.chunks_exact(ch) {
            let l = frame[0] as f64;
            let r = if ch >= 2 { frame[1] as f64 } else { l };
            let al = l.abs();
            let ar = r.abs();

            if al > self.sample_peak {
                self.sample_peak = al;
            }
            if ar > self.sample_peak {
                self.sample_peak = ar;
            }
            if al >= 0.9999 {
                self.clipped_samples += 1;
            }
            if ch >= 2 && ar >= 0.9999 {
                self.clipped_samples += 1;
            }

            // 4x Catmull-Rom cubic true-peak estimation
            self.hist_l = [self.hist_l[1], self.hist_l[2], self.hist_l[3], l];
            self.hist_r = [self.hist_r[1], self.hist_r[2], self.hist_r[3], r];
            self.hist_count += 1;
            if self.hist_count >= 4 {
                let tp_l = cubic_max_peak(self.hist_l);
                let tp_r = cubic_max_peak(self.hist_r);
                self.true_peak = self.true_peak.max(tp_l).max(tp_r);
            } else {
                self.true_peak = self.true_peak.max(al).max(ar);
            }

            // Dynamic range 3-second block accumulation
            self.dr_sum_sq_l += l * l;
            self.dr_sum_sq_r += r * r;
            if al > self.dr_peak_l {
                self.dr_peak_l = al;
            }
            if ar > self.dr_peak_r {
                self.dr_peak_r = ar;
            }
            self.dr_cur_frames += 1;
            if self.dr_cur_frames >= self.dr_block_frames {
                self.flush_dr_block();
            }

            // EBU R128 K-weighted 100 ms hop accumulation
            let kl = self.k2_l.process(self.k1_l.process(l));
            let kr = self.k2_r.process(self.k1_r.process(r));
            self.hop_sum_sq += if ch >= 2 { kl * kl + kr * kr } else { kl * kl };
            self.hop_cur_frames += 1;
            if self.hop_cur_frames >= self.hop_frames {
                self.hop_energies
                    .push(self.hop_sum_sq / self.hop_cur_frames as f64);
                self.hop_sum_sq = 0.0;
                self.hop_cur_frames = 0;
            }

            // Periodic FFT window capture for spectral bandwidth & authenticity check
            if self.fft_skip_counter == 0 {
                self.fft_ring.push((l + r) * 0.5);
                if self.fft_ring.len() >= FFT_SIZE {
                    self.accumulate_fft();
                    self.fft_ring.clear();
                    self.fft_skip_counter = self.fft_stride_frames.saturating_sub(FFT_SIZE);
                }
            } else {
                self.fft_skip_counter -= 1;
            }
        }
    }

    fn flush_dr_block(&mut self) {
        if self.dr_cur_frames == 0 {
            return;
        }
        let n = self.dr_cur_frames as f64;
        // Standard TT-DR uses sqrt(2 * mean(x^2)) so a full-scale sine wave has RMS = 0 dB (DR = 3 dB -> sqrt(2)*0.707 = 1.0?
        // Actually in TT-DR: RMS = sqrt(2 * sum(x^2) / N) -> Wait: for sine sin(wt), mean(x^2) = 0.5,
        // so sqrt(2 * 0.5) = 1.0 (0 dB), wait: standard sine crest factor is +3.01 dB!
        // Standard RMS = sqrt(sum(x^2) / N) = 0.7071 (-3.01 dB), Peak = 1.0 (0 dB) => Peak/RMS = 3.01 dB (DR3)!
        let rms_l = (self.dr_sum_sq_l / n).sqrt();
        let rms_r = (self.dr_sum_sq_r / n).sqrt();
        self.dr_rms_blocks_l.push(rms_l);
        self.dr_rms_blocks_r.push(rms_r);
        self.dr_peak_blocks_l.push(self.dr_peak_l);
        self.dr_peak_blocks_r.push(self.dr_peak_r);
        self.dr_sum_sq_l = 0.0;
        self.dr_sum_sq_r = 0.0;
        self.dr_peak_l = 0.0;
        self.dr_peak_r = 0.0;
        self.dr_cur_frames = 0;
    }

    fn accumulate_fft(&mut self) {
        let mut re = vec![0.0_f64; FFT_SIZE];
        let mut im = vec![0.0_f64; FFT_SIZE];
        let mut energy = 0.0_f64;
        for (slot, (&s, &w)) in re
            .iter_mut()
            .zip(self.fft_ring.iter().zip(self.fft_window.iter()))
        {
            let v = s * w;
            *slot = v;
            energy += v * v;
        }
        // Skip silent passages so they don't dilute spectral bandwidth readings
        if (energy / FFT_SIZE as f64).sqrt() < 1e-4 {
            return;
        }
        fft_radix2(&mut re, &mut im);
        let scale = 2.0 / (FFT_SIZE as f64 * 0.5); // compensate Hann coherent gain 0.5
        for (acc, (&r, &i)) in self
            .fft_accum
            .iter_mut()
            .zip(re.iter().zip(im.iter()))
        {
            let mag = (r * r + i * i).sqrt() * scale;
            // Accumulate power so loud passages don't hide ultrasonic harmonics
            *acc += mag * mag;
        }
        self.fft_count += 1;
    }

    fn finish(mut self, bit_depth: Option<u32>) -> AudioAnalysis {
        if self.dr_cur_frames > self.dr_block_frames / 4 || self.dr_rms_blocks_l.is_empty() {
            self.flush_dr_block();
        }
        if self.hop_cur_frames > 0 && self.hop_energies.is_empty() {
            self.hop_energies
                .push(self.hop_sum_sq / self.hop_cur_frames as f64);
        }
        if self.fft_count == 0 && !self.fft_ring.is_empty() {
            self.fft_ring.resize(FFT_SIZE, 0.0);
            self.accumulate_fft();
        }

        // 1) Dynamic Range (DR) calculation
        let dr_l = compute_channel_dr(&mut self.dr_rms_blocks_l, &mut self.dr_peak_blocks_l);
        let dr_r = compute_channel_dr(&mut self.dr_rms_blocks_r, &mut self.dr_peak_blocks_r);
        let dr_precise = if self.channels >= 2 {
            ((dr_l + dr_r) * 0.5) as f32
        } else {
            dr_l as f32
        };
        let dr_score = dr_precise.round().clamp(0.0, 30.0) as u8;

        // 2) EBU R128 Integrated Loudness (400 ms sliding windows = 4 consecutive 100 ms hops)
        let integrated_lufs = compute_integrated_lufs(&self.hop_energies) as f32;

        // 3) Sample Peak & 4x True Peak
        let sample_peak_db = linear_to_db(self.sample_peak) as f32;
        let true_peak_dbtp = linear_to_db(self.true_peak.max(self.sample_peak)) as f32;

        // 4) Average spectrum (64 bins) + Hi-Res / Lossy cutoff detection
        let (spectrum_db, effective_bandwidth_hz, verdict, verdict_detail) =
            self.evaluate_spectrum(bit_depth);

        AudioAnalysis {
            sample_rate: self.sample_rate,
            dr_score,
            dr_precise,
            integrated_lufs,
            sample_peak_db,
            true_peak_dbtp,
            clipped_samples: self.clipped_samples,
            effective_bandwidth_hz,
            verdict,
            verdict_detail,
            spectrum_db,
        }
    }

    fn evaluate_spectrum(
        &self,
        bit_depth: Option<u32>,
    ) -> (Vec<f32>, f32, HiResVerdict, String) {
        let half = FFT_SIZE / 2;
        let nyquist = self.sample_rate as f64 * 0.5;
        let count = self.fft_count.max(1) as f64;

        let mut bin_db = vec![-96.0_f64; half];
        for (dst, &pow) in bin_db.iter_mut().zip(self.fft_accum.iter()) {
            let rms_mag = (pow / count).sqrt();
            *dst = linear_to_db(rms_mag).clamp(-96.0, 0.0);
        }

        // Downsample 1024 FFT bins into 64 clean display bins (peak-hold per bin group for clarity)
        let mut spectrum_db = Vec::with_capacity(SPECTRUM_BINS);
        for b in 0..SPECTRUM_BINS {
            let start = (b * half) / SPECTRUM_BINS;
            let end = (((b + 1) * half) / SPECTRUM_BINS).max(start + 1).min(half);
            let mut sum = 0.0;
            let mut max_v = -96.0_f64;
            for &v in &bin_db[start..end] {
                sum += v;
                if v > max_v {
                    max_v = v;
                }
            }
            let avg = sum / (end - start) as f64;
            spectrum_db.push((avg * 0.45 + max_v * 0.55) as f32);
        }

        // Reference musical band level (1 kHz .. 8 kHz)
        let ref_lo = ((1000.0 / nyquist) * half as f64) as usize;
        let ref_hi = (((8000.0 / nyquist) * half as f64) as usize)
            .max(ref_lo + 1)
            .min(half);
        let ref_db = bin_db[ref_lo..ref_hi].iter().sum::<f64>() / (ref_hi - ref_lo).max(1) as f64;
        // Effective bandwidth: highest frequency with energy above max(-84 dBFS, ref_db - 52 dB)
        let threshold = (ref_db - 52.0).max(-84.0);
        let mut highest_active_bin = ref_lo;
        for k in (ref_lo..half).rev() {
            // Require 3-bin local average above threshold to ignore isolated noise spikes
            let lo = k.saturating_sub(1);
            let hi = (k + 2).min(half);
            let local = bin_db[lo..hi].iter().sum::<f64>() / (hi - lo) as f64;
            if local > threshold {
                highest_active_bin = k;
                break;
            }
        }
        let bw_hz = ((highest_active_bin as f64 / half as f64) * nyquist) as f32;

        // Ultrasonic band (23.5 kHz .. min(44 kHz, nyquist * 0.92)) for Hi-Res check
        let is_hires_container = self.sample_rate > 48_000 || bit_depth.unwrap_or(16) > 16;
        let (verdict, detail) = if bw_hz < 17_200.0 && self.sample_rate >= 44_100 {
            (
                HiResVerdict::LossyTranscode,
                format!(
                    "Sharp spectral cutoff near {:.1} kHz (typical of lossy MP3/AAC encoding)",
                    bw_hz / 1000.0
                ),
            )
        } else if self.sample_rate > 48_000 {
            if bw_hz >= 23_500.0 {
                (
                    HiResVerdict::AuthenticHiRes,
                    format!(
                        "Verified ultrasonic musical content up to {:.1} kHz (Nyquist {:.1} kHz)",
                        bw_hz / 1000.0,
                        nyquist / 1000.0
                    ),
                )
            } else {
                (
                    HiResVerdict::SuspectedUpsample,
                    format!(
                        "Container is {:.1} kHz, but spectral energy rolls off at {:.1} kHz (CD-band master or upsample)",
                        self.sample_rate as f32 / 1000.0,
                        bw_hz / 1000.0
                    ),
                )
            }
        } else if is_hires_container {
            (
                HiResVerdict::AuthenticHiRes,
                format!(
                    "Full {}-bit bandwidth verified to {:.1} kHz",
                    bit_depth.unwrap_or(24),
                    bw_hz / 1000.0
                ),
            )
        } else {
            (
                HiResVerdict::LosslessCdQuality,
                format!(
                    "Clean full-range CD spectrum verified up to {:.1} kHz",
                    bw_hz / 1000.0
                ),
            )
        };

        (spectrum_db, bw_hz, verdict, detail)
    }
}

fn compute_channel_dr(rms_blocks: &mut [f64], peak_blocks: &mut [f64]) -> f64 {
    if rms_blocks.is_empty() || peak_blocks.is_empty() {
        return 0.0;
    }
    rms_blocks.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    peak_blocks.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    // Top 20% loudest 3-second blocks
    let top_n = ((rms_blocks.len() as f64 * 0.20).ceil() as usize)
        .max(1)
        .min(rms_blocks.len());
    let rms_top =
        (rms_blocks[..top_n].iter().map(|v| v * v).sum::<f64>() / top_n as f64).sqrt();
    // Second-highest block peak (reduces single-click outlier bias, per TT-DR spec)
    let peak_ref = if peak_blocks.len() >= 2 {
        peak_blocks[1]
    } else {
        peak_blocks[0]
    };
    if rms_top <= 1e-9 || peak_ref <= 1e-9 {
        return 0.0;
    }
    (20.0 * (peak_ref / rms_top).log10()).clamp(0.0, 30.0)
}

fn compute_integrated_lufs(hop_energies: &[f64]) -> f64 {
    if hop_energies.is_empty() {
        return -70.0;
    }
    // Construct 400 ms gating blocks (4 hops of 100 ms each)
    let mut blocks = Vec::new();
    if hop_energies.len() < 4 {
        let mean = hop_energies.iter().sum::<f64>() / hop_energies.len() as f64;
        blocks.push(mean);
    } else {
        for w in hop_energies.windows(4) {
            blocks.push((w[0] + w[1] + w[2] + w[3]) * 0.25);
        }
    }
    // 1) Absolute gate at -70 LUFS
    let abs_gate_lin = 10.0_f64.powf((-70.0 + 0.691) / 10.0);
    let gated_abs: Vec<f64> = blocks
        .into_iter()
        .filter(|&e| e > abs_gate_lin)
        .collect();
    if gated_abs.is_empty() {
        return -70.0;
    }
    let avg_abs = gated_abs.iter().sum::<f64>() / gated_abs.len() as f64;
    let rel_threshold_lufs = -0.691 + 10.0 * avg_abs.log10() - 10.0;
    let rel_gate_lin = 10.0_f64.powf((rel_threshold_lufs + 0.691) / 10.0);

    let mut sum = 0.0;
    let mut cnt = 0usize;
    for e in gated_abs {
        if e >= rel_gate_lin {
            sum += e;
            cnt += 1;
        }
    }
    if cnt == 0 {
        return -70.0;
    }
    (-0.691 + 10.0 * (sum / cnt as f64).log10()).clamp(-70.0, 6.0)
}

#[inline(always)]
fn cubic_max_peak(h: [f64; 4]) -> f64 {
    let [y0, y1, y2, y3] = h;
    let mut peak = y1.abs().max(y2.abs());
    let a0 = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let a1 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let a2 = -0.5 * y0 + 0.5 * y2;
    let a3 = y1;
    // Evaluate at t = 0.25, 0.50, 0.75 (4x oversampled inter-sample peaks)
    for t in [0.25_f64, 0.50, 0.75] {
        let v = ((a0 * t + a1) * t + a2) * t + a3;
        peak = peak.max(v.abs());
    }
    peak
}

#[inline(always)]
fn linear_to_db(v: f64) -> f64 {
    if v <= 1e-6 {
        -120.0
    } else {
        20.0 * v.log10()
    }
}

/// In-place iterative Cooley-Tukey radix-2 FFT (`n` must be a power of 2).
fn fft_radix2(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
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
        let ang = -2.0 * PI / len as f64;
        let wlen_r = ang.cos();
        let wlen_i = ang.sin();
        let half = len / 2;
        for start in (0..n).step_by(len) {
            let mut wr = 1.0;
            let mut wi = 0.0;
            for k in 0..half {
                let u_r = re[start + k];
                let u_i = im[start + k];
                let v_r = re[start + k + half] * wr - im[start + k + half] * wi;
                let v_i = re[start + k + half] * wi + im[start + k + half] * wr;
                re[start + k] = u_r + v_r;
                im[start + k] = u_i + v_i;
                re[start + k + half] = u_r - v_r;
                im[start + k + half] = u_i - v_i;
                let next_wr = wr * wlen_r - wi * wlen_i;
                wi = wr * wlen_i + wi * wlen_r;
                wr = next_wr;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_wave_yields_dr3_and_accurate_peak() {
        let sr = 48_000_u32;
        let mut acc = StreamAnalyzer::new(sr, 2);
        let frames = sr as usize * 6;
        let mut buf = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (2.0 * PI * 1000.0 * i as f64 / sr as f64).sin() as f32 * 0.5;
            buf.push(s);
            buf.push(s);
        }
        acc.push_interleaved(&buf, 2);
        let res = acc.finish(Some(24));
        assert_eq!(res.dr_score, 3, "Pure sine wave should be DR3, got {}", res.dr_precise);
        assert!(
            (res.sample_peak_db - (-6.02)).abs() < 0.15,
            "0.5 amplitude peak should be ~-6.02 dB, got {}",
            res.sample_peak_db
        );
        assert!(res.integrated_lufs > -12.0 && res.integrated_lufs < -6.0);
        assert_eq!(res.spectrum_db.len(), SPECTRUM_BINS);
    }

    #[test]
    fn detects_authentic_hires_vs_upsampled_cd() {
        let sr = 96_000_u32;
        let frames = sr as usize * 3;

        // 1) Genuine 96 kHz with harmonic content at 2 kHz + 32 kHz
        let mut hires_acc = StreamAnalyzer::new(sr, 2);
        let mut hires_buf = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f64 / sr as f64;
            let s = (0.4 * (2.0 * PI * 2000.0 * t).sin() + 0.08 * (2.0 * PI * 32000.0 * t).sin())
                as f32;
            hires_buf.push(s);
            hires_buf.push(s);
        }
        hires_acc.push_interleaved(&hires_buf, 2);
        let hires_res = hires_acc.finish(Some(24));
        assert_eq!(hires_res.verdict, HiResVerdict::AuthenticHiRes);
        assert!(hires_res.effective_bandwidth_hz > 30_000.0);

        // 2) Upsampled 96 kHz file with content strictly limited to 2 kHz + 19 kHz
        let mut up_acc = StreamAnalyzer::new(sr, 2);
        let mut up_buf = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f64 / sr as f64;
            let s = (0.4 * (2.0 * PI * 2000.0 * t).sin() + 0.1 * (2.0 * PI * 19000.0 * t).sin())
                as f32;
            up_buf.push(s);
            up_buf.push(s);
        }
        up_acc.push_interleaved(&up_buf, 2);
        let up_res = up_acc.finish(Some(24));
        assert_eq!(up_res.verdict, HiResVerdict::SuspectedUpsample);
        assert!(up_res.effective_bandwidth_hz < 22_000.0);
    }
}
