//! Command registry: the single source of truth for the slash-command surface.
//!
//! `dispatch_input` resolves the first token through [`find`] and executes the
//! matched [`CommandId`] in an EXHAUSTIVE `match` (no wildcard arm), so adding
//! a registry entry without a dispatch branch is a compile error — the registry
//! and the dispatcher cannot drift apart. The same table feeds the Ctrl+P
//! palette, the generated F1 help overlay, and the ops-deck cheatsheet.

/// Stable identifier for every command. `run_command` in main.rs matches on
/// this exhaustively; every variant must have exactly one registry entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    Help,
    Clear,
    Provider,
    Ollama,
    Trust,
    Stream,
    Remember,
    Forget,
    Recall,
    Memory,
    Pin,
    Unpin,
    Video,
    VideoMode,
    Screenshare,
    Mute,
    Youtube,
    Webcam,
    Effects,
    Fx,
    Randomize,
    Theme,
    Analytics,
    Layout,
    Sysmon,
    Games,
    Tiles,
    Host,
    Join,
    Invite,
    Disconnect,
    Connect,
    Server,
    Chat,
    Username,
    Run,
    Curl,
    Brew,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    AiAgent,
    Memory,
    VideoCamera,
    EffectsTheme,
    GamesArcade,
    TilesLayout,
    VideoChat,
    ShellSystem,
    Help,
}

impl Category {
    /// Render/grouping order for the help overlay.
    pub const ALL: [Self; 9] = [
        Self::VideoChat,
        Self::GamesArcade,
        Self::EffectsTheme,
        Self::AiAgent,
        Self::Memory,
        Self::VideoCamera,
        Self::TilesLayout,
        Self::ShellSystem,
        Self::Help,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::AiAgent => "AI & AGENT",
            Self::Memory => "MEMORY",
            Self::VideoCamera => "VIDEO & CAMERA",
            Self::EffectsTheme => "EFFECTS & THEME",
            Self::GamesArcade => "GAMES & ARCADE",
            Self::TilesLayout => "TILES & LAYOUT",
            Self::VideoChat => "VIDEO CHAT",
            Self::ShellSystem => "SHELL & SYSTEM",
            Self::Help => "HELP",
        }
    }

    /// Short tag shown next to palette entries.
    pub fn tag(self) -> &'static str {
        match self {
            Self::AiAgent => "AI",
            Self::Memory => "MEM",
            Self::VideoCamera => "VID",
            Self::EffectsTheme => "FX",
            Self::GamesArcade => "GAME",
            Self::TilesLayout => "TILE",
            Self::VideoChat => "NET",
            Self::ShellSystem => "SH",
            Self::Help => "HELP",
        }
    }
}

/// Whether a command needs an argument to do anything useful.
/// `Required` commands are inserted into the input line by the palette
/// (e.g. "/join ") instead of being executed bare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSpec {
    None,
    Optional,
    Required,
}

pub struct CommandSpec {
    pub id: CommandId,
    /// Canonical name including the leading slash, e.g. "/join".
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description: &'static str,
    pub category: Category,
    pub args: ArgSpec,
    /// Featured commands appear in the ops-deck cheatsheet.
    pub featured: bool,
}

macro_rules! spec {
    ($id:ident, $name:expr, $aliases:expr, $usage:expr, $desc:expr, $cat:ident, $args:ident, $featured:expr) => {
        CommandSpec {
            id: CommandId::$id,
            name: $name,
            aliases: $aliases,
            usage: $usage,
            description: $desc,
            category: Category::$cat,
            args: ArgSpec::$args,
            featured: $featured,
        }
    };
}

pub const REGISTRY: &[CommandSpec] = &[
    // --- video chat ---
    spec!(Host, "/host", &[], "/host [port]", "open a room: start server + join it + print the room code (default port 9999)", VideoChat, Optional, true),
    spec!(Join, "/join", &[], "/join <code>", "join a friend's room by its short room code", VideoChat, Required, true),
    spec!(Invite, "/invite", &[], "/invite", "reprint the room code + connect line for the current room", VideoChat, None, false),
    spec!(Disconnect, "/disconnect", &[], "/disconnect", "leave the video chat room", VideoChat, None, false),
    spec!(Connect, "/connect", &[], "/connect <ws-url>", "connect directly to a chat server, e.g. /connect ws://192.168.1.7:9999", VideoChat, Required, false),
    spec!(Server, "/server", &[], "/server <port>", "start a bare chat server without joining (prefer /host)", VideoChat, Required, false),
    spec!(Chat, "/chat", &[], "/chat <message>", "send a text message to everyone in the room", VideoChat, Required, false),
    spec!(Username, "/username", &[], "/username <name>", "set your display name (applies to the next connection)", VideoChat, Required, false),
    // --- games ---
    spec!(Games, "/games", &[], "/games [name|action]", "arcade bay: pacman, space, penguin, pong, tron, snake, breakout — or play/menu/next/prev", GamesArcade, Optional, true),
    // --- effects & theme ---
    spec!(Fx, "/fx", &[], "/fx [name]", "cycle 3D effects, or jump straight to one: torus, metaballs, tunnel, synthwave, julia, matrix...", EffectsTheme, Optional, true),
    spec!(Effects, "/effects", &["/3d"], "/effects", "toggle the 3D effects engine on/off", EffectsTheme, None, false),
    spec!(Randomize, "/randomize", &[], "/randomize", "randomize the color theme (same as F9)", EffectsTheme, None, false),
    spec!(Theme, "/theme", &[], "/theme <random|reset>", "randomize the palette or restore factory colors", EffectsTheme, Required, false),
    // --- video & camera ---
    spec!(Youtube, "/youtube", &[], "/youtube <url>", "stream a YouTube video into the ASCII video bus", VideoCamera, Required, true),
    spec!(Webcam, "/webcam", &[], "/webcam", "toggle the live ASCII webcam feed (same as F5)", VideoCamera, None, true),
    spec!(Video, "/video", &[], "/video", "toggle the video bus panel (same as F3)", VideoCamera, None, false),
    spec!(VideoMode, "/vmode", &["/videomode"], "/vmode", "cycle video fidelity: ascii glyphs, half-block (2x res), or true pixels", VideoCamera, None, true),
    spec!(Screenshare, "/screenshare", &["/share", "/screen"], "/screenshare", "share your desktop into the room as live ASCII (or preview it locally)", VideoCamera, None, true),
    spec!(Mute, "/mute", &["/unmute"], "/mute", "mute or unmute the video/YouTube audio", VideoCamera, None, false),
    // --- ai & agent ---
    spec!(Provider, "/provider", &[], "/provider <name>", "switch AI provider: claude, grok, gpt, gemini, ollama", AiAgent, Required, true),
    spec!(Ollama, "/ollama", &[], "/ollama", "switch to local Ollama and open the model picker", AiAgent, None, false),
    spec!(Trust, "/trust", &[], "/trust", "cycle agent trust level: confirm-destructive, confirm-all, full-auto", AiAgent, None, false),
    spec!(Stream, "/stream", &["/streaming"], "/stream", "show how response streaming works", AiAgent, None, false),
    // --- memory ---
    spec!(Remember, "/remember", &[], "/remember <key> = <value>", "store a fact in persistent agent memory", Memory, Required, false),
    spec!(Forget, "/forget", &[], "/forget <key>", "delete a stored memory entry", Memory, Required, false),
    spec!(Recall, "/recall", &[], "/recall <key>", "look up one stored memory entry", Memory, Required, false),
    spec!(Memory, "/memory", &[], "/memory", "list everything in agent memory", Memory, None, false),
    spec!(Pin, "/pin", &[], "/pin", "pin the last message so it always stays in AI context", Memory, None, false),
    spec!(Unpin, "/unpin", &[], "/unpin", "unpin the most recently pinned message", Memory, None, false),
    // --- tiles & layout ---
    spec!(Tiles, "/tiles", &[], "/tiles [1-8]", "boot real PTY terminals inside a tile (bare = 2)", TilesLayout, Optional, true),
    spec!(Layout, "/layout", &[], "/layout [name]", "cycle layout presets, or jump: default, dual, triple, quad, webcam, focus, videochat, arcade", TilesLayout, Optional, true),
    spec!(Analytics, "/analytics", &[], "/analytics", "toggle the conversation analytics dashboard", TilesLayout, None, false),
    spec!(Sysmon, "/sysmon", &[], "/sysmon", "focus the system monitor panel", TilesLayout, None, false),
    // --- shell & system ---
    spec!(Run, "/run", &["/bash"], "/run <command>", "run a shell command (same as !<command>)", ShellSystem, Required, false),
    spec!(Curl, "/curl", &[], "/curl <args>", "shortcut for the curl command", ShellSystem, Required, false),
    spec!(Brew, "/brew", &[], "/brew <args>", "shortcut for the brew command", ShellSystem, Required, false),
    // --- help ---
    spec!(Help, "/help", &[], "/help", "toggle the full operations manual (same as F1)", Help, None, true),
    spec!(Clear, "/clear", &[], "/clear", "purge the transcript (same as Ctrl+L)", Help, None, false),
];

/// Resolve a command token ("/join", case-insensitive, leading slash required)
/// to its spec via canonical name or alias.
pub fn find(token: &str) -> Option<&'static CommandSpec> {
    let needle = token.trim().to_ascii_lowercase();
    if !needle.starts_with('/') {
        return None;
    }
    REGISTRY.iter().find(|spec| {
        spec.name == needle || spec.aliases.iter().any(|alias| *alias == needle)
    })
}

/// Commands surfaced in the ops-deck cheatsheet, in registry order.
pub fn featured() -> impl Iterator<Item = &'static CommandSpec> {
    REGISTRY.iter().filter(|spec| spec.featured)
}

/// All specs in a category, in registry order (help overlay grouping).
pub fn in_category(category: Category) -> impl Iterator<Item = &'static CommandSpec> {
    REGISTRY.iter().filter(move |spec| spec.category == category)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_and_aliases_are_unique_and_slash_prefixed() {
        let mut seen: HashSet<&str> = HashSet::new();
        for spec in REGISTRY {
            assert!(spec.name.starts_with('/'), "{} must start with /", spec.name);
            assert!(
                spec.name.chars().skip(1).all(|c| c.is_ascii_lowercase()),
                "{} must be lowercase",
                spec.name
            );
            assert!(seen.insert(spec.name), "duplicate command name {}", spec.name);
            for alias in spec.aliases {
                assert!(alias.starts_with('/'), "alias {} must start with /", alias);
                assert!(seen.insert(alias), "duplicate alias {}", alias);
            }
        }
    }

    #[test]
    fn every_id_has_exactly_one_entry() {
        for i in 0..REGISTRY.len() {
            for j in (i + 1)..REGISTRY.len() {
                assert_ne!(
                    REGISTRY[i].id, REGISTRY[j].id,
                    "CommandId::{:?} appears twice in the registry",
                    REGISTRY[i].id
                );
            }
        }
    }

    #[test]
    fn find_resolves_every_name_and_alias() {
        for spec in REGISTRY {
            let byname = find(spec.name).expect("name resolves");
            assert_eq!(byname.id, spec.id);
            // case-insensitive
            let upper = spec.name.to_ascii_uppercase();
            assert_eq!(find(&upper).expect("uppercase resolves").id, spec.id);
            for alias in spec.aliases {
                assert_eq!(find(alias).expect("alias resolves").id, spec.id);
            }
        }
        assert!(find("/frobnicate").is_none());
        assert!(find("help").is_none(), "slash is required");
        assert!(find("").is_none());
    }

    #[test]
    fn usage_and_descriptions_are_wellformed() {
        for spec in REGISTRY {
            assert!(
                spec.usage.starts_with(spec.name),
                "usage for {} must start with the command name",
                spec.name
            );
            assert!(!spec.description.is_empty(), "{} needs a description", spec.name);
            if spec.args == ArgSpec::Required {
                assert!(
                    spec.usage.contains('<'),
                    "{} requires an arg so usage must show a <placeholder>",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn featured_set_is_a_useful_topten() {
        let count = featured().count();
        assert!((6..=12).contains(&count), "featured count {} out of range", count);
        // the flagship flows must be featured
        for name in ["/host", "/join", "/games", "/fx", "/help"] {
            assert!(
                featured().any(|spec| spec.name == name),
                "{} should be featured",
                name
            );
        }
    }

    #[test]
    fn categories_cover_registry() {
        let total: usize = Category::ALL.iter().map(|c| in_category(*c).count()).sum();
        assert_eq!(total, REGISTRY.len(), "every command must belong to a listed category");
    }
}
