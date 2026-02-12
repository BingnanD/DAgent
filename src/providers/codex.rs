use std::io::{BufRead, BufReader};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use serde_json::Value;

use crate::app::{Provider, WorkerEvent};

fn codex_approval_policy() -> String {
    std::env::var("DAGENT_CODEX_APPROVAL_POLICY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "never".to_string())
}

fn codex_sandbox_mode() -> String {
    std::env::var("DAGENT_CODEX_SANDBOX")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "danger-full-access".to_string())
}

fn run_prompt_once(
    prompt: &str,
    approval_policy: &str,
    sandbox_mode: &str,
) -> std::result::Result<Output, String> {
    Command::new("codex")
        .arg("--ask-for-approval")
        .arg(approval_policy)
        .stdin(Stdio::null())
        .arg("exec")
        .arg("-s")
        .arg(sandbox_mode)
        .arg("--skip-git-repo-check")
        .arg(prompt)
        .output()
        .map_err(|e| format!("codex fallback failed: {e}"))
}

pub(crate) fn run_stream(
    provider: Provider,
    prompt: &str,
    tx: &Sender<WorkerEvent>,
    child_pids: &Arc<Mutex<Vec<u32>>>,
) -> std::result::Result<String, String> {
    let approval_policy = codex_approval_policy();
    let sandbox_mode = codex_sandbox_mode();
    let prefer_zh = prompt.chars().any(is_cjk_char);

    let mut cmd = Command::new("codex");
    cmd.arg("--ask-for-approval")
        .arg(&approval_policy)
        .arg("exec")
        .arg("-s")
        .arg(&sandbox_mode)
        .arg("--json")
        .arg("--skip-git-repo-check")
        .arg(prompt);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("codex spawn failed: {e}"))?;
    if let Ok(mut pids) = child_pids.lock() {
        pids.push(child.id());
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "codex stdout missing".to_string())?;
    let reader = BufReader::new(stdout);

    let mut fallback_lines: Vec<String> = Vec::new();
    let mut last_progress = String::new();
    let mut emitted = false;
    for line in reader.lines() {
        let line = line.map_err(|e| format!("codex stream read failed: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        fallback_lines.push(line.clone());
        if let Some(progress) = extract_progress(&line, prefer_zh) {
            if progress != last_progress {
                let _ = tx.send(WorkerEvent::Tool(progress.clone()));
                last_progress = progress;
            }
        }
        if let Some(chunk) = extract_text(&line) {
            if !chunk.trim().is_empty() {
                emitted = true;
                let _ = tx.send(WorkerEvent::AgentChunk { provider, chunk });
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("codex wait failed: {e}"))?;
    if status.success() {
        if emitted {
            return Ok(String::new());
        }
        if let Some(text) = fallback_lines
            .iter()
            .rev()
            .find_map(|line| extract_text(line))
        {
            return Ok(text);
        }
        return Ok(String::new());
    }

    let output = run_prompt_once(prompt, &approval_policy, &sandbox_mode)?;
    if !output.status.success() {
        return Err(format!(
            "codex failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_json_line(line: &str) -> Option<Value> {
    serde_json::from_str(line).ok()
}

fn humanize_item_type(raw: &str) -> String {
    raw.replace('_', " ").replace('-', " ")
}

fn preview(text: &str, max_chars: usize) -> String {
    let t = text.trim();
    if t.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut seen = 0usize;
    for ch in t.chars() {
        if seen >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
        seen += 1;
    }
    out
}

fn is_cjk_char(ch: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&ch) || ('\u{3400}'..='\u{4DBF}').contains(&ch)
}

fn command_preview(item: &Value) -> String {
    item.get("command")
        .and_then(Value::as_str)
        .map(|s| preview(s, 96))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "command".to_string())
}

fn extract_progress(line: &str, prefer_zh: bool) -> Option<String> {
    let value = parse_json_line(line)?;
    match value.get("type")?.as_str()? {
        "session.started" | "thread.started" => Some(if prefer_zh {
            "codex 会话已开始".to_string()
        } else {
            "codex session started".to_string()
        }),
        "turn.started" => Some(if prefer_zh {
            "codex 正在分析需求".to_string()
        } else {
            "codex analyzing request".to_string()
        }),
        "turn.completed" => Some(if prefer_zh {
            "codex 正在整理回复".to_string()
        } else {
            "codex wrapping up response".to_string()
        }),
        "item.started" => {
            let item = value.get("item")?;
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("step");
            match item_type {
                "command_execution" => {
                    let cmd = command_preview(item);
                    Some(if prefer_zh {
                        format!("codex 正在执行: {}", cmd)
                    } else {
                        format!("codex exec: {}", cmd)
                    })
                }
                "function_call" | "tool_call" => {
                    let name = item
                        .get("name")
                        .or_else(|| item.get("function").and_then(|f| f.get("name")))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let args_preview = item
                        .get("arguments")
                        .or_else(|| item.get("function").and_then(|f| f.get("arguments")))
                        .and_then(|v| {
                            if let Some(s) = v.as_str() {
                                Some(s.to_string())
                            } else {
                                serde_json::to_string(v).ok()
                            }
                        })
                        .unwrap_or_default();
                    if args_preview.is_empty() {
                        Some(if prefer_zh {
                            format!("codex 正在调用工具: {}", name)
                        } else {
                            format!("codex calling tool: {}", name)
                        })
                    } else {
                        let preview = if args_preview.len() > 80 {
                            format!("{}...", &args_preview[..80])
                        } else {
                            args_preview
                        };
                        Some(if prefer_zh {
                            format!("codex 工具调用: {} | {}", name, preview)
                        } else {
                            format!("codex tool: {} | {}", name, preview)
                        })
                    }
                }
                "reasoning" => Some(if prefer_zh {
                    "codex 思考中...".to_string()
                } else {
                    "codex thinking...".to_string()
                }),
                _ => Some(if prefer_zh {
                    format!("codex 正在处理: {}", humanize_item_type(item_type))
                } else {
                    format!("codex {}...", humanize_item_type(item_type))
                }),
            }
        }
        "item.completed" => {
            let item = value.get("item")?;
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("step");
            match item_type {
                "agent_message" => None,
                "reasoning" => {
                    let thought = item
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|s| preview(s, 110))
                        .unwrap_or_default();
                    if thought.is_empty() {
                        Some(if prefer_zh {
                            "codex 思考中...".to_string()
                        } else {
                            "codex thinking...".to_string()
                        })
                    } else {
                        Some(if prefer_zh {
                            format!("codex 思路: {}", thought)
                        } else {
                            format!("codex thought: {}", thought)
                        })
                    }
                }
                "command_execution" => {
                    let cmd = command_preview(item);
                    let exit_code = item.get("exit_code").and_then(Value::as_i64);
                    let status = item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("completed");
                    let mut msg = if let Some(code) = exit_code {
                        if prefer_zh {
                            format!("codex 执行完成({code}): {cmd}")
                        } else {
                            format!("codex exec done ({code}): {cmd}")
                        }
                    } else {
                        if prefer_zh {
                            format!("codex 执行{}: {}", status, cmd)
                        } else {
                            format!("codex exec {}: {}", status, cmd)
                        }
                    };
                    let output_preview = item
                        .get("aggregated_output")
                        .and_then(Value::as_str)
                        .map(|s| {
                            s.lines()
                                .map(str::trim)
                                .filter(|line| !line.is_empty())
                                .take(2)
                                .collect::<Vec<_>>()
                                .join(" | ")
                        })
                        .map(|s| preview(&s, 88))
                        .unwrap_or_default();
                    if !output_preview.is_empty() {
                        if prefer_zh {
                            msg.push_str(" | 输出: ");
                        } else {
                            msg.push_str(" | ");
                        }
                        msg.push_str(&output_preview);
                    }
                    Some(msg)
                }
                "function_call" | "tool_call" => {
                    let name = item
                        .get("name")
                        .or_else(|| item.get("function").and_then(|f| f.get("name")))
                        .and_then(Value::as_str)
                        .unwrap_or("tool");
                    Some(if prefer_zh {
                        format!("codex 工具完成: {}", name)
                    } else {
                        format!("codex finished: {}", name)
                    })
                }
                _ => Some(if prefer_zh {
                    format!("codex 已完成: {}", humanize_item_type(item_type))
                } else {
                    format!("codex finished {}", humanize_item_type(item_type))
                }),
            }
        }
        "error" => Some(if prefer_zh {
            "codex 运行中出现错误事件".to_string()
        } else {
            "codex emitted an error event".to_string()
        }),
        _ => None,
    }
}

fn extract_text(line: &str) -> Option<String> {
    let value = parse_json_line(line)?;
    if value.get("type")?.as_str()? != "item.completed" {
        return None;
    }
    let item = value.get("item")?;
    if item.get("type")?.as_str()? != "agent_message" {
        return None;
    }
    item.get("text")
        .and_then(Value::as_str)
        .map(|s| s.replace('\r', "\n"))
}
