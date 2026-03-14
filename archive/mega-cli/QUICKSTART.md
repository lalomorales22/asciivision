# MEGA-CLI Quick Start 🚀

Get up and running in 3 minutes!

## Installation

```bash
cd mega-cli
./install.sh
```

Or manual install:
```bash
cargo build --release
cp target/release/mega-cli ~/.local/bin/
# Add ~/.local/bin to your PATH if needed
```

## Setup API Keys

1. **Copy the template:**
   ```bash
   cp .env.example .env
   ```

2. **Get your API keys** (you only need ONE to start):
   - **Claude**: https://console.anthropic.com/settings/keys
   - **Grok**: https://console.x.ai/
   - **OpenAI**: https://platform.openai.com/api-keys
   - **Gemini**: https://makersuite.google.com/app/apikey

3. **Edit `.env`** and paste your keys:
   ```bash
   CLAUDE_API_KEY=sk-ant-api03-...
   # Add others as needed
   ```

## Run It!

### Option 1: Skip loading screen (fastest)
```bash
mega-cli --skip-loading
```

### Option 2: With loading animation
```bash
# First, copy a video to use as loading screen
cp /path/to/your/video.mp4 loading.mp4

# Then run
mega-cli
```

### Choose your AI provider
```bash
mega-cli --skip-loading --provider claude
mega-cli --skip-loading --provider grok
mega-cli --skip-loading --provider gpt
mega-cli --skip-loading --provider gemini
```

## Keyboard Shortcuts

- **Enter** - Send message
- **F1** - Help
- **F2** - Switch AI model
- **Ctrl+L** - Clear chat
- **Ctrl+C** - Exit

## Troubleshooting

### "API key not set"
Make sure you created `.env` from `.env.example` and added at least one API key.

### "FFmpeg not found"
**macOS:**
```bash
brew install ffmpeg
```

**Ubuntu/Debian:**
```bash
sudo apt install libavformat-dev libavcodec-dev libswscale-dev libavutil-dev pkg-config
```

### "loading.mp4 not found"
Either:
- Add a `loading.mp4` file to the mega-cli directory, OR
- Run with `--skip-loading` flag

## Example Session

```bash
$ mega-cli --skip-loading --provider claude

# Type your first message:
> What is Rust?

# Claude responds...
# Press F2 to switch to Grok, GPT, or Gemini
# Press Ctrl+L to clear and start fresh
# Press Ctrl+C when done
```

## Next Steps

- Read the full [README.md](README.md)
- Customize your `.env` with multiple API keys
- Add a custom `loading.mp4` video
- Check out the [build.md](build.md) for architecture details

---

**Enjoy! 🎉**
