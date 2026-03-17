use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use ratatui::style::Color;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::env;
use tokio::sync::mpsc;

use crate::tools::{ToolCall, ToolDefinition, ToolResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AIProvider {
    Claude,
    Grok,
    OpenAI,
    Gemini,
    Ollama,
}

impl AIProvider {
    pub fn from_input(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Self::Claude,
            "grok" | "xai" => Self::Grok,
            "gpt" | "openai" | "gpt5" | "gpt-5" => Self::OpenAI,
            "gemini" | "google" => Self::Gemini,
            "ollama" | "local" => Self::Ollama,
            _ => Self::Claude,
        }
    }

    pub fn cycle(&self) -> Self {
        match self {
            Self::Claude => Self::Grok,
            Self::Grok => Self::OpenAI,
            Self::OpenAI => Self::Gemini,
            Self::Gemini => Self::Ollama,
            Self::Ollama => Self::Claude,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Claude => "Claude Haiku 4.5",
            Self::Grok => "Grok 4 Fast",
            Self::OpenAI => "GPT-5 Nano",
            Self::Gemini => "Gemini 3 Flash",
            Self::Ollama => "Ollama Local",
        }
    }

    pub fn badge(&self) -> &'static str {
        match self {
            Self::Claude => "ANTHROPIC",
            Self::Grok => "X.AI",
            Self::OpenAI => "OPENAI",
            Self::Gemini => "GOOGLE",
            Self::Ollama => "OLLAMA",
        }
    }

    pub fn db_key(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::OpenAI => "gpt",
            Self::Gemini => "gemini",
            Self::Ollama => "ollama",
        }
    }

    pub fn color(&self) -> Color {
        match self {
            Self::Claude => Color::Rgb(218, 155, 102),
            Self::Grok => Color::Rgb(104, 206, 232),
            Self::OpenAI => Color::Rgb(108, 197, 181),
            Self::Gemini => Color::Rgb(119, 153, 234),
            Self::Ollama => Color::Rgb(144, 214, 121),
        }
    }

    fn api_url(&self) -> &'static str {
        match self {
            Self::Claude => "https://api.anthropic.com/v1/messages",
            Self::Grok => "https://api.x.ai/v1/chat/completions",
            Self::OpenAI => "https://api.openai.com/v1/chat/completions",
            Self::Gemini => {
                "https://generativelanguage.googleapis.com/v1beta/models/gemini-3-flash-preview:generateContent"
            }
            Self::Ollama => "http://127.0.0.1:11434/v1/chat/completions",
        }
    }

    #[allow(dead_code)]
    fn stream_url(&self) -> &'static str {
        self.api_url()
    }

    fn model(&self) -> &'static str {
        match self {
            Self::Claude => "claude-haiku-4-5",
            Self::Grok => "grok-4-fast-non-reasoning",
            Self::OpenAI => "gpt-5-nano",
            Self::Gemini => "gemini-3-flash-preview",
            Self::Ollama => "",
        }
    }

    fn api_key_env(&self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE_API_KEY",
            Self::Grok => "GROK_API_KEY",
            Self::OpenAI => "OPENAI_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
            Self::Ollama => "",
        }
    }

    fn api_key(&self) -> Result<String> {
        match self {
            Self::Ollama => Ok("ollama".to_string()),
            _ => env::var(self.api_key_env())
                .with_context(|| format!("{} not set in environment", self.api_key_env())),
        }
    }

    fn openai_bearer_token(&self) -> Result<Option<String>> {
        match self {
            Self::OpenAI | Self::Grok => Ok(Some(self.api_key()?)),
            Self::Ollama => Ok(None),
            _ => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub enum AIResponse {
    Text(String),
    ToolCalls(Vec<ToolCall>, String),
}

#[derive(Debug, Clone)]
pub enum StreamChunk {
    Delta(String),
    ToolCallsReceived(Vec<ToolCall>, String),
    Done,
}

#[derive(Debug, Clone)]
pub struct OllamaModelInfo {
    pub name: String,
    pub size_bytes: u64,
    pub parameter_size: Option<String>,
    pub family: Option<String>,
    pub is_cloud: bool,
}

pub fn ollama_install_hint() -> &'static str {
    "Ollama is not installed. To install it, run `curl -fsSL https://ollama.com/install.sh | sh`."
}

pub async fn list_ollama_models() -> Result<Vec<OllamaModelInfo>> {
    let cli_probe = tokio::process::Command::new("ollama")
        .arg("--version")
        .output()
        .await;
    if let Err(error) = cli_probe {
        return if error.kind() == std::io::ErrorKind::NotFound {
            Err(anyhow!(ollama_install_hint()))
        } else {
            Err(anyhow!(error).context("failed to check ollama installation"))
        };
    }

    let client = Client::new();
    let response = client
        .get("http://127.0.0.1:11434/api/tags")
        .send()
        .await
        .map_err(|error| {
            anyhow!(error).context(
                "Ollama is installed, but its local API is not responding on http://127.0.0.1:11434. Start it with `ollama serve` and try again.",
            )
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Ollama model lookup failed with {}: {}",
            status,
            body
        ));
    }

    let payload: OllamaTagsResponse = response
        .json()
        .await
        .context("failed to parse Ollama model list")?;

    if payload.models.is_empty() {
        return Err(anyhow!(
            "Ollama is installed, but no models were found. Pull one with `ollama pull <model>` first."
        ));
    }

    Ok(payload
        .models
        .into_iter()
        .map(|model| OllamaModelInfo {
            name: model.name,
            size_bytes: model.size,
            parameter_size: model.details.parameter_size,
            family: model.details.family,
            is_cloud: model.remote_host.is_some(),
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelEntry {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    remote_host: Option<String>,
    #[serde(default)]
    details: OllamaModelDetails,
}

#[derive(Debug, Default, Deserialize)]
struct OllamaModelDetails {
    #[serde(default)]
    parameter_size: Option<String>,
    #[serde(default)]
    family: Option<String>,
}

// Claude types
#[derive(Debug, Serialize)]
struct ClaudeRequest {
    model: String,
    messages: Vec<ClaudeMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ClaudeTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClaudeMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ClaudeTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ClaudeResponse {
    content: Vec<ClaudeContent>,
    #[serde(default)]
    #[allow(dead_code)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeContent {
    #[serde(rename = "type")]
    content_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

// OpenAI/Grok types
#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    stream: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize)]
struct OpenAIFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIResponseMessage,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

// Gemini types
#[derive(Debug, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiToolDeclaration>>,
}

#[derive(Debug, Serialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

#[derive(Debug, Serialize)]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "functionResponse")]
    function_response: Option<GeminiFunctionResponse>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct GeminiToolDeclaration {
    function_declarations: Vec<GeminiFunctionDecl>,
}

#[derive(Debug, Serialize)]
struct GeminiFunctionDecl {
    name: String,
    description: String,
    parameters: serde_json::Value,
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
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Deserialize)]
struct ClaudeStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    #[allow(dead_code)]
    index: Option<usize>,
    #[serde(default)]
    delta: Option<ClaudeStreamDelta>,
    #[serde(default)]
    content_block: Option<ClaudeStreamContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ClaudeStreamDelta {
    #[serde(rename = "type", default)]
    #[allow(dead_code)]
    delta_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeStreamContentBlock {
    #[serde(rename = "type")]
    content_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamResponse {
    choices: Vec<OpenAIStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChoice {
    delta: OpenAIStreamDelta,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAIStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamToolCall {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAIStreamFunction>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Clone)]
pub struct AIClient {
    provider: AIProvider,
    client: Client,
    model_override: Option<String>,
}

impl AIClient {
    pub fn new(provider: AIProvider, model_override: Option<String>) -> Self {
        Self {
            provider,
            client: Client::new(),
            model_override,
        }
    }

    #[allow(dead_code)]
    pub fn provider(&self) -> &AIProvider {
        &self.provider
    }

    #[allow(dead_code)]
    pub async fn send_message(&self, messages: Vec<Message>) -> Result<String> {
        match self.send_message_with_tools(messages, None).await? {
            AIResponse::Text(text) => Ok(text),
            AIResponse::ToolCalls(_, text) => Ok(text),
        }
    }

    pub async fn send_message_with_tools(
        &self,
        messages: Vec<Message>,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<AIResponse> {
        match self.provider {
            AIProvider::Claude => self.send_claude_with_tools(messages, tools).await,
            AIProvider::Grok | AIProvider::OpenAI | AIProvider::Ollama => {
                self.send_openai_with_tools(messages, tools).await
            }
            AIProvider::Gemini => self.send_gemini_with_tools(messages, tools).await,
        }
    }

    #[allow(dead_code)]
    pub async fn send_streaming(
        &self,
        messages: Vec<Message>,
        chunk_tx: mpsc::UnboundedSender<StreamChunk>,
    ) -> Result<()> {
        match self.provider {
            AIProvider::Claude => self.stream_claude(messages, chunk_tx).await,
            AIProvider::Grok | AIProvider::OpenAI | AIProvider::Ollama => {
                self.stream_openai(messages, chunk_tx).await
            }
            AIProvider::Gemini => {
                // Gemini doesn't have great SSE support, fall back to non-streaming
                let result = self.send_message(messages).await?;
                let _ = chunk_tx.send(StreamChunk::Delta(result));
                let _ = chunk_tx.send(StreamChunk::Done);
                Ok(())
            }
        }
    }

    pub async fn send_with_tool_results(
        &self,
        messages: Vec<Message>,
        tool_calls: &[ToolCall],
        tool_results: &[ToolResult],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<AIResponse> {
        match self.provider {
            AIProvider::Claude => {
                self.send_claude_tool_results(messages, tool_calls, tool_results, tools)
                    .await
            }
            AIProvider::Grok | AIProvider::OpenAI | AIProvider::Ollama => {
                self.send_openai_tool_results(messages, tool_calls, tool_results, tools)
                    .await
            }
            AIProvider::Gemini => {
                self.send_gemini_tool_results(messages, tool_calls, tool_results, tools)
                    .await
            }
        }
    }

    pub async fn send_streaming_with_tools(
        &self,
        messages: Vec<Message>,
        tools: Option<&[ToolDefinition]>,
        chunk_tx: mpsc::UnboundedSender<StreamChunk>,
    ) -> Result<()> {
        match self.provider {
            AIProvider::Claude => {
                self.stream_claude_with_tools(messages, tools, chunk_tx).await
            }
            AIProvider::Grok | AIProvider::OpenAI | AIProvider::Ollama => {
                self.stream_openai_with_tools(messages, tools, chunk_tx).await
            }
            AIProvider::Gemini => {
                let result = self.send_gemini_with_tools(messages, tools).await?;
                match result {
                    AIResponse::Text(text) => {
                        let _ = chunk_tx.send(StreamChunk::Delta(text));
                        let _ = chunk_tx.send(StreamChunk::Done);
                    }
                    AIResponse::ToolCalls(calls, text) => {
                        if !text.is_empty() {
                            let _ = chunk_tx.send(StreamChunk::Delta(text.clone()));
                        }
                        let _ = chunk_tx.send(StreamChunk::ToolCallsReceived(calls, text));
                    }
                }
                Ok(())
            }
        }
    }

    fn model_name(&self) -> Result<&str> {
        match self.provider {
            AIProvider::Ollama => self
                .model_override
                .as_deref()
                .context("Ollama is active but no model is selected"),
            _ => Ok(self.provider.model()),
        }
    }

    // Claude implementation
    async fn send_claude_with_tools(
        &self,
        messages: Vec<Message>,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<AIResponse> {
        let claude_tools = tools.map(|t| {
            t.iter()
                .map(|td| ClaudeTool {
                    name: td.name.clone(),
                    description: td.description.clone(),
                    input_schema: td.parameters.clone(),
                })
                .collect::<Vec<_>>()
        });

        let request = ClaudeRequest {
            model: self.provider.model().to_string(),
            messages: messages
                .iter()
                .map(|m| ClaudeMessage {
                    role: m.role.clone(),
                    content: serde_json::Value::String(m.content.clone()),
                })
                .collect(),
            max_tokens: 4096,
            tools: claude_tools,
            stream: None,
        };

        let response = self
            .client
            .post(self.provider.api_url())
            .header("x-api-key", self.provider.api_key()?)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send Claude request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Claude API error {}: {}", status, body));
        }

        let payload: ClaudeResponse = response
            .json()
            .await
            .context("failed to parse Claude response")?;

        parse_claude_response(payload)
    }

    async fn send_claude_tool_results(
        &self,
        original_messages: Vec<Message>,
        tool_calls: &[ToolCall],
        tool_results: &[ToolResult],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<AIResponse> {
        let claude_tools = tools.map(|t| {
            t.iter()
                .map(|td| ClaudeTool {
                    name: td.name.clone(),
                    description: td.description.clone(),
                    input_schema: td.parameters.clone(),
                })
                .collect::<Vec<_>>()
        });

        let mut messages: Vec<ClaudeMessage> = original_messages
            .iter()
            .map(|m| ClaudeMessage {
                role: m.role.clone(),
                content: serde_json::Value::String(m.content.clone()),
            })
            .collect();

        // Add the assistant message with tool_use blocks
        let tool_use_blocks: Vec<serde_json::Value> = tool_calls
            .iter()
            .map(|tc| {
                serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.name,
                    "input": tc.arguments
                })
            })
            .collect();
        messages.push(ClaudeMessage {
            role: "assistant".to_string(),
            content: serde_json::Value::Array(tool_use_blocks),
        });

        // Add tool results as user message
        let result_blocks: Vec<serde_json::Value> = tool_results
            .iter()
            .map(|tr| {
                serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tr.tool_call_id,
                    "content": tr.content,
                    "is_error": !tr.success
                })
            })
            .collect();
        messages.push(ClaudeMessage {
            role: "user".to_string(),
            content: serde_json::Value::Array(result_blocks),
        });

        let request = ClaudeRequest {
            model: self.provider.model().to_string(),
            messages,
            max_tokens: 4096,
            tools: claude_tools,
            stream: None,
        };

        let response = self
            .client
            .post(self.provider.api_url())
            .header("x-api-key", self.provider.api_key()?)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send Claude tool result")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Claude API error {}: {}", status, body));
        }

        let payload: ClaudeResponse = response
            .json()
            .await
            .context("failed to parse Claude response")?;

        parse_claude_response(payload)
    }

    async fn stream_claude(
        &self,
        messages: Vec<Message>,
        chunk_tx: mpsc::UnboundedSender<StreamChunk>,
    ) -> Result<()> {
        let request = ClaudeRequest {
            model: self.provider.model().to_string(),
            messages: messages
                .iter()
                .map(|m| ClaudeMessage {
                    role: m.role.clone(),
                    content: serde_json::Value::String(m.content.clone()),
                })
                .collect(),
            max_tokens: 4096,
            tools: None,
            stream: Some(true),
        };

        let response = self
            .client
            .post(self.provider.stream_url())
            .header("x-api-key", self.provider.api_key()?)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send Claude stream request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Claude stream error {}: {}", status, body));
        }

        let mut bytes = Vec::new();
        let mut byte_stream = response.bytes_stream();
        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result?;
            bytes.extend_from_slice(&chunk);

            while let Some(pos) = bytes.windows(2).position(|w| w == b"\n\n") {
                let event_data: Vec<u8> = bytes.drain(..pos + 2).collect();
                let text = String::from_utf8_lossy(&event_data);

                for line in text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            let _ = chunk_tx.send(StreamChunk::Done);
                            return Ok(());
                        }
                        if let Ok(event) = serde_json::from_str::<ClaudeStreamEvent>(data) {
                            if event.event_type == "content_block_delta" {
                                if let Some(delta) = &event.delta {
                                    if let Some(ref t) = delta.text {
                                        let _ = chunk_tx.send(StreamChunk::Delta(t.clone()));
                                    }
                                }
                            } else if event.event_type == "message_stop" {
                                let _ = chunk_tx.send(StreamChunk::Done);
                                return Ok(());
                            }
                        }
                    }
                }
            }
        }

        let _ = chunk_tx.send(StreamChunk::Done);
        Ok(())
    }

    async fn stream_claude_with_tools(
        &self,
        messages: Vec<Message>,
        tools: Option<&[ToolDefinition]>,
        chunk_tx: mpsc::UnboundedSender<StreamChunk>,
    ) -> Result<()> {
        let claude_tools = tools.map(|t| {
            t.iter()
                .map(|td| ClaudeTool {
                    name: td.name.clone(),
                    description: td.description.clone(),
                    input_schema: td.parameters.clone(),
                })
                .collect::<Vec<_>>()
        });

        let request = ClaudeRequest {
            model: self.provider.model().to_string(),
            messages: messages
                .iter()
                .map(|m| ClaudeMessage {
                    role: m.role.clone(),
                    content: serde_json::Value::String(m.content.clone()),
                })
                .collect(),
            max_tokens: 4096,
            tools: claude_tools,
            stream: Some(true),
        };

        let response = self
            .client
            .post(self.provider.api_url())
            .header("x-api-key", self.provider.api_key()?)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send Claude streaming request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Claude stream error {}: {}", status, body));
        }

        let mut bytes = Vec::new();
        let mut byte_stream = response.bytes_stream();
        let mut accumulated_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        // Track current content block for tool_use parsing
        struct ToolBlock {
            id: String,
            name: String,
            json_parts: String,
        }
        let mut current_tool: Option<ToolBlock> = None;

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result?;
            bytes.extend_from_slice(&chunk);

            while let Some(pos) = bytes.windows(2).position(|w| w == b"\n\n") {
                let event_data: Vec<u8> = bytes.drain(..pos + 2).collect();
                let text = String::from_utf8_lossy(&event_data);

                for line in text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            if !tool_calls.is_empty() {
                                let _ = chunk_tx.send(StreamChunk::ToolCallsReceived(
                                    tool_calls,
                                    accumulated_text,
                                ));
                            } else {
                                let _ = chunk_tx.send(StreamChunk::Done);
                            }
                            return Ok(());
                        }
                        if let Ok(event) = serde_json::from_str::<ClaudeStreamEvent>(data) {
                            match event.event_type.as_str() {
                                "content_block_start" => {
                                    if let Some(ref block) = event.content_block {
                                        if block.content_type == "tool_use" {
                                            current_tool = Some(ToolBlock {
                                                id: block.id.clone().unwrap_or_default(),
                                                name: block.name.clone().unwrap_or_default(),
                                                json_parts: String::new(),
                                            });
                                        }
                                    }
                                }
                                "content_block_delta" => {
                                    if let Some(ref delta) = event.delta {
                                        if let Some(ref t) = delta.text {
                                            accumulated_text.push_str(t);
                                            let _ = chunk_tx.send(StreamChunk::Delta(t.clone()));
                                        }
                                        if let Some(ref pj) = delta.partial_json {
                                            if let Some(ref mut tool) = current_tool {
                                                tool.json_parts.push_str(pj);
                                            }
                                        }
                                    }
                                }
                                "content_block_stop" => {
                                    if let Some(tool) = current_tool.take() {
                                        let arguments: serde_json::Value =
                                            serde_json::from_str(&tool.json_parts)
                                                .unwrap_or(serde_json::Value::Object(
                                                    serde_json::Map::new(),
                                                ));
                                        tool_calls.push(ToolCall {
                                            id: tool.id,
                                            name: tool.name,
                                            arguments,
                                        });
                                    }
                                }
                                "message_stop" => {
                                    if !tool_calls.is_empty() {
                                        let _ = chunk_tx.send(StreamChunk::ToolCallsReceived(
                                            tool_calls,
                                            accumulated_text,
                                        ));
                                    } else {
                                        let _ = chunk_tx.send(StreamChunk::Done);
                                    }
                                    return Ok(());
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        if !tool_calls.is_empty() {
            let _ = chunk_tx.send(StreamChunk::ToolCallsReceived(tool_calls, accumulated_text));
        } else {
            let _ = chunk_tx.send(StreamChunk::Done);
        }
        Ok(())
    }

    // OpenAI/Grok implementation
    async fn send_openai_with_tools(
        &self,
        messages: Vec<Message>,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<AIResponse> {
        let openai_tools = tools.map(|t| {
            t.iter()
                .map(|td| OpenAITool {
                    tool_type: "function".to_string(),
                    function: OpenAIFunction {
                        name: td.name.clone(),
                        description: td.description.clone(),
                        parameters: td.parameters.clone(),
                    },
                })
                .collect::<Vec<_>>()
        });

        let request = OpenAIRequest {
            model: self.model_name()?.to_string(),
            messages: messages
                .iter()
                .map(|m| OpenAIMessage {
                    role: m.role.clone(),
                    content: Some(m.content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                })
                .collect(),
            tools: openai_tools,
            stream: false,
        };

        let mut request_builder = self
            .client
            .post(self.provider.api_url())
            .header("content-type", "application/json");
        if let Some(token) = self.provider.openai_bearer_token()? {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", token));
        }
        let response = request_builder
            .json(&request)
            .send()
            .await
            .with_context(|| format!("failed to send {} request", self.provider.name()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "{} API error {}: {}",
                self.provider.name(),
                status,
                body
            ));
        }

        let payload: OpenAIResponse = response
            .json()
            .await
            .with_context(|| format!("failed to parse {} response", self.provider.name()))?;

        parse_openai_response(payload)
    }

    async fn send_openai_tool_results(
        &self,
        original_messages: Vec<Message>,
        tool_calls: &[ToolCall],
        tool_results: &[ToolResult],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<AIResponse> {
        let openai_tools = tools.map(|t| {
            t.iter()
                .map(|td| OpenAITool {
                    tool_type: "function".to_string(),
                    function: OpenAIFunction {
                        name: td.name.clone(),
                        description: td.description.clone(),
                        parameters: td.parameters.clone(),
                    },
                })
                .collect::<Vec<_>>()
        });

        let mut msgs: Vec<OpenAIMessage> = original_messages
            .iter()
            .map(|m| OpenAIMessage {
                role: m.role.clone(),
                content: Some(m.content.clone()),
                tool_calls: None,
                tool_call_id: None,
            })
            .collect();

        // Add assistant message with tool calls
        let oai_tool_calls: Vec<OpenAIToolCall> = tool_calls
            .iter()
            .map(|tc| OpenAIToolCall {
                id: tc.id.clone(),
                call_type: "function".to_string(),
                function: OpenAIFunctionCall {
                    name: tc.name.clone(),
                    arguments: serde_json::to_string(&tc.arguments).unwrap_or_default(),
                },
            })
            .collect();

        msgs.push(OpenAIMessage {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(oai_tool_calls),
            tool_call_id: None,
        });

        // Add tool results
        for tr in tool_results {
            msgs.push(OpenAIMessage {
                role: "tool".to_string(),
                content: Some(tr.content.clone()),
                tool_calls: None,
                tool_call_id: Some(tr.tool_call_id.clone()),
            });
        }

        let request = OpenAIRequest {
            model: self.model_name()?.to_string(),
            messages: msgs,
            tools: openai_tools,
            stream: false,
        };

        let mut request_builder = self
            .client
            .post(self.provider.api_url())
            .header("content-type", "application/json");
        if let Some(token) = self.provider.openai_bearer_token()? {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", token));
        }
        let response = request_builder.json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("{} API error {}: {}", self.provider.name(), status, body));
        }

        let payload: OpenAIResponse = response.json().await?;
        parse_openai_response(payload)
    }

    async fn stream_openai(
        &self,
        messages: Vec<Message>,
        chunk_tx: mpsc::UnboundedSender<StreamChunk>,
    ) -> Result<()> {
        let request = OpenAIRequest {
            model: self.model_name()?.to_string(),
            messages: messages
                .iter()
                .map(|m| OpenAIMessage {
                    role: m.role.clone(),
                    content: Some(m.content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                })
                .collect(),
            tools: None,
            stream: true,
        };

        let mut request_builder = self
            .client
            .post(self.provider.stream_url())
            .header("content-type", "application/json");
        if let Some(token) = self.provider.openai_bearer_token()? {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", token));
        }
        let response = request_builder.json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("{} stream error {}: {}", self.provider.name(), status, body));
        }

        let mut bytes = Vec::new();
        let mut byte_stream = response.bytes_stream();
        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result?;
            bytes.extend_from_slice(&chunk);

            while let Some(pos) = bytes.windows(2).position(|w| w == b"\n\n") {
                let event_data: Vec<u8> = bytes.drain(..pos + 2).collect();
                let text = String::from_utf8_lossy(&event_data);

                for line in text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            let _ = chunk_tx.send(StreamChunk::Done);
                            return Ok(());
                        }
                        if let Ok(resp) = serde_json::from_str::<OpenAIStreamResponse>(data) {
                            if let Some(choice) = resp.choices.first() {
                                if let Some(content) = &choice.delta.content {
                                    let _ = chunk_tx.send(StreamChunk::Delta(content.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }

        let _ = chunk_tx.send(StreamChunk::Done);
        Ok(())
    }

    async fn stream_openai_with_tools(
        &self,
        messages: Vec<Message>,
        tools: Option<&[ToolDefinition]>,
        chunk_tx: mpsc::UnboundedSender<StreamChunk>,
    ) -> Result<()> {
        let openai_tools = tools.map(|t| {
            t.iter()
                .map(|td| OpenAITool {
                    tool_type: "function".to_string(),
                    function: OpenAIFunction {
                        name: td.name.clone(),
                        description: td.description.clone(),
                        parameters: td.parameters.clone(),
                    },
                })
                .collect::<Vec<_>>()
        });

        let request = OpenAIRequest {
            model: self.model_name()?.to_string(),
            messages: messages
                .iter()
                .map(|m| OpenAIMessage {
                    role: m.role.clone(),
                    content: Some(m.content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                })
                .collect(),
            tools: openai_tools,
            stream: true,
        };

        let mut request_builder = self
            .client
            .post(self.provider.api_url())
            .header("content-type", "application/json");
        if let Some(token) = self.provider.openai_bearer_token()? {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", token));
        }
        let response = request_builder.json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("{} stream error {}: {}", self.provider.name(), status, body));
        }

        let mut bytes = Vec::new();
        let mut byte_stream = response.bytes_stream();
        let mut accumulated_text = String::new();

        // Track tool call fragments by index
        struct OaiToolAccum {
            id: String,
            name: String,
            arguments: String,
        }
        let mut tool_accum: Vec<OaiToolAccum> = Vec::new();

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result?;
            bytes.extend_from_slice(&chunk);

            while let Some(pos) = bytes.windows(2).position(|w| w == b"\n\n") {
                let event_data: Vec<u8> = bytes.drain(..pos + 2).collect();
                let text = String::from_utf8_lossy(&event_data);

                for line in text.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            if !tool_accum.is_empty() {
                                let calls: Vec<ToolCall> = tool_accum
                                    .drain(..)
                                    .map(|ta| ToolCall {
                                        id: ta.id,
                                        name: ta.name,
                                        arguments: serde_json::from_str(&ta.arguments)
                                            .unwrap_or(serde_json::Value::Object(
                                                serde_json::Map::new(),
                                            )),
                                    })
                                    .collect();
                                let _ = chunk_tx.send(StreamChunk::ToolCallsReceived(
                                    calls,
                                    accumulated_text,
                                ));
                            } else {
                                let _ = chunk_tx.send(StreamChunk::Done);
                            }
                            return Ok(());
                        }
                        if let Ok(resp) = serde_json::from_str::<OpenAIStreamResponse>(data) {
                            if let Some(choice) = resp.choices.first() {
                                if let Some(content) = &choice.delta.content {
                                    accumulated_text.push_str(content);
                                    let _ = chunk_tx.send(StreamChunk::Delta(content.clone()));
                                }
                                if let Some(tc_deltas) = &choice.delta.tool_calls {
                                    for tc in tc_deltas {
                                        while tool_accum.len() <= tc.index {
                                            tool_accum.push(OaiToolAccum {
                                                id: String::new(),
                                                name: String::new(),
                                                arguments: String::new(),
                                            });
                                        }
                                        let entry = &mut tool_accum[tc.index];
                                        if let Some(ref id) = tc.id {
                                            entry.id = id.clone();
                                        }
                                        if let Some(ref func) = tc.function {
                                            if let Some(ref name) = func.name {
                                                entry.name = name.clone();
                                            }
                                            if let Some(ref args) = func.arguments {
                                                entry.arguments.push_str(args);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if !tool_accum.is_empty() {
            let calls: Vec<ToolCall> = tool_accum
                .drain(..)
                .map(|ta| ToolCall {
                    id: ta.id,
                    name: ta.name,
                    arguments: serde_json::from_str(&ta.arguments)
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
                })
                .collect();
            let _ = chunk_tx.send(StreamChunk::ToolCallsReceived(calls, accumulated_text));
        } else {
            let _ = chunk_tx.send(StreamChunk::Done);
        }
        Ok(())
    }

    // Gemini implementation
    async fn send_gemini_with_tools(
        &self,
        messages: Vec<Message>,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<AIResponse> {
        let gemini_tools = tools.map(|t| {
            vec![GeminiToolDeclaration {
                function_declarations: t
                    .iter()
                    .map(|td| GeminiFunctionDecl {
                        name: td.name.clone(),
                        description: td.description.clone(),
                        parameters: td.parameters.clone(),
                    })
                    .collect(),
            }]
        });

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart {
                    text: Some(
                        messages
                            .iter()
                            .map(|m| format!("{}: {}", m.role, m.content))
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                    ),
                    function_call: None,
                    function_response: None,
                }],
                role: Some("user".to_string()),
            }],
            tools: gemini_tools,
        };

        let url = format!(
            "{}?key={}",
            self.provider.api_url(),
            self.provider.api_key()?
        );
        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send Gemini request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Gemini API error {}: {}", status, body));
        }

        let payload: GeminiResponse = response
            .json()
            .await
            .context("failed to parse Gemini response")?;

        parse_gemini_response(payload)
    }

    async fn send_gemini_tool_results(
        &self,
        original_messages: Vec<Message>,
        tool_calls: &[ToolCall],
        tool_results: &[ToolResult],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<AIResponse> {
        let gemini_tools = tools.map(|t| {
            vec![GeminiToolDeclaration {
                function_declarations: t
                    .iter()
                    .map(|td| GeminiFunctionDecl {
                        name: td.name.clone(),
                        description: td.description.clone(),
                        parameters: td.parameters.clone(),
                    })
                    .collect(),
            }]
        });

        let mut contents = vec![GeminiContent {
            parts: vec![GeminiPart {
                text: Some(
                    original_messages
                        .iter()
                        .map(|m| format!("{}: {}", m.role, m.content))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                ),
                function_call: None,
                function_response: None,
            }],
            role: Some("user".to_string()),
        }];

        // Add model's function call
        let fc_parts: Vec<GeminiPart> = tool_calls
            .iter()
            .map(|tc| GeminiPart {
                text: None,
                function_call: Some(GeminiFunctionCall {
                    name: tc.name.clone(),
                    args: tc.arguments.clone(),
                }),
                function_response: None,
            })
            .collect();
        contents.push(GeminiContent {
            parts: fc_parts,
            role: Some("model".to_string()),
        });

        // Add function responses
        let fr_parts: Vec<GeminiPart> = tool_results
            .iter()
            .map(|tr| GeminiPart {
                text: None,
                function_call: None,
                function_response: Some(GeminiFunctionResponse {
                    name: tr.name.clone(),
                    response: serde_json::json!({ "result": tr.content }),
                }),
            })
            .collect();
        contents.push(GeminiContent {
            parts: fr_parts,
            role: Some("user".to_string()),
        });

        let request = GeminiRequest {
            contents,
            tools: gemini_tools,
        };

        let url = format!(
            "{}?key={}",
            self.provider.api_url(),
            self.provider.api_key()?
        );
        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("Gemini API error {}: {}", status, body));
        }

        let payload: GeminiResponse = response.json().await?;
        parse_gemini_response(payload)
    }
}

fn parse_claude_response(payload: ClaudeResponse) -> Result<AIResponse> {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for block in &payload.content {
        match block.content_type.as_str() {
            "text" => {
                if let Some(t) = &block.text {
                    text_parts.push(t.clone());
                }
            }
            "tool_use" => {
                if let (Some(id), Some(name), Some(input)) =
                    (&block.id, &block.name, &block.input)
                {
                    tool_calls.push(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: input.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    let text = text_parts.join("");

    if !tool_calls.is_empty() {
        Ok(AIResponse::ToolCalls(tool_calls, text))
    } else if !text.is_empty() {
        Ok(AIResponse::Text(text))
    } else {
        Err(anyhow!("Claude returned no content"))
    }
}

fn parse_openai_response(payload: OpenAIResponse) -> Result<AIResponse> {
    let choice = payload
        .choices
        .first()
        .ok_or_else(|| anyhow!("no choices in response"))?;

    if let Some(ref tool_calls) = choice.message.tool_calls {
        if !tool_calls.is_empty() {
            let calls: Vec<ToolCall> = tool_calls
                .iter()
                .map(|tc| ToolCall {
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    arguments: serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
                })
                .collect();
            let text = choice.message.content.clone().unwrap_or_default();
            return Ok(AIResponse::ToolCalls(calls, text));
        }
    }

    choice
        .message
        .content
        .clone()
        .map(AIResponse::Text)
        .ok_or_else(|| anyhow!("no content in response"))
}

fn parse_gemini_response(payload: GeminiResponse) -> Result<AIResponse> {
    let candidate = payload
        .candidates
        .first()
        .ok_or_else(|| anyhow!("no candidates in response"))?;

    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for part in &candidate.content.parts {
        if let Some(ref t) = part.text {
            text_parts.push(t.clone());
        }
        if let Some(ref fc) = part.function_call {
            tool_calls.push(ToolCall {
                id: format!("gemini_{}", uuid::Uuid::new_v4()),
                name: fc.name.clone(),
                arguments: fc.args.clone(),
            });
        }
    }

    let text = text_parts.join("");

    if !tool_calls.is_empty() {
        Ok(AIResponse::ToolCalls(tool_calls, text))
    } else if !text.is_empty() {
        Ok(AIResponse::Text(text))
    } else {
        Err(anyhow!("Gemini returned no content"))
    }
}
