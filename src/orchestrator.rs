use std::collections::HashMap;
use std::process::Command;
use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};

use crate::app::{Provider, WorkerEvent};
use crate::providers;
use crate::skills::{normalize_skill_id, Skill, SkillStore};
use crate::DispatchTarget;

const DECOMPOSITION_TIMEOUT_SECS: u64 = 30;
const SKILL_AUTHOR_TIMEOUT_SECS: u64 = 60;

pub(crate) fn execute_line(
    primary_provider: Provider,
    available_providers: Vec<Provider>,
    line: String,
    dispatch_target: DispatchTarget,
    tx: Sender<WorkerEvent>,
    child_pids: Arc<Mutex<Vec<u32>>>,
) {
    if line.starts_with("/") {
        let res = execute_slash_command(&line, primary_provider, &available_providers, &tx);
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

    let providers =
        crate::resolve_dispatch_providers(primary_provider, &available_providers, &dispatch_target);

    if providers.is_empty() {
        let msg = match &dispatch_target {
            DispatchTarget::Primary => {
                format!(
                    "primary agent {} not available on PATH",
                    primary_provider.as_str()
                )
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

    // --- Task decomposition for multi-provider dispatch ---
    let per_provider_prompts: HashMap<Provider, String> = if providers.len() >= 2
        && is_decomposition_enabled()
    {
        let _ = tx.send(WorkerEvent::Tool {
            provider: None,
            msg: "decomposing task for multi-agent dispatch...".to_string(),
        });

        let decomposition_prompt = build_decomposition_prompt(&line, &providers);

        // Try planners in order: prefer Claude, then Codex
        let mut planner_candidates: Vec<Provider> = Vec::new();
        if providers.contains(&Provider::Claude) {
            planner_candidates.push(Provider::Claude);
        }
        for &p in &providers {
            if !planner_candidates.contains(&p) {
                planner_candidates.push(p);
            }
        }

        let mut decomposed = None;
        for planner in &planner_candidates {
            match providers::run_provider_sync(
                *planner,
                &decomposition_prompt,
                DECOMPOSITION_TIMEOUT_SECS,
            ) {
                Ok(response) => {
                    if let Some(tasks) = parse_decomposition(&response, &providers) {
                        decomposed = Some(tasks);
                        break;
                    }
                    // Parse failed; try next planner if available
                }
                Err(ref err) if is_quota_or_limit_error(err) => {
                    let _ = tx.send(WorkerEvent::Tool {
                        provider: None,
                        msg: format!("{} quota limited, trying next planner...", planner.as_str()),
                    });
                    continue;
                }
                Err(_) => {
                    // Non-quota error; don't retry, fallback to unified dispatch
                    break;
                }
            }
        }

        match decomposed {
            Some(tasks) => {
                let _ = tx.send(WorkerEvent::Tool {
                    provider: None,
                    msg: "task decomposed, dispatching to agents...".to_string(),
                });
                let _ = tx.send(WorkerEvent::Tool {
                    provider: None,
                    msg: format_decomposition_summary(&tasks, &providers),
                });
                let mut prompts = HashMap::new();
                for &p in &providers {
                    if let Some(subtask) = tasks.get(&p) {
                        prompts.insert(p, build_agent_prompt(&line, p, subtask, &tasks));
                    }
                }
                prompts
            }
            None => {
                let _ = tx.send(WorkerEvent::Tool {
                    provider: None,
                    msg: "task decomposition failed, using unified dispatch".to_string(),
                });
                HashMap::new()
            }
        }
    } else {
        HashMap::new()
    };

    let handles: Vec<std::thread::JoinHandle<bool>> = providers
        .into_iter()
        .map(|provider| {
            let tx = tx.clone();
            let prompt_for_agent = per_provider_prompts
                .get(&provider)
                .cloned()
                .unwrap_or_else(|| line.clone());
            let available = available_providers.clone();
            let pids = child_pids.clone();
            std::thread::spawn(move || {
                let _ = tx.send(WorkerEvent::AgentStart(provider));
                let success =
                    match providers::run_provider_stream(provider, &prompt_for_agent, &tx, &pids) {
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
    primary_provider: Provider,
    available_providers: &[Provider],
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
        "/skill" => run_skill_command(line, primary_provider, available_providers, tx),
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

    let _ = tx.send(WorkerEvent::Tool {
        provider: None,
        msg: format!("invoke {} {}", tool, input),
    });

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
                    text.push('\n');
                }
                text.push_str(&String::from_utf8_lossy(&output.stderr));
            }
            Ok(text.trim().to_string())
        }
        _ => Err(format!("unknown tool: {}", tool)),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SkillDraft {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    content: String,
}

fn run_skill_command(
    line: &str,
    primary_provider: Provider,
    available_providers: &[Provider],
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<String, String> {
    let rest = line
        .trim()
        .strip_prefix("/skill")
        .unwrap_or_default()
        .trim();
    let (sub, args) = split_once_ws(rest);

    let store = SkillStore::open_default().map_err(|e| format!("open skills failed: {e}"))?;

    if sub.is_empty() || sub == "help" {
        let count = store
            .count()
            .map_err(|e| format!("read skills failed: {e}"))?;
        return Ok(skill_usage_text(count));
    }

    match sub {
        "list" | "ls" => {
            let skills = store
                .list()
                .map_err(|e| format!("list skills failed: {e}"))?;
            if skills.is_empty() {
                return Ok("skills: empty".to_string());
            }
            let mut lines = vec![format!("skills ({}):", skills.len())];
            for s in skills {
                let mut row = format!("- {} | {}", s.id, s.name);
                if !s.description.is_empty() {
                    row.push_str(&format!(" | {}", s.description));
                }
                row.push_str(&format!(" | updated_at={}", s.updated_at));
                lines.push(row);
            }
            Ok(lines.join("\n"))
        }
        "show" | "get" => {
            let id =
                normalize_skill_id(args).ok_or_else(|| "usage: /skill show <id>".to_string())?;
            let Some(skill) = store
                .get(&id)
                .map_err(|e| format!("read skill failed: {e}"))?
            else {
                return Err(format!("skill not found: {}", id));
            };
            Ok(format_skill_detail(&skill))
        }
        "delete" | "rm" => {
            let id =
                normalize_skill_id(args).ok_or_else(|| "usage: /skill delete <id>".to_string())?;
            let deleted = store
                .delete(&id)
                .map_err(|e| format!("delete skill failed: {e}"))?;
            if deleted {
                Ok(format!("skill deleted: {}", id))
            } else {
                Err(format!("skill not found: {}", id))
            }
        }
        "create" | "add" => {
            let (raw_name, intent) = split_once_ws(args);
            if raw_name.is_empty() || intent.is_empty() {
                return Err("usage: /skill create <name> <intent>".to_string());
            }
            let id = normalize_skill_id(raw_name)
                .ok_or_else(|| format!("invalid skill name: {}", raw_name))?;
            if store
                .get(&id)
                .map_err(|e| format!("read skill failed: {e}"))?
                .is_some()
            {
                return Err(format!("skill already exists: {}", id));
            }

            let draft =
                author_skill_create(raw_name, intent, primary_provider, available_providers, tx)?;
            let name = if draft.name.trim().is_empty() {
                raw_name.trim().to_string()
            } else {
                draft.name.trim().to_string()
            };
            let skill = store
                .create(&id, &name, &draft.description, &draft.content)
                .map_err(|e| format!("save skill failed: {e}"))?;
            Ok(format!(
                "skill created: {}\n{}",
                skill.id,
                format_skill_detail(&skill)
            ))
        }
        "update" | "edit" => {
            let (raw_id, intent) = split_once_ws(args);
            if raw_id.is_empty() || intent.is_empty() {
                return Err("usage: /skill update <id> <intent>".to_string());
            }
            let id = normalize_skill_id(raw_id)
                .ok_or_else(|| format!("invalid skill id: {}", raw_id))?;
            let Some(existing) = store
                .get(&id)
                .map_err(|e| format!("read skill failed: {e}"))?
            else {
                return Err(format!("skill not found: {}", id));
            };

            let draft =
                author_skill_update(&existing, intent, primary_provider, available_providers, tx)?;

            let name = if draft.name.trim().is_empty() {
                None
            } else {
                Some(draft.name.trim())
            };
            let description = if draft.description.trim().is_empty() {
                None
            } else {
                Some(draft.description.trim())
            };
            let content = if draft.content.trim().is_empty() {
                None
            } else {
                Some(draft.content.trim())
            };

            let updated = store
                .update(&id, name, description, content)
                .map_err(|e| format!("update skill failed: {e}"))?;
            Ok(format!(
                "skill updated: {}\n{}",
                updated.id,
                format_skill_detail(&updated)
            ))
        }
        _ => Err("usage: /skill [list|show|create|update|delete]".to_string()),
    }
}

fn author_skill_create(
    raw_name: &str,
    intent: &str,
    primary_provider: Provider,
    available_providers: &[Provider],
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<SkillDraft, String> {
    let prompt = build_skill_create_prompt(raw_name, intent);
    let mut draft = run_skill_authoring(prompt, primary_provider, available_providers, tx)?;
    if draft.name.trim().is_empty() {
        draft.name = raw_name.trim().to_string();
    }
    Ok(draft)
}

fn author_skill_update(
    existing: &Skill,
    intent: &str,
    primary_provider: Provider,
    available_providers: &[Provider],
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<SkillDraft, String> {
    let prompt = build_skill_update_prompt(existing, intent);
    let mut draft = run_skill_authoring(prompt, primary_provider, available_providers, tx)?;
    if draft.name.trim().is_empty() {
        draft.name = existing.name.clone();
    }
    if draft.content.trim().is_empty() {
        draft.content = existing.content.clone();
    }
    Ok(draft)
}

fn run_skill_authoring(
    prompt: String,
    primary_provider: Provider,
    available_providers: &[Provider],
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<SkillDraft, String> {
    let chain = select_skill_author_chain(primary_provider, available_providers);
    if chain.is_empty() {
        return Err(
            "no available agent for /skill create|update (need claude and/or codex on PATH)"
                .to_string(),
        );
    }

    let first = chain[0];
    let first_result = generate_skill_draft(first, &prompt, tx);

    let mut base = match first_result {
        Ok(draft) => draft,
        Err(first_err) => {
            if chain.len() < 2 {
                return Err(first_err);
            }
            let fallback = chain[1];
            let _ = tx.send(WorkerEvent::Tool {
                provider: Some(fallback),
                msg: format!(
                    "{} skill authoring fallback after {}",
                    fallback.as_str(),
                    first.as_str()
                ),
            });
            generate_skill_draft(fallback, &prompt, tx)?
        }
    };

    if chain.len() >= 2 {
        let second = chain[1];
        if second != first {
            let refine_prompt = build_skill_refine_prompt(&base, &prompt);
            match generate_skill_draft(second, &refine_prompt, tx) {
                Ok(refined) => {
                    base = refined;
                }
                Err(err) => {
                    let _ = tx.send(WorkerEvent::Tool {
                        provider: Some(second),
                        msg: format!("{} refine skipped: {}", second.as_str(), preview(&err, 80)),
                    });
                }
            }
        }
    }

    Ok(base)
}

fn select_skill_author_chain(
    primary_provider: Provider,
    available_providers: &[Provider],
) -> Vec<Provider> {
    if available_providers.contains(&Provider::Claude)
        && available_providers.contains(&Provider::Codex)
    {
        return vec![Provider::Claude, Provider::Codex];
    }
    if available_providers.contains(&primary_provider) {
        return vec![primary_provider];
    }
    if available_providers.contains(&Provider::Claude) {
        return vec![Provider::Claude];
    }
    if available_providers.contains(&Provider::Codex) {
        return vec![Provider::Codex];
    }
    Vec::new()
}

fn generate_skill_draft(
    provider: Provider,
    prompt: &str,
    tx: &Sender<WorkerEvent>,
) -> std::result::Result<SkillDraft, String> {
    let _ = tx.send(WorkerEvent::Tool {
        provider: Some(provider),
        msg: format!("{} authoring skill...", provider.as_str()),
    });
    let raw = providers::run_provider_sync(provider, prompt, SKILL_AUTHOR_TIMEOUT_SECS)?;
    let draft = parse_skill_draft(&raw).map_err(|e| {
        format!(
            "{} skill parse failed: {} | output={}",
            provider.as_str(),
            e,
            preview(&raw, 120)
        )
    })?;
    let _ = tx.send(WorkerEvent::Tool {
        provider: Some(provider),
        msg: format!("{} skill draft ready", provider.as_str()),
    });
    Ok(draft)
}

fn parse_skill_draft(raw: &str) -> std::result::Result<SkillDraft, String> {
    let direct = serde_json::from_str::<SkillDraft>(raw).ok();
    let parsed = if let Some(v) = direct {
        v
    } else if let Some(json) = extract_json_object(raw) {
        serde_json::from_str::<SkillDraft>(&json).map_err(|e| e.to_string())?
    } else {
        return Err("no json object found".to_string());
    };

    if parsed.content.trim().is_empty() {
        return Err("field 'content' is empty".to_string());
    }
    Ok(parsed)
}

fn extract_json_object(raw: &str) -> Option<String> {
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in raw.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            continue;
        }

        if ch == '{' {
            if depth == 0 {
                start = Some(idx);
            }
            depth += 1;
            continue;
        }

        if ch == '}' {
            if depth == 0 {
                continue;
            }
            depth -= 1;
            if depth == 0 {
                if let Some(begin) = start {
                    return Some(raw[begin..=idx].to_string());
                }
            }
        }
    }
    None
}

fn build_skill_create_prompt(raw_name: &str, intent: &str) -> String {
    format!(
        r#"You are authoring a reusable DAgent skill.

Skill name hint: {name}
User intent:
{intent}

Return only one JSON object with exactly these fields:
{{
  "name": "...",
  "description": "...",
  "content": "..."
}}

Rules:
- Keep "name" concise and practical.
- "description" should be one sentence.
- "content" must be concrete instructions and checks the agent can execute.
- Do not wrap JSON in markdown fences."#,
        name = raw_name,
        intent = intent
    )
}

fn build_skill_update_prompt(existing: &Skill, intent: &str) -> String {
    let existing_json = serde_json::to_string_pretty(existing).unwrap_or_else(|_| "{}".to_string());
    format!(
        r#"You are revising an existing DAgent skill.

Existing skill JSON:
{existing}

Update request:
{intent}

Return only one JSON object with exactly these fields:
{{
  "name": "...",
  "description": "...",
  "content": "..."
}}

Rules:
- Keep the skill focused and executable.
- Preserve useful existing behavior unless the request changes it.
- Do not include extra keys.
- Do not wrap JSON in markdown fences."#,
        existing = existing_json,
        intent = intent
    )
}

fn build_skill_refine_prompt(draft: &SkillDraft, original_prompt: &str) -> String {
    let draft_json = serde_json::to_string_pretty(draft).unwrap_or_else(|_| "{}".to_string());
    format!(
        r#"You are refining a DAgent skill draft to make it implementation-ready.

Original task:
{original}

Current draft:
{draft}

Return only one JSON object with exactly these fields:
{{
  "name": "...",
  "description": "...",
  "content": "..."
}}

Rules:
- Keep what already works.
- Improve specificity, edge-case handling, and actionability.
- Do not wrap JSON in markdown fences."#,
        original = original_prompt,
        draft = draft_json
    )
}

fn split_once_ws(input: &str) -> (&str, &str) {
    let trimmed = input.trim();
    for (idx, ch) in trimmed.char_indices() {
        if ch.is_whitespace() {
            return (&trimmed[..idx], trimmed[idx..].trim());
        }
    }
    (trimmed, "")
}

fn format_skill_detail(skill: &Skill) -> String {
    format!(
        "id: {id}\nname: {name}\ndescription: {desc}\ncreated_at: {created}\nupdated_at: {updated}\ncontent:\n{content}",
        id = skill.id,
        name = skill.name,
        desc = if skill.description.trim().is_empty() {
            "<none>"
        } else {
            skill.description.as_str()
        },
        created = skill.created_at,
        updated = skill.updated_at,
        content = skill.content
    )
}

fn skill_usage_text(count: usize) -> String {
    format!(
        "skills: {} loaded\n\
         usage:\n\
           /skill list\n\
           /skill show <id>\n\
           /skill create <name> <intent>\n\
           /skill update <id> <intent>\n\
           /skill delete <id>\n\
         notes:\n\
           - create/update are agent-authored (claude + codex when both available)\n\
           - mention @skill:<id> in your prompt to force-load a skill",
        count
    )
}

fn preview(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.trim().to_string();
    }
    let mut clipped = text.chars().take(limit).collect::<String>();
    clipped.push_str("...");
    clipped
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
        "  /skill [list|show|create|update|delete]",
        "",
        "tools",
        "  /tool <echo|time|bash> [input]",
        "",
        "dispatch override",
        "  @claude <task>  message to claude",
        "  @codex <task>   message to codex",
        "  @claude @codex <task>  collaborate with selected agents",
        "",
        "keys",
        "  Enter send | Shift+Enter newline | PgUp/PgDn scroll",
        "  Ctrl+R history search",
    ]
    .join("\n")
}

// --- Task decomposition helpers ---

fn is_quota_or_limit_error(err: &str) -> bool {
    let t = err.to_lowercase();
    t.contains("quota")
        || t.contains("rate_limit")
        || t.contains("rate limit")
        || t.contains("hit your limit")
        || t.contains("usage limit")
        || t.contains("credit balance")
        || t.contains("insufficient credits")
}

fn parse_enabled_flag(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

fn is_decomposition_enabled() -> bool {
    std::env::var("DAGENT_DECOMPOSE")
        .ok()
        .map(|v| parse_enabled_flag(&v))
        .unwrap_or(true)
}

fn build_decomposition_prompt(user_task: &str, providers: &[Provider]) -> String {
    let agent_list: Vec<&str> = providers.iter().map(|p| p.as_str()).collect();
    format!(
        r#"You are a task planner for a multi-agent system. Decompose the user task into subtasks for specialized agents.

Available agents:
- claude: Best at analysis, planning, code review, documentation, problem diagnosis, architecture design, explaining concepts.
- codex: Best at code implementation, file editing, running shell commands, testing, making code changes.

User task:
---
{task}
---

Respond in EXACTLY this format with no extra text before or after:

[{a0}]
<subtask for {a0}>

[{a1}]
<subtask for {a1}>

Rules:
- Only use agents: {agents}
- Each subtask should be specific and actionable
- Subtasks should be independent (agents work in parallel)
- Leverage each agent's strengths
- Reference the overall goal so each agent has context"#,
        task = user_task,
        a0 = agent_list[0],
        a1 = if agent_list.len() > 1 {
            agent_list[1]
        } else {
            agent_list[0]
        },
        agents = agent_list.join(", "),
    )
}

fn extract_section_header(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(']')?;
    let name = rest[..end].trim();
    let after = rest[end + 1..].trim();
    if !after.is_empty() || name.is_empty() {
        return None;
    }
    Some(name)
}

fn provider_from_name(name: &str) -> Option<Provider> {
    match name {
        "claude" => Some(Provider::Claude),
        "codex" => Some(Provider::Codex),
        _ => None,
    }
}

fn parse_decomposition(
    response: &str,
    providers: &[Provider],
) -> Option<HashMap<Provider, String>> {
    let mut tasks: HashMap<Provider, String> = HashMap::new();
    let mut current_provider: Option<Provider> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line in response.lines() {
        let trimmed = line.trim();
        if let Some(name) = extract_section_header(trimmed) {
            // Flush previous section
            if let Some(provider) = current_provider.take() {
                let text = current_lines.join("\n").trim().to_string();
                if !text.is_empty() {
                    tasks.insert(provider, text);
                }
            }
            current_lines.clear();
            current_provider = provider_from_name(name);
        } else if current_provider.is_some() {
            current_lines.push(line.to_string());
        }
    }

    // Flush final section
    if let Some(provider) = current_provider {
        let text = current_lines.join("\n").trim().to_string();
        if !text.is_empty() {
            tasks.insert(provider, text);
        }
    }

    // All resolved providers must have a task
    if tasks.len() < 2 || !providers.iter().all(|p| tasks.contains_key(p)) {
        return None;
    }

    Some(tasks)
}

fn format_decomposition_summary(
    tasks: &HashMap<Provider, String>,
    providers: &[Provider],
) -> String {
    let mut lines = vec!["multi-agent plan:".to_string()];
    for provider in providers {
        if let Some(task) = tasks.get(provider) {
            let single_line = task.split_whitespace().collect::<Vec<_>>().join(" ");
            let preview = crate::truncate(&single_line, 180);
            lines.push(format!("- {}: {}", provider.as_str(), preview));
        }
    }
    lines.join("\n")
}

fn build_agent_prompt(
    original_task: &str,
    provider: Provider,
    subtask: &str,
    all_tasks: &HashMap<Provider, String>,
) -> String {
    let mut other_context = String::new();
    for (p, task) in all_tasks.iter() {
        if *p != provider {
            let preview = if task.len() > 200 {
                format!("{}...", &task[..200])
            } else {
                task.clone()
            };
            other_context.push_str(&format!("- {} is working on: {}\n", p.as_str(), preview));
        }
    }

    format!(
        r#"OVERALL TASK: {original_task}

YOUR ASSIGNMENT (you are {agent}): {subtask}

COORDINATION CONTEXT:
Other agents are working on related subtasks in parallel:
{other_context}
Focus on YOUR assignment above. Do not duplicate work assigned to other agents."#,
        original_task = original_task,
        agent = provider.as_str(),
        subtask = subtask,
        other_context = other_context.trim(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    #[test]
    fn commands_output_does_not_include_events_entries() {
        let (tx, _rx) = unbounded();
        let output = execute_slash_command("/commands", Provider::Claude, &[], &tx)
            .expect("commands output");

        assert!(!output.contains("/events on"));
        assert!(!output.contains("/events off"));
        assert!(output.contains("/mem"));
        assert!(output.contains("/skill"));
    }

    #[test]
    fn help_text_does_not_include_events_toggle() {
        let text = help_text();
        assert!(!text.contains("/events"));
        assert!(text.contains("/mem [show|find|prune|clear]"));
        assert!(text.contains("/skill [list|show|create|update|delete]"));
    }

    #[test]
    fn parse_skill_draft_supports_wrapped_json() {
        let wrapped = "some preface\n```json\n{\"name\":\"API Review\",\"description\":\"check api\",\"content\":\"validate response codes\"}\n```";
        let draft = parse_skill_draft(wrapped).expect("parse skill draft");
        assert_eq!(draft.name, "API Review");
        assert!(draft.content.contains("validate"));
    }

    // --- Task decomposition tests ---

    #[test]
    fn parse_decomposition_valid() {
        let response = "[claude]\nReview the code structure and identify issues\n\n[codex]\nImplement the fix based on analysis";
        let providers = vec![Provider::Claude, Provider::Codex];
        let result = parse_decomposition(response, &providers);
        assert!(result.is_some());
        let tasks = result.unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.get(&Provider::Claude).unwrap().contains("Review"));
        assert!(tasks.get(&Provider::Codex).unwrap().contains("Implement"));
    }

    #[test]
    fn parse_decomposition_multiline_subtasks() {
        let response = "[claude]\nAnalyze the architecture.\nCheck for potential issues.\nDocument findings.\n\n[codex]\nFix the bugs.\nRun tests.";
        let providers = vec![Provider::Claude, Provider::Codex];
        let result = parse_decomposition(response, &providers);
        assert!(result.is_some());
        let tasks = result.unwrap();
        assert!(tasks.get(&Provider::Claude).unwrap().contains("Document"));
        assert!(tasks.get(&Provider::Codex).unwrap().contains("Run tests"));
    }

    #[test]
    fn parse_decomposition_missing_provider_returns_none() {
        let response = "[claude]\nDo everything";
        let providers = vec![Provider::Claude, Provider::Codex];
        assert!(parse_decomposition(response, &providers).is_none());
    }

    #[test]
    fn parse_decomposition_with_preamble_text() {
        let response = "Here is my plan:\n\n[claude]\nAnalyze\n\n[codex]\nImplement";
        let providers = vec![Provider::Claude, Provider::Codex];
        let result = parse_decomposition(response, &providers);
        assert!(result.is_some());
    }

    #[test]
    fn parse_decomposition_empty_response() {
        let providers = vec![Provider::Claude, Provider::Codex];
        assert!(parse_decomposition("", &providers).is_none());
    }

    #[test]
    fn parse_decomposition_empty_subtask_returns_none() {
        let response = "[claude]\n\n[codex]\nDo something";
        let providers = vec![Provider::Claude, Provider::Codex];
        assert!(parse_decomposition(response, &providers).is_none());
    }

    #[test]
    fn format_decomposition_summary_lists_each_provider() {
        let providers = vec![Provider::Claude, Provider::Codex];
        let mut tasks = HashMap::new();
        tasks.insert(
            Provider::Claude,
            "Analyze architecture and identify risks".to_string(),
        );
        tasks.insert(Provider::Codex, "Implement fixes and add tests".to_string());

        let summary = format_decomposition_summary(&tasks, &providers);
        assert!(summary.contains("multi-agent plan:"));
        assert!(summary.contains("- claude: Analyze architecture and identify risks"));
        assert!(summary.contains("- codex: Implement fixes and add tests"));
    }

    #[test]
    fn parse_enabled_flag_supports_common_false_values() {
        assert!(!parse_enabled_flag("0"));
        assert!(!parse_enabled_flag("false"));
        assert!(!parse_enabled_flag("no"));
        assert!(!parse_enabled_flag("off"));
        assert!(parse_enabled_flag("true"));
        assert!(parse_enabled_flag("1"));
    }

    #[test]
    fn extract_section_header_valid() {
        assert_eq!(extract_section_header("[claude]"), Some("claude"));
        assert_eq!(extract_section_header("[codex]"), Some("codex"));
    }

    #[test]
    fn extract_section_header_with_trailing_text() {
        assert_eq!(extract_section_header("[claude] some text"), None);
    }

    #[test]
    fn extract_section_header_empty_brackets() {
        assert_eq!(extract_section_header("[]"), None);
    }

    #[test]
    fn extract_section_header_not_a_header() {
        assert_eq!(extract_section_header("not a header"), None);
    }

    #[test]
    fn build_agent_prompt_includes_coordination() {
        let mut tasks = HashMap::new();
        tasks.insert(Provider::Claude, "review code".to_string());
        tasks.insert(Provider::Codex, "implement changes".to_string());
        let prompt = build_agent_prompt("fix the bug", Provider::Claude, "review code", &tasks);
        assert!(prompt.contains("OVERALL TASK: fix the bug"));
        assert!(prompt.contains("YOUR ASSIGNMENT (you are claude)"));
        assert!(prompt.contains("codex is working on"));
        assert!(prompt.contains("Do not duplicate work"));
    }

    #[test]
    fn build_agent_prompt_codex_sees_claude_context() {
        let mut tasks = HashMap::new();
        tasks.insert(Provider::Claude, "analyze architecture".to_string());
        tasks.insert(Provider::Codex, "write tests".to_string());
        let prompt = build_agent_prompt(
            "improve test coverage",
            Provider::Codex,
            "write tests",
            &tasks,
        );
        assert!(prompt.contains("YOUR ASSIGNMENT (you are codex)"));
        assert!(prompt.contains("claude is working on"));
    }
}
