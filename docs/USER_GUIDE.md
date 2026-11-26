# TxPlayer — Complete User Guide & Technical Reference (`v1.11.0`)

**TxPlayer** is a native Rust cross-platform (`macOS`, `Windows`, `Linux`) audiophile music player, real-time acoustic & 88-key concert piano visualizer, and relational master music library database. Zero Electron, zero WebView, zero cloud accounts, zero subscriptions — everything stays fast, local, and under your control.

---

## Table of Contents
1. [Audio Engine & Signal Path](#1-audio-engine--signal-path)
2. [Supported Audio, DSD & Archive Formats](#2-supported-audio-dsd--archive-formats)
3. [64-Bit Studio DSP Suite & Interactive EQ](#3-64-bit-studio-dsp-suite--interactive-eq)
4. [Concert Visualizer Suite (6 Modes)](#4-concert-visualizer-suite-6-modes)
5. [Master Library Database, Collections & Smart Views](#5-master-library-database-collections--smart-views)
6. [Power-Search Query Language](#6-power-search-query-language)
7. [Tag Editor, Online Lookup & Batch Tag Transforms](#7-tag-editor-online-lookup--batch-tag-transforms)
8. [Audiophile Quality Verification (TrueHz, DR, ReplayGain & SACD `.iso`)](#8-audiophile-quality-verification-truehz-dr-replaygain--sacd-iso)
9. [Critical Listening Tools (A–B Loop, Bookmarks, Sleep Timer, Local Radio)](#9-critical-listening-tools-ab-loop-bookmarks-sleep-timer-local-radio)
10. [Library Insights, Database Health & Portable Catalog Export](#10-library-insights-database-health--portable-catalog-export)
11. [Themes, Compact Mode & Customization](#11-themes-compact-mode--customization)
12. [Complete Keyboard Shortcuts & CLI Flags](#12-complete-keyboard-shortcuts--cli-flags)
13. [Data Locations, Backups & Troubleshooting](#13-data-locations-backups--troubleshooting)

---

## 1. Audio Engine & Signal Path

TxPlayer's audio pipeline runs on a dedicated lock-free decode thread feeding a 3-second lock-free `ringbuf` into the OS real-time audio callback (`CoreAudio` on macOS, `WASAPI` on Windows, `ALSA / PipeWire` on Linux).

### Signal Path Inspector
Click the live telemetry pill on the right side of the transport bar to open the **Signal Path Inspector**, which displays all 5 stages live along with a top-level fidelity verdict banner:
1. **Source & Container** — Codec, sample rate, bit depth, channels, and live VBR/CBR bitrate (plus `RAM Buffer` indicator when Play-from-RAM is active).
2. **Sample-Rate & Resampling Stage** — Native hardware match, synchronous power-of-2 oversampling (`2×`, `4×`, `Max DAC`), or Rubato Blackman-Harris Sinc resampling (`Fast`, `High`, `Ultra / Audiophile`).
3. **64-Bit DSP & ReplayGain Stage** — Shows active DSP modules or `Bypassed (Bit-Perfect Direct)`.
4. **Volume Stage** — Logarithmic perceptual curve, linear taper, or `0.0 dBFS Direct Lock`.
5. **Hardware Output** — Active device, wire sample format (`F32` / `I32`), Exclusive Hog Mode / HW Clock Sync status.

### Bit-Perfect & Exclusive DAC Modes
In **Settings → Audio & DSP**:
- **Bit-Perfect Mode:** Hard-bypasses all EQ, PEQ, Convolution, Crossfeed, Mastering Console, and Dither stages so PCM reaches the hardware unaltered.
- **Exclusive DAC Access:** Acquires CoreAudio Hog Mode & native hardware sample-rate switching on macOS (or Exclusive stream access where supported) so OS mixers never resample your audio.
- **Direct 0.0 dBFS Volume Lock:** Locks digital software volume to unity gain (`1.0`) when controlling volume downstream on a preamp or DAC.
- **Native DSD over PCM (DoP v1.1):** Streams 1-bit `DSD64` (`176.4 kHz`) and `DSD128` (`352.8 kHz`) encapsulated in standards-compliant `0x05`/`0xFA` 24-bit `I32` integer frames directly to DoP-compatible DACs. If the hardware doesn't expose `I32` DoP rates, TxPlayer automatically falls back to its pure-Rust 64-bit Blackman-Harris LUT FIR PCM decimator.
- **Gapless Playback & Crossfade:** Pure sample-accurate gapless album transitions by default (`0.0s`), or optional equal-power crossfade (`0.5s – 6.0s`).
- **Play from RAM:** Buffers full tracks into memory before playback to eliminate storage I/O jitter.
- **Per-Device Audio Profiles:** Save independent DSP/Bit-Perfect/Crossfeed configurations per output device. Switching from headphones to a USB DAC automatically restores the matching profile.

---

## 2. Supported Audio, DSD & Archive Formats

| Category | Formats | Notes |
| :--- | :--- | :--- |
| **Lossless PCM** | `FLAC`, `ALAC` (`.m4a`), `WAV`, `AIFF`, `CAF` | Pure-Rust SIMD Symphonia decoding, up to 32-bit / 384 kHz+ |
| **Native 1-Bit DSD** | `DSF` (`.dsf`), `DSDIFF` (`.dff`) | DSD64–DSD512 pure-Rust parser + ID3/artwork + live DoP v1.1 or 64-bit FIR PCM |
| **Archive Codecs** | **WavPack** (`.wv`), **Monkey's Audio** (`.ape`), **Opus** (`.opus`) | Pure-Rust container & APEv2/OpusTags header probers + seamless streaming decode, seeking, Play-from-RAM & DR/TrueHz analysis |
| **Lossy & Web** | `MP3`, `AAC` (`.m4a`), `OGG/Vorbis`, `MKV`, `WebM` | Native pure-Rust decoding |
| **Cue Sheets** | `.cue` (UTF-8, UTF-16, CP1250, Windows-1252, Shift-JIS) | Virtual multi-track slicing, `REM COMPOSER`, `REM DATE`, `REM GENRE`, `PERFORMER`, seamless gapless playback |
| **SACD Images** | `.iso` (Scarlet Book Master TOC) | Pure-Rust sector-510 `SACDMTOC` metadata inspector in Library Insights |
| **Playlists** | `.m3u`, `.m3u8`, `.pls` | Full import & export |

---

## 3. 64-Bit Studio DSP Suite & Interactive EQ

Every DSP processor runs in **64-bit floating-point (`f64`)** directly inside the real-time audio callback with zero heap allocations and zero mutex locks.

### Interactive Drag-to-Shape 10-Band Graphic EQ
- Located in **Settings → Audio & DSP** (`31 Hz, 62 Hz, 125 Hz, 250 Hz, 500 Hz, 1 kHz, 2 kHz, 4 kHz, 8 kHz, 16 kHz`, `Q = 1.414`, Direct Form II Transposed biquads).
- **Click & drag any of the 10 glowing frequency nodes** directly on the `20 Hz – 20 kHz` live response canvas to sculpt gain (`-12.0` to `+12.0 dB` in `0.5 dB` steps) with floating `Hz · dB` telemetry.
- **Double-click any node** to snap that band immediately back to `0.0 dB`.
- Includes 6 studio presets (`Flat`, `Classical`, `Jazz`, `Acoustic`, `Warm`, `Bass Boost`) plus custom curves.

### 16-Band Parametric EQ & AutoEQ / Oratory1990 Import
- Click **Import AutoEQ (.txt)** to load standard EqualizerAPO / AutoEQ / Oratory1990 headphone target curves (`Preamp`, `PK`, `LSC`, `HSC` filters) — plotted live together with the 10-band EQ on the frequency response canvas.
- Built-in reference headphone profiles included out of the box.

### Real-Time Partitioned FFT Stereo Convolution Engine (`.wav` IR)
- Lock-free pure-Rust 64-bit overlap-add partitioned FFT stereo convolver (`512`-sample blocks, up to `16,384` taps).
- **Built-in Acoustic Presets:** `Analog Warmth`, `Intimate Chamber`, `Mastering Air`.
- **Custom `.wav` Impulse Responses:** Click **Load .WAV IR…** to load any 16/24/32-bit integer or 32-bit float mono/stereo `.wav` IR (room correction, speaker/headphone EQ, or concert hall reverb) with automatic resampling to the active DAC rate, **Wet/Dry Mix (`0–100%`)**, and **IR Gain (`-12..+6 dB`)**.

### Headphone Crossfeed, Dither & Mastering Console
- **Named Crossfeed Presets:** `Gentle`, `Chu Moy`, `Jan Meier`, and `Bauer Studio` (700 Hz head-shadow acoustic cross-bleed for fatigue-free headphone listening).
- **Mastering Console:**
  - **L/R Balance:** Constant-power stereo pan (`-100% L` to `+100% R`; double-click to center).
  - **Mono Fold-Down (`0.5*(L+R)`):** Instant phase cancellation and mix-balance check.
  - **Ø Polarity Invert:** 180° absolute phase inversion.
  - **L ↔ R Swap:** Instant channel swap.
- **Output Dither:** `Off`, `TPDF 16-bit`, `TPDF 24-bit`, and `2nd-Order Noise-Shaped` dither.
- **Soft-Knee Limiter & Preamp:** Transparent `-0.7 dBFS` peak protection.
- **Instant DSP A/B Comparison (`D`):** Press `D` anytime to flip between active DSP (`A`) and bit-identical bypass (`B`).

---

## 4. Concert Visualizer Suite (6 Modes)

Press **`V`** to toggle the panoramic Concert Visualizer Stage, **`1`–`5`** to jump directly to a mode, or configure enabled modes & engine diagnostics (`ms/frame`) in **Settings → Plugins**.

1. **88-Key Piano Spectrum (`Precision CQT`):**
   - Analyzes a `16,384`-sample lock-free stereo tap across 3 Butterworth anti-aliased decimation stages (`1×`, `÷4`, `÷16`, auto-scaled for `88.2/96/192 kHz` Hi-Res streams) with Railsback piano stretch tuning and **10-voice iterative harmonic cancellation** (suppressing octave/fifth ghost overtones).
   - Dynamics-aware key illumination (`pp` deep tint → `mf` vivid accent → `ff` luminous gold), weighted key travel, and real-time chord & note readout (`Cm9 · C2 G2 Eb3 C4 G4`).
   - **Pause-Freeze Study Mode:** Pausing playback freezes pressed keys, waterfall, and chord badge for musical transcription.
2. **88-Key Concert Piano Roll:** Scrolling real-time note waterfall rising cleanly from the keybed.
3. **64-Band Spectrum RTA:** Logarithmic `27.5 Hz – 18 kHz` real-time analyzer with dB grid and peak-hold ticks.
4. **Live Acoustic Spectrogram (Sonogram):** Scrolling 2D time-frequency waterfall (`96` frames × `64` log bands) with acoustic thermal-gold heatmap, logarithmic frequency ticks (`100 Hz – 16 kHz`), and a live dominant **`PEAK Hz`** badge.
5. **Stereo Scope & Vectorscope:** Zero-crossing triggered stereo oscilloscope paired with an M/S phase goniometer.
6. **Mastering VU & Phase:** Segmented L/R RMS & peak VU meters, live **EBU R128 Momentary (`400 ms`) & Short-Term (`3 s`) LUFS**, **4× Oversampled True Peak (`dBTP`)**, and stereo **Phase Correlation (`-1.0 .. +1.0`)**.

---

## 5. Master Library Database, Collections & Smart Views

TxPlayer stores your entire catalog in an indexed relational **Library Database v2** (`libraries.json` with automatic rolling `libraries.json.bak` protection).

### Rich Classical & Archival Schema
Every track persists: `Title`, `Artist`, `Album`, `Album Artist`, `Composer`, `Conductor`, `Orchestra`, `Work / Opus`, `Record Label`, `Catalog #`, `Genre`, `Year`, `Track #`, `Disc #`, `Comment`, freeform `#custom-tags`, `Rating (1–5★)`, `Play Count`, `Last Played`, ReplayGain (`Track/Album dB & Peak`), and permanent audio audit metrics (`DR Score`, `Integrated LUFS`, `True Peak dBTP`, `Effective Bandwidth Hz`, `TrueHz Verdict`), plus **Collector Liner Notes** per album and a rolling **2,000-play Listening History** log.

### 9 Facet Browsers & Classical Multi-Disc Album Hero
- Browse by **Tracks · Albums · Artists · Composers · Works · Genres · Labels · Years · Folders**.
- Click any album to open the **Album Hero View**: large artwork, multi-disc `DISC 1` / `DISC 2` visual dividers, persisted **Album DR** badge, inline editable **Collector Liner Notes**, **Album Booklet (`.pdf`)** 1-click launcher, and **High-Res Scans Gallery** modal (auto-detects cover/inlay/booklet scans in the album folder).
- Use the **Live Audiophile Filter Chips** (`All | Hi-Res | DSD | Lossless | ★★★★+`) above the track list for instant 1-click filtering.

### Curated Collections & Smart Views
- **Collections:** Create themed multi-album boxes (e.g., *Beethoven Complete Sonatas*, *Reference Demos*, *ECM Late Night*) with custom descriptions and 6 accent colors. Drag tracks directly onto any sidebar Collection or right-click an album → **Add Album to Collection**.
- **Visual Smart Query Builder:** Create live dynamic views (`Match ALL` / `Match ANY`) over **16 fields** (`Composer`, `Artist`, `Album`, `Title`, `Genre`, `Label`, `Codec`, `Custom Tag`, `Sample Rate`, `Bit Depth`, `DR Score`, `Rating`, `Play Count`, `Year`, `Has ReplayGain`, `TrueHz Verdict`) with live match-count preview and 1-click audiophile presets.

---

## 6. Power-Search Query Language

Type plain text or combine any structured audiophile tokens right in the search bar (`⌘F` / `Ctrl+F`):

| Token | Meaning | Example |
| :--- | :--- | :--- |
| `composer:<text>` | Matches Composer field | `composer:chopin` |
| `artist:<text>` | Matches Artist or Album Artist | `artist:pollini` |
| `album:<text>` | Matches Album title | `album:nocturnes` |
| `work:<text>` | Matches Work / Opus field | `work:op.27` |
| `label:<text>` | Matches Record Label | `label:grammophon` |
| `genre:<text>` | Matches Genre | `genre:classical` |
| `year:<text>` | Matches Year | `year:1977` |
| `codec:<text>` | Matches Codec (`FLAC`, `DSD`, `ALAC`, `WV`, `APE`, `OPUS`) | `codec:dsd` |
| `dr>=N` / `dr<=N` / `dr=N` | Filter by persisted Dynamic Range score | `dr>=13` |
| `rate>=N` / `sr>=N` | Sample rate in kHz (e.g. `96` or `96000`) | `rate>=96` |
| `bits>=N` | Bit depth (`16`, `24`, `32`) | `bits>=24` |
| `rating>=N` | Star rating (`1`–`5`) | `rating>=4` |
| `#tag` | Matches custom hashtag | `#reference` |

**Combined example:** `composer:beethoven codec:flac rate>=96 dr>=12 #reference`

---

## 7. Tag Editor, Online Lookup & Batch Tag Transforms

Select one track or multi-select an entire album (`Shift+Click` / `⌘+Click`) and press **`⌘T` / `Ctrl+T`** to open the **Tag Editor**:

- **Full Classical & Collector Fields:** Edit `Title`, `Artist`, `Album`, `Album Artist`, `Year`, `Genre`, `Composer`, `Conductor`, `Orchestra`, `Work / Opus`, `Record Label`, `Catalog #`, `Comment`, `#Custom Tags`, `Track #`, and `Disc #`.
- **Online Metadata Matching:** Fast 256 KB audio-body hash opens the window instantly while full SHA-256 & optional Chromaprint compute lazily. Search **MusicBrainz**, **iTunes Search** (including 1-click **600 px Cover Art download**), **AcoustID**, and your local hash cache, then cherry-pick fields or click **Apply & Save** (`⌘Enter` / `Ctrl+Enter`).
- **Lossless Disk Writing:** When *Write tags to audio files* is checked, TxPlayer writes updated metadata cleanly via lossless stream copy (`ffmpeg -c copy`) without re-encoding a single audio sample.

### Batch Multi-Track Tag Transformation Tools
Located right inside the Tag Editor (**BATCH TRANSFORMS** bar) and executed off-thread across all selected tracks:
- **`Filename → Tags`:** Parses `01 - Artist - Title` or `01. Title` directly from filenames into `Track #`, `Artist`, and `Title`.
- **`Swap Artist ↔ Composer`:** Instantly fixes classical rips where the Composer was stored in the Artist tag.
- **`Strip Track #`:** Removes leading numeric prefixes (`01 - `, `02. `, `1-03 `) from Titles and captures the track number if missing.
- **`Smart Title Case`:** Normalizes Title & Album capitalization while preserving Roman numerals (`II`, `III`, `IV`, `VIII`), catalog abbreviations (`BWV`, `KV`, `RV`, `SACD`, `DSD`, `PCM`), and ensemble/label acronyms (`DG`, `ECM`, `LSO`, `BBC`).
- **`Auto-Number 1..N`:** Sequentially numbers selected tracks `1..N` in current order.
- **`Find → Replace All`:** Substring find-and-replace across Title, Artist, Album, Composer, and Work.
- **`+ #Tag All`:** Appends a `#custom-tag` to every selected track in one click.

---

## 8. Audiophile Quality Verification (TrueHz, DR, ReplayGain & SACD `.iso`)

- **Batch Album TrueHz / DR Authenticity Auditor:** Right-click any track or album → **Verify Album TrueHz / DR…** (or `⌘K` → `Verify Current Album TrueHz / DR`). TxPlayer analyzes ultrasonic spectral roll-off (`>22 kHz`) and official Dynamic Range (`DR`) across the album, flags **Authentic Hi-Res** vs. **CD Quality** vs. **Likely Upsampled**, and persists results permanently into the Library Database.
- **Inline `DR` Track Pills:** Analyzed tracks automatically display a color-coded **`DR`** badge (`DR14` gold for `≥11`, accent for `8–10`, warning red for `≤7`) right beside the Format pill in the track table.
- **Batch Parallel EBU R128 ReplayGain Scanner:** Right-click any album or selection → **Scan Album ReplayGain (EBU R128)…** to compute & persist Track + Album loudness gain (`-18 LUFS` reference) and True Peak across CPU cores in pure Rust.
- **1-Click Copy Audiophile Specs:** Right-click any track → **Copy Audiophile Specs** (or `⌘K` → `Copy Current Track Audiophile Specs`) to copy a formatted technical summary (`Title — Artist · Album [FLAC · 24-bit/96 kHz · DR13 · -16.4 LUFS · Verified TrueHz Hi-Res]`).
- **SACD `.iso` Disc Inspector:** Open **Library Insights → Inspect SACD .iso…** to probe Scarlet Book Master TOC (`SACDMTOC` sector 510) disc metadata and Stereo/Multichannel area flags.

---

## 9. Critical Listening Tools (A–B Loop, Bookmarks, Sleep Timer, Local Radio)

- **A–B Section Looper (`[` / `]` / `\`):** Press `[` to mark Loop Point A and `]` to mark Loop Point B for repeating difficult piano passages or mastering comparisons. An interactive golden **`A–B 01:12–01:28 ×`** badge appears in the transport bar; click it or press `\` to clear.
- **Variable Playback Speed:** Adjust playback speed (`0.75×`, `0.85×`, `1.0×`, `1.25×`, `1.5×`) inside the Signal Path popover.
- **Per-Track Timestamp Bookmarks (`⌘B` / `Ctrl+B`):** Drop persistent timestamp bookmarks for long symphonies, operas, or live concerts; jump back to any bookmark from the Signal Path popover.
- **Sleep Timer:** Set `15m`, `30m`, `60m`, or `End of Track` via Signal Path popover or `⌘K` — gently fades volume out over 5 seconds before pausing.
- **100% Local "Similar Radio":** Right-click any track → **Start Local Radio (Similar Tracks)** (or `⌘K`) to generate a 40-track offline queue ranked by Composer, Genre, Artist, shared `#Custom Tags`, Record Label, Decade, Hi-Res Tier, and Rating with same-album diversity scoring.

---

## 10. Library Insights, Database Health & Portable Catalog Export

Open **Library Database & Insights** from the sidebar or `⌘K`:
- **KPI Overview:** Tracks, Albums, Artists/Composers, Total Runtime, Hi-Res/DSD counts, Average DR, Verified Hi-Res ratio, ReplayGain coverage, Collections, and Smart Views.
- **Interactive Analytics Charts (Click Any Bar to Filter!):**
  - **Codec Breakdown**
  - **Sample Rate Distribution**
  - **Recording Decades Timeline**
  - **Dynamic Range (`DR`) Health Histogram**
  - **Top Composers & Top Record Labels**
  - **Recent Listening History**
- **Database Health — Duplicate Quality Audit & Missing-Files Cleaner:** Automatically detects duplicate recordings across formats/folders, ranks copies by fidelity (`DSD / Hi-Res > Lossless > Lossy`), marks the `BEST` master copy, and offers 1-click **Keep Best Quality** and **Prune Missing Files**.
- **Portable Catalog Export & Backups:**
  - **HTML Dossier (`.html`):** Self-contained, print-ready (`@media print`) dark/light audiophile catalog grouped by album with KPI cards, Collector Liner Notes, DR badges, LUFS, and `#tags`. Export the full library from Insights or the currently open Collection/Smart View/Playlist via `⌘K`!
  - **Markdown Catalog (`.md`):** Clean album-grouped Markdown tables + liner notes for Obsidian, GitHub, or Notion.
  - **Spreadsheet (`.csv`):** Full RFC-4180 catalog export (25 columns).
  - **Full Database Backup & Restore (`.json`):** Portable backup and 1-click restore of all libraries, playlists, collections, smart views, album notes, and play history.

---

## 11. Themes, Compact Mode & Customization

- **9 Curated Studio Themes** (`Settings → Appearance` or `⌘K`):
  1. `Obsidian Studio` — Velvet charcoal · Emerald & gold
  2. `Studio Reference` — Audirvāna-inspired carbon · Cobalt & gold
  3. `Tokyo Night` — Deep midnight indigo · Sapphire & violet
  4. `Tokyo Night Storm` — Twilight slate · Electric cyan
  5. `Solarized Dark` — Precision cyan · Warm amber
  6. `Solarized Light` — Warm parchment · Crisp teal & blue
  7. `Nord` — Arctic polar night · Frost & aurora
  8. `Catppuccin Mocha` — Warm espresso · Mauve & mint
  9. `Gruvbox Dark` — Retro studio · Warm gold & olive
- **3 Progress Scrubber Styles:** `Acoustic Waveform` (mirrored 160-bin silhouette), `Precision Bars`, and `Studio Rail`.
- **Compact Mini-Player Mode (`⌘⇧M` / `Ctrl+Shift+M`):** Collapses into a sleek desktop widget with expandable queue/playlist drawer, remembering both Classic and Compact window sizes & screen positions across launches.
- **System Tray & OS Media Controls:** Native macOS Now Playing, Windows SMTC, and Linux MPRIS2 integration plus optional Close-to-Tray.

---

## 12. Complete Keyboard Shortcuts & CLI Flags

### Keyboard Shortcuts

| Action | macOS | Windows / Linux |
| :--- | :---: | :---: |
| Command Palette (jump anywhere) | `⌘K` | `Ctrl+K` |
| Focus Power-Search | `⌘F` | `Ctrl+F` |
| Settings | `⌘,` | `Ctrl+,` |
| Play / Pause (freezes Piano Visualizer) | `Space` | `Space` |
| Mute / Unmute | `M` | `M` |
| Toggle Concert Visualizer Stage | `V` | `V` |
| Switch Visualizer Mode | `1`–`5` | `1`–`5` |
| Instant DSP A/B Comparison | `D` | `D` |
| Toggle Audio DSP On/Off | `E` | `E` |
| Toggle Synced Lyrics Drawer | `L` | `L` |
| Set A–B Loop Point A / Point B / Clear | `[` / `]` / `\` | `[` / `]` / `\` |
| Drop Timestamp Bookmark | `⌘B` | `Ctrl+B` |
| Jump to Playing Track | `⌘J` | `Ctrl+J` |
| Edit Tags & Batch Transforms | `⌘T` | `Ctrl+T` |
| Track Info & TrueHz/DR Inspector | `⌘I` | `Ctrl+I` |
| Select All Visible Tracks | `⌘A` | `Ctrl+A` |
| Undo / Redo Playlist Edit | `⌘Z` / `⌘⇧Z` | `Ctrl+Z` / `Ctrl+Y` |
| Toggle Compact Mini-Player Mode | `⌘⇧M` | `Ctrl+Shift+M` |
| Seek ±10 s | `←` / `→` | `←` / `→` |
| Previous / Next Track | `⌘←` / `⌘→` | `Ctrl+←` / `Ctrl+→` |
| Volume ±5% | `⌘↑` / `⌘↓` | `Ctrl+↑` / `Ctrl+↓` |
| Move Selection / Play Selected | `↑` `↓` / `Enter` | `↑` `↓` / `Enter` |
| Dismiss Modal / Popover / Selection | `Esc` | `Esc` |

### Command-Line Flags
```bash
txplayer [AUDIO_FILES_OR_FOLDERS...]
  --theme <name>          # e.g. solarized-dark, studio-reference, obsidian-studio, nord
  --vis-mode <mode>       # piano | roll | spectrum | spectrogram | scope | vu
  --open-settings <tab>   # appearance | visualizer | audio | plugins | library
  --compact | --full      # Force Compact Mini-Player or Full window layout
  --demo-piano            # Launch with frozen Chopin Cm9 piano voicing demo
```

---

## 13. Data Locations, Backups & Troubleshooting

- **Database & Settings Directory:** Open directly via **Library Insights → Open Database Folder**.
  - **macOS:** `~/Library/Application Support/txplayer/`
  - **Linux:** `~/.local/share/txplayer/` (or `$XDG_DATA_HOME/txplayer/`)
  - **Windows:** `%APPDATA%\txplayer\`
- **Rolling Safety Backup:** Every save rotates the previous database cleanly into `libraries.json.bak`.
- **Optional External Tools** (checked live in **Settings → Plugins → Tool Health**):
  - `ffmpeg` — Required only for lossless tag writing to audio files and streaming playback of archive codecs (`.wv`, `.ape`, `.opus`). Core PCM/FLAC/ALAC/WAV/AIFF/DSD/MP3/AAC/OGG playback and all DSP/DR/TrueHz/ReplayGain analysis are 100% pure Rust with zero external dependencies.
  - `fpcalc` (Chromaprint) — Optional acoustic fingerprinting for AcoustID tag lookup.
  - `curl` — Used for online MusicBrainz/iTunes metadata & cover art lookup.
- **macOS Gatekeeper (Manual `.dmg` Download):** Ad-hoc signed builds may show an unidentified-developer prompt on first launch. Drag `TxPlayer.app` to `/Applications`, then right-click → **Open**, or run `xattr -cr /Applications/TxPlayer.app`.
