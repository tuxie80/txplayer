use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, anyhow};

/// Audio format family handled by TxPlayer's pure-Rust audiophile decoder extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsdContainer {
    /// Sony DSF (`.dsf`) — block-interleaved (typically 4096 bytes/ch), LSB-first or MSB-first.
    Dsf,
    /// Philips DSDIFF (`.dff`) — byte-interleaved, MSB-first.
    Dff,
}

#[derive(Debug, Clone)]
pub struct DsdInfo {
    pub container: DsdContainer,
    pub sample_rate_hz: u32,
    pub channels: usize,
    /// Total 1-bit DSD samples per channel.
    pub total_dsd_samples: u64,
    pub data_offset: u64,
    pub data_len: u64,
    /// DSF block size per channel in bytes (0 for DFF byte-interleaved).
    pub block_size_per_ch: usize,
    /// True if each byte's bit 0 is earliest in time (standard DSF); false for MSB-first (DFF).
    pub lsb_first: bool,
    /// Optional byte offset of an embedded ID3v2 tag chunk.
    pub id3_offset: Option<u64>,
    pub id3_len: Option<u64>,
    /// Output PCM sample rate after 64-bit FIR decimation (88.2 / 176.4 / 352.8 kHz).
    pub pcm_sample_rate: u32,
    pub decimation_factor: usize,
    pub duration_secs: f64,
    pub dsd_label: &'static str,
}

#[derive(Debug, Default, Clone)]
pub struct DsdTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub artwork: Option<(Vec<u8>, String)>,
}

pub fn is_dsd_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("dsf") || e.eq_ignore_ascii_case("dff"))
}

pub fn is_raw_cd_bin(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("bin"))
}

/// Returns the DoP v1.1 PCM container sample rate (`176.4 kHz` for DSD64, `352.8 kHz` for DSD128),
/// or `None` if the DSD rate exceeds standard DoP v1.1 transport limits.
pub fn dop_pcm_sample_rate(dsd_rate_hz: u32) -> Option<u32> {
    if (2_500_000..=3_200_000).contains(&dsd_rate_hz)
        || (5_000_000..=6_400_000).contains(&dsd_rate_hz)
    {
        Some(dsd_rate_hz / 16) // DSD64 -> 176.4 kHz, DSD128 -> 352.8 kHz
    } else {
        None
    }
}

/// Pure-Rust DSD over PCM (DoP v1.1) 24-bit integer framer.
/// Packs pairs of 1-bit DSD bytes (MSB-first in time) into 32-bit left-aligned PCM words:
/// `[marker (0x05 / 0xFA) << 24 | msb_dsd << 16 | lsb_dsd << 8 | 0x00]`.
pub struct DopFramer {
    marker_toggle: bool,
    lsb_first_source: bool,
}

impl DopFramer {
    pub fn new(lsb_first_source: bool) -> Self {
        Self {
            marker_toggle: false,
            lsb_first_source,
        }
    }

    /// Packs 2 stereo DSD bytes per channel (`l0, l1`, `r0, r1`) into one stereo pair of DoP v1.1 `i32` samples.
    #[inline(always)]
    pub fn pack_stereo_frame(&mut self, l0: u8, l1: u8, r0: u8, r1: u8) -> (i32, i32) {
        let marker: u32 = if self.marker_toggle { 0xFA } else { 0x05 };
        self.marker_toggle = !self.marker_toggle;
        let fix = |b: u8| -> u32 {
            if self.lsb_first_source {
                b.reverse_bits() as u32
            } else {
                b as u32
            }
        };
        let wl = (marker << 24) | (fix(l0) << 16) | (fix(l1) << 8);
        let wr = (marker << 24) | (fix(r0) << 16) | (fix(r1) << 8);
        (wl as i32, wr as i32)
    }
}

/// Live streaming DSD over PCM (DoP v1.1) reader for stereo DSD64 (176.4 kHz) & DSD128 (352.8 kHz) files.
pub struct DsdDopReader {
    file: File,
    pub info: DsdInfo,
    pub dop_sample_rate: u32,
    framer: DopFramer,
    byte_cursor: u64,
    total_bytes_per_ch: u64,
    dsf_block_idx: Option<u64>,
    dsf_block_buf: Vec<u8>,
}

impl DsdDopReader {
    pub fn open(path: &Path) -> Result<Self> {
        let (info, _) = probe_dsd(path)?;
        if info.channels != 2 {
            return Err(anyhow!("DoP v1.1 passthrough requires stereo DSD (got {} ch)", info.channels));
        }
        let dop_sample_rate = dop_pcm_sample_rate(info.sample_rate_hz).ok_or_else(|| {
            anyhow!(
                "{} ({} Hz) exceeds DoP v1.1 transport rate; using 64-bit FIR PCM",
                info.dsd_label,
                info.sample_rate_hz
            )
        })?;
        let file = File::open(path)?;
        let total_bytes_per_ch = info
            .total_dsd_samples
            .div_ceil(8)
            .min(info.data_len / info.channels.max(1) as u64);
        let dsf_buf_len = if info.container == DsdContainer::Dsf {
            info.block_size_per_ch * info.channels
        } else {
            0
        };
        let framer = DopFramer::new(info.lsb_first);
        Ok(Self {
            file,
            info,
            dop_sample_rate,
            framer,
            byte_cursor: 0,
            total_bytes_per_ch,
            dsf_block_idx: None,
            dsf_block_buf: vec![0u8; dsf_buf_len],
        })
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        let dsd_sample = (seconds.max(0.0) * self.info.sample_rate_hz as f64).round() as u64;
        // Align to 2-byte DoP frame boundary (16 DSD bits per channel per DoP PCM frame)
        let target_byte = ((dsd_sample / 8) / 2) * 2;
        self.byte_cursor = target_byte.min(self.total_bytes_per_ch);
        self.dsf_block_idx = None;
        Ok(())
    }

    /// Reads up to `max_frames` stereo DoP v1.1 `i32` frames into `out_i32` (interleaved L, R)
    /// and a lightweight approximate float copy into `vis_pcm` for live visualizer/VU feedback.
    pub fn read_dop_frames(
        &mut self,
        max_frames: usize,
        out_i32: &mut Vec<i32>,
        vis_pcm: &mut Vec<f32>,
    ) -> Result<usize> {
        out_i32.clear();
        vis_pcm.clear();
        let remaining_bytes = self.total_bytes_per_ch.saturating_sub(self.byte_cursor) as usize;
        let frames = (remaining_bytes / 2).min(max_frames);
        if frames == 0 {
            return Ok(0);
        }
        out_i32.reserve(frames * 2);
        vis_pcm.reserve(frames * 2);

        match self.info.container {
            DsdContainer::Dsf => {
                let bsz = self.info.block_size_per_ch.max(1);
                let read_stereo_byte = |this: &mut Self| -> Result<(u8, u8)> {
                    let blk = this.byte_cursor / bsz as u64;
                    let off = (this.byte_cursor % bsz as u64) as usize;
                    if this.dsf_block_idx != Some(blk) {
                        let file_off = this.info.data_offset + blk * (bsz * 2) as u64;
                        this.file.seek(SeekFrom::Start(file_off))?;
                        let mut filled = 0;
                        while filled < this.dsf_block_buf.len() {
                            let n = this.file.read(&mut this.dsf_block_buf[filled..])?;
                            if n == 0 {
                                this.dsf_block_buf[filled..].fill(0x69);
                                break;
                            }
                            filled += n;
                        }
                        this.dsf_block_idx = Some(blk);
                    }
                    let l = this.dsf_block_buf[off];
                    let r = this.dsf_block_buf[bsz + off];
                    this.byte_cursor += 1;
                    Ok((l, r))
                };
                for _ in 0..frames {
                    let (l0, r0) = read_stereo_byte(self)?;
                    let (l1, r1) = read_stereo_byte(self)?;
                    let (wl, wr) = self.framer.pack_stereo_frame(l0, l1, r0, r1);
                    out_i32.push(wl);
                    out_i32.push(wr);
                    let vl = ((l0.count_ones() + l1.count_ones()) as f32 - 8.0) * 0.125;
                    let vr = ((r0.count_ones() + r1.count_ones()) as f32 - 8.0) * 0.125;
                    vis_pcm.push(vl.clamp(-1.0, 1.0));
                    vis_pcm.push(vr.clamp(-1.0, 1.0));
                }
            }
            DsdContainer::Dff => {
                let bytes_needed = frames * 4;
                let file_off = self.info.data_offset + self.byte_cursor * 2;
                self.file.seek(SeekFrom::Start(file_off))?;
                let mut raw = vec![0u8; bytes_needed];
                let mut filled = 0;
                while filled < raw.len() {
                    let n = self.file.read(&mut raw[filled..])?;
                    if n == 0 {
                        raw[filled..].fill(0x69);
                        break;
                    }
                    filled += n;
                }
                self.byte_cursor += (frames * 2) as u64;
                for &chunk in raw.as_chunks::<4>().0 {
                    let (l0, r0, l1, r1) = (chunk[0], chunk[1], chunk[2], chunk[3]);
                    let (wl, wr) = self.framer.pack_stereo_frame(l0, l1, r0, r1);
                    out_i32.push(wl);
                    out_i32.push(wr);
                    let vl = ((l0.count_ones() + l1.count_ones()) as f32 - 8.0) * 0.125;
                    let vr = ((r0.count_ones() + r1.count_ones()) as f32 - 8.0) * 0.125;
                    vis_pcm.push(vl.clamp(-1.0, 1.0));
                    vis_pcm.push(vr.clamp(-1.0, 1.0));
                }
            }
        }
        Ok(frames)
    }
}

/// Individual stereo track entry parsed from a Scarlet Book SACD `.iso` Stereo Area TOC.
#[derive(Debug, Clone)]
pub struct SacdIsoTrack {
    pub track_number: u32,
    pub title: String,
    pub start_lsn: u32,
    pub length_sectors: u32,
    pub duration_secs: f64,
}

/// Probed metadata & track list from a Scarlet Book SACD `.iso` Master TOC (sector 510) & Stereo Area TOC.
#[derive(Debug, Clone)]
pub struct SacdIsoInfo {
    pub album_title: String,
    pub album_artist: String,
    pub publisher: String,
    pub disc_year: u16,
    pub has_stereo_area: bool,
    pub has_multichannel_area: bool,
    pub stereo_area_lsn: u32,
    pub is_dst_compressed: bool,
    pub tracks: Vec<SacdIsoTrack>,
}

/// Pure-Rust SACD `.iso` container inspector (parses Scarlet Book `SACDMTOC` + Stereo `TWOCHTOC` / `SACDTRL1` / `SACDTRL2`).
pub fn probe_sacd_iso(path: &Path) -> Result<SacdIsoInfo> {
    const SECTOR_BYTES: u64 = 2048;
    const MASTER_TOC_OFFSET: u64 = 510 * SECTOR_BYTES;
    let mut file = File::open(path).with_context(|| format!("Opening SACD ISO {}", path.display()))?;
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    file.seek(SeekFrom::Start(MASTER_TOC_OFFSET))
        .context("Seeking to SACD Master TOC (sector 510)")?;
    let mut sec = [0u8; 2048];
    file.read_exact(&mut sec)
        .context("Reading SACD Master TOC sector")?;
    let mut info = parse_sacd_master_toc(&sec)?;

    // Parse Stereo 2-Channel Area TOC if present inside the image
    if info.stereo_area_lsn > 0 {
        let area_off = info.stereo_area_lsn as u64 * SECTOR_BYTES;
        if area_off + SECTOR_BYTES <= file_len && file.seek(SeekFrom::Start(area_off)).is_ok() {
            let mut area_hdr = [0u8; 2048];
            if file.read_exact(&mut area_hdr).is_ok() && &area_hdr[0..8] == b"TWOCHTOC" {
                let toc_sectors = u16::from_be_bytes([area_hdr[12], area_hdr[13]]).clamp(1, 32) as usize;
                let frame_format = area_hdr[64] & 0x07;
                info.is_dst_compressed = frame_format == 0;
                let track_count = area_hdr[71] as usize;
                let mut toc_buf = vec![0u8; toc_sectors * 2048];
                if file.seek(SeekFrom::Start(area_off)).is_ok()
                    && file.read_exact(&mut toc_buf).is_ok()
                {
                    info.tracks = parse_sacd_stereo_tracks(&toc_buf, track_count);
                }
            }
        }
    }
    Ok(info)
}

fn parse_sacd_master_toc(sec: &[u8]) -> Result<SacdIsoInfo> {
    if sec.len() < 2048 || &sec[0..8] != b"SACDMTOC" {
        return Err(anyhow!("Not a valid Scarlet Book SACD ISO (missing SACDMTOC signature)"));
    }
    let disc_year = u16::from_be_bytes([sec[100], sec[101]]);
    let area1_start = u32::from_be_bytes([sec[64], sec[65], sec[66], sec[67]]);
    let area2_start = u32::from_be_bytes([sec[72], sec[73], sec[74], sec[75]]);
    let read_cstr = |start: usize, max_len: usize| -> String {
        let end = (start + max_len).min(sec.len());
        let slice = &sec[start..end];
        let nul = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        String::from_utf8_lossy(&slice[..nul]).trim().to_string()
    };
    let album_title = read_cstr(128, 128);
    let album_artist = read_cstr(256, 128);
    let publisher = read_cstr(384, 128);
    Ok(SacdIsoInfo {
        album_title,
        album_artist,
        publisher,
        disc_year,
        has_stereo_area: area1_start > 0,
        has_multichannel_area: area2_start > 0,
        stereo_area_lsn: area1_start,
        is_dst_compressed: false,
        tracks: Vec::new(),
    })
}

fn parse_sacd_stereo_tracks(toc_buf: &[u8], track_count: usize) -> Vec<SacdIsoTrack> {
    if track_count == 0 {
        return Vec::new();
    }
    let n = track_count.min(255);
    let mut starts = vec![0u32; n];
    let mut lengths = vec![0u32; n];
    let mut durations = vec![0.0_f64; n];
    let mut titles: Vec<String> = (1..=n).map(|i| format!("SACD Track {i:02}")).collect();

    for sec in toc_buf.as_chunks::<2048>().0 {
        if &sec[0..8] == b"SACDTRL1" {
            for i in 0..n {
                let s_off = 8 + i * 4;
                let l_off = 8 + 255 * 4 + i * 4;
                if l_off + 4 <= sec.len() {
                    starts[i] = u32::from_be_bytes(sec[s_off..s_off + 4].try_into().unwrap());
                    lengths[i] = u32::from_be_bytes(sec[l_off..l_off + 4].try_into().unwrap());
                }
            }
        } else if &sec[0..8] == b"SACDTRL2" {
            for (i, dur_slot) in durations.iter_mut().enumerate().take(n) {
                // Scarlet Book SACDTRL2 stores 4-byte Track_Length (min, sec, frame, flags) at 8 + 255*4 + i*4
                let d_off = 8 + 255 * 4 + i * 4;
                if d_off + 4 <= sec.len() {
                    let mins = sec[d_off] as f64;
                    let secs = sec[d_off + 1] as f64;
                    let frames = sec[d_off + 2] as f64;
                    let total = mins * 60.0 + secs + frames / 75.0;
                    if total > 0.0 {
                        *dur_slot = total;
                    }
                }
            }
        } else if &sec[0..8] == b"SACDTTxt" {
            for (i, slot) in titles.iter_mut().enumerate().take(n) {
                let p_off = 8 + i * 2;
                if p_off + 2 <= sec.len() {
                    let text_off = u16::from_be_bytes([sec[p_off], sec[p_off + 1]]) as usize;
                    if text_off >= 8 && text_off < sec.len() {
                        let tail = &sec[text_off..];
                        let nul = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
                        let name = String::from_utf8_lossy(&tail[..nul]).trim().to_string();
                        if !name.is_empty() {
                            *slot = name;
                        }
                    }
                }
            }
        }
    }

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // Fallback duration estimate from sector byte count (DSD64 stereo = 705,600 bytes/s)
        let dur = if durations[i] > 0.0 {
            durations[i]
        } else {
            (lengths[i] as f64 * 2048.0) / 705_600.0
        };
        out.push(SacdIsoTrack {
            track_number: (i + 1) as u32,
            title: titles[i].clone(),
            start_lsn: starts[i],
            length_sectors: lengths[i],
            duration_secs: dur,
        });
    }
    out
}

/// Direct uncompressed 2-channel DSD64 sector reader for Scarlet Book SACD `.iso` tracks.
/// Decodes raw interleaved stereo DSD sector payloads cleanly into 88.2 kHz stereo `f32` PCM
/// using the exact same 64-tap Blackman-Harris LUT FIR decimator.
pub struct SacdIsoPcmDecoder {
    file: File,
    start_byte: u64,
    total_bytes: u64,
    cursor_byte: u64,
    fir: DsdFirLut,
    hist_l: [u8; 8],
    hist_r: [u8; 8],
}

impl SacdIsoPcmDecoder {
    pub fn open_range(path: &Path, start_lsn: u32, length_sectors: u32) -> Result<Self> {
        let mut file = File::open(path).with_context(|| format!("Opening SACD ISO {}", path.display()))?;
        let start_byte = start_lsn as u64 * 2048;
        let total_bytes = (length_sectors as u64 * 2048) & !1;
        file.seek(SeekFrom::Start(start_byte))?;
        Ok(Self {
            file,
            start_byte,
            total_bytes,
            cursor_byte: 0,
            fir: DsdFirLut::new(32, false), // SACD is MSB-first DSD64 -> 88.2 kHz PCM
            hist_l: [0x69; 8],
            hist_r: [0x69; 8],
        })
    }

    pub fn pcm_sample_rate(&self) -> u32 {
        88_200
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        let byte_off = ((seconds.max(0.0) * 705_600.0).round() as u64) & !1;
        self.cursor_byte = byte_off.min(self.total_bytes);
        self.file.seek(SeekFrom::Start(self.start_byte + self.cursor_byte))?;
        self.hist_l = [0x69; 8];
        self.hist_r = [0x69; 8];
        Ok(())
    }

    pub fn read_pcm_frames(&mut self, max_frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        out.clear();
        const BPC: usize = 4; // 32 bits / 8 = 4 bytes per channel per 88.2 kHz PCM frame
        const BYTES_PER_FRAME: usize = BPC * 2;
        let rem_bytes = self.total_bytes.saturating_sub(self.cursor_byte) as usize;
        let want_bytes = (max_frames * BYTES_PER_FRAME).min(rem_bytes);
        let aligned = (want_bytes / BYTES_PER_FRAME) * BYTES_PER_FRAME;
        if aligned == 0 {
            return Ok(0);
        }
        let mut raw = vec![0u8; aligned];
        let n = self.file.read(&mut raw)?;
        let usable = (n / BYTES_PER_FRAME) * BYTES_PER_FRAME;
        self.cursor_byte += usable as u64;
        let frames = usable / BYTES_PER_FRAME;
        out.reserve(frames * 2);
        for frame_bytes in raw[..usable].as_chunks::<BYTES_PER_FRAME>().0 {
            self.hist_l.copy_within(BPC..8, 0);
            self.hist_r.copy_within(BPC..8, 0);
            for k in 0..BPC {
                self.hist_l[8 - BPC + k] = frame_bytes[k * 2];
                self.hist_r[8 - BPC + k] = frame_bytes[k * 2 + 1];
            }
            let mut acc_l = 0.0_f64;
            let mut acc_r = 0.0_f64;
            for b in 0..8 {
                acc_l += self.fir.lut[b][self.hist_l[b] as usize];
                acc_r += self.fir.lut[b][self.hist_r[b] as usize];
            }
            out.push(acc_l.clamp(-1.0, 1.0) as f32);
            out.push(acc_r.clamp(-1.0, 1.0) as f32);
        }
        Ok(frames)
    }
}

/// Picks optimal integer decimation factor so DSD64 -> 88.2 kHz and DSD128/256/512 -> 176.4 kHz.
fn choose_dsd_decimation(dsd_rate: u32) -> (usize, u32, &'static str) {
    if dsd_rate >= 20_000_000 {
        (128, dsd_rate / 128, "DSD512")
    } else if dsd_rate >= 10_000_000 {
        (64, dsd_rate / 64, "DSD256")
    } else if dsd_rate >= 5_000_000 {
        (32, dsd_rate / 32, "DSD128")
    } else {
        (32, (dsd_rate / 32).max(44_100), "DSD64")
    }
}

/// Probes a `.dsf` or `.dff` file header and extracts embedded ID3v2 metadata & cover art.
pub fn probe_dsd(path: &Path) -> Result<(DsdInfo, DsdTags)> {
    let mut file =
        File::open(path).with_context(|| format!("Opening DSD file {}", path.display()))?;
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)
        .context("Reading DSD magic header")?;

    let info = match &magic {
        b"DSD " => parse_dsf_header(&mut file, file_len)?,
        b"FRM8" => parse_dff_header(&mut file, file_len)?,
        _ => return Err(anyhow!("Unrecognized DSD header in {}", path.display())),
    };

    let mut tags = DsdTags::default();
    if let Some(off) = info.id3_offset
        && off < file_len
        && file.seek(SeekFrom::Start(off)).is_ok()
    {
        let max_len = info
            .id3_len
            .unwrap_or_else(|| file_len.saturating_sub(off))
            .min(16 * 1024 * 1024) as usize;
        let mut raw = vec![0u8; max_len];
        if let Ok(n) = file.read(&mut raw) {
            raw.truncate(n);
            parse_id3v2(&raw, &mut tags);
        }
    }

    Ok((info, tags))
}

fn parse_dsf_header(file: &mut File, file_len: u64) -> Result<DsdInfo> {
    // Remaining 24 bytes of 28-byte DSD chunk
    let mut hdr = [0u8; 24];
    file.read_exact(&mut hdr).context("Reading DSF header")?;
    let chunk_size = u64::from_le_bytes(hdr[0..8].try_into().unwrap());
    let total_file_size = u64::from_le_bytes(hdr[8..16].try_into().unwrap());
    let id3_ptr = u64::from_le_bytes(hdr[16..24].try_into().unwrap());
    if chunk_size < 28 {
        return Err(anyhow!("Invalid DSF DSD chunk size"));
    }
    if chunk_size > 28 {
        file.seek(SeekFrom::Start(chunk_size))?;
    }

    // Read `fmt ` chunk (52 bytes)
    let mut fmt = [0u8; 52];
    file.read_exact(&mut fmt).context("Reading DSF fmt chunk")?;
    if &fmt[0..4] != b"fmt " {
        return Err(anyhow!("Missing DSF fmt chunk"));
    }
    let fmt_size = u64::from_le_bytes(fmt[4..12].try_into().unwrap());
    let format_id = u32::from_le_bytes(fmt[16..20].try_into().unwrap());
    if format_id != 0 {
        return Err(anyhow!("Unsupported DSF format ID {format_id} (expected raw DSD)"));
    }
    let channels = u32::from_le_bytes(fmt[24..28].try_into().unwrap()).clamp(1, 8) as usize;
    let sample_rate = u32::from_le_bytes(fmt[28..32].try_into().unwrap());
    let bits_per_sample = u32::from_le_bytes(fmt[32..36].try_into().unwrap());
    let sample_count = u64::from_le_bytes(fmt[36..44].try_into().unwrap());
    let block_size = u32::from_le_bytes(fmt[44..48].try_into().unwrap()) as usize;
    if sample_rate < 1_000_000 || block_size == 0 || block_size > 1_048_576 {
        return Err(anyhow!(
            "Invalid DSF parameters (rate={sample_rate}, block={block_size})"
        ));
    }

    let data_hdr_pos = chunk_size + fmt_size;
    file.seek(SeekFrom::Start(data_hdr_pos))?;
    let mut data_hdr = [0u8; 12];
    file.read_exact(&mut data_hdr)
        .context("Reading DSF data chunk")?;
    if &data_hdr[0..4] != b"data" {
        return Err(anyhow!("Missing DSF data chunk"));
    }
    let data_chunk_size = u64::from_le_bytes(data_hdr[4..12].try_into().unwrap());
    let data_offset = data_hdr_pos + 12;
    let data_len = data_chunk_size
        .saturating_sub(12)
        .min(file_len.saturating_sub(data_offset));

    let (decim, pcm_sr, dsd_label) = choose_dsd_decimation(sample_rate);
    let duration_secs = sample_count as f64 / sample_rate as f64;
    let id3_offset = (id3_ptr > 0 && id3_ptr < file_len).then_some(id3_ptr);
    let id3_len = id3_offset.map(|off| total_file_size.max(file_len).saturating_sub(off));

    Ok(DsdInfo {
        container: DsdContainer::Dsf,
        sample_rate_hz: sample_rate,
        channels,
        total_dsd_samples: sample_count,
        data_offset,
        data_len,
        block_size_per_ch: block_size,
        lsb_first: bits_per_sample == 1,
        id3_offset,
        id3_len,
        pcm_sample_rate: pcm_sr,
        decimation_factor: decim,
        duration_secs,
        dsd_label,
    })
}

fn parse_dff_header(file: &mut File, file_len: u64) -> Result<DsdInfo> {
    let mut top = [0u8; 12];
    file.read_exact(&mut top).context("Reading DFF FRM8 header")?;
    let form_size = u64::from_be_bytes(top[0..8].try_into().unwrap());
    if &top[8..12] != b"DSD " {
        return Err(anyhow!("DFF container is not a DSD form"));
    }
    let end_pos = (12 + form_size).min(file_len);
    let mut pos = 16u64;

    let mut sample_rate = 2_822_400_u32;
    let mut channels = 2usize;
    let mut data_offset = None;
    let mut data_len = 0u64;
    let mut id3_offset = None;
    let mut id3_len = None;

    while pos + 12 <= end_pos {
        file.seek(SeekFrom::Start(pos))?;
        let mut ck = [0u8; 12];
        if file.read_exact(&mut ck).is_err() {
            break;
        }
        let cid = &ck[0..4];
        let csize = u64::from_be_bytes(ck[4..12].try_into().unwrap());
        let body_start = pos + 12;
        let next_pos = body_start.saturating_add(csize).saturating_add(csize & 1);

        match cid {
            b"PROP" => {
                let mut prop_type = [0u8; 4];
                if file.read_exact(&mut prop_type).is_ok() && &prop_type == b"SND " {
                    let mut p = body_start + 4;
                    let prop_end = (body_start + csize).min(end_pos);
                    while p + 12 <= prop_end {
                        file.seek(SeekFrom::Start(p))?;
                        let mut sub = [0u8; 12];
                        if file.read_exact(&mut sub).is_err() {
                            break;
                        }
                        let sid = &sub[0..4];
                        let ssize = u64::from_be_bytes(sub[4..12].try_into().unwrap());
                        let sbody = p + 12;
                        match sid {
                            b"FS  " if ssize >= 4 => {
                                let mut b = [0u8; 4];
                                if file.read_exact(&mut b).is_ok() {
                                    sample_rate = u32::from_be_bytes(b);
                                }
                            }
                            b"CHNL" if ssize >= 2 => {
                                let mut b = [0u8; 2];
                                if file.read_exact(&mut b).is_ok() {
                                    channels = u16::from_be_bytes(b).clamp(1, 8) as usize;
                                }
                            }
                            b"CMPR" if ssize >= 4 => {
                                let mut b = [0u8; 4];
                                if file.read_exact(&mut b).is_ok() && &b != b"DSD " {
                                    let name = String::from_utf8_lossy(&b);
                                    return Err(anyhow!(
                                        "Compressed DSDIFF ({name}) is not supported; uncompressed DSD required"
                                    ));
                                }
                            }
                            _ => {}
                        }
                        p = sbody.saturating_add(ssize).saturating_add(ssize & 1);
                    }
                }
            }
            b"DSD " => {
                data_offset = Some(body_start);
                data_len = csize.min(file_len.saturating_sub(body_start));
            }
            b"ID3 " => {
                id3_offset = Some(body_start);
                id3_len = Some(csize.min(file_len.saturating_sub(body_start)));
            }
            _ => {}
        }
        if next_pos <= pos {
            break;
        }
        pos = next_pos;
    }

    let data_offset = data_offset.ok_or_else(|| anyhow!("Missing DSD audio chunk in DFF"))?;
    let total_dsd_samples = (data_len / channels.max(1) as u64) * 8;
    let (decim, pcm_sr, dsd_label) = choose_dsd_decimation(sample_rate);
    let duration_secs = total_dsd_samples as f64 / sample_rate.max(1) as f64;

    Ok(DsdInfo {
        container: DsdContainer::Dff,
        sample_rate_hz: sample_rate,
        channels,
        total_dsd_samples,
        data_offset,
        data_len,
        block_size_per_ch: 0,
        lsb_first: false,
        id3_offset,
        id3_len,
        pcm_sample_rate: pcm_sr,
        decimation_factor: decim,
        duration_secs,
        dsd_label,
    })
}

/// 64-tap Blackman-Harris linear-phase FIR decimation filter with 8-byte LUT tables.
/// Rejects DSD ultrasonic quantization noise by >95 dB while preserving flat Hi-Res passband.
struct DsdFirLut {
    /// `lut[byte_pos][byte_value]` for 8 consecutive DSD bytes (= 64 1-bit samples).
    lut: Box<[[f64; 256]; 8]>,
}

impl DsdFirLut {
    fn new(decimation_factor: usize, lsb_first: bool) -> Self {
        const TAPS: usize = 64;
        // Cutoff at ~0.39 of output PCM Nyquist (e.g. ~34 kHz at 88.2 kHz PCM) for clean noise floor
        let fc = 0.39 / (decimation_factor as f64).max(8.0);
        let mut h = [0.0_f64; TAPS];
        let mid = (TAPS - 1) as f64 * 0.5;
        let mut sum = 0.0_f64;
        for (i, slot) in h.iter_mut().enumerate() {
            let x = i as f64 - mid;
            let sinc = if x.abs() < 1e-12 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
            };
            let t = i as f64 / (TAPS - 1) as f64;
            let w = 0.35875
                - 0.48829 * (2.0 * std::f64::consts::PI * t).cos()
                + 0.14128 * (4.0 * std::f64::consts::PI * t).cos()
                - 0.01168 * (6.0 * std::f64::consts::PI * t).cos();
            let v = sinc * w;
            *slot = v;
            sum += v;
        }
        // Normalize to unity DC gain + gentle +3.5 dB SACD modulation headroom recovery (1.49x)
        let norm = 1.49 / sum.max(1e-12);
        for v in &mut h {
            *v *= norm;
        }

        let mut lut = Box::new([[0.0_f64; 256]; 8]);
        for (byte_pos, table) in lut.iter_mut().enumerate() {
            for (val, slot) in table.iter_mut().enumerate() {
                let mut acc = 0.0_f64;
                for bit in 0..8 {
                    let is_one = if lsb_first {
                        ((val >> bit) & 1) != 0
                    } else {
                        ((val >> (7 - bit)) & 1) != 0
                    };
                    let sign = if is_one { 1.0 } else { -1.0 };
                    acc += sign * h[byte_pos * 8 + bit];
                }
                *slot = acc;
            }
        }
        Self { lut }
    }
}

/// Streaming pure-Rust DSD (`.dsf` / `.dff`) to 32-bit float PCM decoder with exact seeking.
pub struct DsdPcmDecoder {
    file: File,
    pub info: DsdInfo,
    lut: DsdFirLut,
    /// Rolling 8-byte (64-bit) shift register per channel for FIR convolution.
    hist: Vec<[u8; 8]>,
    /// Current per-channel byte index into the DSD stream.
    byte_cursor: u64,
    total_bytes_per_ch: u64,
    /// Cached DSF block buffer (`channels * block_size_per_ch`).
    dsf_block_idx: Option<u64>,
    dsf_block_buf: Vec<u8>,
}

impl DsdPcmDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let (info, _) = probe_dsd(path)?;
        let file = File::open(path)?;
        let lut = DsdFirLut::new(info.decimation_factor, info.lsb_first);
        let total_bytes_per_ch = info.total_dsd_samples.div_ceil(8).min(
            info.data_len / info.channels.max(1) as u64,
        );
        let dsf_buf_len = if info.container == DsdContainer::Dsf {
            info.block_size_per_ch * info.channels
        } else {
            0
        };
        Ok(Self {
            file,
            hist: vec![[0x69; 8]; info.channels], // 0x69 = DSD silence pattern (alternating 0/1)
            lut,
            byte_cursor: 0,
            total_bytes_per_ch,
            dsf_block_idx: None,
            dsf_block_buf: vec![0u8; dsf_buf_len],
            info,
        })
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        let dsd_sample = (seconds.max(0.0) * self.info.sample_rate_hz as f64).round() as u64;
        let step_bytes = (self.info.decimation_factor / 8).max(1) as u64;
        let target_byte = ((dsd_sample / 8) / step_bytes) * step_bytes;
        self.byte_cursor = target_byte.min(self.total_bytes_per_ch);
        self.hist.fill([0x69; 8]);
        self.dsf_block_idx = None;
        Ok(())
    }

    /// Decodes up to `max_pcm_frames` interleaved `f32` PCM frames into `out`.
    /// Returns the number of PCM frames decoded (`0` on EOF).
    pub fn read_pcm_frames(&mut self, max_pcm_frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        out.clear();
        let ch = self.info.channels;
        let step_bytes = (self.info.decimation_factor / 8).max(1);
        let remaining_bytes = self.total_bytes_per_ch.saturating_sub(self.byte_cursor) as usize;
        let frames = (remaining_bytes / step_bytes).min(max_pcm_frames);
        if frames == 0 {
            return Ok(0);
        }
        out.reserve(frames * ch);

        match self.info.container {
            DsdContainer::Dsf => {
                let bsz = self.info.block_size_per_ch;
                for _ in 0..frames {
                    for _s in 0..step_bytes {
                        let blk = self.byte_cursor / bsz as u64;
                        let off_in_blk = (self.byte_cursor % bsz as u64) as usize;
                        if self.dsf_block_idx != Some(blk) {
                            let file_off =
                                self.info.data_offset + blk * (bsz * ch) as u64;
                            self.file.seek(SeekFrom::Start(file_off))?;
                            let mut filled = 0;
                            while filled < self.dsf_block_buf.len() {
                                let n = self.file.read(&mut self.dsf_block_buf[filled..])?;
                                if n == 0 {
                                    self.dsf_block_buf[filled..].fill(0x69);
                                    break;
                                }
                                filled += n;
                            }
                            self.dsf_block_idx = Some(blk);
                        }
                        for c in 0..ch {
                            let byte = self.dsf_block_buf[c * bsz + off_in_blk];
                            let h = &mut self.hist[c];
                            h.rotate_left(1);
                            h[7] = byte;
                        }
                        self.byte_cursor += 1;
                    }
                    for h in &self.hist[..ch] {
                        let mut acc = 0.0_f64;
                        for (pos, &b) in h.iter().enumerate() {
                            acc += self.lut.lut[pos][b as usize];
                        }
                        out.push(acc.clamp(-1.0, 1.0) as f32);
                    }
                }
            }
            DsdContainer::Dff => {
                let bytes_needed = frames * step_bytes * ch;
                let file_off = self.info.data_offset + self.byte_cursor * ch as u64;
                self.file.seek(SeekFrom::Start(file_off))?;
                let mut raw = vec![0u8; bytes_needed];
                let mut filled = 0;
                while filled < raw.len() {
                    let n = self.file.read(&mut raw[filled..])?;
                    if n == 0 {
                        raw[filled..].fill(0x69);
                        break;
                    }
                    filled += n;
                }
                let mut idx = 0usize;
                for _ in 0..frames {
                    for _s in 0..step_bytes {
                        for h in &mut self.hist[..ch] {
                            let byte = raw[idx];
                            idx += 1;
                            h.rotate_left(1);
                            h[7] = byte;
                        }
                        self.byte_cursor += 1;
                    }
                    for h in &self.hist[..ch] {
                        let mut acc = 0.0_f64;
                        for (pos, &b) in h.iter().enumerate() {
                            acc += self.lut.lut[pos][b as usize];
                        }
                        out.push(acc.clamp(-1.0, 1.0) as f32);
                    }
                }
            }
        }
        Ok(frames)
    }
}

/// Raw Red-Book CD-DA `.bin` image decoder (44.1 kHz, 16-bit signed stereo PCM).
pub struct RawCdBinDecoder {
    file: File,
    pub total_frames: u64,
    frame_cursor: u64,
    big_endian: bool,
}

impl RawCdBinDecoder {
    pub fn open(path: &Path, big_endian: bool) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("Opening raw CD image {}", path.display()))?;
        let bytes = file.metadata()?.len();
        Ok(Self {
            file,
            total_frames: bytes / 4,
            frame_cursor: 0,
            big_endian,
        })
    }

    pub fn duration_secs(&self) -> f64 {
        self.total_frames as f64 / 44_100.0
    }

    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        let frame = (seconds.max(0.0) * 44_100.0).round() as u64;
        self.frame_cursor = frame.min(self.total_frames);
        self.file.seek(SeekFrom::Start(self.frame_cursor * 4))?;
        Ok(())
    }

    pub fn read_pcm_frames(&mut self, max_frames: usize, out: &mut Vec<f32>) -> Result<usize> {
        out.clear();
        let rem = self.total_frames.saturating_sub(self.frame_cursor) as usize;
        let take = rem.min(max_frames);
        if take == 0 {
            return Ok(0);
        }
        let mut buf = vec![0u8; take * 4];
        let n = self.file.read(&mut buf)?;
        let actual_frames = n / 4;
        out.reserve(actual_frames * 2);
        let inv = 1.0_f32 / 32768.0;
        for &chunk in buf[..actual_frames * 4].as_chunks::<2>().0 {
            let s = if self.big_endian {
                i16::from_be_bytes(chunk)
            } else {
                i16::from_le_bytes(chunk)
            };
            out.push(s as f32 * inv);
        }
        self.frame_cursor += actual_frames as u64;
        Ok(actual_frames)
    }
}

/// Minimal pure-Rust ID3v2.3 / ID3v2.4 parser for embedded `.dsf` / `.dff` metadata & cover art.
fn parse_id3v2(data: &[u8], out: &mut DsdTags) {
    if data.len() < 10 || &data[0..3] != b"ID3" {
        return;
    }
    let ver = data[3];
    if ver != 3 && ver != 4 {
        return;
    }
    let flags = data[5];
    let tag_size = syncsafe_u32(&data[6..10]) as usize;
    let end = (10 + tag_size).min(data.len());
    let mut pos = 10usize;
    // Skip optional extended header
    if (flags & 0x40) != 0 && pos + 4 <= end {
        let ext_sz = if ver == 4 {
            syncsafe_u32(&data[pos..pos + 4]) as usize
        } else {
            u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize + 4
        };
        pos = pos.saturating_add(ext_sz);
    }

    while pos + 10 <= end {
        let fid = &data[pos..pos + 4];
        if fid == [0, 0, 0, 0] {
            break;
        }
        let fsize = if ver == 4 {
            syncsafe_u32(&data[pos + 4..pos + 8]) as usize
        } else {
            u32::from_be_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize
        };
        let body_start = pos + 10;
        let body_end = body_start.saturating_add(fsize).min(end);
        if fsize == 0 || body_end <= body_start {
            break;
        }
        let body = &data[body_start..body_end];
        match fid {
            b"TIT2" => out.title = decode_id3_text(body),
            b"TPE1" => out.artist = decode_id3_text(body),
            b"TALB" => out.album = decode_id3_text(body),
            b"TPE2" => out.album_artist = decode_id3_text(body),
            b"TRCK" => {
                out.track_number = decode_id3_text(body)
                    .and_then(|s| s.split('/').next()?.trim().parse().ok());
            }
            b"TPOS" => {
                out.disc_number = decode_id3_text(body)
                    .and_then(|s| s.split('/').next()?.trim().parse().ok());
            }
            b"TYER" | b"TDRC" => {
                if out.year.is_none() {
                    out.year = decode_id3_text(body).map(|s| s.chars().take(4).collect());
                }
            }
            b"TCON" => out.genre = decode_id3_text(body),
            b"APIC" if out.artwork.is_none() => {
                if let Some(art) = parse_id3_apic(body) {
                    out.artwork = Some(art);
                }
            }
            _ => {}
        }
        pos = body_end;
    }
}

fn syncsafe_u32(b: &[u8]) -> u32 {
    ((b[0] as u32 & 0x7f) << 21)
        | ((b[1] as u32 & 0x7f) << 14)
        | ((b[2] as u32 & 0x7f) << 7)
        | (b[3] as u32 & 0x7f)
}

fn decode_id3_text(body: &[u8]) -> Option<String> {
    if body.len() < 2 {
        return None;
    }
    let enc = body[0];
    let payload = &body[1..];
    let s = match enc {
        0 => payload
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect::<String>(),
        1 | 2 => decode_utf16(payload, enc == 2),
        3 => {
            let end = payload.iter().position(|&b| b == 0).unwrap_or(payload.len());
            String::from_utf8_lossy(&payload[..end]).into_owned()
        }
        _ => String::from_utf8_lossy(payload).into_owned(),
    };
    let trimmed = s.trim_matches('\0').trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn decode_utf16(bytes: &[u8], default_be: bool) -> String {
    let (be, slice) = if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        (true, &bytes[2..])
    } else if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        (false, &bytes[2..])
    } else {
        (default_be, bytes)
    };
    let mut u16s = Vec::with_capacity(slice.len() / 2);
    for &ch in slice.as_chunks::<2>().0 {
        let code = if be {
            u16::from_be_bytes(ch)
        } else {
            u16::from_le_bytes(ch)
        };
        if code == 0 {
            break;
        }
        u16s.push(code);
    }
    String::from_utf16_lossy(&u16s)
}

fn parse_id3_apic(body: &[u8]) -> Option<(Vec<u8>, String)> {
    if body.len() < 6 {
        return None;
    }
    let enc = body[0];
    let rest = &body[1..];
    let mime_end = rest.iter().position(|&b| b == 0)?;
    let mime = String::from_utf8_lossy(&rest[..mime_end]).to_string();
    // Skip null terminator + 1 byte picture type
    let desc_start = mime_end + 2;
    if desc_start >= rest.len() {
        return None;
    }
    let desc = &rest[desc_start..];
    let img_rel = if enc == 1 || enc == 2 {
        desc.as_chunks::<2>()
            .0
            .iter()
            .position(|&c| c == [0, 0])
            .map(|i| i * 2 + 2)?
    } else {
        desc.iter().position(|&b| b == 0).map(|i| i + 1)?
    };
    let img_data = &desc[img_rel..];
    if img_data.len() < 16 {
        return None;
    }
    Some((
        img_data.to_vec(),
        if mime.is_empty() {
            "image/jpeg".to_string()
        } else {
            mime
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthesizes a minimal valid 2-channel DSD64 `.dsf` file with a 1 kHz 1st-order sigma-delta sine wave
    /// and verifies probing, ID3v2 tag extraction, and clean 88.2 kHz PCM decimation.
    #[test]
    fn dsf_probe_id3_and_decimation_roundtrip() {
        let dir = std::env::temp_dir().join(format!("txplayer_dsf_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_tone.dsf");

        let dsd_sr = 2_822_400_u32;
        let block_sz = 4096usize;
        let dsd_samples = (block_sz * 8) as u64; // 1 block per channel (~11.6 ms)
        // Generate 1st-order sigma-delta modulated 1 kHz tone (LSB-first)
        let mut ch0 = vec![0u8; block_sz];
        let mut err = 0.0_f64;
        for s in 0..(block_sz * 8) {
            let target = 0.45 * (2.0 * std::f64::consts::PI * 1000.0 * s as f64 / dsd_sr as f64).sin();
            let bit = if target >= err { 1u8 } else { 0u8 };
            let quant = if bit == 1 { 1.0 } else { -1.0 };
            err += quant - target;
            ch0[s / 8] |= bit << (s % 8);
        }
        let ch1 = ch0.clone();

        // Build minimal ID3v2.3 tag with TIT2="DSD Test"
        let tit2 = b"\x03DSD Test";
        let mut id3 = Vec::new();
        id3.extend_from_slice(b"ID3\x03\x00\x00");
        let id3_body_len = 10 + tit2.len();
        id3.extend_from_slice(&[
            ((id3_body_len >> 21) & 0x7f) as u8,
            ((id3_body_len >> 14) & 0x7f) as u8,
            ((id3_body_len >> 7) & 0x7f) as u8,
            (id3_body_len & 0x7f) as u8,
        ]);
        id3.extend_from_slice(b"TIT2");
        id3.extend_from_slice(&(tit2.len() as u32).to_be_bytes());
        id3.extend_from_slice(&[0, 0]);
        id3.extend_from_slice(tit2);

        let data_chunk_sz = 12 + (block_sz * 2) as u64;
        let id3_ptr = 28 + 52 + data_chunk_sz;
        let total_sz = id3_ptr + id3.len() as u64;

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"DSD ");
        bytes.extend_from_slice(&28u64.to_le_bytes());
        bytes.extend_from_slice(&total_sz.to_le_bytes());
        bytes.extend_from_slice(&id3_ptr.to_le_bytes());
        // fmt chunk (52 bytes)
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&52u64.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes()); // format version
        bytes.extend_from_slice(&0u32.to_le_bytes()); // DSD raw
        bytes.extend_from_slice(&2u32.to_le_bytes()); // stereo
        bytes.extend_from_slice(&2u32.to_le_bytes()); // channels
        bytes.extend_from_slice(&dsd_sr.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes()); // LSB-first
        bytes.extend_from_slice(&dsd_samples.to_le_bytes());
        bytes.extend_from_slice(&(block_sz as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        // data chunk
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_chunk_sz.to_le_bytes());
        bytes.extend_from_slice(&ch0);
        bytes.extend_from_slice(&ch1);
        bytes.extend_from_slice(&id3);
        std::fs::write(&path, &bytes).unwrap();

        let (info, tags) = probe_dsd(&path).unwrap();
        assert_eq!(info.dsd_label, "DSD64");
        assert_eq!(info.pcm_sample_rate, 88_200);
        assert_eq!(tags.title.as_deref(), Some("DSD Test"));

        let mut dec = DsdPcmDecoder::open(&path).unwrap();
        let mut pcm = Vec::new();
        let frames = dec.read_pcm_frames(2048, &mut pcm).unwrap();
        assert_eq!(frames, 1024);
        let peak = pcm.iter().copied().fold(0.0_f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.3 && peak <= 1.0, "Expected clean audio peak, got {peak}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dop_v11_framer_alternates_markers_and_reverses_lsb_dsf_bits() {
        assert_eq!(dop_pcm_sample_rate(2_822_400), Some(176_400));
        assert_eq!(dop_pcm_sample_rate(5_644_800), Some(352_800));
        assert_eq!(dop_pcm_sample_rate(11_289_600), None);

        // DSF is LSB-first, so 0b0000_0011 -> reversed MSB-first 0b1100_0000 (0xC0)
        let mut framer = DopFramer::new(true);
        let (l_a, r_a) = framer.pack_stereo_frame(0b0000_0011, 0b1000_0000, 0xAA, 0x55);
        assert_eq!((l_a as u32) >> 24, 0x05);
        assert_eq!(((l_a as u32) >> 16) & 0xFF, 0xC0);
        assert_eq!(((l_a as u32) >> 8) & 0xFF, 0x01);
        assert_eq!((l_a as u32) & 0xFF, 0x00);
        assert_eq!((r_a as u32) >> 24, 0x05);

        let (l_b, r_b) = framer.pack_stereo_frame(0xFF, 0x00, 0x0F, 0xF0);
        assert_eq!((l_b as u32) >> 24, 0xFA);
        assert_eq!((r_b as u32) >> 24, 0xFA);
        assert_eq!(((r_b as u32) >> 16) & 0xFF, 0xF0);
        assert_eq!(((r_b as u32) >> 8) & 0xFF, 0x0F);
    }

    #[test]
    fn sacd_iso_master_toc_parser_extracts_disc_metadata() {
        let mut sec = [0u8; 2048];
        sec[0..8].copy_from_slice(b"SACDMTOC");
        sec[64..68].copy_from_slice(&600u32.to_be_bytes()); // stereo TOC start
        sec[72..76].copy_from_slice(&12000u32.to_be_bytes()); // multichannel TOC start
        sec[100..102].copy_from_slice(&2003u16.to_be_bytes());
        sec[128..148].copy_from_slice(b"Goldberg Variations\0");
        sec[256..268].copy_from_slice(b"Glenn Gould\0");
        sec[384..398].copy_from_slice(b"Sony Classical");
        let info = parse_sacd_master_toc(&sec).unwrap();
        assert_eq!(info.album_title, "Goldberg Variations");
        assert_eq!(info.album_artist, "Glenn Gould");
        assert_eq!(info.publisher, "Sony Classical");
        assert_eq!(info.disc_year, 2003);
        assert!(info.has_stereo_area && info.has_multichannel_area);
    }

    #[test]
    fn sacd_iso_multitrack_parser_and_uncompressed_dsd_decoder_roundtrip() {
        let dir = std::env::temp_dir().join("txplayer_sacd_iso_test");
        let _ = std::fs::create_dir_all(&dir);
        let iso_path = dir.join("synthetic.iso");

        // Build minimal valid 520-sector Scarlet Book ISO:
        // sector 510: SACDMTOC pointing stereo area to sector 512
        // sector 512: TWOCHTOC (4 sectors long, uncompressed DSD frame_format=2, 2 tracks)
        // sector 513: SACDTRL1 (track 1 @ LSN 516 len 2, track 2 @ LSN 518 len 2)
        // sector 514: SACDTRL2 (durations 3m12s and 4m05s)
        // sector 515: SACDTTxt (track titles)
        // sector 516..520: uncompressed DSD64 audio bytes
        let mut img = vec![0u8; 520 * 2048];
        let mtoc = &mut img[510 * 2048..511 * 2048];
        mtoc[0..8].copy_from_slice(b"SACDMTOC");
        mtoc[64..68].copy_from_slice(&512u32.to_be_bytes());
        mtoc[100..102].copy_from_slice(&1981u16.to_be_bytes());
        mtoc[128..140].copy_from_slice(b"Test SACD\0\0\0");

        let atoc = &mut img[512 * 2048..513 * 2048];
        atoc[0..8].copy_from_slice(b"TWOCHTOC");
        atoc[12..14].copy_from_slice(&4u16.to_be_bytes());
        atoc[64] = 2; // uncompressed DSD
        atoc[71] = 2; // 2 stereo tracks

        let trl1 = &mut img[513 * 2048..514 * 2048];
        trl1[0..8].copy_from_slice(b"SACDTRL1");
        trl1[8..12].copy_from_slice(&516u32.to_be_bytes());
        trl1[12..16].copy_from_slice(&518u32.to_be_bytes());
        trl1[8 + 255 * 4..12 + 255 * 4].copy_from_slice(&2u32.to_be_bytes());
        trl1[12 + 255 * 4..16 + 255 * 4].copy_from_slice(&2u32.to_be_bytes());

        let trl2 = &mut img[514 * 2048..515 * 2048];
        trl2[0..8].copy_from_slice(b"SACDTRL2");
        trl2[8 + 255 * 4] = 3;
        trl2[9 + 255 * 4] = 12;
        trl2[12 + 255 * 4] = 4;
        trl2[13 + 255 * 4] = 5;

        let ttxt = &mut img[515 * 2048..516 * 2048];
        ttxt[0..8].copy_from_slice(b"SACDTTxt");
        ttxt[8..10].copy_from_slice(&64u16.to_be_bytes());
        ttxt[10..12].copy_from_slice(&96u16.to_be_bytes());
        ttxt[64..74].copy_from_slice(b"Aria Da C\0");
        ttxt[96..108].copy_from_slice(b"Variation 1\0");

        // Fill track 1 sectors (516..518) with alternating 1-bit PDM +0.5 wave
        for b in &mut img[516 * 2048..518 * 2048] {
            *b = 0xAA;
        }
        std::fs::write(&iso_path, &img).unwrap();

        let info = probe_sacd_iso(&iso_path).unwrap();
        assert!(!info.is_dst_compressed);
        assert_eq!(info.tracks.len(), 2);
        assert_eq!(info.tracks[0].title, "Aria Da C");
        assert_eq!(info.tracks[0].start_lsn, 516);
        assert!((info.tracks[0].duration_secs - 192.0).abs() < 0.01);
        assert_eq!(info.tracks[1].title, "Variation 1");
        assert!((info.tracks[1].duration_secs - 245.0).abs() < 0.01);

        let mut dec = SacdIsoPcmDecoder::open_range(&iso_path, 516, 2).unwrap();
        assert_eq!(dec.pcm_sample_rate(), 88_200);
        let mut pcm = Vec::new();
        let frames = dec.read_pcm_frames(512, &mut pcm).unwrap();
        assert_eq!(frames, 512);
        assert_eq!(pcm.len(), 1024);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
