<div align="center">

<img src="assets/AppIcon.png" alt="TxPlayer Icon" width="104" height="104" />

# TxPlayer

**Fast, 100% free native Rust music player, 88-key piano &amp; spectrogram visualizer, and offline master music library.**

[![Version](https://img.shields.io/badge/version-1.11.0-2dd4bf?style=for-the-badge)](Cargo.toml)
[![Website](https://img.shields.io/badge/website-tuxie.eu%2Ftxplayer-2dd4bf?style=for-the-badge)](https://tuxie.eu/txplayer)
[![Made with Gemini 4 Neon](https://img.shields.io/badge/Made%20with-Gemini%204%20Neon-4285F4?style=for-the-badge&logo=google&logoColor=white)](https://deepmind.google)
[![Rust](https://img.shields.io/badge/Rust-Edition%202024-dea584?style=for-the-badge&logo=rust&logoColor=white)](Cargo.toml)
[![Platforms](https://img.shields.io/badge/macOS%20%7C%20Windows%20%7C%20Linux-native-1a2731?style=for-the-badge)](#build--run)
[![Audio](https://img.shields.io/badge/Bit--Perfect%20%7C%20DoP%20v1.1%20%7C%2064--bit%20DSP-2aa198?style=for-the-badge)](docs/USER_GUIDE.md)
[![License](https://img.shields.io/badge/license-MIT-0f8c7d?style=for-the-badge)](Cargo.toml)

**[Website &amp; Tour (tuxie.eu/txplayer)](https://tuxie.eu/txplayer)** · **[5-Min Quick Start](docs/QUICK_START.md)** · **[Full User Guide](docs/USER_GUIDE.md)** · **[Roadmap](ROADMAP.md)**

</div>

---

<p align="center">
  <img src="docs/screenshots/txplayer-piano-v1.11.jpg" alt="TxPlayer v1.11.0 — 88-Key Precision CQT Piano Visualizer & Inline DR Badges" width="100%" />
</p>

| Compact Mini-Player (`⌘⇧M`) | Live Acoustic Spectrogram &amp; Inline `DR` |
| :---: | :---: |
| ![Compact Mode](docs/screenshots/txplayer-compact-v1.11.jpg) | ![Live Spectrogram](docs/screenshots/txplayer-spectrogram-v1.11.jpg) |

| Interactive Drag-to-Shape EQ &amp; 64-Bit DSP | Loudness-Matched Blind ABX Lab |
| :---: | :---: |
| ![Interactive EQ & DSP](docs/screenshots/txplayer-dsp-v1.11.jpg) | ![Blind ABX Lab](docs/screenshots/txplayer-abx-v1.11.jpg) |

---

## Highlights

- **Simple to start:** Drag &amp; drop any folder, switch between **Compact Mini-Player** and **Classic Studio** (`⌘⇧M`), pick from **9 themes**, press **`F1`** anytime for built-in help.
- **Bit-Perfect &amp; Native DSD:** CoreAudio / WASAPI / ALSA Exclusive DAC access, Play-from-RAM, **DoP v1.1** (`DSD64`–`DSD512`), multi-track **SACD `.iso`**, plus `FLAC`, `ALAC`, `WAV`, `AIFF`, `WavPack`, `Monkey's Audio`, `Opus`, and `.cue`.
- **64-Bit Studio DSP &amp; Practice:** Draggable 10-band EQ canvas, AutoEQ PEQ import, partitioned FFT Convolution (`.wav` IR), **Pitch-Locked slowdown** (`0.75×–1.5×`), `±12 st` transpose, `A=440/432/415 Hz` tuning, and instant `D` DSP A/B bypass.
- **6 Real-Time Visualizers (`1`–`6`):** 88-Key Precision CQT Piano (with Pause-Freeze chords), Piano Roll, 64-Band RTA, Live Acoustic Spectrogram, Stereo Scope, and Mastering VU + live EBU R128 LUFS.
- **Master Library Database &amp; Collector Tools:** 9 browse facets, Collections, 16-field Smart Views, Power-Search (`composer:beethoven dr>=12 rate>=96 #reference`), 7 Batch Tag Transforms, TrueHz/DR &amp; ReplayGain audits, **Blind ABX Lab**, 100% local Similar Radio, HTML/Markdown/CSV dossiers, and opt-in **LAN Mobile Remote**.
- **100% Free &amp; Local:** Native Rust (~8.5 MB binary), zero Electron, zero cloud, zero telemetry, 55/55 unit tests passing.

---

## Build &amp; Run

```bash
# Test, lint & run release build (Rust 1.95+)
cargo test && cargo run --release

# macOS signed .app bundle & .dmg
./scripts/package-macos.sh
```

> **Linux dev deps (Debian/Ubuntu):** `libasound2-dev libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev pkg-config`  
> **Optional helpers:** `ffmpeg` (lossless tag writing &amp; `.wv`/`.ape`/`.opus`), `fpcalc` (AcoustID), `curl`
