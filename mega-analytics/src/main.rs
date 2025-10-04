use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use notify::{Watcher, RecursiveMode, Event as NotifyEvent, EventKind};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Tabs, Wrap},
};
use rusqlite::{Connection, Result as SqlResult};
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::Duration;

mod video;
use video::VideoPlayer;

#[derive(Debug, Clone, PartialEq)]
enum AppState {
    Loading,
    Dashboard,
    Exiting,
}

#[derive(Debug, Clone)]
struct Message {
    id: i64,
    role: String,
    content: String,
    timestamp: i64,
}

#[derive(Debug, Clone)]
struct Stats {
    total_messages: usize,
    user_messages: usize,
    assistant_messages: usize,
    first_message: Option<DateTime<Local>>,
    last_message: Option<DateTime<Local>>,
}

struct Database {
    conn: Connection,
    db_path: PathBuf,
}

impl Database {
    fn new() -> Result<Self> {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        let db_path = PathBuf::from(home).join(".config/mega-cli/conversations.db");

        if !db_path.exists() {
            anyhow::bail!("Database not found at {:?}. Have you used mega-cli yet?", db_path);
        }

        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open database at {:?}", db_path))?;

        Ok(Self { conn, db_path })
    }

    fn get_db_path(&self) -> &PathBuf {
        &self.db_path
    }

    fn reload(&mut self) -> Result<()> {
        self.conn = Connection::open(&self.db_path)
            .with_context(|| format!("Failed to reopen database at {:?}", self.db_path))?;
        Ok(())
    }

    fn get_messages(&self, provider: &str) -> Result<Vec<Message>> {
        let table_name = format!("{}_messages", provider.to_lowercase());
        let query = format!("SELECT id, role, content, timestamp FROM {} ORDER BY id ASC", table_name);

        let mut stmt = self.conn.prepare(&query)?;
        let messages = stmt
            .query_map([], |row| {
                Ok(Message {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    timestamp: row.get(3)?,
                })
            })?
            .collect::<SqlResult<Vec<_>>>()?;

        Ok(messages)
    }

    fn get_stats(&self, provider: &str) -> Result<Stats> {
        let messages = self.get_messages(provider)?;

        let user_messages = messages.iter().filter(|m| m.role == "user").count();
        let assistant_messages = messages.iter().filter(|m| m.role == "assistant").count();

        let first_message = messages.first().map(|m| {
            DateTime::from_timestamp(m.timestamp, 0)
                .unwrap()
                .with_timezone(&Local)
        });
        let last_message = messages.last().map(|m| {
            DateTime::from_timestamp(m.timestamp, 0)
                .unwrap()
                .with_timezone(&Local)
        });

        Ok(Stats {
            total_messages: messages.len(),
            user_messages,
            assistant_messages,
            first_message,
            last_message,
        })
    }
}

struct App {
    state: AppState,
    video_player: Option<VideoPlayer>,
    db: Database,
    selected_provider: usize,
    providers: Vec<&'static str>,
    messages: Vec<Message>,
    stats: Stats,
    scroll_offset: usize,
    view_mode: ViewMode,
    last_update: String,
}

#[derive(Debug, Clone, PartialEq)]
enum ViewMode {
    Stats,
    Messages,
}

impl App {
    fn new(skip_loading: bool) -> Result<Self> {
        let video_player = if !skip_loading {
            Some(VideoPlayer::new("loading.mp4")?)
        } else {
            None
        };

        let db = Database::new()?;
        let providers = vec!["claude", "grok", "gpt", "gemini"];
        let messages = db.get_messages(providers[0])?;
        let stats = db.get_stats(providers[0])?;
        let last_update = Local::now().format("%H:%M:%S").to_string();

        Ok(Self {
            state: if skip_loading { AppState::Dashboard } else { AppState::Loading },
            video_player,
            db,
            selected_provider: 0,
            providers,
            messages,
            stats,
            scroll_offset: 0,
            view_mode: ViewMode::Stats,
            last_update,
        })
    }

    fn switch_provider(&mut self, index: usize) -> Result<()> {
        self.selected_provider = index;
        self.refresh_data()?;
        self.scroll_offset = 0;
        Ok(())
    }

    fn refresh_data(&mut self) -> Result<()> {
        self.db.reload()?;
        self.messages = self.db.get_messages(self.providers[self.selected_provider])?;
        self.stats = self.db.get_stats(self.providers[self.selected_provider])?;
        self.last_update = Local::now().format("%H:%M:%S").to_string();
        Ok(())
    }

    fn handle_input(&mut self) -> Result<bool> {
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                match self.state {
                    AppState::Loading => {
                        // Only allow quitting during loading
                        if key.code == KeyCode::Char('q')
                            || key.code == KeyCode::Esc
                            || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')) {
                            self.state = AppState::Exiting;
                            return Ok(true);
                        }
                    }
                    AppState::Dashboard => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                            self.state = AppState::Exiting;
                            return Ok(true);
                        }

                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => {
                                self.state = AppState::Exiting;
                                return Ok(true);
                            }
                            KeyCode::Tab => {
                                self.view_mode = match self.view_mode {
                                    ViewMode::Stats => ViewMode::Messages,
                                    ViewMode::Messages => ViewMode::Stats,
                                };
                            }
                            KeyCode::Left => {
                                if self.selected_provider > 0 {
                                    self.switch_provider(self.selected_provider - 1)?;
                                }
                            }
                            KeyCode::Right => {
                                if self.selected_provider < self.providers.len() - 1 {
                                    self.switch_provider(self.selected_provider + 1)?;
                                }
                            }
                            KeyCode::Char('1') => self.switch_provider(0)?,
                            KeyCode::Char('2') => self.switch_provider(1)?,
                            KeyCode::Char('3') => self.switch_provider(2)?,
                            KeyCode::Char('4') => self.switch_provider(3)?,
                            KeyCode::Up => {
                                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                            }
                            KeyCode::Down => {
                                if self.scroll_offset < self.messages.len().saturating_sub(1) {
                                    self.scroll_offset += 1;
                                }
                            }
                            KeyCode::PageUp => {
                                self.scroll_offset = self.scroll_offset.saturating_sub(10);
                            }
                            KeyCode::PageDown => {
                                self.scroll_offset = (self.scroll_offset + 10).min(self.messages.len().saturating_sub(1));
                            }
                            KeyCode::Home => {
                                self.scroll_offset = 0;
                            }
                            KeyCode::End => {
                                self.scroll_offset = self.messages.len().saturating_sub(1);
                            }
                            _ => {}
                        }
                    }
                    AppState::Exiting => return Ok(true),
                }
            }
        }
        Ok(false)
    }

    fn update(&mut self) -> Result<()> {
        match self.state {
            AppState::Loading => {
                if let Some(ref mut player) = self.video_player {
                    if player.is_finished() {
                        self.state = AppState::Dashboard;
                    }
                }
            }
            AppState::Dashboard => {}
            AppState::Exiting => {}
        }
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();

        match self.state {
            AppState::Loading => {
                if let Some(ref mut player) = self.video_player {
                    let _ = player.render(frame);
                } else {
                    // Fallback if no video
                    let loading = Paragraph::new("MEGA-Analytics Loading...")
                        .alignment(Alignment::Center)
                        .block(Block::default().borders(Borders::ALL).title("MEGA-Analytics"));
                    frame.render_widget(loading, area);
                }
            }
            AppState::Dashboard => {
                // Main layout
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),  // Title
                        Constraint::Length(3),  // Tabs
                        Constraint::Min(5),     // Content
                        Constraint::Length(2),  // Footer
                    ])
                    .split(area);

                // Title
                let title_text = format!("MEGA-CLI Analytics Dashboard 📊  [Last Update: {}]", self.last_update);
                let title = Paragraph::new(title_text)
                    .style(Style::default().fg(Color::Cyan).bold())
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).border_type(ratatui::widgets::BorderType::Thick));
                frame.render_widget(title, chunks[0]);

                // Provider tabs
                let titles: Vec<_> = self.providers.iter().map(|p| p.to_uppercase()).collect();
                let tabs = Tabs::new(titles)
                    .block(Block::default().borders(Borders::ALL).title("AI Providers"))
                    .select(self.selected_provider)
                    .style(Style::default().fg(Color::White))
                    .highlight_style(Style::default().fg(Color::Yellow).bold());
                frame.render_widget(tabs, chunks[1]);

                // Content
                match self.view_mode {
                    ViewMode::Stats => self.render_stats(frame, chunks[2]),
                    ViewMode::Messages => self.render_messages(frame, chunks[2]),
                }

                // Footer
                let footer_text = "←/→ Switch AI | 1-4 Quick Switch | Tab Toggle View | ↑/↓ Scroll | q/Esc Exit";
                let footer = Paragraph::new(footer_text)
                    .style(Style::default().fg(Color::DarkGray))
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL));
                frame.render_widget(footer, chunks[3]);
            }
            AppState::Exiting => {
                let goodbye = Paragraph::new("Goodbye! 👋")
                    .alignment(Alignment::Center);
                frame.render_widget(goodbye, area);
            }
        }
    }

    fn render_stats(&self, frame: &mut Frame, area: Rect) {
        let provider_name = self.providers[self.selected_provider].to_uppercase();

        let stats_text = if self.stats.total_messages == 0 {
            format!(
                "📭 No conversations found for {}\n\n\
                Start chatting with mega-cli to see analytics here!",
                provider_name
            )
        } else {
            let first = self.stats.first_message
                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "N/A".to_string());
            let last = self.stats.last_message
                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "N/A".to_string());

            format!(
                "📊 Statistics for {}\n\n\
                Total Messages:      {}\n\
                User Messages:       {}\n\
                Assistant Messages:  {}\n\n\
                First Message:       {}\n\
                Last Message:        {}\n\n\
                Press Tab to view full conversation history.",
                provider_name,
                self.stats.total_messages,
                self.stats.user_messages,
                self.stats.assistant_messages,
                first,
                last
            )
        };

        let stats = Paragraph::new(stats_text)
            .style(Style::default().fg(Color::Green))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Statistics ")
                    .border_style(Style::default().fg(Color::Green)),
            );

        frame.render_widget(stats, area);
    }

    fn render_messages(&mut self, frame: &mut Frame, area: Rect) {
        if self.messages.is_empty() {
            let empty = Paragraph::new("No messages yet. Start chatting with mega-cli!")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Conversation History ")
                );
            frame.render_widget(empty, area);
            return;
        }

        let provider_name = self.providers[self.selected_provider].to_uppercase();

        let messages_list: Vec<ListItem> = self
            .messages
            .iter()
            .skip(self.scroll_offset)
            .map(|msg| {
                let timestamp = DateTime::from_timestamp(msg.timestamp, 0)
                    .unwrap()
                    .with_timezone(&Local)
                    .format("%H:%M:%S");

                let (prefix, style) = match msg.role.as_str() {
                    "user" => ("You".to_string(), Style::default().fg(Color::Cyan)),
                    "assistant" => (
                        provider_name.clone(),
                        Style::default().fg(Color::Yellow),
                    ),
                    _ => ("Unknown".to_string(), Style::default().fg(Color::Red)),
                };

                let text = format!("[{}] {}: {}", timestamp, prefix, msg.content);
                ListItem::new(text).style(style)
            })
            .collect();

        let messages_widget = List::new(messages_list).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " Conversation History ({}/{}) ",
                    self.scroll_offset + 1,
                    self.messages.len()
                ))
                .border_style(Style::default().fg(Color::Blue)),
        );

        frame.render_widget(messages_widget, area);

        // Render scrollbar
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(self.messages.len())
            .position(self.scroll_offset);
        frame.render_stateful_widget(
            scrollbar,
            area.inner(Margin { vertical: 1, horizontal: 0 }),
            &mut scrollbar_state,
        );
    }
}

fn main() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = match App::new(false) {
        Ok(app) => app,
        Err(e) => {
            // Restore terminal before showing error
            disable_raw_mode()?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // Set up file watcher
    let (tx, rx) = channel();
    let db_path = app.db.get_db_path().clone();

    let mut watcher = notify::recommended_watcher(move |res: Result<NotifyEvent, notify::Error>| {
        if let Ok(event) = res {
            // Only trigger on modify events
            if matches!(event.kind, EventKind::Modify(_)) {
                let _ = tx.send(());
            }
        }
    })?;

    watcher.watch(&db_path, RecursiveMode::NonRecursive)?;

    // Main loop
    loop {
        terminal.draw(|f| app.render(f))?;

        // Check for file changes (non-blocking) - only when in Dashboard state
        if app.state == AppState::Dashboard && rx.try_recv().is_ok() {
            // Database changed, refresh data
            let _ = app.refresh_data();
        }

        if app.handle_input()? {
            break;
        }

        // Update app state (transitions loading -> dashboard)
        app.update()?;

        // Small sleep to prevent CPU spinning
        std::thread::sleep(Duration::from_millis(16));
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
