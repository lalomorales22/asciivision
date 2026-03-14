use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::time::Instant;
use tachyonfx::{fx, EffectManager, Interpolation};
use tokio::sync::mpsc;

use crate::ai::{AIProvider, AIClient, Message};
use crate::db::Database;

#[derive(Debug, Clone)]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    #[allow(dead_code)]
    pub timestamp: Instant,
    pub is_system: bool,
}

pub struct ChatInterface {
    provider: AIProvider,
    ai_client: AIClient,
    messages: Vec<ChatMessage>,
    input_buffer: String,
    scroll_offset: usize,
    is_streaming: bool,
    effects: EffectManager<()>,
    last_update: Instant,
    show_help: bool,
    response_rx: mpsc::UnboundedReceiver<(usize, Result<String>)>,
    response_tx: mpsc::UnboundedSender<(usize, Result<String>)>,
    db: Option<Database>,
    session_id: usize,
}

impl ChatInterface {
    pub fn new(provider: AIProvider) -> Self {
        let ai_client = AIClient::new(provider.clone());

        let mut effects: EffectManager<()> = EffectManager::default();
        let drift = fx::hsl_shift(
            Some([0.0, 0.0, 0.02]),
            None,
            (8_000, Interpolation::SineInOut),
        );
        effects.add_effect(drift);

        let (response_tx, response_rx) = mpsc::unbounded_channel();

        // Initialize database, log error if it fails but continue
        let db = match Database::new() {
            Ok(db) => {
                eprintln!("✓ Database initialized at ~/.config/mega-cli/conversations.db");
                Some(db)
            }
            Err(e) => {
                eprintln!("⚠ Failed to initialize database: {}", e);
                None
            }
        };

        Self {
            provider,
            ai_client,
            messages: Vec::new(),
            input_buffer: String::new(),
            scroll_offset: 0,
            is_streaming: false,
            effects,
            last_update: Instant::now(),
            show_help: false,
            response_rx,
            response_tx,
            db,
            session_id: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('l') => {
                    self.messages.clear();
                    self.scroll_offset = 0;

                    // Clear database history for current provider
                    if let Some(ref db) = self.db {
                        let _ = db.clear_history(self.provider.db_name());
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        match key.code {
            KeyCode::F(1) => {
                self.show_help = !self.show_help;
            }
            KeyCode::F(2) => {
                // Increment session ID to invalidate any pending responses
                self.session_id = self.session_id.wrapping_add(1);

                // Drain any pending responses from the old provider
                while self.response_rx.try_recv().is_ok() {}

                // Stop any streaming
                self.is_streaming = false;

                // Cycle through providers
                self.provider = match self.provider {
                    AIProvider::Claude => AIProvider::Grok,
                    AIProvider::Grok => AIProvider::OpenAI,
                    AIProvider::OpenAI => AIProvider::Gemini,
                    AIProvider::Gemini => AIProvider::Claude,
                };
                self.ai_client = AIClient::new(self.provider.clone());

                // Clear messages when switching providers
                self.messages.clear();
                self.scroll_offset = 0;

                self.add_system_message(&format!("Switched to {}", self.provider.name()));
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            KeyCode::Enter => {
                if !self.input_buffer.is_empty() && !self.is_streaming {
                    let user_input = self.input_buffer.clone();
                    self.input_buffer.clear();

                    // Add user message
                    self.messages.push(ChatMessage {
                        role: MessageRole::User,
                        content: user_input.clone(),
                        timestamp: Instant::now(),
                        is_system: false,
                    });

                    // Save user message to database
                    if let Some(ref db) = self.db {
                        let _ = db.save_message(self.provider.db_name(), "user", &user_input);
                    }

                    // Start streaming response
                    self.is_streaming = true;
                    self.send_message(user_input);
                }
            }
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
            _ => {}
        }

        Ok(())
    }

    fn send_message(&mut self, _content: String) {
        // Build message history - exclude system messages
        let messages: Vec<Message> = self
            .messages
            .iter()
            .filter(|m| !m.is_system)
            .map(|m| Message {
                role: match m.role {
                    MessageRole::User => "user".to_string(),
                    MessageRole::Assistant => "assistant".to_string(),
                },
                content: m.content.clone(),
            })
            .collect();

        // Spawn async task to get response
        let client = self.ai_client.clone();
        let tx = self.response_tx.clone();
        let session_id = self.session_id;
        tokio::spawn(async move {
            let result = client.send_message(messages).await;
            let _ = tx.send((session_id, result));
        });
    }

    pub fn update(&mut self) -> Result<()> {
        // Poll for streaming responses
        if let Ok((response_session_id, result)) = self.response_rx.try_recv() {
            // Only process responses from the current session
            if response_session_id == self.session_id {
                self.is_streaming = false;
                match result {
                    Ok(response) => {
                        self.messages.push(ChatMessage {
                            role: MessageRole::Assistant,
                            content: response.clone(),
                            timestamp: Instant::now(),
                            is_system: false,
                        });

                        // Save assistant response to database
                        if let Some(ref db) = self.db {
                            let _ = db.save_message(self.provider.db_name(), "assistant", &response);
                        }

                        // Auto-scroll to bottom
                        self.scroll_offset = self.messages.len().saturating_sub(1);
                    }
                    Err(e) => {
                        self.add_system_message(&format!("Error: {}", e));
                    }
                }
            }
            // If response_session_id != self.session_id, ignore the response (it's from a previous provider)
        }

        Ok(())
    }

    fn add_system_message(&mut self, content: &str) {
        self.messages.push(ChatMessage {
            role: MessageRole::Assistant,
            content: format!("🔧 {}", content),
            timestamp: Instant::now(),
            is_system: true,
        });
    }

    pub fn render(&mut self, frame: &mut Frame) -> Result<()> {
        let area = frame.area();

        // Main layout
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),      // Header
                Constraint::Min(5),         // Messages
                Constraint::Length(3),      // Input
                Constraint::Length(1),      // Footer
            ])
            .split(area);

        // Header
        let header_text = format!("MEGA-CLI // {} ", self.provider.name());
        let header = Paragraph::new(header_text)
            .style(Style::default().fg(self.provider.color()).bold())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(ratatui::widgets::BorderType::Thick),
            );
        frame.render_widget(header, chunks[0]);

        // Messages area
        if self.show_help {
            self.render_help(frame, chunks[1]);
        } else {
            self.render_messages(frame, chunks[1]);
        }

        // Input area
        let input_text = if self.is_streaming {
            format!("⏳ Waiting for response...")
        } else {
            format!("> {}_", self.input_buffer)
        };
        let input = Paragraph::new(input_text)
            .style(Style::default().fg(Color::Cyan))
            .block(Block::default().borders(Borders::ALL).title("Input"));
        frame.render_widget(input, chunks[2]);

        // Footer
        let footer_text = "F1 Help | F2 Switch Model | Ctrl+C Exit | Ctrl+L Clear";
        let footer = Paragraph::new(footer_text)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(footer, chunks[3]);

        // Apply effects
        let elapsed = self.last_update.elapsed();
        self.last_update = Instant::now();
        self.effects
            .process_effects(elapsed.into(), frame.buffer_mut(), area);

        Ok(())
    }

    fn render_messages(&self, frame: &mut Frame, area: Rect) {
        if self.messages.is_empty() {
            let welcome = Paragraph::new(format!(
                "Welcome to MEGA-CLI!\n\n\
                Connected to: {}\n\n\
                Type your message and press Enter to start chatting.\n\
                Press F1 for help.",
                self.provider.name()
            ))
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::Gray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Messages")
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
            frame.render_widget(welcome, area);
            return;
        }

        // Build message text with proper formatting
        let mut lines = vec![];
        for (idx, msg) in self.messages.iter().enumerate() {
            if idx < self.scroll_offset {
                continue;
            }

            let (prefix, color) = match msg.role {
                MessageRole::User => ("You", Color::Green),
                MessageRole::Assistant => (
                    self.provider.name(),
                    self.provider.color(),
                ),
            };

            // Add prefix line with color
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", prefix), Style::default().fg(color).bold()),
                Span::styled(&msg.content, Style::default().fg(color)),
            ]));

            // Add empty line between messages for readability
            if idx < self.messages.len() - 1 {
                lines.push(Line::from(""));
            }
        }

        let messages_text = Text::from(lines);
        let messages_paragraph = Paragraph::new(messages_text)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Messages")
                    .border_style(Style::default().fg(Color::DarkGray)),
            );

        frame.render_widget(messages_paragraph, area);
    }

    fn render_help(&self, frame: &mut Frame, area: Rect) {
        let help_text =
"MEGA-CLI Keyboard Shortcuts

Navigation:
  ↑/↓         Scroll messages
  PgUp/PgDn   Scroll 10 messages

Commands:
  Enter       Send message
  F1          Toggle this help
  F2          Switch AI provider
  Ctrl+L      Clear conversation
  Ctrl+C      Exit

AI Providers:
  • Claude Sonnet 4.5
  • Grok 4
  • GPT-5
  • Gemini 2.5 Pro

Press F1 to return to chat.";

        let help = Paragraph::new(help_text)
            .alignment(Alignment::Left)
            .style(Style::default().fg(Color::Cyan))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Help")
                    .border_style(Style::default().fg(Color::Cyan)),
            );
        frame.render_widget(help, area);
    }
}
