# TxPlayer Audiophile Roadmap

TxPlayer is a 100% free, native Rust cross-platform (`macOS`, `Windows`, `Linux`) audiophile music player engineered to combine the sonic purity of **Audirvāna / Roon / HQPlayer**, the archive mastery of **foobar2000**, and the instant responsiveness of **AIMP / Winamp** — with a unique 88-key Concert Grand Piano visualizer.

---

## Completed Foundation (`v0.2.0` Local Dev Builds)
- [x] **Dual Layout Architecture:** Unified Compact Mode (Winamp/AIMP-style mini-player + tabbed playlist drawer) & Classic Mode (full library browser, albums, artists, folders, lyrics, floating selection dock).
- [x] **Audirvāna-Inspired `Studio Reference` Theme** (+ 8 curated dark & light studio themes).
- [x] **64-Bit Internal DSP Engine:** Lock-free real-time 10-band biquad EQ, Bauer acoustic head-shadow crossfeed, soft-knee true-peak limiter, ReplayGain (Track & Album + Peak Guard), and Bit-Perfect Direct Bypass.
- [x] **Play-from-RAM Full-Track Memory Engine:** Pre-decodes tracks into RAM (256 MB – 2 GB cap) for zero disk I/O, instant seeking, and live VBR/CBR kbps telemetry.
- [x] **Audiophile Oversampling Engine:** Native 1:1, Synchronous Power-of-2 (`44.1→176.4`, `48→192 kHz`), and Max DAC Rate with selectable Rubato Sinc filter profiles (`Studio Linear`, `Sharp Precision`, `Gentle Analog`).
- [x] **Direct DAC Volume Lock (`0.0 dBFS` Pass-Through):** Bit-exact fixed digital output for external hardware DACs/amplifiers.
- [x] **5-Stage Signal Path Inspector:** Live vertical bit-path audit (`Source Decoder → Memory Engine → Oversampling & Tempo → DSP & ReplayGain → Hardware Output & Volume`).
- [x] **Pure-Rust Audio Quality & Hi-Res Authenticity Analyzer:** Dynamic Range (`DR` score), EBU R128 Integrated Loudness (`LUFS`), 4× True Peak (`dBTP`), clip counter, ultrasonic bandwidth check (`AUTHENTIC HI-RES` vs `SUSPECTED UPSCALE` vs `LOSSY CUTOFF`), and 64-bin spectral distribution graph with 22 kHz CD reference line.
- [x] **Concert Grand Piano Visualizer:** Multi-rate Constant-Q 88-key analyzer with acoustic harmonic-envelope protection, wide-dynamic `-64 dB` *ppp* sensitivity, unified 1:1 keybed press & atmospheric resonance plumes, and expressive *pp → mf → ff* dynamics shading.
- [x] **Playlist Power Pack:** Drag-and-drop track reordering (All Tracks, Queue, Playlists) with high-contrast insertion line, 40-step Undo/Redo, dedupe, prune missing, reverse, randomize, permanent sort, Stop After Current, Jump to Playing, and M3U8/PLS import/export.

---

## Wave 2 — Audiophile Archive & Format Mastery *(In Progress)*
- [x] **Pure-Rust DSD Playback (`.dsf` & `.dff` DSDIFF):** DSD64 / DSD128 / DSD256 / DSD512 decoding via 64-bit Blackman-Harris LUT FIR decimation to 88.2 / 176.4 kHz PCM + embedded ID3v2 tags & cover art extraction + Track Info Audio Analysis support.
- [x] **Bulletproof CUE + CD Image Mastery:**
  - Auto-decoding non-UTF-8 legacy `.cue` encodings (**UTF-16 LE/BE**, **Windows-1250 Central European**, **Windows-1252 Western**, **Windows-1251 Cyrillic**).
  - **Embedded FLAC CUESHEETs** (both Vorbis `CUESHEET` tag & binary FLAC `METADATA_BLOCK_CUESHEET`) — auto-expands single-file FLAC CD rips with no external `.cue`.
  - **Raw CD `.bin + .cue`** 44.1 kHz / 16-bit PCM image playback (`BINARY` little-endian & `MOTOROLA` big-endian).
  - `REM DISCNUMBER`, `REM COMPOSER`, case-insensitive filename fallback, and **Play-from-RAM for CUE tracks**.
- [ ] **Additional Archive Codecs:** WavPack (`.wv`), Monkey's Audio (`.ape`), Opus (`.opus`), and SACD `.iso` container inspection.

---

## Wave 3 — Hardware Purity & Headphone Mastery (`v1.1.0` Completed)
- [x] **True Exclusive DAC Mode & Hardware Auto Sample-Rate Switching:**
  - macOS CoreAudio Hog Mode (`kAudioDevicePropertyHogMode`) + native nominal rate switching (`kAudioDevicePropertyNominalSampleRate`) with RAII auto-restore.
  - Cross-platform graceful fallback & live hardware clock sync reporting in Signal Path Inspector.
- [x] **Per-Output Device Audio Profiles:** Automatically apply saved Exclusive Mode, `0.0 dB` DAC Volume Lock, Oversampling policy, Crossfeed, and Headphone AutoEQ profile whenever you switch between USB DAC, Headphones, and Built-in Speakers.
- [x] **Parametric EQ + 1-Click AutoEQ / Oratory1990 `.txt` Headphone Import:** Full 16-band 64-bit RBJ parametric biquads (Peak, Low/High Shelf, Low/High Pass with custom Frequency & Q), curated headphone targets (Harman, HD 650, HD 800 S, Sundara, Warm Tilt), live analytical 20 Hz – 20 kHz frequency-response curve preview, and 1-click AutoEQ `.txt` import.
- [ ] **Optional Native DSD DoP (DSD over PCM v1.1)** output (`0x05/0xFA` 24-bit markers at 176.4/352.8 kHz) when a DoP-capable Exclusive DAC is locked.

## Wave 3.5 — Decaud-Killer Audiophile Precision Pack (`v1.2.0` Completed)
- [x] **TPDF & Noise-Shaped Dithered Bit-Depth Reduction:** Off / 24-bit TPDF / 16-bit TPDF / 16-bit 2nd-order psychoacoustic noise-shaped TPDF dither in 64-bit DSP chain (bypassed bit-identically in Bit-Perfect mode, saved per-device profile).
- [x] **Named Classic Headphone Crossfeed Presets:** `Gentle`, `Chu Moy`, `Jan Meier`, and `Bauer Studio` acoustic head-shadow topologies with custom frequency & mix readout.
- [x] **Live Real-Time EBU R128 LUFS & 4× True Peak Metering:** Momentary (400 ms), Short-Term (3.0 s), and 4× cubic Catmull-Rom True Peak (`dBTP`) integrated directly into the Mastering VU stage.
- [x] **Top-Level Signal Path Verdict Banner:** Instant status verdict (`BIT-PERFECT EXCLUSIVE`, `BIT-PERFECT DIRECT`, `EXCLUSIVE LOSSLESS`, `STUDIO ENHANCED (64-BIT DSP)`, `OVERSAMPLED STUDIO`, `LOSSLESS NATIVE`) at the top of the 5-stage Signal Path Inspector.
- [x] **Batch Album TrueHz & Dynamic Range Authenticity Auditor:** 1-click album or multi-track verification queuing the background analyzer with aggregate Album DR, avg LUFS, max True Peak, and per-track Authentic Hi-Res / Lossless CD / Suspected Upscale / Lossy Cutoff table.

---

## Wave 4 — Classical Library & Power-User Excellence (`v1.3.0` Completed)
- [x] **Classical Multi-Disc Album Hero View:** Dedicated album header with large artwork, multi-disc & album ReplayGain badges, **`Disc 1` / `Disc 2`** visual track dividers, **Album Booklet (`.pdf`)** 1-click launcher, and **High-Res Cover & Inlay Scans Gallery** modal.
- [x] **Multi-Facet Live Audiophile Filter Chips:** Instant 1-click library filter chips (`All`, `Hi-Res`, `DSD`, `Lossless`, `★★★★+`) paired with real-time search.
- [x] **Batch Parallel EBU R128 / ReplayGain Scanner:** Right-click any album or selection (or click in Album Hero) to compute & persist Track + Album ReplayGain (`-18 LUFS` reference) & True Peak across CPU cores in pure Rust.
- [x] **Sleep Timer & Per-Track Bookmarks:** Configurable sleep timer (`15m`, `30m`, `45m`, `60m`, `End of Track`) with gentle 5-second volume fade-out and live countdown pill, plus persistent per-track timestamp bookmarks (`Cmd/Ctrl+B` or Signal Path popover) for long symphonies & concerts.
- [x] **Remember Custom Window Size & Screen Position:** Persists resized Compact and Classic window dimensions and screen coordinates cleanly across launches.

---

## Wave 5 — Master Library Database, Collections & Smart Views (`v1.4.0` Completed)
- [x] **Rich Relational Library Database & Schema Indexing:**
  - Persist full classical & audiophile metadata per track (`composer`, `conductor`, `orchestra`, `work`, `label`, `catalog_number`, `comment`, freeform `#custom-tags`, `last_played_at`) plus **persisted audio analysis results** (`dr_score`, `integrated_lufs`, `true_peak_dbtp`, `effective_bandwidth_hz`, `authenticity`) so DR/TrueHz audits live permanently in the database!
  - Automatic rolling backup (`libraries.json.bak`), schema versioning (v2), and **1-click Database Export / Backup** (full JSON backup + RFC-4180 CSV spreadsheet catalog).
- [x] **Curated Collections (Multi-Album / Multi-Track Boxes & Themes):**
  - Create named Collections with descriptions & 6 curated color badges (e.g. *Beethoven Complete Sonatas*, *Reference Audiophile Demos*, *Deutsche Grammophon Originals*, *Late-Night Piano*).
  - Right-click any track or multi-selection → **Add to Collection…**, with dedicated Collections section in the sidebar.
- [x] **Custom Saved Views & Visual Smart Query Builder:**
  - Build & save unlimited dynamic rule-based database views (`Match ALL` / `Match ANY`) over 16 fields (`Composer`, `Artist`, `Album`, `Title`, `Genre`, `Label`, `Codec`, `Custom Tag`, `Sample Rate`, `Bit Depth`, `DR Score`, `Rating`, `Play Count`, `Year`, `Has ReplayGain`, `TrueHz Verdict`) with live match-count preview & 1-click audiophile presets.
- [x] **Dedicated Classical & Facet Browsers + Library Insights Dashboard:**
  - **Composers** browser, **Genres** browser, and a **Library Database & Insights Dashboard** (format breakdown FLAC/DSD/ALAC/WAV, sample-rate distribution, total listening hours, average DR, verified Hi-Res share, ReplayGain coverage, and CSV/JSON export).
