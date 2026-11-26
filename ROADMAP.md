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
- [x] **Additional Archive Codecs:** WavPack (`.wv`), Monkey's Audio (`.ape`), Opus (`.opus`) pure-Rust container/APEv2/OpusTags probers + transparent lossless PCM playback & DR/LUFS analysis, and Scarlet Book SACD `.iso` container inspector.

---

## Wave 3 — Hardware Purity & Headphone Mastery (`v1.1.0` Completed)
- [x] **True Exclusive DAC Mode & Hardware Auto Sample-Rate Switching:**
  - macOS CoreAudio Hog Mode (`kAudioDevicePropertyHogMode`) + native nominal rate switching (`kAudioDevicePropertyNominalSampleRate`) with RAII auto-restore.
  - Cross-platform graceful fallback & live hardware clock sync reporting in Signal Path Inspector.
- [x] **Per-Output Device Audio Profiles:** Automatically apply saved Exclusive Mode, `0.0 dB` DAC Volume Lock, Oversampling policy, Crossfeed, and Headphone AutoEQ profile whenever you switch between USB DAC, Headphones, and Built-in Speakers.
- [x] **Parametric EQ + 1-Click AutoEQ / Oratory1990 `.txt` Headphone Import:** Full 16-band 64-bit RBJ parametric biquads (Peak, Low/High Shelf, Low/High Pass with custom Frequency & Q), curated headphone targets (Harman, HD 650, HD 800 S, Sundara, Warm Tilt), live analytical 20 Hz – 20 kHz frequency-response curve preview, and 1-click AutoEQ `.txt` import.
- [x] **Native DSD DoP (DSD over PCM v1.1) Framing Engine:** Pure-Rust `0x05`/`0xFA` 24-bit integer DoP v1.1 framer (`176.4 kHz` DSD64 / `352.8 kHz` DSD128 with automatic DSF LSB→MSB bit reversal) + settings toggle gated to Exclusive DAC Lock.

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

---

## Wave 6 — Classical Tagging, Database Health & Collector Polish (`v1.5.0` Completed)
- [x] **Full Classical Metadata & `#Custom Tags` Editor + Track Inspector:** View, edit, and write `Composer`, `Conductor`, `Orchestra`, `Work / Opus`, `Record Label`, `Catalog #`, `Comment`, and `#custom-tags` directly into both the Library Database and embedded audio file tags, plus live persisted DB DR/LUFS/TrueHz summary & 1-click `#tag` removal inside Track Info.
- [x] **Database Health — Duplicate Quality Audit & Missing-Files Cleaner:** Automatic duplicate group detection across formats/folders ranking copies by audio fidelity (`DSD / Hi-Res > Lossless > Lossy`), flagging `BEST` master copies with 1-click **Keep Best Quality (Remove Lower-Res Duplicates)** and **Prune Missing Files**.
- [x] **Record Labels Facet Browser, Drag-to-Collection & Database Restore:** Dedicated **Labels** browser (`Deutsche Grammophon`, `Decca`, `ECM`, etc.), direct drag-and-drop of tracks onto sidebar Collections, right-click **Add Album to Collection**, and 1-click **Restore Backup (`.json`)**.
- [x] **Pure-Rust DSD over PCM (DoP v1.1) Framing Engine:** Standards-compliant `0x05`/`0xFA` 24-bit integer framer (`176.4 kHz` / `352.8 kHz`) with DSF LSB→MSB bit reversal and Settings toggle.

---

## Wave 7 — Live DoP v1.1 DAC Transport, Listening History & Power-Query Database (`v1.6.0` Completed)
- [x] **Live End-to-End Native DSD over PCM (DoP v1.1) Audio Path:** Streams raw 1-bit DSD64 (`176.4 kHz`) & DSD128 (`352.8 kHz`) bit-identically via `DsdDopReader` into an `I32` Exclusive DAC stream with zero volume/DSP alteration, live visual sidechain, gold **`NATIVE DSD OVER PCM (DoP v1.1)`** Signal Path verdict banner, and automatic fallback to 64-bit Blackman-Harris LUT FIR PCM when hardware doesn't expose `I32` DoP rates.
- [x] **Persistent Listening History & `Recently Played` Smart View:** Logs every track play (`PlayHistoryEntry`, rolling 2,000 plays) directly in the Library Database, surfaced in the new **Recently Played** sidebar view and **Library Insights** listening log.
- [x] **Classical `Works / Opus` & `Years` Database Browsers:** Dedicated browse modes (`Tracks | Albums | Artists | Composers | Works | Genres | Labels | Years | Folders`) for deep classical & archival exploration.
- [x] **Audiophile Power-Search Query Syntax & Inline `#Tag` Entry:** Instant live search tokens (`composer:beethoven`, `dr>=12`, `rate>=96`, `bits>=24`, `codec:dsd`, `label:dg`, `work:27`, `rating>=4`, `#reference`) + inline `+ #tag` entry right inside Track Info.
- [x] **Pure-Rust SACD `.iso` Master TOC Prober:** Parses Scarlet Book `SACDMTOC` (sector 510) disc metadata & Stereo/Multichannel area flags with unit test coverage.

---

## Wave 8 — Archive Codecs, Album Liner Notes & Command Palette Mastery (`v1.7.0` Completed)
- [x] **Extended Archive Codecs (`.wv` WavPack, `.ape` Monkey's Audio, `.opus` Opus):** Pure-Rust container headers (`wvpk`, `MAC `, `OpusHead`/`OpusTags`), full APEv2 & VorbisComment classical tag/cover/ReplayGain extraction, plus seamless lossless streaming playback, seeking, Play-from-RAM, and DR/LUFS/TrueHz analysis.
- [x] **Persistent Album Collector Liner Notes & DB Average DR Badge:** Write & save permanent pressing/edition/listening notes on any album right inside the Album Hero header, paired with an instant persisted **`DR`** badge.
- [x] **SACD `.iso` Disc Inspector UI:** 1-click `Inspect SACD .iso…` inside Library Database & Insights probing Scarlet Book Master TOC metadata.
- [x] **Supercharged `Cmd/Ctrl+K` Command Palette:** Instant keyboard jump to any Collection, Smart View, Library Insights, Duplicate Auditor, Sleep Timer, Bookmark drop, or Current Album TrueHz/ReplayGain scan.

---

## Wave 9 — Studio Mastering DSP & Collector Intelligence (`v1.8.0` Completed)
- [x] **Real-Time Partitioned FFT Stereo Convolution Engine (`.wav` IR + Built-In Acoustic Presets):** Lock-free pure-Rust 64-bit overlap-add partitioned FFT stereo convolver (`512`-sample blocks, up to `16,384` taps) with built-in acoustic presets (`Analog Warmth`, `Intimate Chamber`, `Mastering Air`), custom stereo/mono `.wav` Impulse Response loader (16/24/32-bit int & float PCM with automatic sample-rate resampling), Wet/Dry Mix, and IR Gain trim — strictly bypassed in Bit-Perfect mode.
- [x] **Studio Mastering Console (`L/R Balance`, `Mono Fold-Down`, `Ø Polarity Invert`, `L ↔ R Swap`):** Constant-power stereo balance with double-click center reset plus instant mastering check switches directly in **Settings → Audio & DSP**.
- [x] **100% Local "Similar Radio" Queue Generator:** Zero-cloud Roon-Radio-style similarity ranker combining Composer, Genre, Artist, shared `#Custom Tags`, Record Label, Era/Decade, Hi-Res Tier, and Rating with same-album diversity scoring — accessible from Track right-click menus and `Cmd/Ctrl+K` Command Palette.
- [x] **Inline Persisted `DR` Quality Pill:** Displays color-coded Dynamic Range badges (`DR14` gold, `DR9` accent, `DR6` warning) right beside the Track Table Format pill whenever persisted audit scores exist in the Library Database.

---

## Wave 10 — Live Acoustic Spectrogram, Collector Analytics & Critical Listening Pack (`v1.9.0` Completed)
- [x] **Live Real-Time Acoustic Spectrogram / Sonogram (`VisualizerMode::Spectrogram`):** 6th flagship Concert Visualizer mode featuring a scrolling 2D time-frequency waterfall (`96` frames × `64` logarithmic frequency bands, `27.5 Hz – 18 kHz`) with an acoustic thermal-gold heatmap, logarithmic frequency grid (`100 Hz – 16 kHz`), and live peak frequency badge (`PEAK 440 Hz`).
- [x] **Interactive Collector Analytics & 1-Click Drill-Down in Library Insights:** Added chronological **Recording Decades Timeline**, **Dynamic Range (`DR`) Health Histogram**, **Top Composers**, and **Top Record Labels** bar charts inside a scrollable Insights dashboard where clicking any Codec, Sample Rate, DR tier, Composer, Label, or Decade row jumps directly to those filtered tracks.
- [x] **Instant Studio DSP A/B Comparison (`D` hotkey) & Active A–B Loop Transport Pill:** Press `D` (or via `Cmd/Ctrl+K`) to flip instantly between your active DSP chain and bit-identical bypass; active A–B section loops (`[` / `]` / `\`) now display a clickable golden `A–B 01:12–01:28 ×` transport badge and full Command Palette entries.
- [x] **1-Click Copy Track Audiophile Specs:** Right-click any track (or use `Cmd/Ctrl+K`) to copy a clean formatted audiophile summary (`Title — Artist · Album [FLAC · 24-bit/96 kHz · DR13 · -16.4 LUFS · Verified TrueHz Hi-Res]`) straight to the clipboard.

---

## Wave 11 — Batch Tag Transforms, Portable HTML/Markdown Listening Catalogs & Interactive EQ Canvas (`v1.10.0` Completed)
- [x] **Batch Multi-Track Tag Transformation Tools:** 1-click off-thread batch metadata transforms directly inside the Tag Editor (`Filename → Tags`, `Swap Artist ↔ Composer` for classical rips, `Strip Leading Track #`, `Smart Title Case` preserving `Op.`/`BWV`/Roman numerals, `Auto-Number 1..N`, `Find & Replace All`, and batch `+ #Tag All`) updating both the Library Database and audio files cleanly.
- [x] **Portable HTML & Markdown Listening Catalog Exporters:** Export your full Library, any Curated Collection, Smart View, or Playlist into a self-contained, print-ready (`@media print`) **HTML Audiophile Dossier** or clean **Markdown Listening Catalog** grouped by album with Collector Liner Notes, Record Labels, DR badges, LUFS, Verified TrueHz flags, and `#custom-tags` (via Library Insights or `Cmd/Ctrl+K`).
- [x] **Interactive Drag-to-Shape Multi-Band EQ Response Canvas:** Upgraded the `20 Hz – 20 kHz` live frequency response display in **Settings → Audio & DSP** with 10 directly draggable glowing frequency-band control nodes, translucent area shading, double-click `0.0 dB` reset, and floating `Hz · dB` telemetry pills for real-time audio sculpting.

---

## Wave 12 — Blind ABX Lab, Pitch-Locked Practice DSP, SACD `.iso` Playback, `F1` Help & LAN Mobile Remote (`v1.11.0` Completed)
- [x] **Built-In `F1` Quick Guide & Interactive Shortcuts Overlay + `1`–`6` Visualizer Keys:** Press `F1` anywhere for an in-app 4-tab manual (*5-Min First Steps*, *Keyboard Shortcuts*, *Power-Search Tokens*, *Audio, DoP & Practice*), plus `6` hotkey wired for the Live Acoustic Spectrogram.
- [x] **Loudness-Matched Blind ABX Comparison Lab (`src/abx.rs`):** Compare two masters (`A` / `B` / randomized `X`) seamlessly at identical timestamps with automatic EBU R128 Integrated LUFS attenuation (`0.1 dB` accuracy) and exact upper-tail binomial **`p-value`** statistics.
- [x] **Pitch-Preserving Practice Mode & Musical Tuning (`A=440 / 432 / 415 Hz` + `±12 st`):** Zero-allocation lock-free dual-tap Hann-windowed `StereoPitchShifter` inside `RealtimeDsp` keeping exact musical pitch during `0.75×–1.5×` practice slowdown, plus live semitone transpose and Baroque/Verdi concert tuning pills.
- [x] **Multi-Track SACD `.iso` Stereo Track Parser & Direct Uncompressed DSD64 Playback:** Upgrades Scarlet Book parser (`TWOCHTOC` / `SACDTRL1` / `SACDTRL2` / `SACDTTxt`) to import stereo SACD `.iso` tracks directly into the Library and decode uncompressed DSD sectors via `SacdIsoPcmDecoder`.
- [x] **100% Local LAN Mobile Remote Server (`src/remote.rs`):** Opt-in zero-cloud Wi-Fi HTTP server (`Settings → Plugins` or `Cmd/Ctrl+K`) serving a mobile-first dark studio Web Remote (Now Playing, Signal Path, Play/Pause, Prev/Next, Seek, Volume, DSP A/B, and Library Search/Play).
