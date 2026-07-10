# ASCIIVision — Handoff / Current State (v3)

> This file previously documented a terminal-resize UI glitch. **That glitch is
> resolved** (see below). This is now an accurate snapshot of where the project
> stands after the v3 upgrade.

## What it is

A single Rust/`ratatui` binary that runs an absurd amount in the terminal:
multi-AI agentic chat, real-time **video + audio** as ASCII, live webcam &
**screen share**, a WebRTC **browser studio**, ray-marched 3D effects, 7 arcade
games, room-code video chat, a tiling WM, and system telemetry. See `README.md`
for the full feature list and `asciivision-guide.pdf` for a one-page reference.

## v3 status — shipped, committed, pushed (`origin/v3-webgl-upgrade`)

1. **RgbFrame media pipeline** (`render.rs`) — video plays at correct speed
   (wall-clock pacing), renders as truecolor **half-block (2×)** by default or
   **true pixels** via terminal graphics protocols (Kitty/iTerm2/Sixel,
   auto-detected in `gfx.rs`), with `/vmode` to cycle. Half-block is the
   universal fallback.
2. **Audio** (`audio.rs`) — FFmpeg decode → cpal playback via a lock-free ring;
   `/mute`, `--no-audio`; muted automatically when the video panel is off.
3. **Screen share** (`webcam.rs` `CaptureSource::Screen`) — `/screenshare`.
4. **Compressed live-video wire** (`message.rs`) — deflate+base64 RGB, protocol
   v3, hostile-dimension + zip-bomb hardened.
5. **Parallel effects** (`shader.rs`) — rayon per-pixel ray-march.
6. **Browser studio** (`studio.rs` + `studio.html`) — `/studio` serves a web app
   from the terminal: real WebRTC 2-way video, screen share, MediaPipe
   face-tracked AR hats, three.js scene, chat. Uses the WS hub as the WebRTC
   signaling server (`WsMessage::Signal` + server `send_to`), so terminal and
   browser peers share one room.

**134 tests pass. Five adversarial review passes were run; every confirmed
finding was fixed** (screen-share privacy dual-broadcast; audio interleave
2×-speed bug; pixel stale-frame; Signal-relay targeted-DoS; studio slow-loris).

## The old resize glitch — RESOLVED

Fixed in `df06f1c` and verified both in code and empirically (a 5-resize stress
smoke: shrink→grow→tiny→back, no panic, kept rendering):
- `Event::Resize` now resets `scroll_lines = 0` (`main.rs`).
- Tiling enforces `MIN_W`/`MIN_H` and collapses splits that can't fit (`tiling.rs`).
- Direct buffer writers clip (e.g. `render_starburst` takes a clip `Rect`).

## Build / run / test

```bash
cargo build --release        # or ./install.sh for a full first-time setup
./asciivision                 # launcher (builds + runs)
cargo test                    # 134 tests
./target/release/asciivision --skip-intro --no-db   # quick manual poke
```

## Needs manual verification (hardware-dependent, can't be done headlessly)

- **True-pixel video** — only shows in Ghostty / iTerm2 / Kitty / WezTerm
  (Apple Terminal stays on half-block). Play a video, `/vmode` to Pixel.
- **Studio WebRTC + AR hats** — needs two real browsers + cameras: `/host`,
  `/studio`, open the printed URL on two machines.
- **Audio audible** — the decode→resample chain is unit-tested headlessly;
  confirm actual sound on a real output device.

## Known minor items / future work

- **A/V sync** is best-effort (both real-time-paced, no master clock; long clips
  can drift).
- **Audio opens the source twice** (separate video + audio decoders → two
  connections for YouTube/network URLs).
- **Studio is LAN + plaintext** (the WS hub has no TLS/auth beyond a version
  check — by design for a local demo). The studio HTTP server is capped at 64
  concurrent connections with 5s timeouts. Browsers reap non-WebRTC peers after
  ~12s. A future improvement: a Join capability flag so browsers never offer
  WebRTC to terminal peers, and TLS + a token for internet use.
- **Pixel encode is synchronous** on the render thread; `ratatui-image`'s
  `ThreadProtocol` can move it off-thread if a large panel ever strains the
  16ms budget.

## Map

| Area | Files |
|------|-------|
| App shell / render loop / panels | `main.rs` |
| Frame type + blitters | `render.rs` · graphics protocol `gfx.rs` |
| Video / audio / capture | `video.rs` · `audio.rs` · `webcam.rs` |
| Effects / shader | `effects.rs` · `shader.rs` |
| Networking | `server.rs` · `client.rs` · `message.rs` · `roomcode.rs` |
| Browser studio | `studio.rs` · `studio.html` |
| Games / tiling / theme | `games/` · `tiling.rs` · `tiles.rs` · `theme.rs` |
| AI / tools / memory | `ai.rs` · `tools.rs` · `memory.rs` · `db.rs` |
