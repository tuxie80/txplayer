use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

use crate::dsd::{DsdPcmDecoder, RawCdBinDecoder, is_dsd_path, is_raw_cd_bin};
use crate::dsp::{RealtimeDsp, SharedDspParams};
use crate::exclusive::ExclusiveDeviceGuard;
use crate::plugins::{DspConfig, OversamplingMode, ResamplerQuality};

const BUFFER_SECONDS: usize = 3;
const STATS_WINDOW_SECONDS: f64 = 0.5;
const MAX_PACKET_ERROR_STREAK: usize = 5;
const POSITION_UPDATE_INTERVAL: Duration = Duration::from_millis(40);
pub const VISUAL_BUFFER_LEN: usize = 16_384;

/// Lock-free real-time stereo tap fed directly by the audio output callback.
pub struct VisualTap {
    left: Box<[AtomicU32; VISUAL_BUFFER_LEN]>,
    right: Box<[AtomicU32; VISUAL_BUFFER_LEN]>,
    write_pos: AtomicUsize,
    sample_rate: AtomicU32,
    peak_left: AtomicU32,
    peak_right: AtomicU32,
    rms_left: AtomicU32,
    rms_right: AtomicU32,
    correlation: AtomicU32,
    active: AtomicBool,
}

impl VisualTap {
    pub fn new() -> Self {
        let make_buf = || -> Box<[AtomicU32; VISUAL_BUFFER_LEN]> {
            let mut vec = Vec::with_capacity(VISUAL_BUFFER_LEN);
            for _ in 0..VISUAL_BUFFER_LEN {
                vec.push(AtomicU32::new(0));
            }
            vec.into_boxed_slice()
                .try_into()
                .unwrap_or_else(|_| unreachable!())
        };
        Self {
            left: make_buf(),
            right: make_buf(),
            write_pos: AtomicUsize::new(0),
            sample_rate: AtomicU32::new(44_100),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
            rms_left: AtomicU32::new(0),
            rms_right: AtomicU32::new(0),
            correlation: AtomicU32::new(1.0_f32.to_bits()),
            active: AtomicBool::new(false),
        }
    }

    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Relaxed);
        if !active {
            self.peak_left.store(0, Ordering::Relaxed);
            self.peak_right.store(0, Ordering::Relaxed);
            self.rms_left.store(0, Ordering::Relaxed);
            self.rms_right.store(0, Ordering::Relaxed);
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed).max(8_000)
    }

    pub fn set_sample_rate(&self, rate: u32) {
        self.sample_rate.store(rate, Ordering::Relaxed);
    }

    pub fn levels(&self) -> (f32, f32, f32, f32, f32) {
        if !self.is_active() {
            return (0.0, 0.0, 0.0, 0.0, 0.0);
        }
        (
            f32::from_bits(self.rms_left.load(Ordering::Relaxed)),
            f32::from_bits(self.rms_right.load(Ordering::Relaxed)),
            f32::from_bits(self.peak_left.load(Ordering::Relaxed)),
            f32::from_bits(self.peak_right.load(Ordering::Relaxed)),
            f32::from_bits(self.correlation.load(Ordering::Relaxed)),
        )
    }

    pub fn read_latest_mono(&self, out: &mut [f32]) {
        if !self.is_active() {
            out.fill(0.0);
            return;
        }
        let end = self.write_pos.load(Ordering::Acquire);
        let len = out.len().min(VISUAL_BUFFER_LEN);
        let start = end.wrapping_sub(len);
        for (i, slot) in out.iter_mut().take(len).enumerate() {
            let idx = start.wrapping_add(i) & (VISUAL_BUFFER_LEN - 1);
            let l = f32::from_bits(self.left[idx].load(Ordering::Relaxed));
            let r = f32::from_bits(self.right[idx].load(Ordering::Relaxed));
            *slot = (l + r) * 0.5;
        }
        if len < out.len() {
            out[len..].fill(0.0);
        }
    }

    pub fn read_latest_stereo(&self, out_l: &mut [f32], out_r: &mut [f32]) {
        let len = out_l.len().min(out_r.len()).min(VISUAL_BUFFER_LEN);
        if !self.is_active() {
            out_l.fill(0.0);
            out_r.fill(0.0);
            return;
        }
        let end = self.write_pos.load(Ordering::Acquire);
        let start = end.wrapping_sub(len);
        for i in 0..len {
            let idx = start.wrapping_add(i) & (VISUAL_BUFFER_LEN - 1);
            out_l[i] = f32::from_bits(self.left[idx].load(Ordering::Relaxed));
            out_r[i] = f32::from_bits(self.right[idx].load(Ordering::Relaxed));
        }
    }

    /// Reads only the newly arrived stereo samples since `cursor`, updating `cursor` to the latest
    /// write position. Used for continuous sample-accurate real-time EBU R128 LUFS & True Peak metering.
    pub fn read_new_stereo(
        &self,
        cursor: &mut usize,
        out_l: &mut Vec<f32>,
        out_r: &mut Vec<f32>,
    ) -> usize {
        out_l.clear();
        out_r.clear();
        if !self.is_active() {
            return 0;
        }
        let end = self.write_pos.load(Ordering::Acquire);
        let avail = end.wrapping_sub(*cursor);
        if avail == 0 {
            return 0;
        }
        let count = avail.min(VISUAL_BUFFER_LEN);
        let start = end.wrapping_sub(count);
        out_l.reserve(count);
        out_r.reserve(count);
        for i in 0..count {
            let idx = start.wrapping_add(i) & (VISUAL_BUFFER_LEN - 1);
            out_l.push(f32::from_bits(self.left[idx].load(Ordering::Relaxed)));
            out_r.push(f32::from_bits(self.right[idx].load(Ordering::Relaxed)));
        }
        *cursor = end;
        count
    }

    fn push_callback_frame(&self, write_cursor: &mut usize, l: f32, r: f32) {
        let idx = *write_cursor & (VISUAL_BUFFER_LEN - 1);
        self.left[idx].store(l.to_bits(), Ordering::Relaxed);
        self.right[idx].store(r.to_bits(), Ordering::Relaxed);
        *write_cursor = write_cursor.wrapping_add(1);
    }

    fn commit(
        &self,
        write_cursor: usize,
        rms_l: f32,
        rms_r: f32,
        peak_l: f32,
        peak_r: f32,
        corr: f32,
    ) {
        self.write_pos.store(write_cursor, Ordering::Release);
        self.rms_left.store(rms_l.to_bits(), Ordering::Relaxed);
        self.rms_right.store(rms_r.to_bits(), Ordering::Relaxed);
        self.peak_left.store(peak_l.to_bits(), Ordering::Relaxed);
        self.peak_right.store(peak_r.to_bits(), Ordering::Relaxed);
        self.correlation.store(corr.to_bits(), Ordering::Relaxed);
    }
}

impl Default for VisualTap {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct PlaybackSpec {
    pub path: PathBuf,
    /// Virtual track start offset in the underlying file (0.0 for regular files, INDEX 01 for CUE).
    pub cue_start: f64,
    /// Optional virtual track end offset in the underlying file (Some for CUE tracks).
    pub cue_end: Option<f64>,
    /// Track-relative initial seek position (e.g. session resume).
    pub start_offset: f64,
    pub start_paused: bool,
    /// Effective linear ReplayGain multiplier (1.0 when off/neutral).
    pub rg_gain_linear: f32,
    /// Display ReplayGain offset in dB (None when off/absent).
    pub rg_gain_db: Option<f32>,
}

impl PlaybackSpec {
    pub fn file_start_secs(&self) -> f64 {
        (self.cue_start + self.start_offset).max(0.0)
    }

    pub fn effective_duration(&self, file_duration: Option<f64>) -> Option<f64> {
        let end = self.cue_end.or(file_duration)?;
        Some((end - self.cue_start).max(0.0))
    }
}

#[derive(Debug)]
pub enum AudioCommand {
    Play(PlaybackSpec),
    PreloadNext(Option<PlaybackSpec>),
    Pause,
    Resume,
    Stop,
    Seek(f64),
    SelectDevice(Option<String>),
    SetSpeed(f32),
    SetAbLoop(Option<(f64, f64)>),
    SetGapless(bool),
    SetCrossfade(f32),
    SetReplayGainLinear(f32),
    SetRamPlayback {
        enabled: bool,
        max_mb: u32,
    },
    SetOversampling {
        mode: OversamplingMode,
        quality: ResamplerQuality,
    },
    SetExclusiveMode(bool),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum AudioEvent {
    Started {
        source_sample_rate: u32,
        output_sample_rate: u32,
        output_format: String,
        channels: u16,
        bit_depth: Option<u32>,
        duration: Option<f64>,
        device_name: String,
        bit_perfect: bool,
        exclusive_active: bool,
        hw_rate_switched: bool,
        rg_gain_db: Option<f32>,
        ram_bytes: Option<usize>,
        oversampling: OversamplingMode,
        resampler_quality: ResamplerQuality,
    },
    GaplessTransitioned {
        source_sample_rate: u32,
        output_sample_rate: u32,
        output_format: String,
        channels: u16,
        bit_depth: Option<u32>,
        duration: Option<f64>,
        device_name: String,
        bit_perfect: bool,
        exclusive_active: bool,
        hw_rate_switched: bool,
        rg_gain_db: Option<f32>,
        ram_bytes: Option<usize>,
        oversampling: OversamplingMode,
        resampler_quality: ResamplerQuality,
    },
    DeviceRouted(String),
    Playing(bool),
    Stopped,
    Position(f64),
    Bitrate {
        kbps: u32,
        variable: bool,
    },
    Ended,
    Error(String),
}

pub struct AudioHandle {
    commands: Sender<AudioCommand>,
    events: Receiver<AudioEvent>,
    volume: Arc<AtomicU32>,
    rg_gain: Arc<AtomicU32>,
    linear_volume: Arc<AtomicBool>,
    bit_perfect: Arc<AtomicBool>,
    direct_volume_lock: Arc<AtomicBool>,
    visual_tap: Arc<VisualTap>,
    dsp: Arc<SharedDspParams>,
    _join: JoinHandle<()>,
}

impl AudioHandle {
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let volume = Arc::new(AtomicU32::new(0.75_f32.to_bits()));
        let rg_gain = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let linear_volume = Arc::new(AtomicBool::new(false));
        let bit_perfect = Arc::new(AtomicBool::new(false));
        let direct_volume_lock = Arc::new(AtomicBool::new(false));
        let visual_tap = Arc::new(VisualTap::new());
        let dsp = Arc::new(SharedDspParams::new(&DspConfig::default()));
        let thread_volume = Arc::clone(&volume);
        let thread_rg = Arc::clone(&rg_gain);
        let thread_linear = Arc::clone(&linear_volume);
        let thread_bp = Arc::clone(&bit_perfect);
        let thread_vol_lock = Arc::clone(&direct_volume_lock);
        let thread_tap = Arc::clone(&visual_tap);
        let thread_dsp = Arc::clone(&dsp);
        let join = thread::Builder::new()
            .name("txplayer-audio".into())
            .spawn(move || {
                if let Err(error) = run_audio_thread(
                    command_rx,
                    event_tx.clone(),
                    thread_volume,
                    thread_rg,
                    thread_linear,
                    thread_bp,
                    thread_vol_lock,
                    thread_tap,
                    thread_dsp,
                ) {
                    let _ = event_tx.send(AudioEvent::Error(error.to_string()));
                }
            })
            .expect("audio thread should start");
        Self {
            commands: command_tx,
            events: event_rx,
            volume,
            rg_gain,
            linear_volume,
            bit_perfect,
            direct_volume_lock,
            visual_tap,
            dsp,
            _join: join,
        }
    }

    pub fn play_spec(&self, spec: PlaybackSpec) {
        let _ = self.commands.send(AudioCommand::Play(spec));
    }

    pub fn preload_next(&self, spec: Option<PlaybackSpec>) {
        let _ = self.commands.send(AudioCommand::PreloadNext(spec));
    }

    pub fn pause(&self) {
        let _ = self.commands.send(AudioCommand::Pause);
    }

    pub fn resume(&self) {
        let _ = self.commands.send(AudioCommand::Resume);
    }

    pub fn stop(&self) {
        let _ = self.commands.send(AudioCommand::Stop);
    }

    pub fn seek(&self, seconds: f64) {
        let _ = self.commands.send(AudioCommand::Seek(seconds.max(0.0)));
    }

    pub fn select_device(&self, name: Option<String>) {
        let _ = self.commands.send(AudioCommand::SelectDevice(name));
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_linear_volume(&self, linear: bool) {
        self.linear_volume.store(linear, Ordering::Relaxed);
    }

    pub fn set_speed(&self, speed: f32) {
        let _ = self
            .commands
            .send(AudioCommand::SetSpeed(speed.clamp(0.5, 2.0)));
    }

    pub fn set_ab_loop(&self, loop_range: Option<(f64, f64)>) {
        let _ = self.commands.send(AudioCommand::SetAbLoop(loop_range));
    }

    pub fn set_replay_gain_linear(&self, gain: f32) {
        let clamped = gain.clamp(0.1, 4.0);
        self.rg_gain.store(clamped.to_bits(), Ordering::Relaxed);
        let _ = self
            .commands
            .send(AudioCommand::SetReplayGainLinear(clamped));
    }

    pub fn update_dsp(&self, cfg: &DspConfig) {
        self.bit_perfect
            .store(cfg.bit_perfect_mode, Ordering::Relaxed);
        self.direct_volume_lock
            .store(cfg.direct_volume_lock, Ordering::Relaxed);
        self.dsp.update(cfg);
        let _ = self
            .commands
            .send(AudioCommand::SetGapless(cfg.gapless_enabled));
        let _ = self
            .commands
            .send(AudioCommand::SetCrossfade(cfg.crossfade_secs.clamp(0.0, 6.0)));
        let _ = self.commands.send(AudioCommand::SetRamPlayback {
            enabled: cfg.ram_playback,
            max_mb: cfg.ram_buffer_mb.clamp(64, 4096),
        });
        let _ = self.commands.send(AudioCommand::SetOversampling {
            mode: cfg.oversampling,
            quality: cfg.resampler_quality,
        });
        let _ = self
            .commands
            .send(AudioCommand::SetExclusiveMode(cfg.exclusive_mode));
    }

    pub fn visual_tap(&self) -> Arc<VisualTap> {
        Arc::clone(&self.visual_tap)
    }

    pub fn try_event(&self) -> Option<AudioEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(AudioCommand::Shutdown);
    }
}

pub fn list_output_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };
    let mut names: Vec<String> = devices.map(|d| d.to_string()).collect();
    names.sort();
    names.dedup();
    names
}

pub fn default_output_device_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .map(|d| d.to_string())
}

/// Fast background acoustic envelope extraction (normalised 0.0..1.0 bins) for waveform seekbar.
pub fn compute_waveform(path: &Path, bins: usize) -> Option<Vec<f32>> {
    let bins = bins.max(32);
    if is_dsd_path(path) || is_raw_cd_bin(path) {
        let mut dec = DecoderState::open(path).ok()?;
        let mut raw_peaks: Vec<f32> = Vec::with_capacity(bins * 4);
        let mut decode_buf = None;
        let mut scratch = Vec::new();
        let mut errs = 0usize;
        let mut chunk_i = 0usize;
        while let Ok(chunk) = dec.decode_chunk(&mut decode_buf, &mut scratch, &mut errs) {
            match chunk {
                DecodeChunk::Frames { channels, .. } => {
                    chunk_i += 1;
                    if chunk_i.is_multiple_of(2) && dec.duration.unwrap_or(0.0) > 240.0 {
                        continue;
                    }
                    let ch = channels.max(1);
                    if scratch.is_empty() {
                        continue;
                    }
                    let mut sum_sq = 0.0_f32;
                    let mut peak = 0.0_f32;
                    for &s in scratch.iter().step_by(ch) {
                        let a = s.abs();
                        sum_sq += a * a;
                        peak = peak.max(a);
                    }
                    let n = (scratch.len() / ch).max(1) as f32;
                    raw_peaks.push(((sum_sq / n).sqrt() * 0.7 + peak * 0.3).min(1.0));
                }
                DecodeChunk::Reset => continue,
                DecodeChunk::Eof => break,
            }
        }
        return finish_waveform_bins(&raw_peaks, bins);
    }

    let file = std::fs::File::open(path).ok()?;
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
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let total_frames = params.n_frames;
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .ok()?;

    let mut raw_peaks: Vec<f32> = Vec::with_capacity(bins * 4);
    let mut sample_buf: Option<(usize, SampleBuffer<f32>)> = None;
    let mut packet_idx = 0usize;
    let decimate_stride = match total_frames {
        Some(f) if f > 44_100 * 600 => 4,
        Some(f) if f > 44_100 * 240 => 2,
        _ => 1,
    };

    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        packet_idx += 1;
        if decimate_stride > 1 && !packet_idx.is_multiple_of(decimate_stride) {
            continue;
        }
        let Ok(audio) = decoder.decode(&packet) else {
            continue;
        };
        let spec = *audio.spec();
        let cap = audio.capacity();
        let samples = match &mut sample_buf {
            Some((cur_cap, sb)) if *cur_cap >= cap => sb,
            slot => {
                *slot = Some((cap, SampleBuffer::<f32>::new(cap as u64, spec)));
                &mut slot.as_mut().unwrap().1
            }
        };
        samples.copy_interleaved_ref(audio);
        let data = samples.samples();
        if data.is_empty() {
            continue;
        }
        // Compute RMS + peak blend for musical dynamics (classical crescendos read beautifully)
        let mut sum_sq = 0.0_f32;
        let mut peak = 0.0_f32;
        for &s in data.iter().step_by(2) {
            let a = s.abs();
            sum_sq += a * a;
            if a > peak {
                peak = a;
            }
        }
        let n = (data.len() / 2).max(1) as f32;
        let rms = (sum_sq / n).sqrt();
        raw_peaks.push((rms * 0.7 + peak * 0.3).min(1.0));
    }

    finish_waveform_bins(&raw_peaks, bins)
}

fn finish_waveform_bins(raw_peaks: &[f32], bins: usize) -> Option<Vec<f32>> {
    if raw_peaks.is_empty() {
        return None;
    }

    let mut out = vec![0.0_f32; bins];
    let step = raw_peaks.len() as f32 / bins as f32;
    for (i, slot) in out.iter_mut().enumerate() {
        let start = (i as f32 * step).floor() as usize;
        let end = (((i + 1) as f32 * step).ceil() as usize)
            .max(start + 1)
            .min(raw_peaks.len());
        let mut acc = 0.0_f32;
        let mut max_v = 0.0_f32;
        for &v in &raw_peaks[start..end] {
            acc += v;
            max_v = max_v.max(v);
        }
        let avg = acc / (end - start).max(1) as f32;
        *slot = avg * 0.55 + max_v * 0.45;
    }

    let global_max = out.iter().copied().fold(0.0_f32, f32::max).max(0.02);
    for v in &mut out {
        // Gentle perceptual compression so pianissimo passages remain visible alongside fortissimo
        *v = (*v / global_max).powf(0.72).clamp(0.06, 1.0);
    }
    Some(out)
}

fn resolve_output_device(
    preferred: Option<&str>,
) -> Result<(Device, String, Option<cpal::DeviceId>)> {
    let host = cpal::default_host();
    if let Some(wanted) = preferred
        && let Ok(devices) = host.output_devices()
    {
        for device in devices {
            let name = device.to_string();
            if name == wanted {
                let id = device.id().ok();
                return Ok((device, name, id));
            }
        }
    }
    let default_dev = host
        .default_output_device()
        .context("No audio output device is available")?;
    let default_name = default_dev.to_string();
    let default_id = default_dev.id().ok();

    // Resolve the concrete output device from `output_devices()` so CoreAudio pins
    // an explicit HalOutput unit (`kAudioOutputUnitProperty_CurrentDevice`) to the exact
    // active hardware (e.g. Bluetooth A2DP speakers like Klipsch) rather than a stale
    // DefaultOutput unit. Live default-device changes are tracked explicitly in `play_file`.
    if let Some(ref wanted_id) = default_id
        && let Ok(devices) = host.output_devices()
    {
        for device in devices {
            if device.id().ok().as_ref() == Some(wanted_id) {
                return Ok((device, default_name, default_id));
            }
        }
    }
    Ok((default_dev, default_name, default_id))
}

enum PlaybackExit {
    Ended,
    Stopped,
    Command(AudioCommand),
    ReopenSpec(PlaybackSpec),
}

struct AudioEnginePrefs {
    preferred_device: Option<String>,
    gapless_enabled: bool,
    crossfade_secs: f32,
    speed: f32,
    ab_loop: Option<(f64, f64)>,
    next_spec: Option<PlaybackSpec>,
    ram_playback: bool,
    ram_max_bytes: usize,
    oversampling: OversamplingMode,
    resampler_quality: ResamplerQuality,
    exclusive_mode: bool,
}

#[allow(clippy::too_many_arguments)]
fn run_audio_thread(
    commands: Receiver<AudioCommand>,
    events: Sender<AudioEvent>,
    volume: Arc<AtomicU32>,
    rg_gain: Arc<AtomicU32>,
    linear_volume: Arc<AtomicBool>,
    bit_perfect: Arc<AtomicBool>,
    direct_volume_lock: Arc<AtomicBool>,
    visual_tap: Arc<VisualTap>,
    dsp: Arc<SharedDspParams>,
) -> Result<()> {
    let mut prefs = AudioEnginePrefs {
        preferred_device: None,
        gapless_enabled: true,
        crossfade_secs: 0.0,
        speed: 1.0,
        ab_loop: None,
        next_spec: None,
        ram_playback: true,
        ram_max_bytes: 512 * 1024 * 1024,
        oversampling: OversamplingMode::Native,
        resampler_quality: ResamplerQuality::LinearPhaseStudio,
        exclusive_mode: false,
    };
    let mut next = commands.recv();
    while let Ok(command) = next {
        match command {
            AudioCommand::Play(spec) => {
                let device_result = resolve_output_device(prefs.preferred_device.as_deref());
                let (device, device_name, device_id) = match device_result {
                    Ok(triple) => triple,
                    Err(error) => {
                        visual_tap.set_active(false);
                        let _ = events.send(AudioEvent::Error(format!("{error:#}")));
                        next = commands.recv();
                        continue;
                    }
                };
                next = match play_file(
                    &device,
                    device_name,
                    device_id,
                    spec,
                    &commands,
                    &events,
                    &volume,
                    &rg_gain,
                    &linear_volume,
                    &bit_perfect,
                    &direct_volume_lock,
                    &visual_tap,
                    &dsp,
                    &mut prefs,
                ) {
                    Ok(PlaybackExit::Command(command)) => Ok(command),
                    Ok(PlaybackExit::ReopenSpec(new_spec)) => Ok(AudioCommand::Play(new_spec)),
                    Ok(PlaybackExit::Ended | PlaybackExit::Stopped) => {
                        visual_tap.set_active(false);
                        commands.recv()
                    }
                    Err(error) => {
                        visual_tap.set_active(false);
                        let _ = events
                            .send(AudioEvent::Error(format!("Could not play file: {error:#}")));
                        commands.recv()
                    }
                };
            }
            AudioCommand::PreloadNext(spec) => {
                prefs.next_spec = spec;
                next = commands.recv();
            }
            AudioCommand::SelectDevice(name) => {
                prefs.preferred_device = name;
                next = commands.recv();
            }
            AudioCommand::SetSpeed(s) => {
                prefs.speed = s.clamp(0.5, 2.0);
                next = commands.recv();
            }
            AudioCommand::SetAbLoop(l) => {
                prefs.ab_loop = l;
                next = commands.recv();
            }
            AudioCommand::SetGapless(g) => {
                prefs.gapless_enabled = g;
                next = commands.recv();
            }
            AudioCommand::SetCrossfade(c) => {
                prefs.crossfade_secs = c.clamp(0.0, 6.0);
                next = commands.recv();
            }
            AudioCommand::SetReplayGainLinear(g) => {
                rg_gain.store(g.clamp(0.1, 4.0).to_bits(), Ordering::Relaxed);
                next = commands.recv();
            }
            AudioCommand::SetRamPlayback { enabled, max_mb } => {
                prefs.ram_playback = enabled;
                prefs.ram_max_bytes = (max_mb.clamp(64, 4096) as usize) * 1024 * 1024;
                next = commands.recv();
            }
            AudioCommand::SetOversampling { mode, quality } => {
                prefs.oversampling = mode;
                prefs.resampler_quality = quality;
                next = commands.recv();
            }
            AudioCommand::SetExclusiveMode(excl) => {
                prefs.exclusive_mode = excl;
                next = commands.recv();
            }
            AudioCommand::Stop => {
                visual_tap.set_active(false);
                let _ = events.send(AudioEvent::Stopped);
                next = commands.recv();
            }
            AudioCommand::Shutdown => {
                visual_tap.set_active(false);
                return Ok(());
            }
            _ => next = commands.recv(),
        }
    }
    visual_tap.set_active(false);
    Ok(())
}

enum DecoderBackend {
    Symphonia {
        format: Box<dyn FormatReader>,
        decoder: Box<dyn Decoder>,
        track_id: u32,
    },
    Dsd(DsdPcmDecoder),
    RawCdBin(RawCdBinDecoder),
}

enum DecodeChunk {
    Frames {
        channels: usize,
        packet_bytes: usize,
        packet_dur: u64,
    },
    Reset,
    Eof,
}

struct DecoderState {
    backend: DecoderBackend,
    source_sample_rate: u32,
    source_channels: usize,
    bit_depth: Option<u32>,
    duration: Option<f64>,
    time_base: Option<TimeBase>,
    is_variable_bitrate: bool,
    nominal_kbps: Option<u32>,
}

impl DecoderState {
    fn open(path: &Path) -> Result<Self> {
        if is_dsd_path(path) {
            let dec = DsdPcmDecoder::open(path)?;
            let sr = dec.info.pcm_sample_rate;
            let ch = dec.info.channels;
            let dur = dec.info.duration_secs;
            let kbps = ((dec.info.sample_rate_hz as u64 * ch as u64) / 1000) as u32;
            return Ok(Self {
                backend: DecoderBackend::Dsd(dec),
                source_sample_rate: sr,
                source_channels: ch,
                bit_depth: Some(24),
                duration: Some(dur),
                time_base: Some(TimeBase::new(1, sr)),
                is_variable_bitrate: false,
                nominal_kbps: Some(kbps),
            });
        }
        if is_raw_cd_bin(path) {
            let dec = RawCdBinDecoder::open(path, false)?;
            let dur = dec.duration_secs();
            return Ok(Self {
                backend: DecoderBackend::RawCdBin(dec),
                source_sample_rate: 44_100,
                source_channels: 2,
                bit_depth: Some(16),
                duration: Some(dur),
                time_base: Some(TimeBase::new(1, 44_100)),
                is_variable_bitrate: false,
                nominal_kbps: Some(1411),
            });
        }

        let file =
            std::fs::File::open(path).with_context(|| format!("Opening {}", path.display()))?;
        let source = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
            hint.with_extension(extension);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                source,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .context("Recognizing audio format")?;
        let format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| anyhow!("The file contains no default audio track"))?;
        let params = track.codec_params.clone();
        if params.codec == CODEC_TYPE_NULL {
            return Err(anyhow!("The audio codec is not supported"));
        }
        let track_id = track.id;
        let source_sample_rate = params.sample_rate.unwrap_or(44_100);
        let source_channels = params
            .channels
            .map(|channels| channels.count())
            .unwrap_or(2);
        let bit_depth = params.bits_per_sample.or(params.bits_per_coded_sample);
        let time_base = params
            .time_base
            .or(Some(TimeBase::new(1, source_sample_rate)));
        let duration = params.time_base.and_then(|base| {
            params.n_frames.map(|frames| {
                let time = base.calc_time(frames);
                time.seconds as f64 + time.frac
            })
        });
        let is_vbr = is_variable_lossless(params.codec);
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .context("Creating audio decoder")?;
        Ok(Self {
            backend: DecoderBackend::Symphonia {
                format,
                decoder,
                track_id,
            },
            source_sample_rate,
            source_channels,
            bit_depth,
            duration,
            time_base,
            is_variable_bitrate: is_vbr,
            nominal_kbps: None,
        })
    }

    fn seek(&mut self, seconds: f64) -> Result<()> {
        let clamped = seconds.max(0.0);
        match &mut self.backend {
            DecoderBackend::Symphonia {
                format,
                decoder,
                track_id,
            } => {
                let time = Time::new(clamped.trunc() as u64, clamped.fract());
                if format
                    .seek(
                        SeekMode::Accurate,
                        SeekTo::Time {
                            time,
                            track_id: Some(*track_id),
                        },
                    )
                    .is_err()
                {
                    format.seek(
                        SeekMode::Coarse,
                        SeekTo::Time {
                            time,
                            track_id: Some(*track_id),
                        },
                    )?;
                }
                decoder.reset();
                Ok(())
            }
            DecoderBackend::Dsd(dec) => dec.seek(clamped),
            DecoderBackend::RawCdBin(dec) => dec.seek(clamped),
        }
    }

    /// Decodes the next chunk of interleaved `f32` PCM into `pcm_out`.
    fn decode_chunk(
        &mut self,
        decode_buf: &mut Option<(usize, SampleBuffer<f32>)>,
        pcm_out: &mut Vec<f32>,
        packet_errors: &mut usize,
    ) -> Result<DecodeChunk> {
        pcm_out.clear();
        match &mut self.backend {
            DecoderBackend::Dsd(dec) => {
                let frames = dec.read_pcm_frames(2048, pcm_out)?;
                if frames == 0 {
                    Ok(DecodeChunk::Eof)
                } else {
                    let bytes = (frames * dec.info.decimation_factor / 8) * dec.info.channels;
                    Ok(DecodeChunk::Frames {
                        channels: dec.info.channels,
                        packet_bytes: bytes,
                        packet_dur: frames as u64,
                    })
                }
            }
            DecoderBackend::RawCdBin(dec) => {
                let frames = dec.read_pcm_frames(2048, pcm_out)?;
                if frames == 0 {
                    Ok(DecodeChunk::Eof)
                } else {
                    Ok(DecodeChunk::Frames {
                        channels: 2,
                        packet_bytes: frames * 4,
                        packet_dur: frames as u64,
                    })
                }
            }
            DecoderBackend::Symphonia {
                format,
                decoder,
                track_id,
            } => loop {
                let packet = match format.next_packet() {
                    Ok(packet) => packet,
                    Err(SymphoniaError::IoError(error))
                        if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        return Ok(DecodeChunk::Eof);
                    }
                    Err(SymphoniaError::ResetRequired) => {
                        decoder.reset();
                        return Ok(DecodeChunk::Reset);
                    }
                    Err(error) => return Err(error.into()),
                };
                if packet.track_id() != *track_id {
                    continue;
                }
                let packet_bytes = packet.buf().len();
                let packet_dur = packet.dur();
                match decoder.decode(&packet) {
                    Ok(audio) => {
                        *packet_errors = 0;
                        let spec_audio = *audio.spec();
                        let channels = spec_audio.channels.count().max(1);
                        let cap = audio.capacity();
                        let buf = match decode_buf {
                            Some((cur_cap, sb)) if *cur_cap >= cap => sb,
                            slot => {
                                *slot =
                                    Some((cap, SampleBuffer::<f32>::new(cap as u64, spec_audio)));
                                &mut slot.as_mut().unwrap().1
                            }
                        };
                        buf.copy_interleaved_ref(audio);
                        pcm_out.extend_from_slice(buf.samples());
                        return Ok(DecodeChunk::Frames {
                            channels,
                            packet_bytes,
                            packet_dur,
                        });
                    }
                    Err(SymphoniaError::DecodeError(_)) => {
                        *packet_errors += 1;
                        if *packet_errors >= MAX_PACKET_ERROR_STREAK {
                            return Err(anyhow!("Repeated decoder errors; playback stopped"));
                        }
                    }
                    Err(SymphoniaError::IoError(error))
                        if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        return Ok(DecodeChunk::Eof);
                    }
                    Err(error) => return Err(error.into()),
                }
            },
        }
    }
}

struct OutputStream {
    stream: Stream,
    sample_rate: u32,
    channels: usize,
    format_label: String,
    exclusive_active: bool,
    hw_rate_switched: bool,
    _exclusive_guard: ExclusiveDeviceGuard,
    played_frames: Arc<AtomicU64>,
    seek_generation: Arc<AtomicU64>,
    reopen_needed: Arc<AtomicBool>,
}

/// Full-track pre-decoded PCM buffer resident in RAM for zero-disk-I/O playback and instant seeking.
struct RamTrackBuffer {
    samples: Vec<f32>,
    channels: usize,
    total_frames: usize,
    frame_cursor: usize,
    /// Per-0.5s bitrate samples so live VBR/CBR kbps telemetry keeps updating during RAM playback.
    bitrate_timeline: Vec<u32>,
    variable_bitrate: bool,
    last_emitted_bucket: Option<usize>,
}

impl RamTrackBuffer {
    fn byte_len(&self) -> usize {
        self.samples.len() * std::mem::size_of::<f32>()
    }
}

enum RamLoadOutcome {
    Loaded(RamTrackBuffer),
    SkippedTooLarge,
    Interrupted(AudioCommand),
}

/// Pre-decodes a full track (including virtual CUE tracks!) into RAM when it fits under `max_bytes`.
/// Checks `commands` every 64 chunks so skipping or stopping during preload is instantaneous.
fn try_load_track_to_ram(
    decoder: &mut DecoderState,
    max_bytes: usize,
    commands: &Receiver<AudioCommand>,
    spec: &PlaybackSpec,
) -> RamLoadOutcome {
    let ch = decoder.source_channels.max(1);
    let sr = decoder.source_sample_rate.max(1);
    let max_samples = max_bytes / std::mem::size_of::<f32>();
    let track_dur = spec.effective_duration(decoder.duration);
    if let Some(dur) = track_dur {
        let est_bytes = (dur * sr as f64 * ch as f64 * 4.0) as usize;
        if est_bytes > max_bytes || dur > 3600.0 {
            return RamLoadOutcome::SkippedTooLarge;
        }
    }
    if spec.cue_start > 0.05 && decoder.seek(spec.cue_start).is_err() {
        return RamLoadOutcome::SkippedTooLarge;
    }
    let max_track_frames: Option<usize> = spec
        .cue_end
        .map(|end| ((end - spec.cue_start).max(0.0) * sr as f64).round() as usize);

    let prealloc = track_dur
        .map(|d| ((d * sr as f64 * ch as f64) as usize).min(max_samples))
        .unwrap_or(sr as usize * ch * 60);
    let mut samples: Vec<f32> = Vec::with_capacity(prealloc.min(16 * 1024 * 1024));
    let mut sample_buf: Option<(usize, SampleBuffer<f32>)> = None;
    let mut pcm_scratch: Vec<f32> = Vec::with_capacity(8192);
    let mut packet_errors = 0usize;
    let mut pkt_count: usize = 0;
    let mut timeline: Vec<u32> = Vec::new();
    let mut acc_bytes: usize = 0;
    let mut acc_frames: u64 = 0;
    let mut min_kbps: Option<u32> = None;
    let mut max_kbps: Option<u32> = None;
    let mut variable = decoder.is_variable_bitrate;

    loop {
        pkt_count += 1;
        if pkt_count.is_multiple_of(64)
            && let Ok(cmd) = commands.try_recv()
        {
            match cmd {
                AudioCommand::Play(_) | AudioCommand::Stop | AudioCommand::Shutdown => {
                    return RamLoadOutcome::Interrupted(cmd);
                }
                _ => {}
            }
        }
        if let Some(limit_f) = max_track_frames
            && samples.len() / ch >= limit_f
        {
            break;
        }
        match decoder.decode_chunk(&mut sample_buf, &mut pcm_scratch, &mut packet_errors) {
            Ok(DecodeChunk::Frames {
                channels: pkt_ch,
                packet_bytes,
                packet_dur,
            }) => {
                acc_bytes += packet_bytes;
                acc_frames += packet_dur;
                if let Some(tb) = decoder.time_base
                    && acc_frames > 0
                {
                    let t = tb.calc_time(acc_frames);
                    let secs = t.seconds as f64 + t.frac;
                    if secs >= STATS_WINDOW_SECONDS {
                        let kbps = decoder.nominal_kbps.unwrap_or_else(|| {
                            ((acc_bytes as f64 * 8.0 / secs) / 1000.0)
                                .round()
                                .max(1.0) as u32
                        });
                        timeline.push(kbps);
                        min_kbps = Some(min_kbps.map_or(kbps, |m| m.min(kbps)));
                        max_kbps = Some(max_kbps.map_or(kbps, |m| m.max(kbps)));
                        if let (Some(lo), Some(hi)) = (min_kbps, max_kbps) {
                            variable |= hi > lo.saturating_add((lo / 20).max(8));
                        }
                        acc_bytes = 0;
                        acc_frames = 0;
                    }
                }
                let raw = pcm_scratch.as_slice();
                let mut take_frames = raw.len() / pkt_ch.max(1);
                if let Some(limit_f) = max_track_frames {
                    let cur_f = samples.len() / ch;
                    take_frames = take_frames.min(limit_f.saturating_sub(cur_f));
                }
                let raw_slice = &raw[..take_frames * pkt_ch.max(1)];
                if samples.len() + take_frames * ch > max_samples {
                    let _ = decoder.seek(spec.file_start_secs());
                    return RamLoadOutcome::SkippedTooLarge;
                }
                if pkt_ch == ch {
                    samples.extend_from_slice(raw_slice);
                } else {
                    for frame in raw_slice.chunks_exact(pkt_ch.max(1)) {
                        for c in 0..ch {
                            samples.push(frame.get(c).copied().unwrap_or(frame[0]));
                        }
                    }
                }
            }
            Ok(DecodeChunk::Reset) => continue,
            Ok(DecodeChunk::Eof) => break,
            Err(_) => {
                let _ = decoder.seek(spec.file_start_secs());
                return RamLoadOutcome::SkippedTooLarge;
            }
        }
    }

    if samples.is_empty() {
        let _ = decoder.seek(spec.file_start_secs());
        return RamLoadOutcome::SkippedTooLarge;
    }
    if timeline.is_empty()
        && let Some(nom) = decoder.nominal_kbps
    {
        timeline.push(nom);
    }
    let total_frames = samples.len() / ch;
    let start_frame = ((spec.start_offset.max(0.0) * sr as f64).round() as usize).min(total_frames);
    RamLoadOutcome::Loaded(RamTrackBuffer {
        samples,
        channels: ch,
        total_frames,
        frame_cursor: start_frame,
        bitrate_timeline: timeline,
        variable_bitrate: variable,
        last_emitted_bucket: None,
    })
}

struct PendingGapless {
    boundary_frame: u64,
    spec: PlaybackSpec,
    source_sample_rate: u32,
    channels: u16,
    bit_depth: Option<u32>,
    duration: Option<f64>,
    ram_bytes: Option<usize>,
}

#[allow(clippy::too_many_arguments)]
fn play_file(
    device: &Device,
    device_name: String,
    active_device_id: Option<cpal::DeviceId>,
    mut spec: PlaybackSpec,
    commands: &Receiver<AudioCommand>,
    events: &Sender<AudioEvent>,
    volume: &Arc<AtomicU32>,
    rg_gain: &Arc<AtomicU32>,
    linear_volume: &Arc<AtomicBool>,
    bit_perfect: &Arc<AtomicBool>,
    direct_volume_lock: &Arc<AtomicBool>,
    visual_tap: &Arc<VisualTap>,
    dsp: &Arc<SharedDspParams>,
    prefs: &mut AudioEnginePrefs,
) -> Result<PlaybackExit> {
    let mut decoder = DecoderState::open(&spec.path)?;
    let initial_file_pos = spec.file_start_secs();
    let mut ram_buf: Option<RamTrackBuffer> = if prefs.ram_playback && prefs.ram_max_bytes > 0 {
        match try_load_track_to_ram(&mut decoder, prefs.ram_max_bytes, commands, &spec) {
            RamLoadOutcome::Loaded(buf) => Some(buf),
            RamLoadOutcome::SkippedTooLarge => {
                if initial_file_pos > 0.05 {
                    let _ = decoder.seek(initial_file_pos);
                }
                None
            }
            RamLoadOutcome::Interrupted(cmd) => return Ok(PlaybackExit::Command(cmd)),
        }
    } else {
        if initial_file_pos > 0.05 {
            let _ = decoder.seek(initial_file_pos);
        }
        None
    };

    rg_gain.store(spec.rg_gain_linear.clamp(0.1, 4.0).to_bits(), Ordering::Relaxed);

    let effective_oversampling = if bit_perfect.load(Ordering::Relaxed) {
        OversamplingMode::Native
    } else {
        prefs.oversampling
    };
    let (output, mut producer) = create_output_stream(
        device,
        &device_name,
        decoder.source_sample_rate,
        effective_oversampling,
        prefs.exclusive_mode,
        volume,
        rg_gain,
        linear_volume,
        bit_perfect,
        direct_volume_lock,
        visual_tap,
        dsp,
        events,
    )?;
    let output_rate = output.sample_rate;
    let output_format_label = output.format_label.clone();
    let exclusive_active = output.exclusive_active;
    let hw_rate_switched = output.hw_rate_switched;
    visual_tap.set_sample_rate(output_rate);

    let mut track_origin_secs = spec.start_offset.max(0.0);
    let mut track_origin_frame: u64 = 0;
    let mut written_frames: u64 = 0;
    let mut decoded_source_frames: u64 =
        (initial_file_pos * decoder.source_sample_rate as f64).round() as u64;
    let mut fade_in_remaining: usize = 0;
    let mut fade_in_total: usize = 0;
    let mut pending_gapless: Option<PendingGapless> = None;

    let ram_dur = ram_buf
        .as_ref()
        .map(|rb| rb.total_frames as f64 / decoder.source_sample_rate.max(1) as f64);
    let eff_dur = spec.effective_duration(decoder.duration.or(ram_dur));
    let bp_active = bit_perfect.load(Ordering::Relaxed)
        && decoder.source_sample_rate == output_rate
        && (prefs.speed - 1.0).abs() < 0.001;
    let _ = events.send(AudioEvent::Started {
        source_sample_rate: decoder.source_sample_rate,
        output_sample_rate: output_rate,
        output_format: output_format_label.clone(),
        channels: decoder.source_channels.min(u16::MAX as usize) as u16,
        bit_depth: decoder.bit_depth,
        duration: eff_dur,
        device_name: device_name.clone(),
        bit_perfect: bp_active,
        exclusive_active,
        hw_rate_switched,
        rg_gain_db: if bit_perfect.load(Ordering::Relaxed) {
            None
        } else {
            spec.rg_gain_db
        },
        ram_bytes: ram_buf.as_ref().map(RamTrackBuffer::byte_len),
        oversampling: effective_oversampling,
        resampler_quality: prefs.resampler_quality,
    });
    let mut resampler = StreamResampler::new(
        decoder.source_sample_rate,
        output_rate,
        output.channels,
        prefs.speed,
        prefs.resampler_quality,
    )?;
    let mut stats = BitrateMonitor::new(decoder.time_base, decoder.is_variable_bitrate);
    if let Some(nom) = decoder.nominal_kbps {
        let _ = events.send(AudioEvent::Bitrate {
            kbps: nom,
            variable: false,
        });
    }
    let mut current_block = Vec::new();
    let mut block_offset = 0;
    let mut decode_buf: Option<(usize, SampleBuffer<f32>)> = None;
    let mut pcm_scratch: Vec<f32> = Vec::with_capacity(8192);
    let mut paused = spec.start_paused;
    let mut decoder_finished = false;
    let mut packet_errors = 0;
    let mut last_position_update = Instant::now();
    let mut last_device_check = Instant::now();

    let calc_position = |played: u64, origin_frame: u64, origin_secs: f64, speed: f32| -> f64 {
        let delta_frames = played.saturating_sub(origin_frame);
        origin_secs + (delta_frames as f64 / output_rate as f64) * speed as f64
    };

    if paused {
        visual_tap.set_active(false);
        let _ = events.send(AudioEvent::Playing(false));
        let _ = events.send(AudioEvent::Position(track_origin_secs));
    } else {
        output.stream.play()?;
        visual_tap.set_active(true);
        let _ = events.send(AudioEvent::Playing(true));
    }

    loop {
        // Detect audible gapless transition boundary as soon as played_frames reaches the splice!
        let played_now = output.played_frames.load(Ordering::Relaxed);
        if let Some(ref pending) = pending_gapless
            && played_now >= pending.boundary_frame
        {
            let trans = pending_gapless.take().unwrap();
            spec = trans.spec;
            track_origin_frame = trans.boundary_frame;
            track_origin_secs = spec.start_offset.max(0.0);
            rg_gain.store(spec.rg_gain_linear.clamp(0.1, 4.0).to_bits(), Ordering::Relaxed);
            let bp = bit_perfect.load(Ordering::Relaxed)
                && trans.source_sample_rate == output_rate
                && (prefs.speed - 1.0).abs() < 0.001;
            let eff_os = if bit_perfect.load(Ordering::Relaxed) {
                OversamplingMode::Native
            } else {
                prefs.oversampling
            };
            let _ = events.send(AudioEvent::GaplessTransitioned {
                source_sample_rate: trans.source_sample_rate,
                output_sample_rate: output_rate,
                output_format: output_format_label.clone(),
                channels: trans.channels,
                bit_depth: trans.bit_depth,
                duration: trans.duration,
                device_name: device_name.clone(),
                bit_perfect: bp,
                exclusive_active,
                hw_rate_switched,
                rg_gain_db: if bit_perfect.load(Ordering::Relaxed) {
                    None
                } else {
                    spec.rg_gain_db
                },
                ram_bytes: trans.ram_bytes,
                oversampling: eff_os,
                resampler_quality: prefs.resampler_quality,
            });
            let _ = events.send(AudioEvent::Position(track_origin_secs));
        }

        // Detect default-device switches or device disconnects and seamlessly reopen
        let mut should_reopen = output.reopen_needed.swap(false, Ordering::AcqRel);
        if !should_reopen
            && !paused
            && prefs.preferred_device.is_none()
            && last_device_check.elapsed() >= Duration::from_millis(1500)
        {
            last_device_check = Instant::now();
            let current_default_id = cpal::default_host()
                .default_output_device()
                .and_then(|d| d.id().ok());
            if current_default_id.is_some() && current_default_id != active_device_id {
                should_reopen = true;
            }
        }
        if should_reopen {
            let pos = calc_position(played_now, track_origin_frame, track_origin_secs, prefs.speed);
            visual_tap.set_active(false);
            if let Some(new_name) = default_output_device_name()
                && prefs.preferred_device.is_none()
            {
                let _ = events.send(AudioEvent::DeviceRouted(new_name));
            }
            let mut reopen = spec;
            reopen.start_offset = pos;
            reopen.start_paused = paused;
            return Ok(PlaybackExit::ReopenSpec(reopen));
        }

        let command = if paused {
            match commands.recv_timeout(Duration::from_millis(60)) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(AudioCommand::Shutdown),
            }
        } else {
            commands.try_recv().ok()
        };

        if let Some(command) = command {
            match command {
                AudioCommand::Pause => {
                    if !paused {
                        let _ = output.stream.pause();
                        paused = true;
                        visual_tap.set_active(false);
                        let _ = events.send(AudioEvent::Playing(false));
                    }
                }
                AudioCommand::Resume => {
                    if paused {
                        output.stream.play()?;
                        paused = false;
                        visual_tap.set_active(true);
                        let _ = events.send(AudioEvent::Playing(true));
                    }
                }
                AudioCommand::Stop => {
                    let _ = output.stream.pause();
                    visual_tap.set_active(false);
                    let _ = events.send(AudioEvent::Stopped);
                    return Ok(PlaybackExit::Stopped);
                }
                AudioCommand::Seek(rel_seconds) => {
                    let clamped_rel = rel_seconds.max(0.0);
                    let abs_secs = spec.cue_start + clamped_rel;
                    let seek_res = if let Some(ref mut rb) = ram_buf {
                        // RAM buffer holds exactly this track (already sliced to cue_start..cue_end)
                        let target_f = ((clamped_rel * decoder.source_sample_rate as f64).round()
                            as usize)
                            .min(rb.total_frames);
                        rb.frame_cursor = target_f;
                        rb.last_emitted_bucket = None;
                        Ok(())
                    } else {
                        decoder.seek(abs_secs)
                    };
                    match seek_res {
                        Ok(()) => {
                            resampler.reset();
                            current_block.clear();
                            block_offset = 0;
                            decoder_finished = false;
                            pending_gapless = None;
                            stats.reset();
                            decoded_source_frames =
                                (abs_secs * decoder.source_sample_rate as f64).round() as u64;
                            let cur_played = output.played_frames.load(Ordering::Relaxed);
                            track_origin_frame = cur_played;
                            track_origin_secs = clamped_rel;
                            written_frames = cur_played;
                            output.seek_generation.fetch_add(1, Ordering::Release);
                            let _ = events.send(AudioEvent::Position(clamped_rel));
                        }
                        Err(err) => {
                            let _ = events.send(AudioEvent::Error(format!("Seek failed: {err}")));
                        }
                    }
                }
                AudioCommand::PreloadNext(next_s) => {
                    prefs.next_spec = next_s;
                }
                AudioCommand::SetSpeed(new_speed) => {
                    let new_speed = new_speed.clamp(0.5, 2.0);
                    if (new_speed - prefs.speed).abs() > 0.001 {
                        let cur_played = output.played_frames.load(Ordering::Relaxed);
                        track_origin_secs =
                            calc_position(cur_played, track_origin_frame, track_origin_secs, prefs.speed);
                        track_origin_frame = cur_played;
                        prefs.speed = new_speed;
                        if let Ok(new_r) = StreamResampler::new(
                            decoder.source_sample_rate,
                            output_rate,
                            output.channels,
                            prefs.speed,
                            prefs.resampler_quality,
                        ) {
                            resampler = new_r;
                        }
                    }
                }
                AudioCommand::SetAbLoop(l) => {
                    prefs.ab_loop = l;
                }
                AudioCommand::SetGapless(g) => {
                    prefs.gapless_enabled = g;
                }
                AudioCommand::SetCrossfade(c) => {
                    prefs.crossfade_secs = c.clamp(0.0, 6.0);
                }
                AudioCommand::SetReplayGainLinear(g) => {
                    let clamped = g.clamp(0.1, 4.0);
                    spec.rg_gain_linear = clamped;
                    rg_gain.store(clamped.to_bits(), Ordering::Relaxed);
                }
                AudioCommand::SetRamPlayback { enabled, max_mb } => {
                    prefs.ram_playback = enabled;
                    prefs.ram_max_bytes = (max_mb.clamp(64, 4096) as usize) * 1024 * 1024;
                }
                AudioCommand::SetOversampling { mode, quality } => {
                    let changed = prefs.oversampling != mode || prefs.resampler_quality != quality;
                    prefs.oversampling = mode;
                    prefs.resampler_quality = quality;
                    if changed {
                        let pos = calc_position(
                            played_now,
                            track_origin_frame,
                            track_origin_secs,
                            prefs.speed,
                        );
                        visual_tap.set_active(false);
                        let mut reopen = spec;
                        reopen.start_offset = pos;
                        reopen.start_paused = paused;
                        return Ok(PlaybackExit::ReopenSpec(reopen));
                    }
                }
                AudioCommand::SetExclusiveMode(excl) => {
                    let changed = prefs.exclusive_mode != excl;
                    prefs.exclusive_mode = excl;
                    if changed {
                        let pos = calc_position(
                            played_now,
                            track_origin_frame,
                            track_origin_secs,
                            prefs.speed,
                        );
                        visual_tap.set_active(false);
                        let mut reopen = spec;
                        reopen.start_offset = pos;
                        reopen.start_paused = paused;
                        return Ok(PlaybackExit::ReopenSpec(reopen));
                    }
                }
                AudioCommand::SelectDevice(name) => {
                    prefs.preferred_device = name;
                    let pos =
                        calc_position(played_now, track_origin_frame, track_origin_secs, prefs.speed);
                    visual_tap.set_active(false);
                    let mut reopen = spec;
                    reopen.start_offset = pos;
                    reopen.start_paused = paused;
                    return Ok(PlaybackExit::ReopenSpec(reopen));
                }
                command @ (AudioCommand::Play(_) | AudioCommand::Shutdown) => {
                    visual_tap.set_active(false);
                    return Ok(PlaybackExit::Command(command));
                }
            }
            continue;
        }

        if paused {
            continue;
        }

        let current_pos =
            calc_position(played_now, track_origin_frame, track_origin_secs, prefs.speed);

        // Enforce A-B section loop cleanly in real time
        if let Some((a, b)) = prefs.ab_loop
            && b > a + 0.1
            && current_pos >= b
        {
            let abs_a = spec.cue_start + a.max(0.0);
            let seek_ok = if let Some(ref mut rb) = ram_buf {
                rb.frame_cursor = ((a.max(0.0) * decoder.source_sample_rate as f64).round()
                    as usize)
                    .min(rb.total_frames);
                true
            } else {
                decoder.seek(abs_a).is_ok()
            };
            if seek_ok {
                resampler.reset();
                current_block.clear();
                block_offset = 0;
                decoder_finished = false;
                pending_gapless = None;
                decoded_source_frames =
                    (abs_a * decoder.source_sample_rate as f64).round() as u64;
                let cur_played = output.played_frames.load(Ordering::Relaxed);
                track_origin_frame = cur_played;
                track_origin_secs = a.max(0.0);
                written_frames = cur_played;
                output.seek_generation.fetch_add(1, Ordering::Release);
                let _ = events.send(AudioEvent::Position(track_origin_secs));
                continue;
            }
        }

        if last_position_update.elapsed() >= POSITION_UPDATE_INTERVAL {
            let _ = events.send(AudioEvent::Position(current_pos));
            // Emit live VBR/CBR kbps telemetry from pre-recorded RAM timeline
            if let Some(ref mut rb) = ram_buf
                && !rb.bitrate_timeline.is_empty()
            {
                let bucket = ((current_pos / STATS_WINDOW_SECONDS).floor() as usize)
                    .min(rb.bitrate_timeline.len() - 1);
                if rb.last_emitted_bucket != Some(bucket) {
                    rb.last_emitted_bucket = Some(bucket);
                    let _ = events.send(AudioEvent::Bitrate {
                        kbps: rb.bitrate_timeline[bucket],
                        variable: rb.variable_bitrate,
                    });
                }
            }
            last_position_update = Instant::now();
        }

        if block_offset < current_block.len() {
            let pushed = producer.push_slice(&current_block[block_offset..]);
            block_offset += pushed;
            written_frames += (pushed / output.channels.max(1)) as u64;
            if pushed == 0 {
                thread::sleep(Duration::from_millis(2));
            }
            continue;
        }
        current_block.clear();
        block_offset = 0;
        if let Some(mut block) = resampler.next_block() {
            // Apply smooth equal-power fade-in right after a crossfade transition
            if fade_in_remaining > 0 && fade_in_total > 0 {
                let ch = output.channels.max(1);
                for frame in block.chunks_mut(ch) {
                    if fade_in_remaining == 0 {
                        break;
                    }
                    let done = fade_in_total - fade_in_remaining;
                    let t = (done as f32 / fade_in_total as f32).clamp(0.0, 1.0);
                    let gain = (t * std::f32::consts::FRAC_PI_2).sin();
                    for s in frame.iter_mut() {
                        *s *= gain;
                    }
                    fade_in_remaining -= 1;
                }
            }
            current_block = block;
            continue;
        }
        if decoder_finished {
            // Attempt seamless gapless splice into the pre-queued next track inside the same open OutputStream!
            if prefs.gapless_enabled
                && pending_gapless.is_none()
                && let Some(next_s) = prefs.next_spec.take()
                && let Ok(mut next_dec) = DecoderState::open(&next_s.path)
            {
                let rate_switch_needed = bit_perfect.load(Ordering::Relaxed)
                    && next_dec.source_sample_rate != output_rate;
                if !rate_switch_needed {
                    let start_f = next_s.file_start_secs();
                    if start_f > 0.05 {
                        let _ = next_dec.seek(start_f);
                    }
                    if let Ok(next_resampler) = StreamResampler::new(
                        next_dec.source_sample_rate,
                        output_rate,
                        output.channels,
                        prefs.speed,
                        prefs.resampler_quality,
                    ) {
                        let next_ram = if prefs.ram_playback && prefs.ram_max_bytes > 0 {
                            match try_load_track_to_ram(
                                &mut next_dec,
                                prefs.ram_max_bytes,
                                commands,
                                &next_s,
                            ) {
                                RamLoadOutcome::Loaded(buf) => Some(buf),
                                RamLoadOutcome::SkippedTooLarge => None,
                                RamLoadOutcome::Interrupted(cmd) => {
                                    visual_tap.set_active(false);
                                    return Ok(PlaybackExit::Command(cmd));
                                }
                            }
                        } else {
                            None
                        };
                        let next_ram_dur = next_ram.as_ref().map(|rb| {
                            rb.total_frames as f64 / next_dec.source_sample_rate.max(1) as f64
                        });
                        let next_dur = next_ram_dur.or_else(|| next_s.effective_duration(next_dec.duration));
                        pending_gapless = Some(PendingGapless {
                            boundary_frame: written_frames,
                            spec: next_s,
                            source_sample_rate: next_dec.source_sample_rate,
                            channels: next_dec.source_channels.min(u16::MAX as usize) as u16,
                            bit_depth: next_dec.bit_depth,
                            duration: next_dur,
                            ram_bytes: next_ram.as_ref().map(RamTrackBuffer::byte_len),
                        });
                        if prefs.crossfade_secs > 0.05 {
                            let fade_f = ((prefs.crossfade_secs * 0.5) * output_rate as f32).round()
                                as usize;
                            fade_in_total = fade_f;
                            fade_in_remaining = fade_f;
                        }
                        ram_buf = next_ram;
                        decoder = next_dec;
                        resampler = next_resampler;
                        stats = BitrateMonitor::new(decoder.time_base, decoder.is_variable_bitrate);
                        if let Some(nom) = decoder.nominal_kbps {
                            let _ = events.send(AudioEvent::Bitrate {
                                kbps: nom,
                                variable: false,
                            });
                        }
                        decoded_source_frames =
                            (start_f * decoder.source_sample_rate as f64).round() as u64;
                        decoder_finished = false;
                        continue;
                    }
                } else {
                    // Put back so the UI advances and reopens the DAC at the new native sample rate!
                    prefs.next_spec = Some(next_s);
                }
            }

            if producer.is_empty() {
                visual_tap.set_active(false);
                let _ = events.send(AudioEvent::Ended);
                return Ok(PlaybackExit::Ended);
            }
            thread::sleep(Duration::from_millis(2));
            continue;
        }

        // Feed directly from RAM PCM buffer when Play-from-RAM is active (zero disk I/O!)
        if let Some(ref mut rb) = ram_buf {
            if rb.frame_cursor >= rb.total_frames {
                resampler.finish();
                decoder_finished = true;
                continue;
            }
            let chunk_frames = 4096.min(rb.total_frames - rb.frame_cursor);
            let ch = rb.channels;
            let start_idx = rb.frame_cursor * ch;
            let end_idx = (rb.frame_cursor + chunk_frames) * ch;
            let raw_samples = &rb.samples[start_idx..end_idx];

            let end_secs = Some(rb.total_frames as f64 / decoder.source_sample_rate.max(1) as f64);
            let sr = decoder.source_sample_rate.max(1) as f64;
            let fade_s = (prefs.crossfade_secs * 0.5) as f64;
            let pkt_end_s = (decoded_source_frames + chunk_frames as u64) as f64 / sr;
            if prefs.crossfade_secs > 0.05
                && let Some(end_s) = end_secs
                && pkt_end_s > (end_s - fade_s).max(0.0)
            {
                let fade_start = (end_s - fade_s).max(0.0);
                let mut faded = raw_samples.to_vec();
                for f_idx in 0..chunk_frames {
                    let t_s = (decoded_source_frames + f_idx as u64) as f64 / sr;
                    if t_s > fade_start {
                        let rem = ((end_s - t_s) / fade_s).clamp(0.0, 1.0) as f32;
                        let gain = (rem * std::f32::consts::FRAC_PI_2).sin();
                        for c in 0..ch {
                            faded[f_idx * ch + c] *= gain;
                        }
                    }
                }
                resampler.push_interleaved(&faded, ch);
            } else {
                resampler.push_interleaved(raw_samples, ch);
            }
            rb.frame_cursor += chunk_frames;
            decoded_source_frames += chunk_frames as u64;
            continue;
        }

        // Enforce CUE virtual track end boundary cleanly
        if let Some(cue_end) = spec.cue_end {
            let decoded_secs =
                decoded_source_frames as f64 / decoder.source_sample_rate.max(1) as f64;
            if decoded_secs >= cue_end {
                resampler.finish();
                decoder_finished = true;
                continue;
            }
        }

        match decoder.decode_chunk(&mut decode_buf, &mut pcm_scratch, &mut packet_errors)? {
            DecodeChunk::Frames {
                channels,
                packet_bytes,
                packet_dur,
            } => {
                if decoder.nominal_kbps.is_none() {
                    stats.push(packet_bytes, packet_dur, events);
                }
                let raw_samples = pcm_scratch.as_slice();
                let frames_in_pkt = raw_samples.len() / channels.max(1);

                // Optional smooth fade-out near end of track when crossfade is enabled (zero copy otherwise!)
                let end_secs = spec.cue_end.or(decoder.duration);
                let sr = decoder.source_sample_rate.max(1) as f64;
                let fade_s = (prefs.crossfade_secs * 0.5) as f64;
                let pkt_end_s = (decoded_source_frames + frames_in_pkt as u64) as f64 / sr;
                if prefs.crossfade_secs > 0.05
                    && let Some(end_s) = end_secs
                    && pkt_end_s > (end_s - fade_s).max(spec.cue_start)
                {
                    let fade_start = (end_s - fade_s).max(spec.cue_start);
                    let mut faded = raw_samples.to_vec();
                    for f_idx in 0..frames_in_pkt {
                        let t_s = (decoded_source_frames + f_idx as u64) as f64 / sr;
                        if t_s > fade_start {
                            let rem = ((end_s - t_s) / fade_s).clamp(0.0, 1.0) as f32;
                            let gain = (rem * std::f32::consts::FRAC_PI_2).sin();
                            for c in 0..channels {
                                faded[f_idx * channels + c] *= gain;
                            }
                        }
                    }
                    resampler.push_interleaved(&faded, channels);
                } else {
                    resampler.push_interleaved(raw_samples, channels);
                }

                decoded_source_frames += frames_in_pkt as u64;
            }
            DecodeChunk::Reset => {
                resampler.reset();
            }
            DecodeChunk::Eof => {
                resampler.finish();
                decoder_finished = true;
            }
        }
    }
}

struct OutputConfig {
    config: StreamConfig,
    format: SampleFormat,
}

/// Selects the optimal DAC hardware sample rate based on the audiophile oversampling policy.
pub fn choose_target_sample_rate(
    source_rate: u32,
    supported_ranges: &[(u32, u32)],
    mode: OversamplingMode,
) -> Option<u32> {
    if supported_ranges.is_empty() {
        return None;
    }
    let supports = |rate: u32| -> bool {
        supported_ranges
            .iter()
            .any(|&(lo, hi)| rate >= lo && rate <= hi)
    };
    const MAX_CAP: u32 = 384_000;
    match mode {
        OversamplingMode::Native => {
            if supports(source_rate) {
                Some(source_rate)
            } else {
                None
            }
        }
        OversamplingMode::PowerOf2 => {
            // Try highest synchronous integer power-of-2 multiple (8x, 4x, 2x, 1x) up to 384 kHz
            for mult in [8_u32, 4, 2, 1] {
                let candidate = source_rate.saturating_mul(mult);
                if candidate <= MAX_CAP && supports(candidate) {
                    return Some(candidate);
                }
            }
            None
        }
        OversamplingMode::MaxDeviceRate => {
            // Prefer standard audiophile rates from highest to lowest within supported ranges
            for &rate in &[384_000_u32, 352_800, 192_000, 176_400, 96_000, 88_200, 48_000, 44_100] {
                if supports(rate) {
                    return Some(rate);
                }
            }
            supported_ranges
                .iter()
                .map(|&(_, hi)| hi.min(MAX_CAP))
                .max()
        }
    }
}

fn format_sample_format_label(fmt: SampleFormat) -> String {
    match fmt {
        SampleFormat::F32 => "32-bit float".into(),
        SampleFormat::F64 => "64-bit float".into(),
        SampleFormat::I32 => "32-bit int".into(),
        SampleFormat::U32 => "32-bit uint".into(),
        SampleFormat::I16 => "16-bit int".into(),
        SampleFormat::U16 => "16-bit uint".into(),
        SampleFormat::I8 => "8-bit int".into(),
        SampleFormat::U8 => "8-bit uint".into(),
        other => format!("{other}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn create_output_stream(
    device: &Device,
    device_name: &str,
    source_rate: u32,
    oversampling: OversamplingMode,
    exclusive_mode: bool,
    volume: &Arc<AtomicU32>,
    rg_gain: &Arc<AtomicU32>,
    linear_volume: &Arc<AtomicBool>,
    bit_perfect: &Arc<AtomicBool>,
    direct_volume_lock: &Arc<AtomicBool>,
    visual_tap: &Arc<VisualTap>,
    dsp: &Arc<SharedDspParams>,
    events: &Sender<AudioEvent>,
) -> Result<(OutputStream, HeapProd<f32>)> {
    let default = device.default_output_config()?;
    let ranges_vec: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_output_configs()
        .map(|it| it.filter(|r| r.channels() >= 1).collect())
        .unwrap_or_default();
    let rate_bounds: Vec<(u32, u32)> = ranges_vec
        .iter()
        .map(|r| (r.min_sample_rate(), r.max_sample_rate()))
        .collect();
    let target_rate = choose_target_sample_rate(source_rate, &rate_bounds, oversampling)
        .unwrap_or(source_rate);

    // Acquire hardware Exclusive Hog Mode & native hardware DAC sample-rate clock BEFORE opening stream
    let want_hw_clock = exclusive_mode
        || bit_perfect.load(Ordering::Relaxed)
        || oversampling != OversamplingMode::Native;
    let exclusive_guard =
        ExclusiveDeviceGuard::acquire(device_name, target_rate, exclusive_mode, want_hw_clock);

    let preferred_config = ranges_vec
        .into_iter()
        .filter(|range| {
            target_rate >= range.min_sample_rate() && target_rate <= range.max_sample_rate()
        })
        .min_by_key(|range| {
            let chan_diff = (range.channels() as u32).abs_diff(2);
            let fmt_rank = match range.sample_format() {
                SampleFormat::F32 => 0,
                SampleFormat::I32 => 1,
                SampleFormat::I16 => 2,
                _ => 3,
            };
            (chan_diff, fmt_rank)
        })
        .map(|range| range.with_sample_rate(target_rate))
        .unwrap_or(default);

    let primary = OutputConfig {
        config: StreamConfig {
            channels: preferred_config.channels(),
            sample_rate: preferred_config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        },
        format: preferred_config.sample_format(),
    };

    match open_with_config(
        device,
        &primary,
        exclusive_guard,
        volume,
        rg_gain,
        linear_volume,
        bit_perfect,
        direct_volume_lock,
        visual_tap,
        dsp,
        events,
    ) {
        Ok(res) => Ok(res),
        Err((primary_err, guard_back)) => {
            let fallback = OutputConfig {
                config: StreamConfig {
                    channels: default.channels(),
                    sample_rate: default.sample_rate(),
                    buffer_size: cpal::BufferSize::Default,
                },
                format: default.sample_format(),
            };
            if fallback.config.sample_rate == primary.config.sample_rate
                && fallback.config.channels == primary.config.channels
                && fallback.format == primary.format
            {
                return Err(primary_err);
            }
            open_with_config(
                device,
                &fallback,
                guard_back,
                volume,
                rg_gain,
                linear_volume,
                bit_perfect,
                direct_volume_lock,
                visual_tap,
                dsp,
                events,
            )
            .map_err(|(e, _)| e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn open_with_config(
    device: &Device,
    output: &OutputConfig,
    exclusive_guard: ExclusiveDeviceGuard,
    volume: &Arc<AtomicU32>,
    rg_gain: &Arc<AtomicU32>,
    linear_volume: &Arc<AtomicBool>,
    bit_perfect: &Arc<AtomicBool>,
    direct_volume_lock: &Arc<AtomicBool>,
    visual_tap: &Arc<VisualTap>,
    dsp: &Arc<SharedDspParams>,
    events: &Sender<AudioEvent>,
) -> std::result::Result<(OutputStream, HeapProd<f32>), (anyhow::Error, ExclusiveDeviceGuard)> {
    let output_rate = output.config.sample_rate;
    let channels = output.config.channels as usize;
    let ring = HeapRb::<f32>::new(output_rate as usize * channels * BUFFER_SECONDS);
    let (producer, consumer) = ring.split();
    let played_frames = Arc::new(AtomicU64::new(0));
    let seek_generation = Arc::new(AtomicU64::new(0));
    let reopen_needed = Arc::new(AtomicBool::new(false));

    macro_rules! mk {
        ($ty:ty) => {
            match build_stream::<$ty>(
                device,
                &output.config,
                consumer,
                volume,
                rg_gain,
                linear_volume,
                bit_perfect,
                direct_volume_lock,
                visual_tap,
                dsp,
                &played_frames,
                &seek_generation,
                &reopen_needed,
                events,
            ) {
                Ok(s) => s,
                Err(e) => return Err((e, exclusive_guard)),
            }
        };
    }

    let stream = match output.format {
        SampleFormat::F32 => mk!(f32),
        SampleFormat::F64 => mk!(f64),
        SampleFormat::I32 => mk!(i32),
        SampleFormat::U32 => mk!(u32),
        SampleFormat::I16 => mk!(i16),
        SampleFormat::U16 => mk!(u16),
        SampleFormat::I8 => mk!(i8),
        SampleFormat::U8 => mk!(u8),
        format => {
            return Err((
                anyhow!("Unsupported output sample format: {format}"),
                exclusive_guard,
            ));
        }
    };
    let exclusive_active = exclusive_guard.status.hog_acquired;
    let hw_rate_switched = exclusive_guard.status.hw_rate_switched;
    Ok((
        OutputStream {
            stream,
            sample_rate: output_rate,
            channels,
            format_label: format_sample_format_label(output.format),
            exclusive_active,
            hw_rate_switched,
            _exclusive_guard: exclusive_guard,
            played_frames,
            seek_generation,
            reopen_needed,
        },
        producer,
    ))
}

trait CpalSample: cpal::SizedSample + Send + 'static {
    fn from_float(sample: f32) -> Self;
}

impl CpalSample for f32 {
    fn from_float(sample: f32) -> Self {
        sample
    }
}
impl CpalSample for f64 {
    fn from_float(sample: f32) -> Self {
        sample as f64
    }
}
impl CpalSample for i32 {
    fn from_float(sample: f32) -> Self {
        (sample.clamp(-1.0, 1.0) as f64 * i32::MAX as f64) as i32
    }
}
impl CpalSample for u32 {
    fn from_float(sample: f32) -> Self {
        ((sample.clamp(-1.0, 1.0) as f64 * 0.5 + 0.5) * u32::MAX as f64) as u32
    }
}
impl CpalSample for i16 {
    fn from_float(sample: f32) -> Self {
        (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
    }
}
impl CpalSample for u16 {
    fn from_float(sample: f32) -> Self {
        ((sample.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16
    }
}
impl CpalSample for i8 {
    fn from_float(sample: f32) -> Self {
        (sample.clamp(-1.0, 1.0) * i8::MAX as f32) as i8
    }
}
impl CpalSample for u8 {
    fn from_float(sample: f32) -> Self {
        ((sample.clamp(-1.0, 1.0) * 0.5 + 0.5) * u8::MAX as f32) as u8
    }
}

fn volume_to_gain(volume: f32, linear: bool) -> f32 {
    if volume <= 0.0001 {
        0.0
    } else if linear {
        volume.clamp(0.0, 1.0)
    } else {
        volume.clamp(0.0, 1.0).powf(1.8)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_stream<T: CpalSample>(
    device: &Device,
    config: &StreamConfig,
    mut consumer: HeapCons<f32>,
    volume: &Arc<AtomicU32>,
    rg_gain: &Arc<AtomicU32>,
    linear_volume: &Arc<AtomicBool>,
    bit_perfect: &Arc<AtomicBool>,
    direct_volume_lock: &Arc<AtomicBool>,
    visual_tap: &Arc<VisualTap>,
    dsp_params: &Arc<SharedDspParams>,
    played_frames: &Arc<AtomicU64>,
    seek_generation: &Arc<AtomicU64>,
    reopen_needed: &Arc<AtomicBool>,
    events: &Sender<AudioEvent>,
) -> Result<Stream> {
    let channels = config.channels as usize;
    let volume = Arc::clone(volume);
    let rg_gain = Arc::clone(rg_gain);
    let linear_volume = Arc::clone(linear_volume);
    let bit_perfect = Arc::clone(bit_perfect);
    let direct_volume_lock = Arc::clone(direct_volume_lock);
    let visual_tap = Arc::clone(visual_tap);
    let dsp_params = Arc::clone(dsp_params);
    let played_frames = Arc::clone(played_frames);
    let seek_generation = Arc::clone(seek_generation);
    let reopen_flag = Arc::clone(reopen_needed);
    let mut callback_generation = 0;
    let mut smoothed_gain = if direct_volume_lock.load(Ordering::Relaxed) {
        1.0
    } else {
        volume_to_gain(
            f32::from_bits(volume.load(Ordering::Relaxed)),
            linear_volume.load(Ordering::Relaxed),
        )
    };
    // Sample-rate-independent ~4.5 ms volume ramp constant (identical feel from 44.1 kHz to 192 kHz)
    let gain_alpha = (1.0 - (-1.0_f32 / (config.sample_rate.max(8_000) as f32 * 0.0045)).exp())
        .clamp(0.0005, 0.05);
    let mut dsp = RealtimeDsp::new(config.sample_rate, &dsp_params);
    let mut write_cursor = visual_tap.write_pos.load(Ordering::Relaxed);
    let error_events = events.clone();

    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _| {
            let generation = seek_generation.load(Ordering::Acquire);
            if generation != callback_generation {
                consumer.clear();
                dsp.reset_state();
                callback_generation = generation;
            }
            dsp.sync(&dsp_params);
            let bp = bit_perfect.load(Ordering::Relaxed);
            let vol_lock = direct_volume_lock.load(Ordering::Relaxed);
            let rg = if bp {
                1.0
            } else {
                f32::from_bits(rg_gain.load(Ordering::Relaxed)).clamp(0.1, 4.0)
            };
            let target_gain = if vol_lock {
                rg
            } else {
                volume_to_gain(
                    f32::from_bits(volume.load(Ordering::Relaxed)),
                    linear_volume.load(Ordering::Relaxed),
                ) * rg
            };
            let mut sum_sq_l = 0.0_f32;
            let mut sum_sq_r = 0.0_f32;
            let mut sum_lr = 0.0_f32;
            let mut peak_l = 0.0_f32;
            let mut peak_r = 0.0_f32;
            let mut frame_count = 0_usize;

            for frame in data.chunks_mut(channels) {
                smoothed_gain += (target_gain - smoothed_gain) * gain_alpha;
                let effective_gain = if vol_lock && bp && (smoothed_gain - 1.0).abs() < 1e-5 {
                    1.0
                } else {
                    smoothed_gain
                };
                if consumer.occupied_len() >= channels {
                    let mut raw_l = 0.0_f32;
                    let mut raw_r = 0.0_f32;
                    let mut extras = [0.0_f32; 6];
                    for ch_idx in 0..channels {
                        let s = consumer.try_pop().unwrap_or(0.0);
                        if ch_idx == 0 {
                            raw_l = s;
                        } else if ch_idx == 1 {
                            raw_r = s;
                        } else if ch_idx - 2 < extras.len() {
                            extras[ch_idx - 2] = s;
                        }
                    }
                    if channels == 1 {
                        raw_r = raw_l;
                    }
                    let (proc_l, proc_r) = if bp {
                        (raw_l, raw_r)
                    } else {
                        dsp.process_stereo(raw_l, raw_r)
                    };
                    for (ch_idx, sample) in frame.iter_mut().enumerate() {
                        let out = if ch_idx == 0 {
                            proc_l
                        } else if ch_idx == 1 {
                            proc_r
                        } else if ch_idx - 2 < extras.len() {
                            extras[ch_idx - 2]
                        } else {
                            0.0
                        };
                        *sample = T::from_float(out * effective_gain);
                    }
                    // Tap full-resolution pre-fader musical signal (muted gently only if muted)
                    let vis_scale = if target_gain <= 0.0001 { 0.12 } else { 1.0 };
                    let left = proc_l * vis_scale;
                    let right = proc_r * vis_scale;
                    visual_tap.push_callback_frame(&mut write_cursor, left, right);
                    sum_sq_l += left * left;
                    sum_sq_r += right * right;
                    sum_lr += left * right;
                    peak_l = peak_l.max(left.abs());
                    peak_r = peak_r.max(right.abs());
                    frame_count += 1;
                } else {
                    for sample in frame.iter_mut() {
                        *sample = T::from_float(0.0);
                    }
                    visual_tap.push_callback_frame(&mut write_cursor, 0.0, 0.0);
                }
            }

            if frame_count > 0 {
                // Single batched atomic update per callback buffer (eliminates up to 192k atomic RMW/sec)
                played_frames.fetch_add(frame_count as u64, Ordering::Relaxed);
                let n = frame_count as f32;
                let rms_l = (sum_sq_l / n).sqrt().min(1.0);
                let rms_r = (sum_sq_r / n).sqrt().min(1.0);
                let denom = (sum_sq_l * sum_sq_r).sqrt();
                let corr = if denom > 1e-7 {
                    (sum_lr / denom).clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                visual_tap.commit(
                    write_cursor,
                    rms_l,
                    rms_r,
                    peak_l.min(1.0),
                    peak_r.min(1.0),
                    corr,
                );
            } else {
                visual_tap.commit(write_cursor, 0.0, 0.0, 0.0, 0.0, 0.0);
            }
        },
        move |error| {
            if matches!(
                error.kind(),
                cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::DeviceNotAvailable
            ) {
                reopen_flag.store(true, Ordering::Release);
            } else {
                let _ =
                    error_events.send(AudioEvent::Error(format!("Audio output error: {error}")));
            }
        },
        Some(Duration::from_millis(800)),
    )?;
    Ok(stream)
}

struct StreamResampler {
    channels: usize,
    resampler: Option<SincFixedIn<f32>>,
    pending: Vec<Vec<f32>>,
    ready: VecDeque<Vec<f32>>,
    chunk_size: usize,
}

impl StreamResampler {
    fn new(
        source_rate: u32,
        output_rate: u32,
        channels: usize,
        speed: f32,
        quality: ResamplerQuality,
    ) -> Result<Self> {
        let channels = channels.max(1);
        let chunk_size = 1024;
        let speed = speed.clamp(0.5, 2.0) as f64;
        let ratio = output_rate as f64 / (source_rate.max(1) as f64 * speed);
        let resampler = if (ratio - 1.0).abs() > 1e-4 {
            let parameters = match quality {
                ResamplerQuality::LinearPhaseStudio => SincInterpolationParameters {
                    sinc_len: 256,
                    f_cutoff: 0.95,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 256,
                    window: WindowFunction::BlackmanHarris2,
                },
                ResamplerQuality::SharpPrecision => SincInterpolationParameters {
                    sinc_len: 256,
                    f_cutoff: 0.97,
                    interpolation: SincInterpolationType::Cubic,
                    oversampling_factor: 256,
                    window: WindowFunction::Hann2,
                },
                ResamplerQuality::GentleAnalog => SincInterpolationParameters {
                    sinc_len: 128,
                    f_cutoff: 0.91,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 128,
                    window: WindowFunction::Blackman,
                },
            };
            Some(SincFixedIn::new(
                ratio,
                2.5,
                parameters,
                chunk_size,
                channels,
            )?)
        } else {
            None
        };
        Ok(Self {
            channels,
            resampler,
            pending: vec![Vec::with_capacity(chunk_size * 2); channels],
            ready: VecDeque::new(),
            chunk_size,
        })
    }

    fn push_interleaved(&mut self, samples: &[f32], packet_channels: usize) {
        if packet_channels == 0 || samples.is_empty() {
            return;
        }
        // Fast 1:1 passthrough path: zero planar deinterleave/reinterleave overhead!
        if self.resampler.is_none() && packet_channels == self.channels {
            let valid_len = (samples.len() / packet_channels) * packet_channels;
            if valid_len > 0 {
                self.ready.push_back(samples[..valid_len].to_vec());
            }
            return;
        }
        for frame in samples
            .chunks(packet_channels)
            .filter(|frame| frame.len() == packet_channels)
        {
            for target in 0..self.channels {
                let value = if packet_channels == 1 {
                    frame[0]
                } else if self.channels == 1 {
                    frame.iter().sum::<f32>() / packet_channels as f32
                } else {
                    frame.get(target).copied().unwrap_or(0.0)
                };
                self.pending[target].push(value);
            }
        }
        self.process_blocks(false);
    }

    fn next_block(&mut self) -> Option<Vec<f32>> {
        self.ready.pop_front()
    }

    fn process_blocks(&mut self, finish: bool) {
        if self.resampler.is_none() {
            if !self.pending[0].is_empty() {
                self.ready.push_back(interleave(&self.pending));
                for channel in &mut self.pending {
                    channel.clear();
                }
            }
            return;
        }
        loop {
            let available = self.pending[0].len();
            if available < self.chunk_size && !(finish && available > 0) {
                break;
            }
            let input: Vec<Vec<f32>> = self
                .pending
                .iter()
                .map(|channel| channel.iter().copied().take(self.chunk_size).collect())
                .collect();
            let resampler = self.resampler.as_mut().expect("resampler exists");
            let result = if available < self.chunk_size {
                resampler.process_partial(Some(&input), None)
            } else {
                resampler.process(&input, None)
            };
            match result {
                Ok(output) => {
                    let consumed = available.min(self.chunk_size);
                    for channel in &mut self.pending {
                        channel.drain(..consumed);
                    }
                    self.ready.push_back(interleave(&output));
                }
                Err(_) => break,
            }
            if available < self.chunk_size {
                break;
            }
        }
        if finish
            && self.pending[0].is_empty()
            && let Some(resampler) = self.resampler.as_mut()
            && let Ok(output) = resampler.process_partial::<Vec<f32>>(None, None)
        {
            self.ready.push_back(interleave(&output));
        }
    }

    fn finish(&mut self) {
        self.process_blocks(true);
    }

    fn reset(&mut self) {
        for channel in &mut self.pending {
            channel.clear();
        }
        self.ready.clear();
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
    }
}

fn interleave(planar: &[Vec<f32>]) -> Vec<f32> {
    let frames = planar.first().map_or(0, Vec::len);
    let mut output = Vec::with_capacity(frames * planar.len());
    for frame in 0..frames {
        for channel in planar {
            output.push(channel.get(frame).copied().unwrap_or(0.0));
        }
    }
    output
}

struct BitrateMonitor {
    time_base: Option<TimeBase>,
    accumulated_bytes: usize,
    accumulated_frames: u64,
    min_kbps: Option<u32>,
    max_kbps: Option<u32>,
    variable: bool,
    last_kbps: Option<u32>,
    last_variable: bool,
}

impl BitrateMonitor {
    fn new(time_base: Option<TimeBase>, variable: bool) -> Self {
        Self {
            time_base,
            accumulated_bytes: 0,
            accumulated_frames: 0,
            min_kbps: None,
            max_kbps: None,
            variable,
            last_kbps: None,
            last_variable: false,
        }
    }

    fn push(&mut self, bytes: usize, frames: u64, events: &Sender<AudioEvent>) {
        self.accumulated_bytes += bytes;
        self.accumulated_frames += frames;
        let Some(time_base) = self.time_base else {
            return;
        };
        if self.accumulated_frames == 0 {
            return;
        }
        let time = time_base.calc_time(self.accumulated_frames);
        let seconds = time.seconds as f64 + time.frac;
        if seconds < STATS_WINDOW_SECONDS {
            return;
        }
        let kbps = ((self.accumulated_bytes as f64 * 8.0 / seconds) / 1000.0)
            .round()
            .max(1.0) as u32;
        self.accumulated_bytes = 0;
        self.accumulated_frames = 0;
        self.min_kbps = Some(self.min_kbps.map_or(kbps, |old| old.min(kbps)));
        self.max_kbps = Some(self.max_kbps.map_or(kbps, |old| old.max(kbps)));
        if let (Some(min), Some(max)) = (self.min_kbps, self.max_kbps) {
            self.variable |= max > min.saturating_add((min / 20).max(8));
        }
        if self.last_kbps != Some(kbps) || self.last_variable != self.variable {
            self.last_kbps = Some(kbps);
            self.last_variable = self.variable;
            let _ = events.send(AudioEvent::Bitrate {
                kbps,
                variable: self.variable,
            });
        }
    }

    fn reset(&mut self) {
        self.accumulated_bytes = 0;
        self.accumulated_frames = 0;
        self.last_kbps = None;
    }
}

fn is_variable_lossless(codec: symphonia::core::codecs::CodecType) -> bool {
    codec == symphonia::core::codecs::CODEC_TYPE_FLAC
        || codec == symphonia::core::codecs::CODEC_TYPE_ALAC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_curve_bounds_and_monotonicity() {
        assert_eq!(volume_to_gain(0.0, false), 0.0);
        assert!((volume_to_gain(1.0, false) - 1.0).abs() < 1e-6);
        assert!(volume_to_gain(0.5, false) < 0.5);
        assert!((volume_to_gain(0.5, true) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn stereo_visual_tap_roundtrip() {
        let tap = VisualTap::new();
        tap.set_active(true);
        let mut cursor = 0;
        for i in 0..64 {
            tap.push_callback_frame(&mut cursor, i as f32 / 64.0, -(i as f32 / 64.0));
        }
        tap.commit(cursor, 0.5, 0.4, 0.9, 0.8, -0.95);
        let mut out_l = [0.0_f32; 64];
        let mut out_r = [0.0_f32; 64];
        tap.read_latest_stereo(&mut out_l, &mut out_r);
        assert!((out_l[63] - 63.0 / 64.0).abs() < 1e-6);
        assert!((out_r[63] + 63.0 / 64.0).abs() < 1e-6);
        assert_eq!(tap.levels(), (0.5, 0.4, 0.9, 0.8, -0.95));
    }

    #[test]
    fn oversampling_picks_synchronous_power_of_2_and_max_dac_rates() {
        let dac_192k = &[(44_100, 192_000)];
        assert_eq!(
            choose_target_sample_rate(44_100, dac_192k, OversamplingMode::Native),
            Some(44_100)
        );
        assert_eq!(
            choose_target_sample_rate(44_100, dac_192k, OversamplingMode::PowerOf2),
            Some(176_400)
        );
        assert_eq!(
            choose_target_sample_rate(48_000, dac_192k, OversamplingMode::PowerOf2),
            Some(192_000)
        );
        assert_eq!(
            choose_target_sample_rate(44_100, dac_192k, OversamplingMode::MaxDeviceRate),
            Some(192_000)
        );
        let dac_96k = &[(44_100, 96_000)];
        assert_eq!(
            choose_target_sample_rate(44_100, dac_96k, OversamplingMode::PowerOf2),
            Some(88_200)
        );
    }
}
