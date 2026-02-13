use std::process::Command;
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;

use crate::app::{Provider, WorkerEvent};
use crate::providers;
use crate::DispatchTarget;

pub(crate) fn execute_line(
    primary_provider: Provider,
    available_providers: Vec<Provider>,
    line: String,
    dispatch_target: DispatchTarget,
    tx: Sender<WorkerEvent>,
    child_pids: Arc<Mutex<Vec<u32>>>,
) {
    if line.starts_with("/") {
        let res = execute_slash_command(&line, &tx);
        match res {
            Ok(text) => {
                let _ = tx.send(WorkerEvent::Done(text));
            }
            Err(err) => {
                let _ = tx.send(WorkerEvent::Error(err));
            }
        }
        return;
    }

    let providers = match &dispatch_target {
        DispatchTarget::Primary => {
            if available_providers.contains(&primary_provider) {
                vec![primary_provider]
            } else {
                Vec::new()
            }
        }
        DispatchTarget::All => crate::ordered_providers(primary_provider, &available_providers),
        DispatchTarget::Provider(provider) => {
            if available_providers.contains(provider) {
                vec![*provider]
            } else {
                Vec::new()
            }
        }
        DispatchTarget::Providers(targets) => targets
            .iter()
            .copied()
            .filter(|provider| available_providers.contains(provider))
            .fold(Vec::new(), |mut acc, provider| {
                if !acc.contains(&provider) {
                    acc.push(provider);
                }
                acc
            }),
    };

    if providers.is_empty() {
        let msg = match &dispatch_target {
            DispatchTarget::Primary => {
                format!(
                    "primary agent {} not available on PATH",
                    primary_provider.as_str()
                )
            }
            DispatchTarget::All => {
                "no available agent found (need claude and/or codex on PATH)".to_string()
            }
            DispatchTarget::Provider(provider) => {
                format!("{} not available on PATH", provider.as_str())
            }
            DispatchTarget::Providers(targets) => {
                let missing = targets
                    .iter()
                    .filter(|p| !available_providers.contains(*p))
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>();
                if missing.is_empty() {
                    "requested agents not available on PATH".to_string()
                } else {
                    format!(
                        "requested agents not available on PATH: {}",
                        missing.join(",")
                    )
                }
            }
        };
        let _ = tx.send(WorkerEvent::Error(msg));
        return;
    }

    let handles: Vec<std::thread::JoinHandle<bool>> = providers
        .into_iter()
        .map(|provider| {
            let tx = tx.clone();
            let line = line.clone();
            let available = available_providers.clone();
            let pids = child_pids.clone();
            std::thread::spawn(move || {
                let _ = tx.send(WorkerEvent::AgentStart(provider));
                let success = match providers::run_provider_stream(provider, &line, &tx, &pids) {
                    Ok(final_text) => {
                        if !final_text.trim().is_empty() {
                            let _ = tx.send(WorkerEvent::AgentChunk {
                                provider,
                                chunk: final_text.trim().to_string(),
                            });
                        }
                        true
                    }
                    Err(err) => {
                        if let Some(to) =
                            providers::pick_promoted_provider(provider, &available, &err)
                        {
                            let _ = tx.send(WorkerEvent::PromotePrimary {
                                to,
                                reason: err.clone(),
                            });
                        }
                        let _ = tx.send(WorkerEvent::AgentChunk {
                            provider,
                            chunk: format!("{} error: {}", provider.as_str(), err),
                        });
                        false
                    }
                };
                let _ = tx.send(WorkerEvent::AgentDone(provider));
                success
            })
        })
        .collect();

    let mut had_success = false;
    for h in handles {
        if let Ok(true) = h.join() {
            had_success = true;
        }
    }

    if had_success {
        let _ = tx.send(WorkerEvent::Done(String::new()));
    } else {
        let _ = tx.send(WorkerEvent::Error(
            "all available agents failed for this request".to_string(),
        ));
    }
}

fn execute_slash_command(
    line: &str,
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<String, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(String::new());
    }
    match parts[0] {
        "/help" => Ok(help_text()),
        "/commands" => Ok(crate::default_commands().join("\n")),
        "/tool" => run_tool(parts, tx),
        "/provider" => Ok("provider alias enabled; use /primary".to_string()),
        "/primary" => Ok("primary change handled in UI".to_string()),
        "/theme" => Ok("theme change handled in UI".to_string()),
        "/clear" => Ok("clear handled in UI".to_string()),
        "/mem" => Ok("memory command handled in UI".to_string()),
        _ => Err("unknown command. use /help".to_string()),
    }
}

fn run_tool(parts: Vec<&str>, tx: &Sender<WorkerEvent>) -> std::result::Result<String, String> {
    if parts.len() < 2 {
        return Err("usage: /tool <name> [input]".to_string());
    }
    let tool = parts[1];
    let input = if parts.len() > 2 {
        parts[2..].join(" ")
    } else {
        String::new()
    };

    let _ = tx.send(WorkerEvent::Tool(format!("invoke {} {}", tool, input)));

    match tool {
        "echo" => Ok(input),
        "time" => Ok(format!("{:?}", std::time::SystemTime::now())),
        "bash" => {
            if input.trim().is_empty() {
                return Err("usage: /tool bash <command>".to_string());
            }
            let output = Command::new("bash")
                .arg("-lc")
                .arg(&input)
                .output()
                .map_err(|e| format!("bash failed: {e}"))?;
            let mut text = String::new();
            if !output.stdout.is_empty() {
                text.push_str(&String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                if !text.is_empty() {
                    text.push_str("\n");
                }
                text.push_str(&String::from_utf8_lossy(&output.stderr));
            }
            Ok(text.trim().to_string())
        }
        _ => Err(format!("unknown tool: {}", tool)),
    }
}

fn help_text() -> String {
    [
        "commands",
        "",
        "conversation",
        "  /help",
        "  /commands",
        "  /clear",
        "  /exit",
        "",
        "routing",
        "  /primary [claude|codex]",
        "  /provider [claude|codex]",
        "",
        "visibility",
        "  /theme [fjord|graphite|solarized|aurora|ember]",
        "  /mem [show|find|prune|clear]",
        "",
        "tools",
        "  /tool <echo|time|bash> [input]",
        "",
        "dispatch override",
        "  @claude <task>  message to claude",
        "  @codex <task>   message to codex",
        "  @all <task>     single message to all agents",
        "  @claude @codex <task>  collaborate with selected agents",
        "",
        "keys",
        "  Enter send | Shift+Enter newline | PgUp/PgDn scroll",
        "  Ctrl+R history search",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    #[test]
    fn commands_output_does_not_include_events_entries() {
        let (tx, _rx) = unbounded();
        let output = execute_slash_command("/commands", &tx).expect("commands output");

        assert!(!output.contains("/events on"));
        assert!(!output.contains("/events off"));
        assert!(output.contains("/mem"));
    }

    #[test]
    fn help_text_does_not_include_events_toggle() {
        let text = help_text();
        assert!(!text.contains("/events"));
        assert!(text.contains("/mem [show|find|prune|clear]"));
    }
}
