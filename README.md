# ASCIIVision Ecosystem 🚀

> A complete terminal-based multimedia suite featuring ASCII video playback, multi-AI chat interface, and conversation analytics!
<img width="1870" height="423" alt="Screenshot 2025-10-04 at 12 18 50 AM" src="https://github.com/user-attachments/assets/3fdc60cb-0436-43a7-ab80-68468b29c10b" />


## 📦 Projects

This repository contains three powerful terminal applications:

### 1. 📺 ASCIIVision - Terminal Video Player
Play MP4 videos as ASCII art directly in your terminal with retro CRT-style effects!

### 2. 🤖 MEGA-CLI - Multi-AI Chat Interface
Chat with all 4 major AI providers (Claude, Grok, GPT, Gemini) in a beautiful terminal interface with seamless provider switching.

### 3. 📊 MEGA-Analytics - Conversation Dashboard
Real-time analytics dashboard for tracking and viewing your AI conversations across all providers.

---

## 📺 ASCIIVision - Terminal Video Player

### ✨ Features

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

### 🚀 Installation

#### Prerequisites

- **Rust** (latest stable version)
- **FFmpeg** development libraries installed on your system

##### Installing FFmpeg (macOS)
```bash
brew install ffmpeg
```

##### Installing FFmpeg (Ubuntu/Debian)
```bash
sudo apt update
sudo apt install libavformat-dev libavcodec-dev libswscale-dev libavutil-dev pkg-config
```

#### Build from Source

```bash
cd asciivision
cargo build --release
```

The compiled binary will be available at `target/release/gpt5-asciivision`.

### 🎮 Usage

#### Basic Usage
```bash
./target/release/gpt5-asciivision video.mp4
```

#### Advanced Options
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

### 🎛️ Controls

| Key | Action |
|-----|--------|
| `Space` | Pause/Resume playback |
| `Q` or `Esc` | Quit |
| `C` | Toggle between color and monochrome |
| `G` | Toggle glitch effects |

---

## 🤖 MEGA-CLI - Multi-AI Chat Interface

### ✨ Features

- **🔄 Multi-AI Support**: Seamlessly switch between Claude Sonnet 4.5, Grok 4, GPT-5, and Gemini 2.5 Pro
- **⚡ Real-time Streaming**: Fast responses with async API calls
- **🎨 Beautiful UI**: Color-coded interfaces for each AI provider
- **💾 Conversation History**: Automatic saving to SQLite database
- **🎬 Cinematic Loading**: Optional video loading animation
- **🔧 Smart Switching**: Press F2 to cycle through AI providers without losing context
- **📊 Database Integration**: All conversations automatically saved for analytics

### 🚀 Installation

#### Prerequisites

- **Rust** (latest stable version)
- **FFmpeg** (for loading animation)
- **API Keys** for AI providers you want to use:
  - `CLAUDE_API_KEY` - Anthropic API key
  - `GROK_API_KEY` - xAI API key
  - `OPENAI_API_KEY` - OpenAI API key
  - `GEMINI_API_KEY` - Google AI API key

#### Setup

1. Create a `.env` file in the `mega-cli` directory:
```bash
cd mega-cli
cat > .env << EOF
CLAUDE_API_KEY=your_claude_key_here
GROK_API_KEY=your_grok_key_here
OPENAI_API_KEY=your_openai_key_here
GEMINI_API_KEY=your_gemini_key_here
EOF
```

2. Build the project:
```bash
cargo build --release
```

### 🎮 Usage

```bash
# Start with default provider (Claude)
./target/release/mega-cli

# Skip loading video
./target/release/mega-cli --skip-loading

# Start with a specific provider
./target/release/mega-cli --provider grok
```

### 🎛️ Controls

| Key | Action |
|-----|--------|
| `F1` | Toggle help screen |
| `F2` | Switch AI provider (Claude → Grok → GPT → Gemini) |
| `Ctrl+L` | Clear current conversation |
| `Ctrl+C` | Exit application |
| `Enter` | Send message |
| `↑/↓` | Scroll through messages |
| `PgUp/PgDn` | Scroll 10 messages at a time |

### 🎨 AI Providers

- **Claude Sonnet 4.5** - Copper theme
- **Grok 4** - Cyan theme
- **GPT-5** - Teal theme
- **Gemini 2.5 Pro** - Blue theme

---

## 📊 MEGA-Analytics - Conversation Dashboard

### ✨ Features

- **📈 Real-time Updates**: Automatically refreshes when new conversations are saved
- **🔄 Multi-Provider Views**: Switch between Claude, Grok, GPT, and Gemini analytics
- **📊 Statistics Dashboard**: View message counts, timestamps, and conversation metrics
- **💬 Full Message History**: Browse complete conversation logs with timestamps
- **🎬 Cinematic Loading**: Optional video loading animation
- **⚡ Live Monitoring**: File-watching system detects database changes instantly

### 🚀 Installation

#### Prerequisites

- **Rust** (latest stable version)
- **FFmpeg** (for loading animation)
- **MEGA-CLI** must be used first to create the conversation database

#### Build

```bash
cd mega-analytics
cargo build --release
```

### 🎮 Usage

```bash
# Start the analytics dashboard
./target/release/mega-analytics
```

The dashboard will automatically connect to `~/.config/mega-cli/conversations.db` and display your conversation history.

### 🎛️ Controls

| Key | Action |
|-----|--------|
| `←/→` | Switch between AI providers |
| `1-4` | Quick switch to specific provider (1=Claude, 2=Grok, 3=GPT, 4=Gemini) |
| `Tab` | Toggle between Statistics and Messages view |
| `↑/↓` | Scroll through messages |
| `PgUp/PgDn` | Scroll 10 messages at a time |
| `Home/End` | Jump to start/end of conversation |
| `Q` or `Esc` | Exit |
| `Ctrl+C` | Force quit |

### 📊 Views

#### Statistics View
- Total message count
- User messages vs AI responses
- First and last message timestamps
- Per-provider conversation metrics

#### Messages View
- Complete conversation history
- Color-coded by role (User/Assistant)
- Timestamps for each message
- Scrollable with visual scrollbar

---

## 🛠️ Technical Details

### Shared Architecture

All three applications are built with:
- **Rust**: Safe, fast, and concurrent
- **Ratatui**: Terminal UI framework
- **TachyonFX**: Visual effects system
- **Crossterm**: Cross-platform terminal manipulation
- **FFmpeg**: Video decoding (ASCIIVision & loading screens)

### Database Schema

MEGA-CLI and MEGA-Analytics share a SQLite database at `~/.config/mega-cli/conversations.db`:

```sql
-- Separate tables for each provider
CREATE TABLE claude_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    timestamp INTEGER NOT NULL
);

CREATE TABLE grok_messages (...);
CREATE TABLE gpt_messages (...);
CREATE TABLE gemini_messages (...);
```

---

## 📋 System Requirements

- **Operating System**: macOS, Linux, or Windows
- **Terminal**: Any modern terminal with RGB color support
- **Rust**: 1.70 or later
- **FFmpeg**: 4.0 or later (for video features)

---

## 🐛 Troubleshooting

### FFmpeg Errors
If you get FFmpeg-related errors:
1. Ensure FFmpeg development libraries are installed
2. Try re-encoding your video: `ffmpeg -i input.mp4 -c:v libx264 output.mp4`

### API Key Issues (MEGA-CLI)
- Ensure your `.env` file is in the `mega-cli` directory
- Check that API keys are valid and have proper permissions
- Keys are loaded when the application starts

### Database Issues (MEGA-Analytics)
- Make sure you've used MEGA-CLI at least once to create the database
- Check that `~/.config/mega-cli/conversations.db` exists
- Database is created automatically on first MEGA-CLI run

---

## 🏗️ Project Structure

```
asciivision/
├── README.md                 # This file
├── asciivision/             # Video player
│   ├── src/
│   │   ├── main.rs
│   │   └── video.rs
│   └── Cargo.toml
├── mega-cli/                # Multi-AI chat interface
│   ├── src/
│   │   ├── main.rs
│   │   ├── chat.rs
│   │   ├── ai.rs
│   │   ├── db.rs
│   │   └── video.rs
│   ├── .env                 # API keys (create this)
│   └── Cargo.toml
└── mega-analytics/          # Analytics dashboard
    ├── src/
    │   ├── main.rs
    │   └── video.rs
    └── Cargo.toml
```

---

## 🎯 Getting Started

1. **Clone the repository**
   ```bash
   git clone <your-repo-url>
   cd asciivision
   ```

2. **Install FFmpeg**
   ```bash
   # macOS
   brew install ffmpeg

   # Ubuntu/Debian
   sudo apt install ffmpeg libavformat-dev libavcodec-dev libswscale-dev libavutil-dev
   ```

3. **Set up MEGA-CLI** (optional, for AI chat)
   ```bash
   cd mega-cli
   # Create .env with your API keys
   echo "CLAUDE_API_KEY=your_key" > .env
   cargo build --release
   ```

4. **Build all projects**
   ```bash
   # From the asciivision root directory
   cd asciivision && cargo build --release && cd ..
   cd mega-cli && cargo build --release && cd ..
   cd mega-analytics && cargo build --release && cd ..
   ```

5. **Run the apps**
   ```bash
   # Video player
   ./asciivision/target/release/gpt5-asciivision video.mp4

   # AI chat
   ./mega-cli/target/release/mega-cli

   # Analytics (run after using mega-cli)
   ./mega-analytics/target/release/mega-analytics
   ```

---

## 📝 License

[Add your license here]

## 🤝 Contributing

Contributions are welcome! Please feel free to submit pull requests or open issues for bugs and feature requests.

---

*Experience the future of terminal computing! 🚀*
