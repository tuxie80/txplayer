# TxPlayer

A calm, fast, high-resolution native music player for macOS written in Rust (`eframe` / `egui` + `symphonia` + `cpal` + `rubato`).

## Highlights
- **Hi-Res Audio Engine:** Lossless & lossy playback (`FLAC`, `ALAC`, `WAV/AIFF/CAF`, `MP3`, `AAC/M4A`, `OGG/Vorbis`, `MKV/WebM`), native CoreAudio sample-rate matching, `rubato` Blackman-Harris `SincFixedIn` resampling, and seamless device hot-switching.
- **Real-Time 64-Bit Audio DSP:** Lock-free 10-band Parametric EQ (with curated presets), Bauer-style Headphone Crossfeed, preamp, soft-knee limiter, and an interactive **Signal Path Inspector**.
- **Precision 88-Key Piano Visualizer:** Multi-rate Constant-Q semitone analyzer, 10-voice iterative harmonic overtone cancellation, hammer-strike onset glow, live chord & note readout, freeze-on-pause inspection, and an **88-Key Concert Piano Roll** mode.
- **Pluggable Settings Hub:** Enable/disable visualizers, switch piano detection engines (`Precision CQT` / `Classic FFT`) with live `ms/frame` telemetry, configure DSP, and inspect tag providers & system tools (`ffmpeg`, `fpcalc`, `curl`).
- **Low-Click, Theme-Aligned macOS UI:** Full-bleed custom themed window chrome (no grey OS title bar), 8 studio themes, 1-click hover play & inline row actions, `Cmd+K` Command Palette, mirrored acoustic waveform seekbar, multiple libraries, playlists, and online Tag Editor (MusicBrainz, iTunes + 600 px cover art, AcoustID, local hash cache).

## Build & Package (macOS)

```bash
# Run tests
cargo test

# Run optimized build
cargo run --release

# Build signed macOS .app bundle
./scripts/package-macos.sh
```
