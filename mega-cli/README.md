# MEGA-CLI 🤖⚡

> A next-generation AI chatbot that runs entirely in your terminal with cinematic loading animations

[![Built with Rust](https://img.shields.io/badge/Built%20with-Rust-orange)](https://www.rust-lang.org/)

## Features

🎬 **Cinematic Loading** - Watch `loading.mp4` play as ASCII art while the app initializes
🤖 **Multi-AI Support** - Chat with Claude Sonnet 4.5, Grok 4, GPT-5, or Gemini 2.5 Pro
🎨 **CRT Effects** - Retro visual effects powered by tachyonfx
⚡ **Real-time Streaming** - Async response handling for instant feedback
🎮 **Intuitive Controls** - Keyboard shortcuts for power users
🌈 **Color-coded UI** - Each AI gets its own distinctive theme

---

## Installation

### Prerequisites

- **Rust** (latest stable)
- **FFmpeg** development libraries
- API keys for your chosen AI providers

#### Install FFmpeg (macOS)
```bash
brew install ffmpeg
```

#### Install FFmpeg (Ubuntu/Debian)
```bash
sudo apt update
sudo apt install libavformat-dev libavcodec-dev libswscale-dev libavutil-dev pkg-config
```

### Build from Source

```bash
cd mega-cli
cargo build --release
```

The binary will be at `target/release/mega-cli`

### Installation Script

```bash
# Build and install to /usr/local/bin
cargo build --release
sudo cp target/release/mega-cli /usr/local/bin/

# Or install to your user directory
cargo install --path .
```

---

## Setup

### 1. Configure API Keys

Copy the example environment file:
```bash
cp .env.example .env
```

Edit `.env` and add your API keys:
```bash
CLAUDE_API_KEY=sk-ant-...
GROK_API_KEY=xai-...
OPENAI_API_KEY=sk-...
GEMINI_API_KEY=AI...
```

**Getting API Keys:**
- **Claude**: https://console.anthropic.com/
- **Grok**: https://x.ai/
- **OpenAI**: https://platform.openai.com/
- **Gemini**: https://ai.google.dev/

### 2. Add Loading Video

Place your `loading.mp4` file in the mega-cli directory, or skip the loading animation with `--skip-loading`.

### 3. Run

```bash
mega-cli
```

Or with options:
```bash
mega-cli --provider claude
mega-cli --skip-loading --provider gpt
```

---

## Usage

### Command Line Options

```
mega-cli [OPTIONS]

Options:
  --skip-loading           Skip the loading video
  --provider <PROVIDER>    AI provider to use (claude, grok, gpt, gemini) [default: claude]
  -h, --help              Print help
```

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `F1` | Toggle help screen |
| `F2` | Switch AI provider |
| `↑` / `↓` | Scroll messages |
| `PgUp` / `PgDn` | Scroll 10 messages |
| `Ctrl+L` | Clear conversation |
| `Ctrl+C` | Exit |

---

## Architecture

### Tech Stack

- **ratatui** - Terminal UI framework
- **crossterm** - Cross-platform terminal control
- **tachyonfx** - Visual effects engine
- **tokio** - Async runtime
- **reqwest** - HTTP client
- **ffmpeg-next** - Video decoding
- **serde** - JSON serialization

### Modules

```
mega-cli/
├── src/
│   ├── main.rs       # Entry point & state machine
│   ├── video.rs      # ASCII video player (from asciivision)
│   ├── chat.rs       # Chat UI & message handling
│   └── ai.rs         # Multi-provider AI client
├── Cargo.toml        # Dependencies
├── .env.example      # API key template
└── loading.mp4       # Loading screen video (user-provided)
```

### API Integration

All four AI providers are implemented with their native APIs:

- **Claude**: Anthropic Messages API
- **Grok**: X.AI Chat Completions API
- **OpenAI**: OpenAI Chat Completions API
- **Gemini**: Google Generative AI API

---

## Development

### Build for Development

```bash
cargo build
cargo run -- --skip-loading
```

### Check for Errors

```bash
cargo check
cargo clippy
```

### Run Tests

```bash
cargo test
```

---

## Troubleshooting

### "FFmpeg not found"
Make sure FFmpeg development libraries are installed. On macOS:
```bash
brew install ffmpeg
```

### "API key not set"
Create a `.env` file from `.env.example` and add your keys.

### "Video not found"
Either add a `loading.mp4` file or run with `--skip-loading`.

### Performance Issues
- Use `--skip-loading` to bypass video playback
- Close other terminal-heavy applications
- Ensure your terminal supports RGB colors

---

## Roadmap

- [ ] Token streaming (character-by-character display)
- [ ] Conversation export (JSON, Markdown)
- [ ] Session history & persistence
- [ ] Custom system prompts
- [ ] Image upload support (for multimodal models)
- [ ] Syntax highlighting for code blocks
- [ ] Model parameter tuning (temperature, max tokens)

---

## Credits

Built with inspiration from:
- **asciivision** - ASCII video playback engine
- **ratatui** - Excellent TUI framework
- **tachyonfx** - Beautiful terminal effects

---

## License

[Add your license here]

---

**Enjoy chatting with AI in the terminal! 🚀**
