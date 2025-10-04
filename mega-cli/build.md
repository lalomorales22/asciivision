# MEGA-CLI - Terminal AI Chatbot 🤖

## Vision
A mind-blowing multi-AI chatbot that runs entirely in the terminal. When launched, it plays a loading animation (loading.mp4) using asciivision's video rendering tech, then transitions to an immersive chat interface. The goal: make people say "how is this even possible in a terminal?!"

---

## Architecture Overview

### Core Components
1. **Video Player Integration** - Reuse asciivision's ASCII video rendering engine
2. **Chat Interface** - Dual-pane TUI with message history + input
3. **Multi-AI Backend** - Connect to Claude, Grok, GPT, and Gemini APIs
4. **State Machine** - Loading → Chat → Response streaming

### Key Technologies
- `ratatui` - Terminal UI framework (same as asciivision)
- `tachyonfx` - Visual effects for transitions and animations
- `crossterm` - Terminal control
- `tokio` - Async runtime for API calls
- `reqwest` - HTTP client for API requests
- API crates: `anthropic-sdk`, custom clients for others

---

## Features to Implement

### Phase 1: Core Infrastructure ✅
- [x] Project structure created
- [x] Cargo.toml with dependencies
- [x] Main binary entry point
- [x] State machine (Loading, Chat, Streaming)
- [x] Video player module (adapted from asciivision)

### Phase 2: Video Loading Screen ✅
- [x] Integrate asciivision's video rendering
- [x] Load and play loading.mp4
- [x] Smooth transition from video → chat interface
- [x] Power-on effect + scanlines for retro feel

### Phase 3: Chat Interface ✅
- [x] Dual-pane layout:
  - Top: Message history (scrollable)
  - Bottom: User input box
- [x] Model selector (Claude/Grok/GPT/Gemini)
- [x] Typing indicators with animations
- [ ] Markdown rendering for responses (basic version done)
- [ ] Syntax highlighting for code blocks (future enhancement)
- [x] Auto-scroll for new messages

### Phase 4: AI Integration ✅
- [x] Environment variable handling for API keys
- [x] Claude API client (Anthropic Messages API)
- [x] Grok API client (X.AI API)
- [x] OpenAI API client (GPT)
- [x] Gemini API client (Google Generative AI)
- [x] Async response handlers with channels
- [x] Error handling + retry logic

### Phase 5: Visual Polish ✅
- [x] CRT-style effects on chat interface (HSL shift)
- [ ] Glitch effects during model switching (future)
- [x] Color themes per AI model
- [ ] Smooth typing animations for AI responses (future)
- [ ] Sound effects (optional, terminal bell usage) (future)
- [x] Status bar with model info

### Phase 6: UX Enhancements
- [ ] Command history (up/down arrows) (future)
- [ ] Multi-line input support (future)
- [x] Copy/paste functionality (terminal native)
- [ ] Export conversation to file (future)
- [ ] Session persistence (future)
- [x] Keyboard shortcuts overlay (F1 for help)

### Phase 7: Installation & Distribution ✅
- [x] Build script for release binary
- [x] Install to ~/.local/bin
- [x] Shell command (`mega-cli`)
- [x] README with installation instructions
- [x] QUICKSTART guide
- [x] .env.example for easy setup

---

## Technical Details

### API Integration Reference
```rust
// From all-llms.txt
Claude:
  - URL: https://api.anthropic.com/v1/messages
  - Model: claude-sonnet-4.5
  - Key: $CLAUDE_API_KEY

Grok:
  - URL: https://api.x.ai/v1/chat/completions
  - Model: grok-4
  - Key: $GROK_API_KEY

OpenAI:
  - URL: https://api.openai.com/v1/chat/completions
  - Model: gpt-5
  - Key: $OPENAI_API_KEY

Gemini:
  - URL: https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent
  - Model: gemini-2.5-pro
  - Key: $GEMINI_API_KEY
```

### Borrowed from asciivision
- ASCII rendering pipeline (luminance-based character mapping)
- FFmpeg integration for video decoding
- TachyonFX effects system
- Ratatui UI rendering patterns
- Multi-threaded frame buffering

### New Additions Needed
- Async API handling (tokio)
- JSON parsing for API responses
- Markdown-to-terminal formatting
- Input state management
- Message history storage

---

## UI Layout Concept

```
┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
┃ MEGA-CLI // Model: Claude Sonnet 4.5     [Tokens: 1.2k] ┃
┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫
┃                                                           ┃
┃ > You: What is Rust's ownership model?                   ┃
┃                                                           ┃
┃ 🤖 Claude: Rust's ownership model is a unique memory     ┃
┃ management system that ensures memory safety without     ┃
┃ garbage collection...                                     ┃
┃                                                           ┃
┃ > You: Show me an example                                ┃
┃                                                           ┃
┃ 🤖 Claude: Here's a simple example:                      ┃
┃ ┌─────────────────────────────────────┐                  ┃
┃ │ fn main() {                         │                  ┃
┃ │     let s = String::from("hello");  │                  ┃
┃ │     takes_ownership(s);             │                  ┃
┃ │     // s is no longer valid here    │                  ┃
┃ │ }                                   │                  ┃
┃ └─────────────────────────────────────┘                  ┃
┃                                                           ┃
┃ [▓▓▓░░░░░░░] Generating response...                      ┃
┃                                                           ┃
┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫
┃ > _                                                       ┃
┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫
┃ F1 Help | F2 Switch Model | Ctrl+C Exit | Ctrl+L Clear  ┃
┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
```

---

## Visual Effects Ideas
1. **Loading Screen**:
   - Play loading.mp4 with scanline overlay
   - Fade-out to chat interface

2. **Model Switch Animation**:
   - Brief glitch effect
   - Color theme transition

3. **Message Arrival**:
   - Typewriter effect for AI responses
   - Subtle glow on code blocks

4. **Background Effects**:
   - HSL shift (like asciivision)
   - Gentle scanlines
   - Optional "matrix rain" in margins

---

## Performance Considerations
- Stream API responses token-by-token for perceived speed
- Buffer video frames efficiently (8-frame buffer like asciivision)
- Lazy-render only visible message area
- Use event-driven input handling (crossterm events)

---

## Installation Flow
```bash
# Build
cd mega-cli
cargo build --release

# Install
cp target/release/mega-cli /usr/local/bin/
# or
cargo install --path .

# Run
mega-cli
```

---

## Next Steps
1. Set up Cargo.toml with all dependencies
2. Create main.rs with state machine skeleton
3. Extract video player logic into reusable module
4. Build chat UI foundation
5. Implement first API (Claude) for testing
6. Add visual effects and polish
7. Expand to all 4 AI providers
8. Package and test installation

---

**Goal**: Ship a terminal chatbot so visually stunning and smooth that it feels like magic. Combine asciivision's video prowess with cutting-edge AI for an unforgettable CLI experience.
