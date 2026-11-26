<div align="center">

<img src="assets/AppIcon.png" alt="TxPlayer Icon" width="104" height="104" />

# TxPlayer

**Fast, 100% free native Rust music player, 88-key Piano &amp; Grand Staff visualizer, and offline master music library.**

[![Version](https://img.shields.io/badge/version-1.12.0-2dd4bf)](https://github.com/tuxie80/txplayer/releases/latest) [![License](https://img.shields.io/badge/license-MIT-0f8c7d)](LICENSE) [![Platforms](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-lightgrey)](#download) [![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust&logoColor=white)](Cargo.toml) [![Audio](https://img.shields.io/badge/audio-Bit--Perfect%20%C2%B7%20DoP%20%C2%B7%2064--bit%20DSP-2aa198)](docs/USER_GUIDE.md) [![Built with Gemini 4 Neon](https://img.shields.io/badge/Built%20with-Gemini%204%20Neon-4285F4?logo=googlegemini&logoColor=white)](https://gemini.google.com/)

**[Website &amp; Tour (tuxie.eu/txplayer)](https://tuxie.eu/txplayer)** · **[Download Installers](https://github.com/tuxie80/txplayer/releases/latest)** · **[5-Min Quick Start](docs/QUICK_START.md)** · **[Full User Guide](docs/USER_GUIDE.md)**

</div>

---

<p align="center">
  <img src="docs/screenshots/txplayer-piano-v1.11.jpg" alt="TxPlayer — 88-Key Precision CQT Piano Visualizer & Inline DR Badges" width="100%" />
</p>

| Compact Mini-Player (`⌘⇧M`) | Live Grand Staff Sheet Music (`Staff` + `ASCII / ABC`) |
| :---: | :---: |
| ![Compact Mode](docs/screenshots/txplayer-compact-v1.11.jpg) | ![Grand Staff Sheet Music](docs/screenshots/txplayer-staff-v1.12.jpg) |

| Live Acoustic Spectrogram &amp; Inline `DR` | Loudness-Matched Blind ABX Lab |
| :---: | :---: |
| ![Live Spectrogram](docs/screenshots/txplayer-spectrogram-v1.11.jpg) | ![Blind ABX Lab](docs/screenshots/txplayer-abx-v1.11.jpg) |

---

## Download

Grab ready-to-run native installers from **[GitHub Releases (`v1.12.0`)](https://github.com/tuxie80/txplayer/releases/latest)**:

| Platform | Installer | Portable |
| :--- | :--- | :--- |
| **macOS (Apple Silicon)** | [`TxPlayer-v1.12.0-macos-arm64.dmg`](https://github.com/tuxie80/txplayer/releases/download/v1.12.0/TxPlayer-v1.12.0-macos-arm64.dmg) | [`txplayer-v1.12.0-macos-arm64.tar.gz`](https://github.com/tuxie80/txplayer/releases/download/v1.12.0/txplayer-v1.12.0-macos-arm64.tar.gz) |
| **Windows (x64)** | [`TxPlayer-v1.12.0-windows-x86_64-setup.exe`](https://github.com/tuxie80/txplayer/releases/download/v1.12.0/TxPlayer-v1.12.0-windows-x86_64-setup.exe) | [`txplayer-v1.12.0-windows-x86_64.zip`](https://github.com/tuxie80/txplayer/releases/download/v1.12.0/txplayer-v1.12.0-windows-x86_64.zip) |
| **Linux (x86_64)** | [`txplayer_1.12.0_amd64.deb`](https://github.com/tuxie80/txplayer/releases/download/v1.12.0/txplayer_1.12.0_amd64.deb) | [`txplayer-v1.12.0-linux-x86_64.tar.gz`](https://github.com/tuxie80/txplayer/releases/download/v1.12.0/txplayer-v1.12.0-linux-x86_64.tar.gz) |

---

## Highlights

- **Simple to start:** Drag &amp; drop any folder, switch between **Compact Mini-Player** and **Classic Studio** (`⌘⇧M`), pick from **9 themes**, press **`F1`** anytime for built-in help.
- **Bit-Perfect &amp; Native DSD:** CoreAudio / WASAPI / ALSA Exclusive DAC access, Play-from-RAM, **DoP v1.1** (`DSD64`–`DSD512`), multi-track **SACD `.iso`**, plus `FLAC`, `ALAC`, `WAV`, `AIFF`, `WavPack`, `Monkey's Audio`, `Opus`, and `.cue`.
- **64-Bit Studio DSP &amp; Practice:** Draggable 10-band EQ canvas, AutoEQ PEQ import, partitioned FFT Convolution (`.wav` IR), **Pitch-Locked slowdown** (`0.75×–1.5×`), `±12 st` transpose, `A=440/432/415 Hz` tuning, and instant `D` DSP A/B bypass.
- **7 Real-Time Visualizers (`1`–`7`):** 88-Key Precision CQT Piano, Piano Roll, **Grand Staff Sheet Music** (Treble &amp; Bass clefs + 1-click **ASCII Grand Staff &amp; ABC musical notation** copy), 64-Band RTA, Live Acoustic Spectrogram, Stereo Scope, and Mastering VU + live EBU R128 LUFS.
- **Master Library Database &amp; Collector Tools:** 9 browse facets, Collections, 16-field Smart Views, Power-Search (`composer:beethoven dr>=12 rate>=96 #reference`), 7 Batch Tag Transforms, TrueHz/DR &amp; ReplayGain audits, **Blind ABX Lab**, 100% local Similar Radio, HTML/Markdown/CSV dossiers, and opt-in **LAN Mobile Remote**.
- **100% Free &amp; Local:** Native Rust (~8.5 MB binary), zero Electron, zero cloud, zero telemetry, 57/57 unit tests passing.

---

## Build from Source

```bash
# Test, lint & run release build (Rust 1.95+)
cargo test && cargo run --release

# macOS signed .app bundle & .dmg
./scripts/package-macos.sh

# Linux .deb & portable .tar.gz
./scripts/package-linux.sh
```

> **Linux dev deps (Debian/Ubuntu):** `libasound2-dev libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev pkg-config`  
> **Optional helpers:** `ffmpeg` (lossless tag writing &amp; `.wv`/`.ape`/`.opus`), `fpcalc` (AcoustID), `curl`
