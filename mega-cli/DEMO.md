# MEGA-CLI Demo Script 🎬

Use this script to showcase MEGA-CLI's capabilities!

## Pre-Demo Setup

```bash
# 1. Make sure you have at least one API key in .env
cp .env.example .env
# Edit .env and add CLAUDE_API_KEY or another provider

# 2. Build the app
cargo build --release

# 3. (Optional) Copy to your PATH
cp target/release/mega-cli ~/.local/bin/
```

## Demo Flow

### 1. Launch with Loading Animation (The WOW Moment)
```bash
./target/release/mega-cli
```

**What happens:**
- Watch loading.mp4 play as beautiful ASCII art in the terminal
- CRT effects with color drift and fade-in animation
- Automatically transitions to chat interface

**Talking points:**
- "This is an MP4 video playing *in the terminal* using ASCII characters"
- "The video decoder runs in a separate thread with 8-frame buffering"
- "Notice the retro CRT effects - HSL color shifting, fade-in"

### 2. First Chat Message
```
> Explain Rust's ownership model in one sentence
```

**What happens:**
- Message appears immediately
- Loading indicator: "⏳ Waiting for response..."
- AI response streams in

**Talking points:**
- "Async request handling with tokio"
- "Real-time UI updates with no blocking"
- "Notice the color coding - each AI has its own theme"

### 3. Switch AI Providers (Press F2)
```
Press F2
```

**What happens:**
- Cycles through: Claude → Grok → GPT → Gemini → Claude
- UI updates with new provider name and color theme
- System message confirms the switch

**Talking points:**
- "Hot-swapping between 4 different AI providers"
- "Each has its own API implementation"
- "Color themes help identify which AI you're talking to"

### 4. Show Help Screen (Press F1)
```
Press F1
```

**What happens:**
- Full-screen help overlay appears
- Shows all keyboard shortcuts
- Lists all available AI providers

**Talking points:**
- "Complete keyboard-driven interface"
- "No mouse needed - perfect for terminal power users"

### 5. Complex Multi-line Response
```
> Write a quick Rust function that checks if a number is prime
```

**What happens:**
- AI returns formatted code
- Markdown-style response rendering

**Talking points:**
- "Handles code snippets and formatting"
- "Future: syntax highlighting for code blocks"

### 6. Scroll Through History
```
Press ↑ and ↓ arrows
Press PgUp and PgDn
```

**What happens:**
- Message history scrolls smoothly
- Pagination for long conversations

**Talking points:**
- "Unlimited message history"
- "Smooth scrolling with keyboard shortcuts"

### 7. Clear Conversation (Ctrl+L)
```
Press Ctrl+L
```

**What happens:**
- All messages cleared instantly
- Fresh conversation starts

### 8. Exit Gracefully (Ctrl+C)
```
Press Ctrl+C
```

**What happens:**
- Terminal restored to normal state
- Clean exit with no artifacts

---

## Advanced Demo (If Time Permits)

### Skip Loading for Speed
```bash
./target/release/mega-cli --skip-loading --provider grok
```

**Talking points:**
- "CLI flags for power users"
- "Choose your provider at startup"
- "Skip video for quick access"

### Show the Code
```bash
# Show the clean architecture
tree src/

src/
├── main.rs     # State machine & event loop
├── video.rs    # ASCII video player (from asciivision)
├── chat.rs     # TUI & message handling
└── ai.rs       # Multi-provider API client
```

**Talking points:**
- "Modular design - each file has one job"
- "video.rs is extracted from the asciivision project"
- "ai.rs abstracts 4 different API formats"

### Show the API Integration
```bash
# Open ai.rs and show the provider enum
cat src/ai.rs | head -30
```

**Talking points:**
- "Polymorphic API client"
- "Each provider has different API formats"
- "Claude uses Anthropic API, OpenAI/Grok share format, Gemini is unique"

---

## Impressive Technical Details to Mention

1. **Video Rendering:**
   - FFmpeg integration for MP4 decoding
   - Real-time scaling to terminal size
   - Luminance-based ASCII character selection from 64-char palette
   - RGB color preservation

2. **Async Architecture:**
   - Tokio async runtime
   - Non-blocking API calls
   - Channel-based message passing between UI and AI client

3. **Visual Effects:**
   - TachyonFX for CRT effects
   - HSL color shifting
   - Fade transitions
   - Per-provider color themes

4. **Terminal UI:**
   - Ratatui framework
   - Crossterm for cross-platform support
   - Raw mode for keyboard control
   - Alternate screen buffer (no terminal pollution)

5. **API Clients:**
   - 4 different AI providers
   - Unified interface
   - Error handling with anyhow
   - Environment-based configuration

---

## Showstopper Quotes

- "This is a fully functional AI chatbot that runs *entirely in your terminal*"
- "The loading screen is an actual MP4 video converted to ASCII in real-time"
- "You can switch between Claude, Grok, GPT, and Gemini with a single keypress"
- "Built in Rust - blazing fast, memory-safe, zero garbage collection"
- "From video playback to AI chat, all with keyboard shortcuts"

---

## Questions You Might Get

**Q: Can I use my own loading video?**
A: Yes! Just replace `loading.mp4` with any MP4 file. The video decoder handles any resolution and framerate.

**Q: Do I need all 4 API keys?**
A: No, just one is enough to start. Add more as needed.

**Q: Can I run this on Windows?**
A: Yes! Crossterm is cross-platform. Just install FFmpeg dev libraries for Windows.

**Q: How does the video playback work?**
A: We use FFmpeg to decode the MP4, scale it to terminal size, then convert each pixel to ASCII based on luminance. Colors are preserved as RGB terminal escape codes.

**Q: Is this using GPT-4 or GPT-5?**
A: The app is configured for GPT-5, but you can modify `src/ai.rs` to use any OpenAI model.

**Q: Can it handle images?**
A: Not yet, but it's on the roadmap! Claude and GPT support image inputs via their APIs.

---

**Have fun demoing! 🚀**
