use anyhow::{anyhow, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use parking_lot::Mutex;
use portable_pty::{
    native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize,
};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};
use std::{
    env,
    io::{Read, Write},
    sync::Arc,
    thread,
};

use crate::theme::t;

const DEFAULT_TILE_COUNT: usize = 2;
const MAX_TILE_COUNT: usize = 8;
const SCROLLBACK_LINES: usize = 4_000;

pub struct TilesPanel {
    sessions: Vec<TerminalSession>,
    active: usize,
    status: String,
}

impl TilesPanel {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            active: 0,
            status: "tiles: dormant // run /tiles to boot live shells".to_string(),
        }
    }

    pub fn status_note(&self) -> &str {
        &self.status
    }

    pub fn activate_default(&mut self) -> Result<()> {
        if self.sessions.is_empty() {
            self.activate_count(DEFAULT_TILE_COUNT)?;
        } else {
            self.status = format!(
                "tiles: {} live terminal{} // term {} active",
                self.sessions.len(),
                if self.sessions.len() == 1 { "" } else { "s" },
                self.active + 1
            );
        }
        Ok(())
    }

    pub fn activate_count(&mut self, count: usize) -> Result<()> {
        if !(1..=MAX_TILE_COUNT).contains(&count) {
            return Err(anyhow!("tiles count must be between 1 and {}", MAX_TILE_COUNT));
        }

        while self.sessions.len() < count {
            let index = self.sessions.len() + 1;
            self.sessions.push(TerminalSession::spawn(index)?);
        }
        while self.sessions.len() > count {
            self.sessions.pop();
        }

        self.active = self.active.min(self.sessions.len().saturating_sub(1));
        self.status = format!(
            "tiles: {} live terminal{} // term {} active",
            self.sessions.len(),
            if self.sessions.len() == 1 { "" } else { "s" },
            self.active + 1
        );
        Ok(())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.sessions.is_empty() {
            return false;
        }

        // Ctrl+j/k cycles between inner terminals; Ctrl+h/l is left for outer app focus
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    return self.cycle_inner_focus(1);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    return self.cycle_inner_focus(-1);
                }
                _ => {}
            }
        }

        let app_cursor = self.sessions[self.active].application_cursor();
        let bytes = key_event_bytes(key, app_cursor);
        if bytes.is_empty() {
            return false;
        }

        if !self.sessions[self.active].send_bytes(&bytes) {
            self.status = format!(
                "tiles: term {} is no longer writable // run /tiles to reopen it",
                self.active + 1
            );
        }
        true
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, is_focused: bool) {
        let title = if self.sessions.is_empty() {
            " TILES // OFFLINE ".to_string()
        } else {
            format!(" TILES // {} PTY{} ", self.sessions.len(), if self.sessions.len() == 1 { "" } else { "S" })
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
        if inner.width < 16 || inner.height < 6 {
            frame.render_widget(
                Paragraph::new("Grow this tile for live terminals.")
                    .style(Style::default().fg(t().muted).bg(t().panel_bg))
                    .alignment(Alignment::Center),
                inner,
            );
            return;
        }

        if self.sessions.is_empty() {
            frame.render_widget(
                Paragraph::new(
                    "Run /tiles or press F7 to boot live embedded terminals.\n\nUse /tiles 4 for a 2x2 shell grid, or any count from 1-8.",
                )
                .style(Style::default().fg(t().accent4).bg(t().panel_bg))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
                inner,
            );
            return;
        }

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        let info = Paragraph::new(
            "type directly into the shell | Ctrl+j/k cycle inner terminals | Ctrl+h/l move app focus",
        )
        .style(Style::default().fg(t().muted).bg(t().panel_bg))
        .wrap(Wrap { trim: false });
        frame.render_widget(info, layout[0]);

        for (index, rect) in terminal_grid(layout[1], self.sessions.len()).into_iter().enumerate() {
            if rect.width < 4 || rect.height < 3 {
                continue;
            }

            let is_active = index == self.active;
            let exit_label = self.sessions[index]
                .exit_label()
                .unwrap_or_else(|| "LIVE".to_string());
            let title = format!(" TERM {} // {} ", index + 1, exit_label);
            let border = if is_active { t().accent4 } else { t().accent3 };
            let block = Block::default()
                .title(title)
                .title_style(Style::default().fg(if is_active { t().accent2 } else { t().accent4 }).bold())
                .borders(Borders::ALL)
                .border_type(if is_active {
                    BorderType::Double
                } else {
                    BorderType::Plain
                })
                .border_style(Style::default().fg(border));
            frame.render_widget(block, rect);

            let terminal_area = rect.inner(Margin {
                horizontal: 1,
                vertical: 1,
            });
            self.sessions[index].render(frame.buffer_mut(), terminal_area, is_active);
        }
    }

    /// Cycle inner terminal focus forward (+1) or backward (-1), wrapping around.
    fn cycle_inner_focus(&mut self, direction: isize) -> bool {
        let count = self.sessions.len();
        if count <= 1 {
            return false;
        }
        let next = (self.active as isize + direction).rem_euclid(count as isize) as usize;
        self.active = next;
        self.status = format!("tiles: term {} focused", self.active + 1);
        true
    }
}

struct TerminalSession {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    exit_status: Arc<Mutex<Option<String>>>,
    last_size: (u16, u16),
}

impl TerminalSession {
    fn spawn(index: usize) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to allocate PTY")?;

        let mut command = terminal_command();
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("ASCIIVISION_TILE", index.to_string());
        if let Ok(cwd) = env::current_dir() {
            command.cwd(cwd);
        }

        let child = pair
            .slave
            .spawn_command(command)
            .context("failed to launch terminal shell")?;
        let killer = child.clone_killer();

        let exit_status = Arc::new(Mutex::new(None));
        let exit_status_wait = Arc::clone(&exit_status);
        thread::spawn(move || {
            let mut child = child;
            let status = child.wait().map(|status| {
                if let Some(signal) = status.signal() {
                    format!("EXIT {}", signal)
                } else {
                    format!("EXIT {}", status.exit_code())
                }
            });
            *exit_status_wait.lock() = Some(match status {
                Ok(status) => status,
                Err(error) => format!("EXIT {}", error),
            });
        });

        let parser = Arc::new(Mutex::new(vt100::Parser::new(24, 80, SCROLLBACK_LINES)));
        let parser_reader = Arc::clone(&parser);
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        thread::spawn(move || {
            let mut buf = [0_u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(size) => parser_reader.lock().process(&buf[..size]),
                    Err(_) => break,
                }
            }
        });

        let writer = Arc::new(Mutex::new(
            pair.master
                .take_writer()
                .context("failed to acquire PTY writer")?,
        ));

        Ok(Self {
            parser,
            writer,
            master: pair.master,
            killer,
            exit_status,
            last_size: (80, 24),
        })
    }

    fn application_cursor(&self) -> bool {
        self.parser.lock().screen().application_cursor()
    }

    fn send_bytes(&self, bytes: &[u8]) -> bool {
        let mut writer = self.writer.lock();
        writer.write_all(bytes).and_then(|_| writer.flush()).is_ok()
    }

    fn exit_label(&self) -> Option<String> {
        self.exit_status.lock().clone()
    }

    fn render(&mut self, buffer: &mut Buffer, area: Rect, is_active: bool) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        self.resize(area.width, area.height);

        let theme = t();
        let default_fg = theme.text;
        let default_bg = theme.panel_bg;

        let parser = self.parser.lock();
        let screen = parser.screen();
        let (cursor_row, cursor_col) = screen.cursor_position();
        let show_cursor = !screen.hide_cursor() && is_active;

        for row in 0..area.height {
            for col in 0..area.width {
                let x = area.x + col;
                let y = area.y + row;
                if let Some(out) = buffer.cell_mut((x, y)) {
                    if let Some(cell) = screen.cell(row, col) {
                        let symbol = if cell.is_wide_continuation() {
                            " "
                        } else if cell.has_contents() {
                            cell.contents()
                        } else {
                            " "
                        };

                        let mut fg = map_color(cell.fgcolor(), default_fg);
                        let mut bg = map_color(cell.bgcolor(), default_bg);
                        if cell.inverse() {
                            std::mem::swap(&mut fg, &mut bg);
                        }
                        let mut style = Style::default().fg(fg).bg(bg);
                        if cell.bold() {
                            style = style.add_modifier(Modifier::BOLD);
                        }
                        if cell.underline() {
                            style = style.add_modifier(Modifier::UNDERLINED);
                        }
                        if show_cursor && row == cursor_row && col == cursor_col {
                            style = style
                                .bg(t().accent2)
                                .fg(t().bg_base)
                                .add_modifier(Modifier::BOLD);
                        }

                        out.set_symbol(symbol);
                        out.set_style(style);
                    } else {
                        out.set_symbol(" ");
                        out.set_style(Style::default().fg(default_fg).bg(default_bg));
                    }
                }
            }
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 || self.last_size == (cols, rows) {
            return;
        }

        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.parser.lock().screen_mut().set_size(rows, cols);
        self.last_size = (cols, rows);
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

fn terminal_command() -> CommandBuilder {
    #[cfg(windows)]
    let shell = env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    #[cfg(not(windows))]
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());

    CommandBuilder::new(shell)
}

fn terminal_grid(area: Rect, count: usize) -> Vec<Rect> {
    let (cols, rows) = grid_dims(count);
    if cols == 0 || rows == 0 {
        return Vec::new();
    }

    let row_constraints: Vec<Constraint> = (0..rows)
        .map(|_| Constraint::Ratio(1, rows as u32))
        .collect();
    let row_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);

    let mut rects = Vec::with_capacity(count);
    for row in 0..rows {
        let col_constraints: Vec<Constraint> = (0..cols)
            .map(|_| Constraint::Ratio(1, cols as u32))
            .collect();
        let col_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(row_layout[row]);

        for col in 0..cols {
            if rects.len() == count {
                break;
            }
            rects.push(col_layout[col]);
        }
    }

    rects
}

fn grid_dims(count: usize) -> (usize, usize) {
    match count {
        0 => (0, 0),
        1 => (1, 1),
        2 => (2, 1),
        3 | 4 => (2, 2),
        5 | 6 => (3, 2),
        _ => (4, 2),
    }
}

fn map_color(color: vt100::Color, fallback: Color) -> Color {
    match color {
        vt100::Color::Default => fallback,
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn key_event_bytes(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    match key.code {
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Left => {
            if application_cursor {
                b"\x1bOD".to_vec()
            } else {
                b"\x1b[D".to_vec()
            }
        }
        KeyCode::Right => {
            if application_cursor {
                b"\x1bOC".to_vec()
            } else {
                b"\x1b[C".to_vec()
            }
        }
        KeyCode::Up => {
            if application_cursor {
                b"\x1bOA".to_vec()
            } else {
                b"\x1b[A".to_vec()
            }
        }
        KeyCode::Down => {
            if application_cursor {
                b"\x1bOB".to_vec()
            } else {
                b"\x1b[B".to_vec()
            }
        }
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Tab => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                b"\x1b[Z".to_vec()
            } else {
                vec![b'\t']
            }
        }
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Char(c) => encode_char_key(c, key.modifiers),
        _ => Vec::new(),
    }
}

fn encode_char_key(c: char, modifiers: KeyModifiers) -> Vec<u8> {
    let mut bytes = if modifiers.contains(KeyModifiers::CONTROL) {
        control_char_bytes(c)
    } else {
        c.to_string().into_bytes()
    };

    if modifiers.contains(KeyModifiers::ALT) {
        let mut prefixed = vec![0x1b];
        prefixed.append(&mut bytes);
        prefixed
    } else {
        bytes
    }
}

fn control_char_bytes(c: char) -> Vec<u8> {
    match c {
        'a'..='z' => vec![(c as u8) - b'a' + 1],
        'A'..='Z' => vec![(c as u8) - b'A' + 1],
        ' ' | '@' => vec![0x00],
        '[' => vec![0x1b],
        '\\' => vec![0x1c],
        ']' => vec![0x1d],
        '^' => vec![0x1e],
        '_' => vec![0x1f],
        '6' => vec![0x1e],
        '7' => vec![0x1f],
        '8' => vec![0x7f],
        _ => c.to_string().into_bytes(),
    }
}
