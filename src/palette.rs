//! Ctrl+P command palette: fuzzy-searchable list of every command plus
//! dynamic action entries (effects, games, layouts, providers).
//!
//! The palette owns only UI state (query, selection, scroll, filter cache);
//! executing an entry is the App's job — `handle_key` returns a
//! [`PaletteAction`] and main.rs interprets it. This module never touches
//! App state, sockets, or panels, which keeps it fully unit-testable.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::ai::AIProvider;
use crate::commands::{ArgSpec, CommandId, CommandSpec};
use crate::games::GameKind;
use crate::theme::t;
use crate::tiling::LayoutPreset;

/// What the App should do with an executed entry.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryAction {
    /// Run a registry command with the given (possibly empty) argument string.
    Command { id: CommandId, args: &'static str },
    /// Put this text into the input line (commands that need an argument).
    Insert(String),
    /// Select + activate a 3D effect by name.
    Effect(&'static str),
    /// Launch a game in the arcade panel.
    Game(GameKind),
    /// Apply a tiling layout preset.
    Layout(LayoutPreset),
    /// Switch AI provider.
    Provider(AIProvider),
}

/// Result of feeding a key into the palette.
#[derive(Debug, Clone, PartialEq)]
pub enum PaletteAction {
    /// Key consumed; nothing further to do.
    Consumed,
    /// Close the palette without executing anything.
    Close,
    /// Close the palette and perform this action.
    Execute { action: EntryAction, title: String },
}

pub struct PaletteEntry {
    /// List line, e.g. "/join" or "FX: Torus Knot".
    pub title: String,
    /// Short category tag, e.g. "NET", "FX", "GAME".
    pub tag: &'static str,
    /// Footer line shown while highlighted (usage + description).
    pub detail: String,
    /// Lowercase haystack the fuzzy matcher runs against.
    search: String,
    pub action: EntryAction,
}

impl PaletteEntry {
    pub fn new(
        title: impl Into<String>,
        tag: &'static str,
        detail: impl Into<String>,
        extra_search: &str,
        action: EntryAction,
    ) -> Self {
        let title = title.into();
        let detail = detail.into();
        let search = format!("{} {} {}", title, extra_search, detail).to_lowercase();
        Self {
            title,
            tag,
            detail,
            search,
            action,
        }
    }

    /// Entry for a registry command. Commands that require an argument are
    /// inserted into the input line ("/join ") instead of executed bare.
    pub fn for_command(spec: &'static CommandSpec) -> Self {
        let action = if spec.args == ArgSpec::Required {
            EntryAction::Insert(format!("{} ", spec.name))
        } else {
            EntryAction::Command {
                id: spec.id,
                args: "",
            }
        };
        let aliases = spec.aliases.join(" ");
        Self::new(
            spec.name,
            spec.category.tag(),
            format!("{} — {}", spec.usage, spec.description),
            &aliases,
            action,
        )
    }

    pub fn effect(name: &'static str) -> Self {
        Self::new(
            format!("FX: {}", name),
            "FX",
            format!("activate the {} 3D effect", name),
            "effect shader",
            EntryAction::Effect(name),
        )
    }

    pub fn game(kind: GameKind) -> Self {
        let multiplayer = if kind.multiplayer() { " [multiplayer]" } else { "" };
        Self::new(
            format!("Game: {}{}", kind.label(), multiplayer),
            "GAME",
            format!("launch {} — {}", kind.label(), kind.subtitle()),
            "game play arcade",
            EntryAction::Game(kind),
        )
    }

    pub fn layout(preset: LayoutPreset) -> Self {
        Self::new(
            format!("Layout: {}", preset.name()),
            "TILE",
            format!("apply the {} tiling preset", preset.name()),
            "layout preset tiling",
            EntryAction::Layout(preset),
        )
    }

    pub fn provider(provider: AIProvider) -> Self {
        let name = provider.name();
        Self::new(
            format!("AI: {}", name),
            "AI",
            format!("switch the model uplink to {}", name),
            "provider model switch",
            EntryAction::Provider(provider),
        )
    }
}

/// All providers offered as palette entries.
pub const PROVIDERS: [AIProvider; 5] = [
    AIProvider::Claude,
    AIProvider::Grok,
    AIProvider::OpenAI,
    AIProvider::Gemini,
    AIProvider::Ollama,
];

// ---------------------------------------------------------------------------
// Fuzzy matcher
// ---------------------------------------------------------------------------

/// Case-insensitive subsequence match with ranking. Returns `None` when
/// `query` is not a subsequence of `haystack`; higher scores rank first.
/// Ranking: whole-prefix > word-start > contiguous > scattered; earlier and
/// tighter matches beat later, spread-out ones.
pub fn fuzzy_score(query: &str, haystack: &str) -> Option<i32> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    if query.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();

    let mut score: i32 = 0;
    let mut hi: usize = 0;
    let mut prev_match: Option<usize> = None;
    let mut first_match: Option<usize> = None;

    for &qc in &query {
        // greedy leftmost match for this query char
        let mut found = None;
        while hi < hay.len() {
            if hay[hi] == qc {
                found = Some(hi);
                break;
            }
            hi += 1;
        }
        let pos = found?;
        if first_match.is_none() {
            first_match = Some(pos);
        }

        score += 1;
        if pos == 0 {
            score += 24; // absolute prefix
        }
        let word_start = pos > 0
            && matches!(hay[pos - 1], ' ' | '/' | '-' | '_' | ':' | '.' | '(' | '[');
        if word_start {
            score += 12;
        }
        if prev_match == Some(pos.saturating_sub(1)) && pos > 0 {
            score += 8; // contiguous run
        }

        prev_match = Some(pos);
        hi = pos + 1;
    }

    // penalties: late first match and stretched-out spans lose ties
    let first = first_match.unwrap_or(0) as i32;
    let span = prev_match.unwrap_or(0) as i32 - first;
    score -= first.min(12);
    score -= (span - (query.len() as i32 - 1)).max(0).min(12);

    Some(score)
}

// ---------------------------------------------------------------------------
// Palette state machine
// ---------------------------------------------------------------------------

pub struct Palette {
    open: bool,
    entries: Vec<PaletteEntry>,
    query: String,
    /// Indices into `entries`, best match first.
    filtered: Vec<usize>,
    /// Selection position within `filtered`.
    selected: usize,
    /// First visible row of the list viewport.
    scroll: usize,
    /// Rows the list showed last render (used to keep selection visible).
    view_rows: usize,
}

impl Palette {
    pub fn new() -> Self {
        Self {
            open: false,
            entries: Vec::new(),
            query: String::new(),
            filtered: Vec::new(),
            selected: 0,
            scroll: 0,
            view_rows: 10,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, entries: Vec<PaletteEntry>) {
        self.entries = entries;
        self.query.clear();
        self.selected = 0;
        self.scroll = 0;
        self.open = true;
        self.refilter();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.entries.clear();
        self.filtered.clear();
        self.query.clear();
    }

    #[cfg(test)]
    pub fn filtered_titles(&self) -> Vec<&str> {
        self.filtered
            .iter()
            .map(|&i| self.entries[i].title.as_str())
            .collect()
    }

    fn refilter(&mut self) {
        let mut scored: Vec<(i32, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                fuzzy_score(&self.query, &entry.search).map(|score| (score, idx))
            })
            .collect();
        // stable: higher score first, registry order breaks ties
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.filtered = scored.into_iter().map(|(_, idx)| idx).collect();
        self.selected = 0;
        self.scroll = 0;
    }

    fn selected_entry(&self) -> Option<&PaletteEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&idx| self.entries.get(idx))
    }

    fn move_selection(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let last = self.filtered.len() - 1;
        let next = self.selected as i32 + delta;
        self.selected = next.clamp(0, last as i32) as usize;
        // keep selection inside the viewport
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        let rows = self.view_rows.max(1);
        if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
    }

    /// Feed a key. The caller has already bypassed F-keys / Ctrl+C / Ctrl+L.
    pub fn handle_key(&mut self, key: KeyEvent) -> PaletteAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => PaletteAction::Close,
            KeyCode::Char('p') if ctrl => PaletteAction::Close, // Ctrl+P toggles
            KeyCode::Enter => match self.selected_entry() {
                Some(entry) => PaletteAction::Execute {
                    action: entry.action.clone(),
                    title: entry.title.clone(),
                },
                None => PaletteAction::Consumed,
            },
            KeyCode::Up => {
                self.move_selection(-1);
                PaletteAction::Consumed
            }
            KeyCode::Down => {
                self.move_selection(1);
                PaletteAction::Consumed
            }
            KeyCode::PageUp => {
                self.move_selection(-(self.view_rows.max(1) as i32));
                PaletteAction::Consumed
            }
            KeyCode::PageDown => {
                self.move_selection(self.view_rows.max(1) as i32);
                PaletteAction::Consumed
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
                PaletteAction::Consumed
            }
            KeyCode::Tab => PaletteAction::Consumed,
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.refilter();
                PaletteAction::Consumed
            }
            _ => PaletteAction::Consumed,
        }
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open {
            return;
        }
        let popup = popup_rect(area);
        if popup.width < 20 || popup.height < 7 {
            return;
        }
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .title(" COMMAND PALETTE ")
            .title_style(Style::default().fg(t().accent2).bold())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(t().accent4))
            .style(Style::default().bg(t().panel_bg));
        frame.render_widget(block, popup);

        let inner = popup.inner(Margin {
            horizontal: 2,
            vertical: 1,
        });
        // rows: query line, blank, list..., footer
        let list_rows = inner.height.saturating_sub(3) as usize;
        self.view_rows = list_rows.max(1);
        // clamp scroll so the selection stays visible after resizes
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + self.view_rows {
            self.scroll = self.selected + 1 - self.view_rows;
        }

        let mut lines: Vec<Line> = Vec::with_capacity(list_rows + 3);
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(t().accent4).bold()),
            Span::styled(self.query.clone(), Style::default().fg(t().text).bold()),
            Span::styled("_", Style::default().fg(t().accent2).bold()),
            Span::styled(
                format!("   {} match{}", self.filtered.len(), if self.filtered.len() == 1 { "" } else { "es" }),
                Style::default().fg(t().muted),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            "-".repeat(inner.width as usize),
            Style::default().fg(t().muted),
        )));

        if self.filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                "  no matches — Backspace to widen, Esc to close",
                Style::default().fg(t().muted),
            )));
        } else {
            let end = (self.scroll + list_rows).min(self.filtered.len());
            for row in self.scroll..end {
                let entry = &self.entries[self.filtered[row]];
                let is_selected = row == self.selected;
                let pointer = if is_selected { ">" } else { " " };
                let title_style = if is_selected {
                    Style::default().fg(t().accent2).bg(t().panel_alt).bold()
                } else {
                    Style::default().fg(t().text)
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{} ", pointer),
                        Style::default().fg(t().accent4).bold(),
                    ),
                    Span::styled(
                        format!("[{:<4}] ", entry.tag),
                        Style::default().fg(t().accent3),
                    ),
                    Span::styled(entry.title.clone(), title_style),
                ]));
            }
        }

        frame.render_widget(
            Paragraph::new(Text::from(lines)).style(Style::default().bg(t().panel_bg)),
            inner,
        );

        // footer: usage/description of the highlighted entry
        let footer_y = inner.y + inner.height.saturating_sub(1);
        let footer_area = Rect::new(inner.x, footer_y, inner.width, 1);
        let footer = match self.selected_entry() {
            Some(entry) => entry.detail.clone(),
            None => "type to filter // Enter runs // Up/Down navigate // Esc closes".to_string(),
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                crate::truncate(&footer, inner.width as usize),
                Style::default().fg(t().accent4),
            )))
            .style(Style::default().bg(t().panel_alt)),
            footer_area,
        );
    }
}

/// Centered popup: ~62% x 70% of the screen, clamped to sane bounds.
fn popup_rect(area: Rect) -> Rect {
    let width = (area.width as u32 * 62 / 100).clamp(42, 92) as u16;
    let height = (area.height as u32 * 70 / 100).clamp(11, 32) as u16;
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn sample_entries() -> Vec<PaletteEntry> {
        vec![
            PaletteEntry::new("/help", "HELP", "/help — manual", "", EntryAction::Command { id: CommandId::Help, args: "" }),
            PaletteEntry::new("/join", "NET", "/join <code> — join a room", "", EntryAction::Insert("/join ".to_string())),
            PaletteEntry::game(GameKind::Pong),
            PaletteEntry::effect("Torus Knot"),
            PaletteEntry::layout(LayoutPreset::Quad),
            PaletteEntry::provider(AIProvider::Grok),
        ]
    }

    // --- fuzzy matcher ---

    #[test]
    fn fuzzy_requires_subsequence() {
        assert!(fuzzy_score("xyz", "/help").is_none());
        assert!(fuzzy_score("pong", "pog").is_none());
        assert!(fuzzy_score("png", "pong").is_some()); // scattered subsequence ok
        assert!(fuzzy_score("", "anything").is_some()); // empty matches all
    }

    #[test]
    fn fuzzy_is_case_insensitive() {
        assert_eq!(fuzzy_score("PONG", "Game: Pong"), fuzzy_score("pong", "game: pong"));
        assert!(fuzzy_score("TORUS", "fx: torus knot").is_some());
    }

    #[test]
    fn fuzzy_prefix_beats_word_start_beats_scattered() {
        let prefix = fuzzy_score("host", "host a room").unwrap();
        let word_start = fuzzy_score("host", "self host setup").unwrap();
        let scattered = fuzzy_score("host", "has to sit").unwrap();
        assert!(prefix > word_start, "prefix {} <= word-start {}", prefix, word_start);
        assert!(word_start > scattered, "word-start {} <= scattered {}", word_start, scattered);
    }

    #[test]
    fn fuzzy_contiguous_beats_spread() {
        let tight = fuzzy_score("pong", "xx pong").unwrap();
        let spread = fuzzy_score("pong", "xxp o n g").unwrap();
        assert!(tight > spread);
    }

    #[test]
    fn filter_is_stable_for_ties() {
        let mut palette = Palette::new();
        palette.open(vec![
            PaletteEntry::new("alpha one", "AI", "d", "", EntryAction::Effect("a")),
            PaletteEntry::new("alpha two", "AI", "d", "", EntryAction::Effect("b")),
        ]);
        for c in "alpha".chars() {
            palette.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(palette.filtered_titles(), vec!["alpha one", "alpha two"]);
    }

    // --- palette state machine ---

    #[test]
    fn open_filter_navigate_execute() {
        let mut palette = Palette::new();
        palette.open(sample_entries());
        assert!(palette.is_open());
        assert_eq!(palette.filtered_titles().len(), 6);

        for c in "pong".chars() {
            assert_eq!(palette.handle_key(key(KeyCode::Char(c))), PaletteAction::Consumed);
        }
        assert_eq!(palette.filtered_titles()[0], "Game: PONG [multiplayer]");

        match palette.handle_key(key(KeyCode::Enter)) {
            PaletteAction::Execute { action, .. } => {
                assert_eq!(action, EntryAction::Game(GameKind::Pong));
            }
            other => panic!("expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn required_arg_command_executes_as_insert() {
        let mut palette = Palette::new();
        palette.open(sample_entries());
        for c in "join".chars() {
            palette.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(palette.filtered_titles()[0], "/join");
        match palette.handle_key(key(KeyCode::Enter)) {
            PaletteAction::Execute { action, .. } => {
                assert_eq!(action, EntryAction::Insert("/join ".to_string()));
            }
            other => panic!("expected Execute, got {:?}", other),
        }
    }

    #[test]
    fn esc_and_ctrl_p_close_without_executing() {
        let mut palette = Palette::new();
        palette.open(sample_entries());
        assert_eq!(palette.handle_key(key(KeyCode::Esc)), PaletteAction::Close);
        assert_eq!(palette.handle_key(ctrl('p')), PaletteAction::Close);
    }

    #[test]
    fn selection_clamps_and_scrolls() {
        let mut palette = Palette::new();
        palette.open(sample_entries());
        palette.view_rows = 2;
        // walk past the end: selection clamps at the last entry
        for _ in 0..20 {
            palette.handle_key(key(KeyCode::Down));
        }
        assert_eq!(palette.selected, 5);
        assert!(palette.scroll >= 4, "viewport must follow the selection");
        for _ in 0..20 {
            palette.handle_key(key(KeyCode::Up));
        }
        assert_eq!(palette.selected, 0);
        assert_eq!(palette.scroll, 0);
    }

    #[test]
    fn backspace_widens_the_filter() {
        let mut palette = Palette::new();
        palette.open(sample_entries());
        for c in "torus".chars() {
            palette.handle_key(key(KeyCode::Char(c)));
        }
        // scattered subsequences in long descriptions may also match, but the
        // real hit must rank first and the set must be narrowed
        assert_eq!(palette.filtered_titles()[0], "FX: Torus Knot");
        assert!(palette.filtered_titles().len() < 6);
        for _ in 0..5 {
            palette.handle_key(key(KeyCode::Backspace));
        }
        assert_eq!(palette.filtered_titles().len(), 6);
    }

    #[test]
    fn command_entries_map_arg_specs() {
        // a Required-arg spec becomes Insert; a no-arg spec becomes Command
        let join = crate::commands::find("/join").unwrap();
        let help = crate::commands::find("/help").unwrap();
        assert_eq!(
            PaletteEntry::for_command(join).action,
            EntryAction::Insert("/join ".to_string())
        );
        assert_eq!(
            PaletteEntry::for_command(help).action,
            EntryAction::Command { id: CommandId::Help, args: "" }
        );
    }
}
