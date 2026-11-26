# TxPlayer — Quick Start & First Steps (`v1.10.0`)

Welcome to **TxPlayer** — a 100% free, native Rust audiophile music player, real-time 88-key concert piano & acoustic visualizer, and offline master music library database for **macOS, Windows, and Linux**.

This 5-minute guide walks you through the exact first steps to unlock TxPlayer's full audiophile and collector potential.

---

## Step 1 — Load Your Music Library (30 seconds)

1. Launch **TxPlayer** and drag & drop any music folder directly onto the window — or click **`+ Folder`** in the sidebar (or press `⌘K` / `Ctrl+K` → `Add Music Directory…`).
2. TxPlayer recursively scans lossless, Hi-Res, DSD, and archive formats (`FLAC`, `ALAC`, `WAV`, `AIFF`, `CAF`, `DSF`, `DFF`, `WV`, `APE`, `OPUS`, `MP3`, `AAC/M4A`, `OGG`, plus `.cue` multi-track sheets with automatic UTF-8/CP1250/Shift-JIS detection).
3. **Auto-Watch Folders** (enabled in **Settings → Library & Tags**) automatically detects newly added albums on disk.
4. Want separate worlds (e.g., *Classical Archive*, *Jazz Hi-Res*, *Testing Masters*)? Open **Libraries** in the sidebar to create completely isolated databases.

---

## Step 2 — Unlock True Bit-Perfect & Exclusive DAC Playback (1 minute)

Click the **Signal Path telemetry pill** in the bottom-right transport bar (or press `⌘,` / `Ctrl+,` → **Audio & DSP**):

1. **Select your USB DAC or Studio Interface** using the output selector.
2. **Choose your listening philosophy:**
   - **Pure Bit-Perfect DAC Path:** Enable **Bit-Perfect Mode** + **Exclusive DAC Access** (CoreAudio Hog Mode + hardware clock switching on macOS; WASAPI Exclusive on Windows) + **Direct 0.0 dBFS Volume Lock** when using an external amplifier. Every DSP filter is hard-bypassed so samples reach your DAC bit-for-bit identically.
   - **Native DSD over PCM (DoP v1.1):** Enable **Native DSD over PCM (DoP)** to stream 1-bit `DSD64` (`176.4 kHz`) and `DSD128` (`352.8 kHz`) bit-identically to DoP-capable DACs with a gold **`NATIVE DSD OVER PCM (DoP v1.1)`** Signal Path verdict. If your DAC lacks native `I32` DoP support, TxPlayer seamlessly falls back to its 64-bit Blackman-Harris LUT FIR PCM decimator.
   - **Play from RAM:** Pre-buffers entire tracks into memory for zero disk-I/O playback.
   - **Per-Device Profiles:** Save a dedicated Audio/DSP profile per output device (e.g., *Bit-Perfect* for your desktop DAC, *AutoEQ + Bauer Crossfeed* for your headphones) — TxPlayer automatically switches profiles whenever you switch outputs!

---

## Step 3 — Shape Your Sound with 64-Bit Studio DSP (1 minute)

When listening on headphones or tuning room acoustics, open **Settings → Audio & DSP**:

1. **Interactive Drag-to-Shape EQ Curve:** Click and drag any of the 10 glowing frequency nodes (`31 Hz – 16 kHz`) directly on the live frequency-response canvas to sculpt your sound in real time. Double-click any node to snap back to `0.0 dB`.
2. **Import AutoEQ / Oratory1990 Profiles:** Click **Import AutoEQ (.txt)** to load up to 16-band parametric headphone correction curves (or pick built-in studio presets like *Sennheiser HD 650*, *Focal Clear*, *HiFiMAN Sundara*, *AirPods Max*).
3. **Real-Time FFT Stereo Convolution (`.wav` IR):** Try **Analog Warmth**, **Intimate Chamber**, or **Mastering Air**, or click **Load .WAV IR…** to load custom room-correction or acoustic impulse responses (`64-bit` partitioned FFT, up to `16,384` taps).
4. **Headphone Crossfeed & Mastering Console:** Enable **Bauer Studio / Jan Meier / Chu Moy** crossfeed for natural soundstage imaging, tweak **L/R Balance**, or check mixes with **Mono**, **Ø Polarity**, and **L ↔ R Swap**.
5. **Instant DSP A/B Test:** Press **`D`** at any time during playback to flip instantaneously between your active DSP chain (`A`) and bit-identical bypass (`B`).

---

## Step 4 — Experience the 6 Concert Visualizers (1 minute)

Press **`V`** to open the panoramic **Concert Visualizer Stage**, then press **`1`–`5`** (or click the compact visualizer / double-click the stage) to cycle modes:

1. **88-Key Piano Spectrum (`Precision CQT`):** Real-time multi-rate Constant-Q analyzer with 10-voice harmonic cancellation, weighted keybed travel, dynamics-aware coloring (`pp → ff`), and live chord readout. **Press `Space` to Pause — pressed keys & detected chords freeze** so you can study exact voicings!
2. **88-Key Piano Roll:** Scrolling real-time note waterfall rising off the grand piano keybed.
3. **64-Band Spectrum RTA:** Logarithmic `27.5 Hz – 18 kHz` real-time analyzer with peak hold.
4. **Live Acoustic Spectrogram (Sonogram):** Scrolling 2D time-frequency thermal heatmap with logarithmic frequency ticks (`100 Hz – 16 kHz`) and live dominant `PEAK Hz` readout.
5. **Stereo Scope & Vectorscope:** Zero-crossing triggered stereo oscilloscope paired with an M/S phase goniometer.
6. **Mastering VU & Phase:** Segmented stereo VU meters + live **EBU R128 Momentary (M) / Short-Term (S) LUFS**, **4× True Peak (dBTP)**, and stereo phase correlation.

---

## Step 5 — Master Your Music Database & Collector Tools (1.5 minutes)

TxPlayer isn't just a player — it's an offline **Master Music Library Database**:

- **Audiophile Power-Search (`⌘F` / `Ctrl+F`):** Combine instant tokens right in the search bar:
  ```text
  composer:beethoven dr>=12 rate>=96 #reference
  label:grammophon codec:flac rating>=4
  work:27 bits>=24
  ```
- **9 Browse Facets:** Switch effortlessly between `Tracks | Albums | Artists | Composers | Works | Genres | Labels | Years | Folders`.
- **Curated Collections & Smart Views:** Group multi-album box sets into color-coded **Collections** (drag tracks straight onto sidebar collections), or build live rule-based **Smart Views** (`Match ALL` / `Match ANY` across 16 fields).
- **Verify Hi-Res Authenticity & Dynamic Range (`TrueHz / DR`):** Right-click any album → **Verify Album TrueHz / DR…** to detect upsampled fake Hi-Res vs. authentic ultrasonic content and persist **`DR`** badges (`DR14` gold, `DR9` accent, `DR6` warning) straight into your database and track list.
- **Scan ReplayGain (EBU R128):** Right-click any album → **Scan Album ReplayGain (EBU R128)…** for parallel pure-Rust loudness leveling.
- **Classical Batch Tag Cleanup (`⌘T` / `Ctrl+T`):** Select an entire album → open **Tag Editor** → use 1-click **Batch Transforms** (`Filename → Tags`, `Swap Artist ↔ Composer`, `Strip Track #`, `Smart Title Case`, `Auto-Number 1..N`, `Find & Replace All`, `+ #Tag All`).
- **Start 100% Local Radio:** Right-click any track → **Start Local Radio (Similar Tracks)** (or `⌘K` → `Start Local Radio`) for an offline queue ranked by composer, tags, genre, label, era, and fidelity.
- **Export Your Catalog:** Open **Library Insights** (or `⌘K`) to inspect interactive **Codec, Sample Rate, Decade, DR Health, Top Composers, and Top Labels** charts (click any bar to filter!), or export a print-ready **HTML Audiophile Dossier (`.html`)**, **Markdown Catalog (`.md`)**, or **Spreadsheet (`.csv`)**.

---

## Essential Keyboard Shortcuts Cheat Sheet

| Shortcut (macOS / Win·Linux) | Action |
| :--- | :--- |
| `⌘K` / `Ctrl+K` | **Command Palette** — fuzzy jump to any track, collection, view, theme, EQ, or command |
| `⌘F` / `Ctrl+F` | Focus Power-Search bar |
| `Space` | Play / Pause (freezes pressed piano keys & chord badge when paused) |
| `D` | **Instant DSP A/B Comparison** (Active Chain ↔ Bit-Identical Bypass) |
| `[` / `]` / `\` | Set **A–B Section Loop** Point A / Point B / Clear Loop |
| `⌘B` / `Ctrl+B` | Drop persistent **Timestamp Bookmark** at current position |
| `V` | Toggle panoramic **Concert Visualizer Stage** |
| `1`–`5` | Switch Visualizer Mode directly |
| `L` | Toggle **Synced Lyrics (`.lrc`)** drawer |
| `⌘T` / `Ctrl+T` | Open **Tag Editor & Batch Transforms** |
| `⌘I` / `Ctrl+I` | Open **Track Inspector & TrueHz/DR Analysis** |
| `⌘J` / `Ctrl+J` | Jump to currently playing track |
| `⌘⇧M` / `Ctrl+Shift+M` | Toggle **Compact Mini-Player Mode** |
| `←` `→` / `⌘←` `⌘→` | Seek ±10 s / Previous & Next Track |
| `⌘↑` `⌘↓` / `M` | Volume ±5% / Mute |
| `⌘,` / `Ctrl+,` | Open Settings |
| `Esc` | Dismiss any modal, popover, or selection |

Ready to go deeper? Read the complete **[User Guide & Reference Manual](USER_GUIDE.md)**.
