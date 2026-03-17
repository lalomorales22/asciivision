# What I'd Do Next -- 3-Phase Plan

A roadmap for turning ASCIIVision from an impressive terminal app into an indispensable agentic terminal operating system.

---

## Phase 1: Make It Truly Agentic

The app currently chats with AI and runs shell commands separately. Phase 1 fuses them -- the AI becomes an autonomous agent that can observe, decide, and act inside the terminal.

### Tool-Use Loop

Give the AI models actual tool definitions (function calling). When you ask "deploy the staging server," the AI doesn't just tell you what to do -- it calls tools:

- `run_shell(command)` -- execute a command and read stdout/stderr
- `read_file(path)` -- inspect files without leaving the chat
- `write_file(path, content)` -- create or edit files
- `search_files(pattern, dir)` -- ripgrep the codebase
- `http_request(method, url, body)` -- hit APIs directly
- `get_system_info()` -- pull CPU/memory/disk/network from the sysmon module

The AI gets the tool results back, reasons about them, and chains the next action. Multi-step tasks like "find all TODO comments, create a GitHub issue for each one, and post a summary to Slack" become one prompt.

### Approval Gates

Add a confirmation step before destructive actions. The AI proposes a command, the user sees it highlighted in amber, and presses Enter to approve or Esc to reject. Configurable trust levels: full-auto, confirm-destructive, confirm-all.

### Context Window Management

The transcript already feeds into the AI context. Extend this:

- Automatically inject the last 5 shell outputs as context
- Let the user @-mention files (`@src/main.rs explain this`) to inject file contents
- Summarize old context when the window fills up instead of truncating
- Pin important messages so they never scroll out of context

### Persistent Agent Memory

Store key facts the agent learns across sessions in SQLite:

- Project structure and conventions it discovered
- User preferences it inferred ("you always deploy to staging first")
- Command history patterns ("you run `cargo test` after every edit")
- Named memory slots the user can set: `/remember deploy_cmd = make deploy-staging`

### Streaming Responses

Replace the current fire-and-forget API calls with streaming (SSE/chunked). Characters appear in real-time as the model generates them instead of waiting for the full response. The reveal animation already exists -- wire it to stream chunks instead of a static string.

---

## Phase 2: Make the Graphics Insane

The current visual system is strong. Phase 2 pushes it into territory nobody expects from a terminal.

### Ray Marching in ASCII

Add a signed-distance-field ray marcher that renders 3D scenes to ASCII. Think rotating toruses, morphing metaballs, infinite tunnels, mandelbulb fractals -- all in real-time ASCII with true color. The math is just float operations and the output is characters + colors, so it fits perfectly.

Scenes to ship:
- Infinite tunnel fly-through (concentric rings with perspective)
- Metaball blobs that merge and split
- Terrain heightmap with fog and lighting
- Rotating torus knot
- Mandelbrot / Julia set zoom with smooth coloring

### Shader Pipeline

Create a mini shader system where each effect is a function `(x, y, time, resolution) -> (char, r, g, b)`. Users can write custom shaders in a simple DSL or even Lua, drop them in `~/.config/asciivision/shaders/`, and they appear in the effect cycle. Ship 20+ built-in shaders.

### Video Compositing

Layer multiple video sources with blend modes:
- Webcam feed with a rain overlay (multiply blend)
- MP4 background with the AI chat transcript overlaid semi-transparently
- Picture-in-picture: small webcam in the corner of the video panel
- Chroma key: replace a solid-color background in the webcam feed with an effect or video

### GPU-Accelerated Rendering

For terminals that support the Kitty graphics protocol or sixel, bypass ASCII entirely and render actual pixel graphics. Detect terminal capabilities at startup and upgrade automatically. The ASCII path stays as the universal fallback.

### Animated Transitions

When switching layouts, panels slide and resize with eased animations instead of snapping instantly. When a new message arrives, it fades in. When panels swap, they cross-dissolve. Use the existing `tachyonfx` crate to drive these.

### Audio Visualization

Capture system audio or microphone input and render:
- Waveform display (oscilloscope style)
- FFT spectrum analyzer with colored bars
- Spectrogram (time-frequency heatmap in ASCII)

Wire it into the effects panel or as a standalone tile.

---

## Phase 3: Make It a Platform

Phase 3 turns ASCIIVision from a cool app into something people depend on daily.

### Plugin System

Define a plugin API where plugins are separate binaries that communicate over stdin/stdout JSON-RPC:

- Plugins can register new panel types, slash commands, and keybindings
- Ship a plugin SDK crate with the protocol types
- Example plugins: Spotify controller, GitHub notifications, Docker dashboard, Kubernetes pod viewer, stock ticker, RSS reader, Pomodoro timer

### SSH Remote Sessions

Connect to remote machines and run ASCIIVision panels over SSH:

- `/ssh user@host` opens a remote shell panel
- Remote sysmon panel shows the remote machine's CPU/memory/network
- Split view: local transcript + remote shell side by side
- Multi-machine dashboard: monitor 4 servers in a quad layout

### Collaborative Editing

Extend the WebSocket protocol beyond video chat:

- Shared terminal sessions (like tmux but over WebSocket)
- Collaborative AI chat (multiple users prompting the same agent)
- Shared file editor panel with cursor presence
- Screen sharing: broadcast your entire ASCIIVision layout to connected clients

### Workflow Automation

Let users define multi-step workflows in TOML:

```toml
[[workflow]]
name = "deploy"
steps = [
  { shell = "cargo test" },
  { confirm = "Tests passed. Deploy to staging?" },
  { shell = "make deploy-staging" },
  { ai = "Summarize the deploy output and check for errors" },
  { shell = "curl -s https://staging.example.com/health" },
  { ai = "Is the health check passing? If not, suggest a fix." },
]
```

Trigger with `/run deploy`. The agent executes each step, handles errors, and asks for confirmation at gates.

### Embedded Database Browser

Add a panel that can connect to PostgreSQL, MySQL, or SQLite databases:

- Run queries and see results in a table view
- Schema browser with table/column exploration
- Query history with re-execution
- The AI agent can query databases as a tool ("what were last week's signups?")

### Notification Center

Aggregate notifications from connected services:

- GitHub: PR reviews, CI failures, mentions
- Slack: DMs and channel mentions
- Email: unread count and subject lines
- Custom webhooks: any JSON payload rendered as a notification

Show them in a dedicated tile or as toast overlays on any layout.

### Terminal Multiplexer Mode

Replace tmux entirely:

- Multiple tabs/workspaces, each with its own tiling layout
- Named workspaces: "code", "deploy", "monitor"
- Session save/restore: serialize the entire layout + panel states to disk
- Detach/reattach like tmux (`asciivision --attach`)

### AI-Powered Shell

Instead of just running commands, enhance the shell panel:

- Natural language to command translation ("show me large files" -> `find . -size +100M`)
- Command explanation on hover (pipe any command through the AI for a plain-English breakdown)
- Error recovery: when a command fails, the AI automatically suggests a fix
- Smart autocomplete: the AI predicts the next command based on your history and current context

---

## Priority Order

If I had to pick the single highest-impact item from each phase:

1. **Phase 1**: Tool-use loop with approval gates. This is the difference between a chatbot and an agent.
2. **Phase 2**: Ray marching. Nothing else in any terminal app comes close. It would make every demo video go viral.
3. **Phase 3**: Workflow automation. This is what makes people actually use it every day instead of just showing it off.
