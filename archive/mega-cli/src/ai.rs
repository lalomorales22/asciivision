use anyhow::{anyhow, Context, Result};
use ratatui::style::Color;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;

#[derive(Debug, Clone, PartialEq)]
pub enum AIProvider {
    Claude,
    Grok,
    OpenAI,
    Gemini,
}

impl AIProvider {
    pub fn name(&self) -> &str {
        match self {
            AIProvider::Claude => "Claude Sonnet 4",
            AIProvider::Grok => "Grok 4",
            AIProvider::OpenAI => "GPT 5",
            AIProvider::Gemini => "Gemini 2.5 Pro",
        }
    }

    pub fn db_name(&self) -> &str {
        match self {
            AIProvider::Claude => "claude",
            AIProvider::Grok => "grok",
            AIProvider::OpenAI => "gpt",
            AIProvider::Gemini => "gemini",
        }
    }

    pub fn color(&self) -> Color {
        match self {
            AIProvider::Claude => Color::Rgb(204, 143, 102), // Copper
            AIProvider::Grok => Color::Rgb(100, 200, 255),   // Cyan
            AIProvider::OpenAI => Color::Rgb(116, 195, 194), // Teal
            AIProvider::Gemini => Color::Rgb(138, 180, 248), // Blue
        }
    }

    fn api_url(&self) -> &str {
        match self {
            AIProvider::Claude => "https://api.anthropic.com/v1/messages",
            AIProvider::Grok => "https://api.x.ai/v1/chat/completions",
            AIProvider::OpenAI => "https://api.openai.com/v1/chat/completions",
            AIProvider::Gemini => {
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:generateContent"
            }
        }
    }

    fn model(&self) -> &str {
        match self {
            AIProvider::Claude => "claude-sonnet-4-5-20250929", // Latest Claude Sonnet 4
            AIProvider::Grok => "grok-4",
            AIProvider::OpenAI => "gpt-5",
            AIProvider::Gemini => "gemini-2.5-pro",
        }
    }

    fn api_key_env(&self) -> &str {
        match self {
            AIProvider::Claude => "CLAUDE_API_KEY",
            AIProvider::Grok => "GROK_API_KEY",
            AIProvider::OpenAI => "OPENAI_API_KEY",
            AIProvider::Gemini => "GEMINI_API_KEY",
        }
    }

    pub fn get_api_key(&self) -> Result<String> {
        env::var(self.api_key_env())
            .with_context(|| format!("{} not set in environment", self.api_key_env()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

// Claude API structures
#[derive(Debug, Serialize)]
struct ClaudeRequest {
    model: String,
    messages: Vec<ClaudeMessage>,
    max_tokens: u32,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClaudeMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ClaudeResponse {
    content: Vec<ClaudeContent>,
}

#[derive(Debug, Deserialize)]
struct ClaudeContent {
    text: Option<String>,
}

// OpenAI/Grok API structures (they use the same format)
#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

// Gemini API structures
#[derive(Debug, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
}

#[derive(Debug, Serialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContentResponse,
}

#[derive(Debug, Deserialize)]
struct GeminiContentResponse {
    parts: Vec<GeminiPartResponse>,
}

#[derive(Debug, Deserialize)]
struct GeminiPartResponse {
    text: String,
}

#[derive(Clone)]
pub struct AIClient {
    provider: AIProvider,
    client: Client,
}

impl AIClient {
    pub fn new(provider: AIProvider) -> Self {
        Self {
            provider,
            client: Client::new(),
        }
    }

    pub async fn send_message(&self, messages: Vec<Message>) -> Result<String> {
        match self.provider {
            AIProvider::Claude => self.send_claude(messages).await,
            AIProvider::Grok => self.send_openai_compatible(messages).await,
            AIProvider::OpenAI => self.send_openai_compatible(messages).await,
            AIProvider::Gemini => self.send_gemini(messages).await,
        }
    }

    async fn send_claude(&self, messages: Vec<Message>) -> Result<String> {
        let api_key = self.provider.get_api_key()?;

        let request = ClaudeRequest {
            model: self.provider.model().to_string(),
            messages: messages
                .into_iter()
                .map(|m| ClaudeMessage {
                    role: m.role,
                    content: m.content,
                })
                .collect(),
            max_tokens: 4096,
            stream: false,
        };

        let response = self
            .client
            .post(self.provider.api_url())
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Claude")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Claude API error {}: {}", status, error_text));
        }

        let claude_response: ClaudeResponse = response
            .json()
            .await
            .context("Failed to parse Claude response")?;

        claude_response
            .content
            .first()
            .and_then(|c| c.text.clone())
            .ok_or_else(|| anyhow!("No content in Claude response"))
    }

    async fn send_openai_compatible(&self, messages: Vec<Message>) -> Result<String> {
        let api_key = self.provider.get_api_key()?;

        let request = OpenAIRequest {
            model: self.provider.model().to_string(),
            messages: messages
                .into_iter()
                .map(|m| OpenAIMessage {
                    role: m.role,
                    content: m.content,
                })
                .collect(),
            stream: false,
        };

        let response = self
            .client
            .post(self.provider.api_url())
            .header("Authorization", format!("Bearer {}", api_key))
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .with_context(|| format!("Failed to send request to {}", self.provider.name()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "{} API error {}: {}",
                self.provider.name(),
                status,
                error_text
            ));
        }

        let openai_response: OpenAIResponse = response
            .json()
            .await
            .with_context(|| format!("Failed to parse {} response", self.provider.name()))?;

        openai_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| anyhow!("No content in {} response", self.provider.name()))
    }

    async fn send_gemini(&self, messages: Vec<Message>) -> Result<String> {
        let api_key = self.provider.get_api_key()?;

        // Gemini expects a different format - combine all messages into one content
        let combined_text = messages
            .into_iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart {
                    text: combined_text,
                }],
            }],
        };

        let url = format!("{}?key={}", self.provider.api_url(), api_key);

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Gemini")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Gemini API error {}: {}", status, error_text));
        }

        let gemini_response: GeminiResponse = response
            .json()
            .await
            .context("Failed to parse Gemini response")?;

        gemini_response
            .candidates
            .first()
            .and_then(|c| c.content.parts.first().map(|p| p.text.clone()))
            .ok_or_else(|| anyhow!("No content in Gemini response"))
    }
}
