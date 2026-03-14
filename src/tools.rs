use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub name: String,
    pub content: String,
    pub success: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    FullAuto,
    ConfirmDestructive,
    ConfirmAll,
}

impl TrustLevel {
    pub fn name(&self) -> &'static str {
        match self {
            Self::FullAuto => "FULL AUTO",
            Self::ConfirmDestructive => "CONFIRM DESTRUCTIVE",
            Self::ConfirmAll => "CONFIRM ALL",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::FullAuto => Self::ConfirmDestructive,
            Self::ConfirmDestructive => Self::ConfirmAll,
            Self::ConfirmAll => Self::FullAuto,
        }
    }
}

pub fn is_destructive(tool_name: &str, args: &serde_json::Value) -> bool {
    match tool_name {
        "run_shell" => {
            if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                let destructive_patterns = [
                    "rm ", "rm\t", "rmdir", "dd ", "mkfs", "chmod", "chown",
                    "kill ", "pkill", "shutdown", "reboot", "halt",
                    "> /dev/", "mv ", "git push", "git reset --hard",
                    "drop table", "drop database", "truncate",
                    "curl.*|.*sh", "wget.*|.*sh", "sudo",
                ];
                let lower = cmd.to_lowercase();
                destructive_patterns.iter().any(|p| lower.contains(p))
            } else {
                true
            }
        }
        "write_file" => true,
        "http_request" => {
            if let Some(method) = args.get("method").and_then(|v| v.as_str()) {
                matches!(method.to_uppercase().as_str(), "POST" | "PUT" | "DELETE" | "PATCH")
            } else {
                true
            }
        }
        _ => false,
    }
}

pub fn all_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "run_shell".to_string(),
            description: "Execute a shell command and return stdout/stderr".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read the contents of a file at the given path".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative file path to read"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Write content to a file, creating or overwriting it".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path to write to"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: "search_files".to_string(),
            description: "Search for a pattern in files using ripgrep-style matching".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (regex)"
                    },
                    "directory": {
                        "type": "string",
                        "description": "Directory to search in (default: current directory)"
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: "http_request".to_string(),
            description: "Make an HTTP request and return the response".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "description": "HTTP method: GET, POST, PUT, DELETE, PATCH"
                    },
                    "url": {
                        "type": "string",
                        "description": "Request URL"
                    },
                    "body": {
                        "type": "string",
                        "description": "Request body (optional)"
                    },
                    "headers": {
                        "type": "object",
                        "description": "Request headers as key-value pairs (optional)"
                    }
                },
                "required": ["method", "url"]
            }),
        },
        ToolDefinition {
            name: "get_system_info".to_string(),
            description: "Get current system information: CPU, memory, disk, network".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
    ]
}

pub async fn execute_tool(call: &ToolCall) -> ToolResult {
    let result = match call.name.as_str() {
        "run_shell" => execute_run_shell(&call.arguments).await,
        "read_file" => execute_read_file(&call.arguments).await,
        "write_file" => execute_write_file(&call.arguments).await,
        "search_files" => execute_search_files(&call.arguments).await,
        "http_request" => execute_http_request(&call.arguments).await,
        "get_system_info" => execute_get_system_info(&call.arguments).await,
        _ => Err(anyhow!("unknown tool: {}", call.name)),
    };

    match result {
        Ok(content) => ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            content,
            success: true,
        },
        Err(e) => ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            content: format!("error: {}", e),
            success: false,
        },
    }
}

async fn execute_run_shell(args: &serde_json::Value) -> Result<String> {
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing 'command' argument"))?;

    let outcome = crate::shell::run(command.to_string()).await;
    Ok(crate::shell::format_outcome(&outcome, 8000))
}

async fn execute_read_file(args: &serde_json::Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing 'path' argument"))?;

    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| anyhow!("failed to read {}: {}", path, e))?;

    if content.len() > 16000 {
        Ok(format!(
            "{}\n\n[truncated at 16000 chars, total {} chars]",
            &content[..16000],
            content.len()
        ))
    } else {
        Ok(content)
    }
}

async fn execute_write_file(args: &serde_json::Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing 'path' argument"))?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing 'content' argument"))?;

    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    tokio::fs::write(path, content)
        .await
        .map_err(|e| anyhow!("failed to write {}: {}", path, e))?;

    Ok(format!("wrote {} bytes to {}", content.len(), path))
}

async fn execute_search_files(args: &serde_json::Value) -> Result<String> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing 'pattern' argument"))?;
    let directory = args
        .get("directory")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    let output = tokio::process::Command::new("grep")
        .args(["-rn", "--include=*", pattern, directory])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| anyhow!("grep failed: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.len() > 8000 {
        Ok(format!(
            "{}\n\n[truncated at 8000 chars]",
            &stdout[..8000]
        ))
    } else if stdout.is_empty() {
        Ok("no matches found".to_string())
    } else {
        Ok(stdout)
    }
}

async fn execute_http_request(args: &serde_json::Value) -> Result<String> {
    let method = args
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing 'method' argument"))?;
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing 'url' argument"))?;
    let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");

    let client = reqwest::Client::new();
    let mut req = match method.to_uppercase().as_str() {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        "PATCH" => client.patch(url),
        _ => return Err(anyhow!("unsupported method: {}", method)),
    };

    if let Some(headers) = args.get("headers").and_then(|v| v.as_object()) {
        for (k, v) in headers {
            if let Some(val) = v.as_str() {
                req = req.header(k.as_str(), val);
            }
        }
    }

    if !body.is_empty() {
        req = req.body(body.to_string());
    }

    let response = req.send().await.map_err(|e| anyhow!("request failed: {}", e))?;
    let status = response.status();
    let response_body = response
        .text()
        .await
        .map_err(|e| anyhow!("failed to read response: {}", e))?;

    let result = format!("HTTP {} {}\n\n{}", status.as_u16(), status.as_str(), response_body);
    if result.len() > 8000 {
        Ok(format!("{}\n\n[truncated at 8000 chars]", &result[..8000]))
    } else {
        Ok(result)
    }
}

async fn execute_get_system_info(_args: &serde_json::Value) -> Result<String> {
    use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(MemoryRefreshKind::everything()),
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    sys.refresh_cpu_usage();

    let cpu_usage = sys.global_cpu_usage();
    let mem_total = sys.total_memory();
    let mem_used = sys.used_memory();
    let swap_total = sys.total_swap();
    let swap_used = sys.used_swap();
    let load = System::load_average();
    let cores = sys.cpus().len();

    Ok(format!(
        "CPU: {:.1}% ({} cores)\nMemory: {}/{} ({:.1}% used)\nSwap: {}/{}\nLoad Average: {:.2} {:.2} {:.2}",
        cpu_usage,
        cores,
        fmt_bytes(mem_used),
        fmt_bytes(mem_total),
        if mem_total > 0 { mem_used as f64 / mem_total as f64 * 100.0 } else { 0.0 },
        fmt_bytes(swap_used),
        fmt_bytes(swap_total),
        load.one,
        load.five,
        load.fifteen,
    ))
}

fn fmt_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0}KB", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}
