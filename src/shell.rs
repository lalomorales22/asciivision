use std::time::{Duration, Instant};
use tokio::process::Command;

pub struct ShellOutcome {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration: Duration,
    pub timed_out: bool,
}

pub async fn run(command: String) -> ShellOutcome {
    let started = Instant::now();
    let child = match Command::new("zsh")
        .arg("-lc")
        .arg(&command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return ShellOutcome {
                command,
                stdout: String::new(),
                stderr: format!("failed to spawn shell: {}", error),
                exit_code: None,
                duration: started.elapsed(),
                timed_out: false,
            };
        }
    };

    let pid = child.id();

    match tokio::time::timeout(Duration::from_secs(90), child.wait_with_output()).await {
        Ok(Ok(output)) => ShellOutcome {
            command,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code(),
            duration: started.elapsed(),
            timed_out: false,
        },
        Ok(Err(error)) => ShellOutcome {
            command,
            stdout: String::new(),
            stderr: format!("shell execution failed: {}", error),
            exit_code: None,
            duration: started.elapsed(),
            timed_out: false,
        },
        Err(_) => {
            if let Some(pid) = pid {
                let _ = Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .status()
                    .await;
            }
            ShellOutcome {
                command,
                stdout: String::new(),
                stderr: "command timed out after 90 seconds".to_string(),
                exit_code: None,
                duration: started.elapsed(),
                timed_out: true,
            }
        }
    }
}

pub fn format_outcome(outcome: &ShellOutcome, max_chars: usize) -> String {
    let mut body = String::new();
    body.push_str(&format!("$ {}\n", outcome.command));
    body.push_str(&format!(
        "exit={} time={:.2}s{}\n\n",
        outcome
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "?".to_string()),
        outcome.duration.as_secs_f32(),
        if outcome.timed_out { " timeout" } else { "" },
    ));

    if !outcome.stdout.trim().is_empty() {
        body.push_str("stdout:\n");
        body.push_str(&outcome.stdout);
        if !outcome.stdout.ends_with('\n') {
            body.push('\n');
        }
        body.push('\n');
    }

    if !outcome.stderr.trim().is_empty() {
        body.push_str("stderr:\n");
        body.push_str(&outcome.stderr);
        if !outcome.stderr.ends_with('\n') {
            body.push('\n');
        }
    }

    if body.is_empty() {
        body.push_str("command returned no output");
    }

    if body.chars().count() > max_chars {
        let truncated: String = body.chars().take(max_chars).collect();
        format!("{}\n\n[output truncated]", truncated)
    } else {
        body
    }
}
