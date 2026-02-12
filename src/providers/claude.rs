use std::io::{BufRead, BufReader};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use serde_json::Value;

use crate::app::{Provider, WorkerEvent};

fn claude_permission_mode() -> String {
    std::env::var("DAGENT_CLAUDE_PERMISSION_MODE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "acceptEdits".to_string())
}

fn claude_allowed_tools() -> Option<String> {
    std::env::var("DAGENT_CLAUDE_ALLOWED_TOOLS")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| Some("Bash".to_string()))
}

fn add_allowed_tools_arg(cmd: &mut Command, allowed_tools: Option<&str>) {
    if let Some(tools) = allowed_tools {
        cmd.arg("--allowedTools").arg(tools);
    }
}

fn is_root_bypass_error(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("--dangerously-skip-permissions") && (t.contains("root") || t.contains("sudo"))
}

fn run_prompt_once(
    prompt: &str,
    permission_mode: &str,
    allowed_tools: Option<&str>,
) -> std::result::Result<Output, String> {
    let mut cmd = Command::new("claude");
    cmd.stdin(Stdio::null());
    cmd.arg("--permission-mode").arg(permission_mode);
    add_allowed_tools_arg(&mut cmd, allowed_tools);
    cmd.arg("-p")
        .arg(prompt)
        .output()
        .map_err(|e| format!("claude fallback failed: {e}"))
}

pub(crate) fn run_stream(
    provider: Provider,
    prompt: &str,
    tx: &Sender<WorkerEvent>,
    child_pids: &Arc<Mutex<Vec<u32>>>,
) -> std::result::Result<String, String> {
    let permission_mode = claude_permission_mode();
    let allowed_tools = claude_allowed_tools();

    let mut cmd = Command::new("claude");
    cmd.arg("--print")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--include-partial-messages")
        .arg("--permission-mode")
        .arg(&permission_mode);
    add_allowed_tools_arg(&mut cmd, allowed_tools.as_deref());
    cmd.arg(prompt);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("claude spawn failed: {e}"))?;
    if let Ok(mut pids) = child_pids.lock() {
        pids.push(child.id());
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "claude stdout missing".to_string())?;
    let reader = BufReader::new(stdout);

    let mut fallback_lines: Vec<String> = Vec::new();
    let mut quota_message = String::new();
    let mut saw_quota_error = false;
    let mut emitted = false;
    let mut emitted_non_quota = false;
    for line in reader.lines() {
        let line = line.map_err(|e| format!("claude stream read failed: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        fallback_lines.push(line.clone());
        if is_quota_error_text(&line) {
            saw_quota_error = true;
        }
        if let Some(tool_info) = extract_tool_use(&line) {
            let _ = tx.send(WorkerEvent::Tool(tool_info));
        }
        if let Some(chunk) = extract_delta_text(&line) {
            if !chunk.trim().is_empty() {
                if is_quota_error_text(&chunk) {
                    saw_quota_error = true;
                    if quota_message.is_empty() {
                        quota_message = chunk;
                    }
                } else {
                    emitted = true;
                    emitted_non_quota = true;
                    let _ = tx.send(WorkerEvent::AgentChunk { provider, chunk });
                }
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("claude wait failed: {e}"))?;
    if status.success() {
        if saw_quota_error && !emitted_non_quota {
            let msg = if quota_message.is_empty() {
                "claude quota/rate limit reached".to_string()
            } else {
                format!("claude quota/rate limit: {}", quota_message)
            };
            return Err(msg);
        }
        if emitted {
            return Ok(String::new());
        }
        if let Some(text) = fallback_lines
            .iter()
            .rev()
            .find_map(|line| extract_fallback_text(line))
        {
            if is_quota_error_text(&text) {
                return Err(format!("claude quota/rate limit: {}", text));
            }
            return Ok(text);
        }
        let last = fallback_lines.last().cloned().unwrap_or_default();
        if is_quota_error_text(&last) {
            return Err(format!("claude quota/rate limit: {}", last));
        }
        return Ok(last);
    }

    let mut mode_for_fallback = permission_mode.clone();
    let mut output = run_prompt_once(prompt, &mode_for_fallback, allowed_tools.as_deref())?;
    if !output.status.success()
        && mode_for_fallback == "bypassPermissions"
        && is_root_bypass_error(&String::from_utf8_lossy(&output.stderr))
    {
        mode_for_fallback = "acceptEdits".to_string();
        let _ = tx.send(WorkerEvent::Tool(
            "claude bypassPermissions blocked under root; retrying with acceptEdits".to_string(),
        ));
        output = run_prompt_once(prompt, &mode_for_fallback, allowed_tools.as_deref())?;
    }

    if !output.status.success() {
        return Err(format!(
            "claude failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let fallback_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if is_quota_error_text(&fallback_text) {
        return Err(format!("claude quota/rate limit: {}", fallback_text));
    }
    Ok(fallback_text)
}

pub(crate) fn is_quota_error_text(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("hit your limit")
        || t.contains("rate_limit")
        || t.contains("rate limit")
        || t.contains("quota")
        || t.contains("credit balance is too low")
        || t.contains("insufficient credits")
        || t.contains("usage limit")
}

fn parse_json_line(line: &str) -> Option<Value> {
    serde_json::from_str(line).ok()
}

fn extract_delta_text(line: &str) -> Option<String> {
    let value = parse_json_line(line)?;
    if value.get("type")?.as_str()? != "stream_event" {
        return None;
    }
    let event = value.get("event")?;
    if event.get("type")?.as_str()? != "content_block_delta" {
        return None;
    }
    let delta = event.get("delta")?;
    if let Some(delta_type) = delta.get("type").and_then(Value::as_str) {
        if delta_type != "text_delta" {
            return None;
        }
    }
    delta.get("text")?.as_str().map(|s| s.to_string())
}

fn extract_tool_use(line: &str) -> Option<String> {
    let value = parse_json_line(line)?;
    // Claude stream-json emits various event types; look for tool use patterns.
    let event_type = value.get("type").and_then(Value::as_str)?;
    match event_type {
        "stream_event" => {
            let event = value.get("event")?;
            let inner_type = event.get("type").and_then(Value::as_str)?;
            match inner_type {
                "content_block_start" => {
                    let cb = event.get("content_block")?;
                    if cb.get("type").and_then(Value::as_str)? == "tool_use" {
                        let name = cb.get("name").and_then(Value::as_str).unwrap_or("unknown");
                        Some(format!("claude calling tool: {}", name))
                    } else {
                        None
                    }
                }
                "content_block_stop" => None,
                _ => None,
            }
        }
        "tool_use" | "tool" => {
            let name = value
                .get("name")
                .or_else(|| value.get("tool"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let input_preview = value
                .get("input")
                .and_then(|v| {
                    if let Some(cmd) = v.get("command").and_then(Value::as_str) {
                        Some(cmd.to_string())
                    } else {
                        serde_json::to_string(v).ok()
                    }
                })
                .unwrap_or_default();
            if input_preview.is_empty() {
                Some(format!("claude tool: {}", name))
            } else {
                let preview = if input_preview.len() > 80 {
                    format!("{}...", &input_preview[..80])
                } else {
                    input_preview
                };
                Some(format!("claude tool: {} | {}", name, preview))
            }
        }
        _ => None,
    }
}

fn extract_fallback_text(line: &str) -> Option<String> {
    let value = parse_json_line(line)?;
    match value.get("type")?.as_str()? {
        "assistant" => value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .and_then(|arr| {
                arr.iter().find_map(|item| {
                    if item.get("type").and_then(Value::as_str) == Some("text") {
                        item.get("text")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            }),
        "result" => value
            .get("result")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
        _ => None,
    }
}
