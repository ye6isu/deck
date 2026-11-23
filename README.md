# deck

A fast, hardware-accelerated desktop audio player built with Rust and egui. Play lossless audio with real-time speed adjustments and studio reverb without needing pre-rendered edits.

## Preview

`deck` dynamically adapts its layout to your window dimensions—seamlessly transitioning between landscape and portrait orientations.

| Landscape View | Portrait View |
| :---: | :---: |
| ![Landscape Mode](docs/images/preview-landscape.png) | ![Portrait Mode](docs/images/preview-portrait.png) |

## Features

- **Live DSP Engine**: Real-time speed scaling (sped up / slowed down) and studio hall reverb applied on the fly to high-res audio.
- **Synced & Unsynced Lyrics**: Interactive lyrics view with smooth auto-scrolling for timestamped LRC tracks (click-to-seek, auto-snap) and free persistent scrolling for unsynced plain text lyrics.
- **Adaptive Layout**: Smooth, dynamic UI scaling that transitions seamlessly between landscape and portrait orientations based on window size.
- **Hardware-Accelerated UI**: Immediate-mode GUI powered by `eframe` and `egui`.
- **Low-Latency Playback**: High-performance audio pipeline using `rodio` and `symphonia`.
- **Metadata & Album Art**: Automatic tag extraction and embedded artwork rendering via `lofty`.
- **Native Media Controls**: System integration (media keys and notifications) via `souvlaki`.

## Building from Source

Prerequisites: [Rust & Cargo](https://rustup.rs/)

```bash
git clone https://github.com/ye6isu/deck.git
cd deck
cargo run --release
```

## License

MIT © Andreu Liam Gamboa
