# ASCIIVision UI Glitch Handoff

## The Problem

When the terminal window is resized (especially width changes), the UI glitches out:
- Text in the TRANSCRIPT panel jumps/scrolls up and off the visible area
- Panel contents get displaced and garbled during and after resize
- Stray characters and visual artifacts appear in panels that don't fully fill their area
- The layout does not gracefully adapt to smaller or changing terminal dimensions

This has been partially addressed but the core issue remains: **the app has no responsive resize handling**.

## What Was Already Fixed (Previous Session)

1. **Scroll calculation** in `render_messages_inner` (`src/main.rs` ~line 1883): Changed from counting raw `Line` items to estimating wrapped line count (`line.width() / wrap_width`). This helped but didn't fully solve the scroll-jump-on-resize problem.
2. **Background bleed**: `render_background` no longer writes sparkle/ember chars (`*`, `.`, `'`, `` ` ``), only spaces with bg color.
3. **Starburst bounds**: `render_starburst` now clips against buffer area.
4. **TRANSMIT area**: Reduced from `Constraint::Length(5)` to `Length(4)` to eliminate empty row showing artifacts.

## Root Cause Analysis

The real issue is multi-layered:

### 1. Scroll state not reset on resize (`src/main.rs`)
- `scroll_lines` and `follow_tail` are not recalculated when the terminal size changes.
- `Event::Resize` handler (in `handle_input`, ~line 455) only sets `follow_tail = true` -- it does NOT reset `scroll_lines` to 0 or force a recalc of max_scroll based on the new dimensions.
- The wrapped line count changes when width changes, but the scroll offset is stale from the old width.

### 2. Tiling layout doesn't adapt to small terminals (`src/tiling.rs`)
- `render_tile_panel` skips panels with `area.width < 6 || area.height < 4`, but the tiling tree still allocates space for them, leaving empty gaps.
- The layout presets use fixed ratios that don't collapse gracefully. A 6-panel Default layout at 80 columns means some panels get ~15 chars wide which is too narrow for their content.
- No minimum-width/height enforcement per panel type. The TRANSCRIPT needs more width than TELEMETRY, etc.
- No fallback to simpler layouts when the terminal is too small.

### 3. Paragraph wrapping + scroll interaction (`src/main.rs`)
- `Paragraph::wrap(Wrap { trim: false })` with `.scroll((n, 0))` counts scroll in **wrapped** lines, but the app estimates wrapped count with `(line.width() + wrap_width - 1) / wrap_width`. This is approximate -- ratatui's internal wrapping may differ (word boundaries, unicode width).
- When the width changes, the number of wrapped lines changes, so the scroll offset is suddenly wrong (pointing past the end of content = blank screen, or too low = content appears to jump up).

### 4. Direct buffer writes don't respect panel boundaries
- Several render functions write directly to `buffer.cell_mut((x, y))` using coordinates computed from the panel's `area` but don't clip properly:
  - `render_gradient_text` -- writes text at `x + index` without checking if it exceeds the area width
  - `render_equalizer` -- in TELEMETRY panel, draws bars that could overflow
  - `render_synthetic_scope` -- dot grid pattern in VIDEO panel
  - `render_logo` in intro -- shadow offset `+1` can go past area

## What Needs To Be Done

### Priority 1: Fix resize scroll behavior
In `handle_input`, the `Event::Resize` branch should:
```rust
Event::Resize(_, _) => {
    self.follow_tail = true;
    self.scroll_lines = 0; // reset stale scroll offset
}
```
And in `render_messages_inner`, when `follow_tail` is true, the scroll should be clamped more conservatively -- or better yet, don't scroll at all when content fits.

### Priority 2: Responsive layout system
Add terminal-size-aware layout selection in `render_chat`:
- If terminal width < ~100 cols, auto-switch to a simpler layout (DualPane or FullFocus) instead of the multi-panel Default.
- If terminal width < ~60 cols, force FullFocus (transcript only).
- When the user manually chose a layout via F6, respect it but still collapse panels that are too small to be useful.
- Consider adding minimum dimensions per `PanelKind` in `tiling.rs` and collapsing panels that can't meet their minimum.

### Priority 3: Clip all direct buffer writes
Create a helper like:
```rust
fn set_cell_clipped(buffer: &mut Buffer, x: u16, y: u16, area: Rect, ch: char, fg: Color) {
    if x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height {
        if let Some(cell) = buffer.cell_mut((x, y)) {
            cell.set_char(ch);
            cell.set_fg(fg);
        }
    }
}
```
Then use it in: `render_gradient_text`, `render_starburst`, `render_equalizer`, `render_synthetic_scope`, `render_scroller`, `render_logo`, `render_raster_bars`.

### Priority 4: Review all panel renderers for small-size safety
Each `render_*_panel` function should handle gracefully when its area is tiny:
- `render_telemetry`: Has 5 hardcoded lines + equalizer -- if height < 8 it will look broken
- `render_ops_panel`: Has 7+ hardcoded lines
- `render_header`: Writes at `inner.x + 10` -- if header is < 12 cols wide, text overflows
- Effects3D panel: Effects engine checks `area.width < 4 || area.height < 4` but the panel border eats 2 in each dimension, so the check should be on the inner area (which it is, but the `area.width < 6` check in `render_tile_panel` is the outer check)

## Key Files

| File | What's in it |
|------|-------------|
| `src/main.rs` | All rendering (`render_chat`, `render_tile_panel`, `render_messages_inner`, `render_input`, `render_header`, all `render_*_panel` methods), input handling, scroll state, background |
| `src/tiling.rs` | `TilingManager`, `TileNode` binary tree, `LayoutPreset` definitions, `PanelKind` enum, rect splitting |
| `src/effects.rs` | `EffectsEngine` -- 6 visual effects rendered into a buffer area |
| `src/sysmon.rs` | System monitor panel renderer |
| `src/analytics.rs` | Analytics panel renderer |

## How To Build & Test

```bash
cd /Users/megabrain2/Desktop/asciivision
cargo build          # dev build
cargo build --release # release build
./asciivision        # launcher script (builds + runs)

# useful test flags
./target/debug/asciivision --skip-intro --no-video --no-db
```

To reproduce the glitch: run the app, then drag the terminal window narrower/wider. Watch the TRANSCRIPT panel -- content will jump around and potentially scroll off-screen. Also try cycling layouts with F6 at different window sizes.

## Recent Changes (This Session)

- Added `PanelKind::Effects3D` to all layout presets (Default, DualPane, TripleColumn, Quad, WebcamFocus) in `src/tiling.rs` so the FX visualization window is visible and F7 cycling actually shows something.
- Partial scroll/render fixes in `src/main.rs` as described above.
