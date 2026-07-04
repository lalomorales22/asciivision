//! Arcade bay: registry-driven game panel.
//!
//! Every game implements the [`Game`] trait and is listed in [`GameKind::ALL`];
//! the selector, number keys, and `/games <name>` parsing all derive from that
//! one table. Networked games (Pong, Tron) talk to the outside world only via
//! the abstract `(game, payload)` sender injected with [`GamesPanel::set_net`]
//! and the inbound [`GamesPanel::handle_net`] entry point.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};

use crate::theme::{t, Theme};

mod breakout;
mod invaders;
mod net;
mod pacman;
mod penguin;
mod pong;
mod snake;
mod tron;

// Re-exported for the stage-2 games<->network glue (not referenced by main.rs yet).
#[allow(unused_imports)]
pub use net::NetRole;
use net::NetHandle;

// ---------------------------------------------------------------------------
// Game trait
// ---------------------------------------------------------------------------

/// The contract every arcade game implements.
pub(crate) trait Game {
    fn tick(&mut self, dt: f32);
    /// Returns true when the key was consumed by the game.
    fn handle_key(&mut self, key: KeyEvent) -> bool;
    fn render(&self, buffer: &mut Buffer, area: Rect);
    fn status(&self) -> Option<String> {
        None
    }
    /// Inbound network payload addressed to this game (already name-routed).
    fn handle_net(&mut self, _from_id: &str, _from_name: &str, _payload: &serde_json::Value) {}
    /// A peer left the room (`Some(id)`) or our own connection dropped
    /// (`None`). Online games treat a matching opponent as having quit.
    fn peer_disconnected(&mut self, _from_id: Option<&str>) {}
}

// ---------------------------------------------------------------------------
// GameKind registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameKind {
    PacMan,
    SpaceInvaders,
    Penguin3D,
    Pong,
    Tron,
    Snake,
    Breakout,
}

impl GameKind {
    pub const ALL: [Self; 7] = [
        Self::PacMan,
        Self::SpaceInvaders,
        Self::Penguin3D,
        Self::Pong,
        Self::Tron,
        Self::Snake,
        Self::Breakout,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PacMan => "PAC-MAN",
            Self::SpaceInvaders => "SPACE INVADERS",
            Self::Penguin3D => "3D PENGUIN",
            Self::Pong => "PONG",
            Self::Tron => "TRON CYCLES",
            Self::Snake => "SNAKE",
            Self::Breakout => "BREAKOUT",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Self::PacMan => "maze pellets, ghosts, power mode",
            Self::SpaceInvaders => "retro lane shooter with shield pulse",
            Self::Penguin3D => "snow-run fish collector with faux 3D view",
            Self::Pong => "paddle duel: AI, local 2P, or online",
            Self::Tron => "light cycles: trap rivals, rounds to 3",
            Self::Snake => "eat, grow, speed ramps, walls kill",
            Self::Breakout => "brick-buster with paddle english",
        }
    }

    /// True for games that support online host/join play.
    pub fn multiplayer(self) -> bool {
        matches!(self, Self::Pong | Self::Tron)
    }

    /// Wire name used to route network payloads.
    pub fn net_name(self) -> Option<&'static str> {
        match self {
            Self::Pong => Some("pong"),
            Self::Tron => Some("tron"),
            _ => None,
        }
    }

    pub fn from_net_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.net_name() == Some(name))
    }

    fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::PacMan => &["pac", "pacman", "pac-man"],
            Self::SpaceInvaders => &["space", "invaders", "space-invaders", "space invaders"],
            Self::Penguin3D => &["penguin", "3d", "3d-penguin", "peng"],
            Self::Pong => &["pong", "paddle"],
            Self::Tron => &["tron", "cycles", "lightcycles", "light cycles", "light-cycles"],
            Self::Snake => &["snake", "snek"],
            Self::Breakout => &["breakout", "bricks", "brick"],
        }
    }

    pub fn from_input(input: &str) -> Option<Self> {
        let needle = input.trim().to_lowercase();
        if let Ok(number) = needle.parse::<usize>() {
            if (1..=Self::ALL.len()).contains(&number) {
                return Some(Self::ALL[number - 1]);
            }
            return None;
        }
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.aliases().contains(&needle.as_str()))
    }

    fn cycle_next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn cycle_prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    fn create(self, net: NetHandle) -> Box<dyn Game> {
        match self {
            Self::PacMan => Box::new(pacman::PacManGame::new()),
            Self::SpaceInvaders => Box::new(invaders::SpaceInvadersGame::new()),
            Self::Penguin3D => Box::new(penguin::PenguinGame::new()),
            Self::Pong => Box::new(pong::PongGame::new(net)),
            Self::Tron => Box::new(tron::TronGame::new(net)),
            Self::Snake => Box::new(snake::SnakeGame::new()),
            Self::Breakout => Box::new(breakout::BreakoutGame::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// GamesPanel
// ---------------------------------------------------------------------------

struct Session {
    kind: GameKind,
    game: Box<dyn Game>,
}

struct PendingInvite {
    from_name: String,
    kind: GameKind,
}

pub struct GamesPanel {
    selected: GameKind,
    session: Option<Session>,
    status: String,
    net: NetHandle,
    pending_invite: Option<PendingInvite>,
    session_generation: u64,
}

impl GamesPanel {
    pub fn new() -> Self {
        Self {
            selected: GameKind::PacMan,
            session: None,
            status: "games bay online".to_string(),
            net: NetHandle::new(),
            pending_invite: None,
            session_generation: 0,
        }
    }

    pub fn status_note(&self) -> &str {
        &self.status
    }

    /// Kind of the currently running game, if any.
    #[allow(dead_code)] // stage-2 glue surface
    pub fn active_kind(&self) -> Option<GameKind> {
        self.session.as_ref().map(|session| session.kind)
    }

    /// Monotonic counter bumped every time a session is (re)created.
    #[allow(dead_code)] // stage-2 glue surface / tests
    pub fn session_generation(&self) -> u64 {
        self.session_generation
    }

    /// Inject (or clear) the outbound network sender used by online games.
    /// `tx` carries `(game_name, payload)` pairs; `my_id` is our connection id.
    pub fn set_net(
        &mut self,
        tx: Option<tokio::sync::mpsc::UnboundedSender<(String, serde_json::Value)>>,
        my_id: Option<String>,
    ) {
        self.net.configure(tx, my_id);
    }

    /// Inbound game payload from the network layer.
    pub fn handle_net(
        &mut self,
        from_id: &str,
        from_name: &str,
        game: &str,
        payload: &serde_json::Value,
    ) {
        if self.net.my_id().as_deref() == Some(from_id) {
            return; // defensive: ignore echoes of our own messages
        }
        if let Some(session) = &mut self.session {
            if session.kind.net_name() == Some(game) {
                session.game.handle_net(from_id, from_name, payload);
                return;
            }
        }
        // No matching session running: surface invites in the selector.
        let tag = payload.get("t").and_then(|value| value.as_str()).unwrap_or("");
        if let Some(kind) = GameKind::from_net_name(game) {
            match tag {
                "invite" => {
                    self.status = format!(
                        "games: {} wants to play {} — /games {} then press 4 to JOIN",
                        from_name,
                        kind.label(),
                        game
                    );
                    self.pending_invite = Some(PendingInvite {
                        from_name: from_name.to_string(),
                        kind,
                    });
                }
                "quit" => {
                    if self
                        .pending_invite
                        .as_ref()
                        .is_some_and(|invite| invite.kind == kind && invite.from_name == from_name)
                    {
                        self.pending_invite = None;
                    }
                }
                _ => {}
            }
        }
    }

    /// Network layer lost a peer, or the local connection itself.
    /// `Some(id)`: that user left the room — an online session whose locked
    /// opponent has this id ends as if they sent quit. `None`: we
    /// disconnected — any online session ends. Local games are unaffected.
    pub fn peer_disconnected(&mut self, from_id: Option<&str>) {
        if let Some(session) = &mut self.session {
            session.game.peer_disconnected(from_id);
            if let Some(status) = session.game.status() {
                self.status = status;
            }
        }
    }

    pub fn next_game(&mut self) {
        self.selected = self.selected.cycle_next();
        self.status = format!("games: selected {}", self.selected.label());
    }

    pub fn previous_game(&mut self) {
        self.selected = self.selected.cycle_prev();
        self.status = format!("games: selected {}", self.selected.label());
    }

    pub fn activate_selected(&mut self) {
        self.start_session(self.selected);
    }

    pub fn launch(&mut self, game: GameKind) {
        self.selected = game;
        self.activate_selected();
    }

    fn start_session(&mut self, kind: GameKind) {
        // Drop the old session first so any online game sends its quit.
        self.session = None;
        self.session = Some(Session {
            kind,
            game: kind.create(self.net.clone()),
        });
        self.session_generation = self.session_generation.wrapping_add(1);
        if self
            .pending_invite
            .as_ref()
            .is_some_and(|invite| invite.kind == kind)
        {
            self.pending_invite = None;
        }
        self.status = format!("games: {}", kind.label());
    }

    pub fn stop(&mut self) {
        self.session = None;
        self.status = "games: selector ready".to_string();
    }

    pub fn tick(&mut self, dt: f32) {
        if let Some(session) = &mut self.session {
            session.game.tick(dt.min(0.05));
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if let Some(session) = &mut self.session {
            if key.code == KeyCode::Esc {
                self.stop();
                return true;
            }
            // The game gets first refusal (mode menus use digits, etc.).
            if session.game.handle_key(key) {
                self.status = session
                    .game
                    .status()
                    .unwrap_or_else(|| format!("games: {}", session.kind.label()));
                return true;
            }
            if let Some(kind) = digit_selection(key.code) {
                if kind == session.kind {
                    // Same digit as the running game: do NOT restart it.
                    return true;
                }
                self.launch(kind);
                return true;
            }
            if matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
                let kind = session.kind;
                self.start_session(kind);
                self.status = format!("games: restarted {}", kind.label());
                return true;
            }
            return false;
        }

        if let Some(kind) = digit_selection(key.code) {
            self.launch(kind);
            return true;
        }

        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.activate_selected();
                true
            }
            KeyCode::Up | KeyCode::Left => {
                self.previous_game();
                true
            }
            KeyCode::Down | KeyCode::Right => {
                self.next_game();
                true
            }
            KeyCode::Char('w') | KeyCode::Char('W') | KeyCode::Char('a') | KeyCode::Char('A') => {
                self.previous_game();
                true
            }
            KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Char('d') | KeyCode::Char('D') => {
                self.next_game();
                true
            }
            _ => false,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, phase: f32, is_focused: bool) {
        let title = if let Some(session) = &self.session {
            format!(" GAMES // {} ", session.kind.label())
        } else {
            " GAMES // SELECT ".to_string()
        };
        let border_color = if is_focused { t().accent4 } else { t().accent1 };
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(t().accent2).bold())
            .borders(Borders::ALL)
            .border_type(if is_focused {
                BorderType::Double
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(border_color));
        frame.render_widget(block, area);

        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        if inner.width < 18 || inner.height < 8 {
            frame.render_widget(
                Paragraph::new("Grow this tile to play.")
                    .style(Style::default().fg(t().muted).bg(t().panel_bg))
                    .alignment(Alignment::Center),
                inner,
            );
            return;
        }

        if let Some(session) = &self.session {
            session.game.render(frame.buffer_mut(), inner);
        } else {
            self.render_menu(frame, inner, phase);
        }
    }

    fn render_menu(&self, frame: &mut Frame, area: Rect, phase: f32) {
        let spinner = ["|", "/", "-", "\\"][((phase * 7.0) as usize) % 4];
        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    format!("{} ARCADE BAY READY ", spinner),
                    Style::default().fg(t().accent4).bold(),
                ),
                Span::styled(
                    format!(
                        "Focus this tile, then use 1-{} or WASD to choose.",
                        GameKind::ALL.len()
                    ),
                    Style::default().fg(t().text),
                ),
            ]),
            Line::from(""),
        ];

        if let Some(invite) = &self.pending_invite {
            lines.push(Line::from(Span::styled(
                format!(
                    "* {} wants to play {} — open it and pick JOIN (4)",
                    invite.from_name,
                    invite.kind.label()
                ),
                Style::default().fg(t().accent2).bold(),
            )));
            lines.push(Line::from(""));
        }

        for (idx, game) in GameKind::ALL.iter().enumerate() {
            let selected = *game == self.selected;
            let accent = if selected { t().accent2 } else { t().muted };
            let pointer = if selected { ">" } else { " " };
            let mut spans = vec![Span::styled(
                format!("{} {}. {:<15}", pointer, idx + 1, game.label()),
                Style::default().fg(accent).bold(),
            )];
            spans.push(Span::styled(game.subtitle(), Style::default().fg(t().text)));
            if game.multiplayer() {
                spans.push(Span::styled(
                    " [MULTIPLAYER]",
                    Style::default().fg(t().accent3).bold(),
                ));
            }
            lines.push(Line::from(spans));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Enter/Space", Style::default().fg(t().accent4).bold()),
            Span::styled(" launch selected game", Style::default().fg(t().text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Active Tile", Style::default().fg(t().accent4).bold()),
            Span::styled(
                " WASD routes into the game only when this panel is focused and the prompt is empty",
                Style::default().fg(t().text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Esc", Style::default().fg(t().accent4).bold()),
            Span::styled(" back to selector", Style::default().fg(t().text)),
        ]));

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .style(Style::default().bg(t().panel_bg))
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

fn digit_selection(code: KeyCode) -> Option<GameKind> {
    if let KeyCode::Char(ch) = code {
        if let Some(digit) = ch.to_digit(10) {
            if digit >= 1 {
                return GameKind::ALL.get(digit as usize - 1).copied();
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Shared helpers used by the individual games
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Dir {
    Up,
    Right,
    Down,
    Left,
}

impl Dir {
    pub(crate) const ALL: [Self; 4] = [Self::Up, Self::Right, Self::Down, Self::Left];

    pub(crate) fn delta(self) -> (i32, i32) {
        match self {
            Self::Up => (0, -1),
            Self::Right => (1, 0),
            Self::Down => (0, 1),
            Self::Left => (-1, 0),
        }
    }

    pub(crate) fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Right => Self::Left,
            Self::Down => Self::Up,
            Self::Left => Self::Right,
        }
    }

    pub(crate) fn code(self) -> u8 {
        match self {
            Self::Up => 0,
            Self::Right => 1,
            Self::Down => 2,
            Self::Left => 3,
        }
    }

    pub(crate) fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Up),
            1 => Some(Self::Right),
            2 => Some(Self::Down),
            3 => Some(Self::Left),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct GridCell {
    ch: char,
    fg: Color,
    bg: Color,
}

/// Software framebuffer the games plot into; blitted to the ratatui buffer
/// via [`CellGrid::present`]. `set` clips out-of-bounds writes.
pub(crate) struct CellGrid {
    pub(crate) width: u16,
    pub(crate) height: u16,
    cells: Vec<GridCell>,
}

impl CellGrid {
    pub(crate) fn new(width: u16, height: u16, bg: Color, fg: Color) -> Self {
        Self {
            width,
            height,
            cells: vec![GridCell { ch: ' ', fg, bg }; width as usize * height as usize],
        }
    }

    pub(crate) fn set(&mut self, x: i32, y: i32, ch: char, fg: Color, bg: Color) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let idx = y as usize * self.width as usize + x as usize;
        if let Some(cell) = self.cells.get_mut(idx) {
            *cell = GridCell { ch, fg, bg };
        }
    }

    pub(crate) fn text(&mut self, x: i32, y: i32, text: &str, fg: Color, bg: Color) {
        for (i, ch) in text.chars().enumerate() {
            self.set(x + i as i32, y, ch, fg, bg);
        }
    }

    pub(crate) fn center_text(&mut self, y: i32, text: &str, fg: Color, bg: Color) {
        let width = text.chars().count() as i32;
        let x = ((self.width as i32 - width) / 2).max(0);
        self.text(x, y, text, fg, bg);
    }

    pub(crate) fn present(&self, buffer: &mut Buffer, area: Rect) {
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y as usize * self.width as usize + x as usize;
                if let Some(cell) = self.cells.get(idx) {
                    if let Some(buf_cell) = buffer.cell_mut((area.x + x, area.y + y)) {
                        buf_cell.set_char(cell.ch);
                        buf_cell.set_fg(cell.fg);
                        buf_cell.set_bg(cell.bg);
                    }
                }
            }
        }
    }
}

pub(crate) fn project_axis(value: f32, src_max: f32, dst: u16) -> i32 {
    if dst == 0 {
        return 0;
    }
    let scaled = (value / src_max.max(1.0)) * dst.saturating_sub(1) as f32;
    scaled.round() as i32
}

/// Shared mode-select menu for multiplayer-capable games.
/// Returns nothing; the caller interprets digits 1-4 itself.
pub(crate) fn draw_mode_menu(
    grid: &mut CellGrid,
    label: &str,
    error: Option<&str>,
    theme: &Theme,
) {
    let top = (grid.height as i32 / 2 - 6).max(1);
    grid.center_text(top, label, theme.accent2, theme.panel_bg);
    grid.center_text(top + 1, "SELECT MODE", theme.accent4, theme.panel_bg);
    grid.center_text(top + 3, "1  VS COMPUTER", theme.text, theme.panel_bg);
    grid.center_text(top + 4, "2  LOCAL 2P (W/S vs ARROWS)", theme.text, theme.panel_bg);
    grid.center_text(top + 5, "3  HOST ONLINE MATCH", theme.text, theme.panel_bg);
    grid.center_text(top + 6, "4  JOIN ONLINE MATCH", theme.text, theme.panel_bg);
    if let Some(error) = error {
        grid.center_text(top + 8, error, theme.danger, theme.panel_bg);
    }
    grid.center_text(top + 10, "Esc back to arcade", theme.muted, theme.panel_bg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn from_input_accepts_digits_and_aliases() {
        assert_eq!(GameKind::from_input("1"), Some(GameKind::PacMan));
        assert_eq!(GameKind::from_input("pac-man"), Some(GameKind::PacMan));
        assert_eq!(GameKind::from_input("space invaders"), Some(GameKind::SpaceInvaders));
        assert_eq!(GameKind::from_input("PONG"), Some(GameKind::Pong));
        assert_eq!(GameKind::from_input("tron"), Some(GameKind::Tron));
        assert_eq!(GameKind::from_input("snake"), Some(GameKind::Snake));
        assert_eq!(GameKind::from_input("bricks"), Some(GameKind::Breakout));
        assert_eq!(GameKind::from_input("7"), Some(GameKind::Breakout));
        assert_eq!(GameKind::from_input("9"), None);
        assert_eq!(GameKind::from_input("doom"), None);
    }

    #[test]
    fn registry_is_consistent() {
        for (idx, kind) in GameKind::ALL.iter().enumerate() {
            assert_eq!(kind.index(), idx);
            // every alias parses back to the same kind
            for alias in kind.aliases() {
                assert_eq!(GameKind::from_input(alias), Some(*kind));
            }
            // digit parsing matches ALL order
            assert_eq!(GameKind::from_input(&(idx + 1).to_string()), Some(*kind));
        }
        assert!(GameKind::Pong.multiplayer());
        assert!(GameKind::Tron.multiplayer());
        assert!(!GameKind::Snake.multiplayer());
        assert_eq!(GameKind::from_net_name("pong"), Some(GameKind::Pong));
        assert_eq!(GameKind::from_net_name("tron"), Some(GameKind::Tron));
        assert_eq!(GameKind::from_net_name("snake"), None);
    }

    #[test]
    fn same_digit_does_not_restart_running_game() {
        let mut panel = GamesPanel::new();
        panel.launch(GameKind::PacMan);
        let generation = panel.session_generation();
        assert_eq!(panel.active_kind(), Some(GameKind::PacMan));

        // Pressing '1' (Pac-Man's digit) again must not recreate the session.
        assert!(panel.handle_key(key(KeyCode::Char('1'))));
        assert_eq!(panel.session_generation(), generation);
        assert_eq!(panel.active_kind(), Some(GameKind::PacMan));

        // A different digit still switches games.
        assert!(panel.handle_key(key(KeyCode::Char('2'))));
        assert_eq!(panel.active_kind(), Some(GameKind::SpaceInvaders));
        assert!(panel.session_generation() > generation);
    }

    #[test]
    fn selector_keys_choose_and_launch() {
        let mut panel = GamesPanel::new();
        assert!(panel.handle_key(key(KeyCode::Down)));
        assert!(panel.handle_key(key(KeyCode::Enter)));
        assert_eq!(panel.active_kind(), Some(GameKind::SpaceInvaders));
        // Esc in a session returns to the selector...
        assert!(panel.handle_key(key(KeyCode::Esc)));
        assert_eq!(panel.active_kind(), None);
        // ...and Esc in the selector is NOT consumed (falls through to app).
        assert!(!panel.handle_key(key(KeyCode::Esc)));
    }

    #[test]
    fn peer_disconnected_reaches_online_session() {
        let mut panel = GamesPanel::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        panel.set_net(Some(tx), Some("me".to_string()));
        panel.launch(GameKind::Pong);
        // Enter online play as guest, then let the host's start lock the match.
        assert!(panel.handle_key(key(KeyCode::Char('4'))));
        assert!(panel.status_note().contains("searching for a host"));
        panel.handle_net(
            "host-id",
            "HOSTY",
            "pong",
            &serde_json::json!({"t": "start", "seed": 9}),
        );

        // An unrelated user leaving changes nothing (still in the match).
        panel.peer_disconnected(Some("bystander-id"));
        assert!(
            panel.status_note().contains("ONLINE vs HOSTY"),
            "match must survive unrelated leavers, got: {}",
            panel.status_note()
        );

        // The opponent dropping ends the match and surfaces the menu status.
        panel.peer_disconnected(Some("host-id"));
        assert!(
            panel.status_note().contains("choose a mode"),
            "session must fall back to the mode menu, got: {}",
            panel.status_note()
        );

        // A local disconnect (None) ends any online session too.
        assert!(panel.handle_key(key(KeyCode::Char('4'))));
        panel.handle_net(
            "host-id",
            "HOSTY",
            "pong",
            &serde_json::json!({"t": "start", "seed": 9}),
        );
        panel.peer_disconnected(None);
        assert!(panel.status_note().contains("choose a mode"));
    }

    #[test]
    fn idle_invite_sets_banner_and_status() {
        let mut panel = GamesPanel::new();
        let payload = serde_json::json!({"t": "invite"});
        panel.handle_net("id-7", "K7", "pong", &payload);
        assert!(panel.pending_invite.is_some());
        assert!(panel.status_note().contains("K7"));
        assert!(panel.status_note().contains("PONG"));
        // opponent quitting clears the banner
        let quit = serde_json::json!({"t": "quit"});
        panel.handle_net("id-7", "K7", "pong", &quit);
        assert!(panel.pending_invite.is_none());
    }
}
