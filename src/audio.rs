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

use crate::dsp::{RealtimeDsp, SharedDspParams};
use crate::plugins::DspConfig;

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

#[derive(Debug)]
pub enum AudioCommand {
    Play {
        path: PathBuf,
        start_seconds: f64,
        start_paused: bool,
    },
    Pause,
    Resume,
    Stop,
    Seek(f64),
    SelectDevice(Option<String>),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum AudioEvent {
    Started {
        source_sample_rate: u32,
        output_sample_rate: u32,
        channels: u16,
        bit_depth: Option<u32>,
        duration: Option<f64>,
        device_name: String,
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
    linear_volume: Arc<AtomicBool>,
    visual_tap: Arc<VisualTap>,
    dsp: Arc<SharedDspParams>,
    _join: JoinHandle<()>,
}

impl AudioHandle {
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let volume = Arc::new(AtomicU32::new(0.75_f32.to_bits()));
        let linear_volume = Arc::new(AtomicBool::new(false));
        let visual_tap = Arc::new(VisualTap::new());
        let dsp = Arc::new(SharedDspParams::new(&DspConfig::default()));
        let thread_volume = Arc::clone(&volume);
        let thread_linear = Arc::clone(&linear_volume);
        let thread_tap = Arc::clone(&visual_tap);
        let thread_dsp = Arc::clone(&dsp);
        let join = thread::Builder::new()
            .name("txplayer-audio".into())
            .spawn(move || {
                if let Err(error) = run_audio_thread(
                    command_rx,
                    event_tx.clone(),
                    thread_volume,
                    thread_linear,
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
            linear_volume,
            visual_tap,
            dsp,
            _join: join,
        }
    }

    pub fn play(&self, path: PathBuf) {
        let _ = self.commands.send(AudioCommand::Play {
            path,
            start_seconds: 0.0,
            start_paused: false,
        });
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

    pub fn update_dsp(&self, cfg: &DspConfig) {
        self.dsp.update(cfg);
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
        let mut samples = SampleBuffer::<f32>::new(audio.capacity() as u64, spec);
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
    ReopenAt {
        path: PathBuf,
        position: f64,
        paused: bool,
    },
}

fn run_audio_thread(
    commands: Receiver<AudioCommand>,
    events: Sender<AudioEvent>,
    volume: Arc<AtomicU32>,
    linear_volume: Arc<AtomicBool>,
    visual_tap: Arc<VisualTap>,
    dsp: Arc<SharedDspParams>,
) -> Result<()> {
    let mut preferred_device: Option<String> = None;
    let mut next = commands.recv();
    while let Ok(command) = next {
        match command {
            AudioCommand::Play {
                path,
                start_seconds,
                start_paused,
            } => {
                let device_result = resolve_output_device(preferred_device.as_deref());
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
                    path,
                    start_seconds,
                    start_paused,
                    &commands,
                    &events,
                    &volume,
                    &linear_volume,
                    &visual_tap,
                    &dsp,
                    &mut preferred_device,
                ) {
                    Ok(PlaybackExit::Command(command)) => Ok(command),
                    Ok(PlaybackExit::ReopenAt {
                        path,
                        position,
                        paused,
                    }) => Ok(AudioCommand::Play {
                        path,
                        start_seconds: position,
                        start_paused: paused,
                    }),
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
            AudioCommand::SelectDevice(name) => {
                preferred_device = name;
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

struct DecoderState {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    source_sample_rate: u32,
    source_channels: usize,
    bit_depth: Option<u32>,
    duration: Option<f64>,
    time_base: Option<TimeBase>,
}

impl DecoderState {
    fn open(path: &Path) -> Result<Self> {
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
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .context("Creating audio decoder")?;
        Ok(Self {
            format,
            decoder,
            track_id,
            source_sample_rate,
            source_channels,
            bit_depth,
            duration,
            time_base,
        })
    }

    fn seek(&mut self, seconds: f64) -> Result<()> {
        let clamped = seconds.max(0.0);
        let time = Time::new(clamped.trunc() as u64, clamped.fract());
        if self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .is_err()
        {
            self.format.seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )?;
        }
        self.decoder.reset();
        Ok(())
    }
}

struct OutputStream {
    stream: Stream,
    sample_rate: u32,
    channels: usize,
    played_frames: Arc<AtomicU64>,
    seek_generation: Arc<AtomicU64>,
    reopen_needed: Arc<AtomicBool>,
}

#[allow(clippy::too_many_arguments)]
fn play_file(
    device: &Device,
    device_name: String,
    active_device_id: Option<cpal::DeviceId>,
    path: PathBuf,
    start_seconds: f64,
    start_paused: bool,
    commands: &Receiver<AudioCommand>,
    events: &Sender<AudioEvent>,
    volume: &Arc<AtomicU32>,
    linear_volume: &Arc<AtomicBool>,
    visual_tap: &Arc<VisualTap>,
    dsp: &Arc<SharedDspParams>,
    preferred_device: &mut Option<String>,
) -> Result<PlaybackExit> {
    let mut decoder = DecoderState::open(&path)?;
    if start_seconds > 0.05 {
        let _ = decoder.seek(start_seconds);
    }
    let (output, mut producer) = create_output_stream(
        device,
        decoder.source_sample_rate,
        volume,
        linear_volume,
        visual_tap,
        dsp,
        events,
    )?;
    let output_rate = output.sample_rate;
    visual_tap.set_sample_rate(output_rate);
    if start_seconds > 0.05 {
        output
            .played_frames
            .store((start_seconds * output_rate as f64) as u64, Ordering::Relaxed);
    }
    let _ = events.send(AudioEvent::Started {
        source_sample_rate: decoder.source_sample_rate,
        output_sample_rate: output_rate,
        channels: decoder.source_channels.min(u16::MAX as usize) as u16,
        bit_depth: decoder.bit_depth,
        duration: decoder.duration,
        device_name: device_name.clone(),
    });
    let mut resampler =
        StreamResampler::new(decoder.source_sample_rate, output_rate, output.channels)?;
    let mut stats = BitrateMonitor::new(
        decoder.time_base,
        is_variable_lossless(decoder.decoder.codec_params().codec),
    );
    let mut current_block = Vec::new();
    let mut block_offset = 0;
    let mut decode_buf: Option<(usize, SampleBuffer<f32>)> = None;
    let mut paused = start_paused;
    let mut decoder_finished = false;
    let mut packet_errors = 0;
    let mut last_position_update = Instant::now();
    let mut last_device_check = Instant::now();

    if paused {
        visual_tap.set_active(false);
        let _ = events.send(AudioEvent::Playing(false));
    } else {
        output.stream.play()?;
        visual_tap.set_active(true);
        let _ = events.send(AudioEvent::Playing(true));
    }

    loop {
        // Detect CoreAudio default-device switches (e.g. connecting Bluetooth Klipsch speakers)
        // or device disconnects even mid-track or while paused, and seamlessly reopen!
        let mut should_reopen = output.reopen_needed.swap(false, Ordering::AcqRel);
        if !should_reopen
            && preferred_device.is_none()
            && last_device_check.elapsed() >= Duration::from_millis(500)
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
            let position =
                output.played_frames.load(Ordering::Relaxed) as f64 / output_rate as f64;
            visual_tap.set_active(false);
            if let Some(new_name) = default_output_device_name()
                && preferred_device.is_none()
            {
                let _ = events.send(AudioEvent::DeviceRouted(new_name));
            }
            return Ok(PlaybackExit::ReopenAt {
                path,
                position,
                paused,
            });
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
                AudioCommand::Seek(seconds) => match decoder.seek(seconds) {
                    Ok(()) => {
                        resampler.reset();
                        current_block.clear();
                        block_offset = 0;
                        decoder_finished = false;
                        stats.reset();
                        output
                            .played_frames
                            .store((seconds * output_rate as f64) as u64, Ordering::Relaxed);
                        output.seek_generation.fetch_add(1, Ordering::Release);
                        let _ = events.send(AudioEvent::Position(seconds));
                    }
                    Err(err) => {
                        let _ = events.send(AudioEvent::Error(format!("Seek failed: {err}")));
                    }
                },
                AudioCommand::SelectDevice(name) => {
                    *preferred_device = name;
                    let position =
                        output.played_frames.load(Ordering::Relaxed) as f64 / output_rate as f64;
                    visual_tap.set_active(false);
                    return Ok(PlaybackExit::ReopenAt {
                        path,
                        position,
                        paused,
                    });
                }
                command @ (AudioCommand::Play { .. } | AudioCommand::Shutdown) => {
                    visual_tap.set_active(false);
                    return Ok(PlaybackExit::Command(command));
                }
            }
            continue;
        }

        if paused {
            continue;
        }

        if last_position_update.elapsed() >= POSITION_UPDATE_INTERVAL {
            let position =
                output.played_frames.load(Ordering::Relaxed) as f64 / output_rate as f64;
            let _ = events.send(AudioEvent::Position(position));
            last_position_update = Instant::now();
        }

        if block_offset < current_block.len() {
            let pushed = producer.push_slice(&current_block[block_offset..]);
            block_offset += pushed;
            if pushed == 0 {
                thread::sleep(Duration::from_millis(2));
            }
            continue;
        }
        current_block.clear();
        block_offset = 0;
        if let Some(block) = resampler.next_block() {
            current_block = block;
            continue;
        }
        if decoder_finished {
            if producer.is_empty() {
                visual_tap.set_active(false);
                let _ = events.send(AudioEvent::Ended);
                return Ok(PlaybackExit::Ended);
            }
            thread::sleep(Duration::from_millis(2));
            continue;
        }

        let packet = match decoder.format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                resampler.finish();
                decoder_finished = true;
                continue;
            }
            Err(SymphoniaError::ResetRequired) => {
                decoder.decoder.reset();
                resampler.reset();
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if packet.track_id() != decoder.track_id {
            continue;
        }
        stats.push(packet.buf().len(), packet.dur(), events);
        match decoder.decoder.decode(&packet) {
            Ok(audio) => {
                packet_errors = 0;
                let spec = *audio.spec();
                let channels = spec.channels.count();
                let cap = audio.capacity();
                let buf = match &mut decode_buf {
                    Some((cur_cap, sb)) if *cur_cap >= cap => sb,
                    slot => {
                        *slot = Some((cap, SampleBuffer::<f32>::new(cap as u64, spec)));
                        &mut slot.as_mut().unwrap().1
                    }
                };
                buf.copy_interleaved_ref(audio);
                resampler.push_interleaved(buf.samples(), channels);
            }
            Err(SymphoniaError::DecodeError(_)) => {
                packet_errors += 1;
                if packet_errors >= MAX_PACKET_ERROR_STREAK {
                    return Err(anyhow!("Repeated decoder errors; playback stopped"));
                }
            }
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                resampler.finish();
                decoder_finished = true;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

struct OutputConfig {
    config: StreamConfig,
    format: SampleFormat,
}

#[allow(clippy::too_many_arguments)]
fn create_output_stream(
    device: &Device,
    source_rate: u32,
    volume: &Arc<AtomicU32>,
    linear_volume: &Arc<AtomicBool>,
    visual_tap: &Arc<VisualTap>,
    dsp: &Arc<SharedDspParams>,
    events: &Sender<AudioEvent>,
) -> Result<(OutputStream, HeapProd<f32>)> {
    let default = device.default_output_config()?;
    let preferred_config = device
        .supported_output_configs()
        .ok()
        .and_then(|ranges| {
            ranges
                .filter(|range| range.channels() >= 1)
                .filter(|range| {
                    source_rate >= range.min_sample_rate() && source_rate <= range.max_sample_rate()
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
                .map(|range| range.with_sample_rate(source_rate))
        })
        .unwrap_or(default);

    let primary = OutputConfig {
        config: StreamConfig {
            channels: preferred_config.channels(),
            sample_rate: preferred_config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        },
        format: preferred_config.sample_format(),
    };

    match open_with_config(device, &primary, volume, linear_volume, visual_tap, dsp, events) {
        Ok(res) => Ok(res),
        Err(primary_err) => {
            // Bluetooth A2DP speakers (e.g. Klipsch) often reject sample-rate switches and
            // require their active default hardware rate (resampled cleanly via SincFixedIn).
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
            open_with_config(device, &fallback, volume, linear_volume, visual_tap, dsp, events)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn open_with_config(
    device: &Device,
    output: &OutputConfig,
    volume: &Arc<AtomicU32>,
    linear_volume: &Arc<AtomicBool>,
    visual_tap: &Arc<VisualTap>,
    dsp: &Arc<SharedDspParams>,
    events: &Sender<AudioEvent>,
) -> Result<(OutputStream, HeapProd<f32>)> {
    let output_rate = output.config.sample_rate;
    let channels = output.config.channels as usize;
    let ring = HeapRb::<f32>::new(output_rate as usize * channels * BUFFER_SECONDS);
    let (producer, consumer) = ring.split();
    let played_frames = Arc::new(AtomicU64::new(0));
    let seek_generation = Arc::new(AtomicU64::new(0));
    let reopen_needed = Arc::new(AtomicBool::new(false));

    macro_rules! mk {
        ($ty:ty) => {
            build_stream::<$ty>(
                device,
                &output.config,
                consumer,
                volume,
                linear_volume,
                visual_tap,
                dsp,
                &played_frames,
                &seek_generation,
                &reopen_needed,
                events,
            )?
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
        format => return Err(anyhow!("Unsupported output sample format: {format}")),
    };
    Ok((
        OutputStream {
            stream,
            sample_rate: output_rate,
            channels,
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
    linear_volume: &Arc<AtomicBool>,
    visual_tap: &Arc<VisualTap>,
    dsp_params: &Arc<SharedDspParams>,
    played_frames: &Arc<AtomicU64>,
    seek_generation: &Arc<AtomicU64>,
    reopen_needed: &Arc<AtomicBool>,
    events: &Sender<AudioEvent>,
) -> Result<Stream> {
    let channels = config.channels as usize;
    let volume = Arc::clone(volume);
    let linear_volume = Arc::clone(linear_volume);
    let visual_tap = Arc::clone(visual_tap);
    let dsp_params = Arc::clone(dsp_params);
    let played_frames = Arc::clone(played_frames);
    let seek_generation = Arc::clone(seek_generation);
    let reopen_flag = Arc::clone(reopen_needed);
    let mut callback_generation = 0;
    let mut smoothed_gain = volume_to_gain(
        f32::from_bits(volume.load(Ordering::Relaxed)),
        linear_volume.load(Ordering::Relaxed),
    );
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
            let target_gain = volume_to_gain(
                f32::from_bits(volume.load(Ordering::Relaxed)),
                linear_volume.load(Ordering::Relaxed),
            );
            let mut sum_sq_l = 0.0_f32;
            let mut sum_sq_r = 0.0_f32;
            let mut sum_lr = 0.0_f32;
            let mut peak_l = 0.0_f32;
            let mut peak_r = 0.0_f32;
            let mut frame_count = 0_usize;

            for frame in data.chunks_mut(channels) {
                smoothed_gain += (target_gain - smoothed_gain) * 0.005;
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
                    let (proc_l, proc_r) = dsp.process_stereo(raw_l, raw_r);
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
                        *sample = T::from_float(out * smoothed_gain);
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
                    played_frames.fetch_add(1, Ordering::Relaxed);
                } else {
                    for sample in frame.iter_mut() {
                        *sample = T::from_float(0.0);
                    }
                    visual_tap.push_callback_frame(&mut write_cursor, 0.0, 0.0);
                }
            }

            if frame_count > 0 {
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
    fn new(source_rate: u32, output_rate: u32, channels: usize) -> Result<Self> {
        let channels = channels.max(1);
        let chunk_size = 1024;
        let resampler = if source_rate != output_rate {
            let parameters = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            Some(SincFixedIn::new(
                output_rate as f64 / source_rate as f64,
                2.0,
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
        if packet_channels == 0 {
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
}
