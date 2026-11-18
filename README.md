# deck

A fast, responsive desktop audio player written in Rust and egui.

## Adaptive Layout

`deck` automatically reorganizes its layout based on window dimensions—seamlessly shifting between an expanded view with library details and a compact deck mode for minimal desktop footprints.

| Expanded View | Compact View |
| :---: | :---: |
| ![Expanded Mode](docs/images/preview-full.png) | ![Compact Mode](docs/images/preview-compact.png) |

## Features

- **Hardware-accelerated UI**: Immediate-mode GUI powered by `eframe` / `egui`.
- **Low-latency playback**: Symphonia audio engine integration through `rodio`.
- **Metadata parsing**: Tag and album art extraction via `lofty`.
- **System media integration**: Native OS media controls powered by `souvlaki`.
- **Responsive design**: Smooth adaptive transition between full library and compact playback views.

## Building from Source

Ensure [Rust](https://rustup.rs/) is installed:

```bash
git clone https://github.com/ye6isu/deck.git
cd deck
cargo run --release
```

## License

MIT © Andreu Liam Gamboa
