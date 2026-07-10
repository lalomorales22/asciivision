//! Terminal graphics-protocol capability probe.
//!
//! Wraps `ratatui-image`'s `Picker` to decide whether this terminal can render
//! TRUE PIXELS (Kitty graphics protocol / iTerm2 inline images / Sixel) or must
//! fall back to our half-block renderer. The contract is "bulletproof
//! fallback": any failure, unknown terminal, tmux, or Apple Terminal resolves to
//! half-block so the guaranteed-everywhere path is never broken.

use crate::render::RgbFrame;
use image::{DynamicImage, RgbImage};
use ratatui_image::picker::{Picker, ProtocolType};

/// Whether to skip the interactive stdio probe entirely. We skip terminals that
/// support no protocol (Apple Terminal) or where out-of-band graphics escapes
/// are unreliable (tmux/screen) -- probing them can leak stray query bytes into
/// the UI. Pure function so it can be unit-tested without a tty.
pub fn should_skip_query(term: &str, term_program: &str, multiplexed: bool) -> bool {
    let t = term.to_ascii_lowercase();
    multiplexed
        || t.starts_with("screen")
        || t.starts_with("tmux")
        || term_program.eq_ignore_ascii_case("Apple_Terminal")
}

/// Build a `Picker` and report whether a true-pixel protocol is available.
/// Must be called AFTER entering the alternate screen and BEFORE the event loop
/// reads input (the probe does its own brief stdio round-trip).
pub fn init_picker() -> (Picker, bool, ProtocolType) {
    let term = std::env::var("TERM").unwrap_or_default();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let multiplexed = std::env::var("TMUX").is_ok() || std::env::var("STY").is_ok();

    let picker = if should_skip_query(&term, &term_program, multiplexed) {
        Picker::halfblocks()
    } else {
        // any probe error -> half-block, never a crash or a hang past its timeout
        Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())
    };
    let proto = picker.protocol_type();
    let hw = proto != ProtocolType::Halfblocks;
    (picker, hw, proto)
}

/// Human-readable name for a detected protocol.
pub fn protocol_label(proto: ProtocolType) -> &'static str {
    match proto {
        ProtocolType::Halfblocks => "half-block",
        ProtocolType::Sixel => "sixel",
        ProtocolType::Kitty => "kitty graphics",
        ProtocolType::Iterm2 => "iterm2 images",
    }
}

/// Wrap an [`RgbFrame`] as an `image::DynamicImage` for the pixel protocols.
/// Copies the pixel buffer (RgbImage owns its Vec); returns None only if the
/// declared dimensions don't match the buffer length (never for real frames).
pub fn frame_to_dynamic(frame: &RgbFrame) -> Option<DynamicImage> {
    RgbImage::from_raw(frame.width as u32, frame.height as u32, frame.data.clone())
        .map(DynamicImage::ImageRgb8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_query_for_apple_terminal_and_multiplexers() {
        assert!(should_skip_query("xterm-256color", "Apple_Terminal", false));
        assert!(should_skip_query("screen.xterm", "", false));
        assert!(should_skip_query("tmux-256color", "", false));
        assert!(should_skip_query("xterm-256color", "", true)); // $TMUX set
    }

    #[test]
    fn allows_query_for_capable_terminals() {
        assert!(!should_skip_query("xterm-kitty", "ghostty", false));
        assert!(!should_skip_query("xterm-256color", "iTerm.app", false));
        assert!(!should_skip_query("xterm-256color", "WezTerm", false));
    }

    #[test]
    fn frame_to_dynamic_roundtrips_dimensions() {
        let mut f = RgbFrame::new(4, 2);
        for (i, b) in f.data.iter_mut().enumerate() {
            *b = i as u8;
        }
        let img = frame_to_dynamic(&f).expect("valid frame converts");
        assert_eq!(img.width(), 4);
        assert_eq!(img.height(), 2);
    }
}
