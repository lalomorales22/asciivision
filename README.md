# GPT-5 ASCIIVision 📺

> Play MP4 videos as ASCII art directly in your terminal with retro CRT-style effects!
<img width="492" height="520" alt="Screenshot 2025-08-15 at 10 37 35 PM" src="https://github.com/user-attachments/assets/f9f6a5fd-7b77-46b5-bfa0-7bd76cf7fb8f" />

## ✨ Features

- **🎬 Video Playback**: Converts MP4 videos to real-time ASCII art using FFmpeg
- **🌈 Color Support**: Full RGB color support with automatic ASCII character mapping based on luminance
- **📺 Retro CRT Effects**: Built-in visual effects including:
  - Power-on sweep animation
  - HSL color drift
  - Terminal palette effects
  - Glitch effects (toggleable)
- **⚡ Smart Scaling**: Automatic aspect ratio preservation with terminal cell compensation
- **🎛️ Interactive Controls**: Pause, color/mono toggle, and glitch effects control
- **🚀 High Performance**: Multi-threaded decoding with frame buffering

## 🚀 Installation

### Prerequisites

- **Rust** (latest stable version)
- **FFmpeg** development libraries installed on your system

#### Installing FFmpeg (macOS)
```bash
brew install ffmpeg
```

#### Installing FFmpeg (Ubuntu/Debian)
```bash
sudo apt update
sudo apt install libavformat-dev libavcodec-dev libswscale-dev libavutil-dev pkg-config
```

### Build from Source

```bash
git clone <your-repo-url>
cd asciivision-game
cargo build --release
```

The compiled binary will be available at `target/release/gpt5-asciivision`.

## 🎮 Usage

### Basic Usage
```bash
./target/release/gpt5-asciivision video.mp4
```

### Advanced Options
```bash
./target/release/gpt5-asciivision [OPTIONS] <INPUT>

Arguments:
  <INPUT>  Path to an .mp4 (H.264/H.265/etc supported by system FFmpeg)

Options:
      --max-width <MAX_WIDTH>  Target max width in terminal cells (height auto) [default: 140]
      --fps-cap <FPS_CAP>      Limit FPS (0 = use stream rate) [default: 0]
      --mono                   Force monochrome output
  -h, --help                   Print help
```

### Examples

```bash
# Play video at default settings
./target/release/gpt5-asciivision demo.mp4

# Play with custom width and FPS limit
./target/release/gpt5-asciivision --max-width 100 --fps-cap 24 demo.mp4

# Play in monochrome mode
./target/release/gpt5-asciivision --mono demo.mp4
```

## 🎛️ Controls

| Key | Action |
|-----|--------|
| `Space` | Pause/Resume playback |
| `Q` or `Esc` | Quit |
| `C` | Toggle between color and monochrome |
| `G` | Toggle glitch effects |

## 🛠️ Technical Details

### Architecture

- **Multi-threaded Design**: Separate decode thread prevents UI blocking
- **FFmpeg Integration**: Uses `ffmpeg-next` for robust video decoding
- **Smart Scaling**: Bilinear scaling with terminal cell aspect ratio compensation
- **ASCII Mapping**: Luminance-based character selection using a 64-character palette

### ASCII Palette
The app uses a carefully crafted 64-character palette optimized for luminance progression:
```
 .'`^",:;Il!i><~+_-?][}{1)(|\tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$
```

### Performance

- **Frame Buffering**: 8-frame buffer prevents stutter
- **Optimized Rendering**: Direct terminal buffer manipulation for smooth playback
- **Memory Efficient**: Streaming decode with minimal memory footprint

## 📋 Requirements

- **Terminal**: Any terminal with RGB color support (most modern terminals)
- **Video Formats**: Any format supported by your system's FFmpeg installation
- **Minimum Terminal Size**: Recommended 80x24 or larger for best experience

## 🐛 Troubleshooting

### FFmpeg Errors
If you get FFmpeg-related errors:
1. Ensure FFmpeg development libraries are installed
2. Try re-encoding your video: `ffmpeg -i input.mp4 -c:v libx264 output.mp4`

### Performance Issues
- Try reducing `--max-width` for better performance on slower systems
- Use `--fps-cap` to limit framerate if needed
- Ensure your terminal supports hardware acceleration

### Display Issues
- Some terminals may not display all characters correctly
- Try different terminal emulators if characters appear garbled
- Ensure your terminal font supports the full ASCII range

## 🏗️ Dependencies

- **anyhow**: Error handling
- **clap**: Command-line argument parsing
- **crossterm**: Cross-platform terminal manipulation
- **ratatui**: Terminal UI framework
- **tachyonfx**: Visual effects system
- **ffmpeg-next**: FFmpeg bindings for Rust
- **crossbeam-channel**: Thread-safe communication

## 📝 License

[Add your license here]

## 🤝 Contributing

Contributions are welcome! Please feel free to submit pull requests or open issues for bugs and feature requests.

## 🎯 Future Enhancements

- [ ] Audio playback support
- [ ] Subtitle overlay
- [ ] More visual effects
- [ ] Playlist support
- [ ] Network streaming
- [ ] Custom ASCII palettes

---

*Enjoy watching your videos in glorious ASCII art! 🎬*
