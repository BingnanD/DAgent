use super::*;
use crossbeam_channel::unbounded;
use std::sync::{Arc, Mutex};

impl App {
    pub(super) fn submit_current_line(&mut self, force: bool) {
        let typed_line = self.input.trim().to_string();
        if typed_line.is_empty() {
            return;
        }

        if typed_line == "/exit" || typed_line == "/quit" {
            self.should_quit = true;
            return;
        }

        // Slash-command worker is running — block everything.
        if self.assistant_idx.is_some() {
            let msg = "task is running, wait...";
            if !self.last_system_entry_is(msg) {
                self.push_entry(EntryKind::System, msg);
            }
            return;
        }

        let mut dispatch_target = DispatchTarget::Primary;
        let mut line = typed_line.clone();
        if !typed_line.starts_with('/') {
            match parse_dispatch_override(&typed_line) {
                Ok(Some((target, prompt))) => {
                    dispatch_target = target;
                    line = prompt;
                }
                Ok(None) => {}
                Err(err) => {
                    self.push_entry(EntryKind::Error, err);
                    self.clear_input_buffer();
                    return;
                }
            }
        }

        // For provider submissions, block only if the target provider(s) are busy.
        if !typed_line.starts_with('/') {
            let target_providers: Vec<Provider> = match &dispatch_target {
                DispatchTarget::Primary => vec![self.primary_provider],
                DispatchTarget::Provider(p) => vec![*p],
                DispatchTarget::Providers(ps) => ps.clone(),
            };
            let busy_names: Vec<&str> = target_providers
                .iter()
                .filter(|&&p| self.running_providers.contains(&p))
                .map(|p| p.as_str())
                .collect();
            if !busy_names.is_empty() {
                let msg = format!("{} is running, wait...", busy_names.join(", "));
                if !self.last_system_entry_is(&msg) {
                    self.push_entry(EntryKind::System, msg);
                }
                return;
            }
        }

        if let Some((tool, reason)) = high_risk_check(&line) {
            if !force && !self.allow_high_risk_tools.contains(&tool) {
                self.approval = Some(PendingApproval { line, tool, reason });
                self.mode = Mode::Approval;
                return;
            }
        }

        if line == "/clear" {
            self.entries.clear();
            self.invalidate_render_cache();
            self.needs_screen_clear = true;
            if let Some(memory) = &self.memory {
                if let Err(err) = memory.clear_session(&self.session_id) {
                    self.push_entry(
                        EntryKind::System,
                        format!("memory clear failed: {}", truncate(&err.to_string(), 80)),
                    );
                }
            }
            self.clear_input_buffer();
            self.last_status = "cleared".to_string();
            return;
        }

        if let Some(rest) = line.strip_prefix("/mem") {
            self.handle_memory_command(rest.trim());
            self.clear_input_buffer();
            return;
        }

        if let Some(rest) = line.strip_prefix("/theme") {
            self.handle_theme_change(rest.trim());
            self.clear_input_buffer();
            return;
        }

        if let Some(rest) = line.strip_prefix("/provider") {
            let target = rest.trim();
            self.handle_primary_change(target);
            self.clear_input_buffer();
            return;
        }

        if let Some(rest) = line.strip_prefix("/primary") {
            let target = rest.trim();
            self.handle_primary_change(target);
            self.clear_input_buffer();
            return;
        }

        if let Some(rest) = line.strip_prefix("/workspace") {
            self.handle_workspace_change(rest.trim());
            self.clear_input_buffer();
            return;
        }

        self.history.push(typed_line.clone());
        self.history_pos = None;

        let is_slash = line.starts_with('/');
        let providers = if is_slash {
            Vec::new()
        } else {
            resolve_dispatch_providers(
                self.primary_provider,
                &self.available_providers,
                &dispatch_target,
            )
        };
        if !is_slash && providers.is_empty() {
            let msg = match &dispatch_target {
                DispatchTarget::Primary => format!(
                    "primary agent {} not available on PATH",
                    self.primary_provider.as_str()
                ),
                DispatchTarget::Provider(provider) => {
                    format!("{} not available on PATH", provider.as_str())
                }
                DispatchTarget::Providers(targets) => {
                    let missing = targets
                        .iter()
                        .filter(|provider| !self.available_providers.contains(provider))
                        .map(|provider| provider.as_str())
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
            self.push_entry(EntryKind::Error, msg);
            self.clear_input_buffer();
            return;
        }

        line = self.consume_pending_pastes(&line);
        self.push_entry(EntryKind::User, typed_line);
        if !is_slash {
            if let Some(memory) = &self.memory {
                if let Err(err) = memory.append_message(&self.session_id, "user", None, &line) {
                    self.push_entry(
                        EntryKind::System,
                        format!("memory write failed: {}", truncate(&err.to_string(), 80)),
                    );
                }
            }
        }
        let run_target = if is_slash {
            "command".to_string()
        } else {
            providers_label(&providers)
        };
        if is_slash {
            self.push_entry(EntryKind::Assistant, WORKING_PLACEHOLDER.to_string());
            self.assistant_idx = Some(self.entries.len() - 1);
        } else {
            for provider in providers.iter().copied() {
                self.push_entry(
                    EntryKind::Assistant,
                    format!("[{}]\n{}", provider.as_str(), WORKING_PLACEHOLDER),
                );
                self.agent_entries.insert(provider, self.entries.len() - 1);
                self.agent_had_chunk.insert(provider, false);
                if self.active_provider.is_none() {
                    self.active_provider = Some(provider);
                }
            }
        }
        self.autoscroll = true;
        self.scroll = self.scroll_max();
        self.last_tool_event.clear();
        self.last_status = format!("dispatching {}", run_target);
        self.clear_input_buffer();

        let line_for_worker = if line.starts_with("/") {
            line.clone()
        } else {
            self.build_contextual_prompt(&line)
        };

        let provider = self.primary_provider;
        let available = self.available_providers.clone();
        let child_pids: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = unbounded::<WorkerEvent>();
        let dispatch_target_for_worker = dispatch_target.clone();
        let run_providers = if is_slash { Vec::new() } else { providers.clone() };
        let child_pids_for_thread = child_pids.clone();
        std::thread::spawn(move || {
            execute_line(
                provider,
                available,
                line_for_worker,
                dispatch_target_for_worker,
                tx,
                child_pids_for_thread,
            )
        });
        self.start_running_state(run_target.clone(), run_providers, rx, child_pids);
    }

    pub(super) fn handle_primary_change(&mut self, target: &str) {
        if target.is_empty() {
            self.push_entry(
                EntryKind::System,
                format!(
                    "primary: {} | agents: {}",
                    self.primary_provider.as_str(),
                    providers_label(&self.available_providers)
                ),
            );
            return;
        }

        let selected = match target {
            "claude" => Provider::Claude,
            "codex" => Provider::Codex,
            _ => {
                self.push_entry(EntryKind::Error, "usage: /primary [claude|codex]");
                return;
            }
        };

        if !self.available_providers.contains(&selected) {
            self.push_entry(
                EntryKind::Error,
                format!("{} not available on PATH", selected.as_str()),
            );
            return;
        }

        self.primary_provider = selected;
        self.push_entry(
            EntryKind::System,
            format!("primary set to {}", self.primary_provider.as_str()),
        );
    }

    pub(super) fn handle_theme_change(&mut self, target: &str) {
        if target.is_empty() {
            self.push_entry(
                EntryKind::System,
                format!(
                    "theme: {} | options: fjord, graphite, solarized, aurora, ember",
                    self.theme.as_str()
                ),
            );
            return;
        }
        let Some(theme) = ThemePreset::parse(target) else {
            self.push_entry(
                EntryKind::Error,
                "usage: /theme [fjord|graphite|solarized|aurora|ember]",
            );
            return;
        };
        self.theme = theme;
        self.invalidate_render_cache();
        self.last_status = format!("theme {}", self.theme.as_str());
        self.push_entry(
            EntryKind::System,
            format!("theme set to {}", self.theme.as_str()),
        );
    }

    pub(super) fn handle_workspace_change(&mut self, path: &str) {
        if path.is_empty() {
            self.push_entry(
                EntryKind::System,
                format!("workspace: {}", self.current_workspace),
            );
            return;
        }

        // Expand leading ~ to $HOME
        let expanded = if path == "~" {
            std::env::var("HOME").unwrap_or_else(|_| path.to_string())
        } else if let Some(rest) = path.strip_prefix("~/") {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{}/{}", home, rest)
        } else {
            path.to_string()
        };

        match std::env::set_current_dir(&expanded) {
            Ok(()) => {
                let new_cwd = std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| expanded.clone());
                self.current_workspace = new_cwd.clone();
                self.push_entry(
                    EntryKind::System,
                    format!("workspace: {}", new_cwd),
                );
                self.last_status = format!("workspace {}", new_cwd);
            }
            Err(err) => {
                self.push_entry(
                    EntryKind::Error,
                    format!("workspace: {}", err),
                );
            }
        }
    }

    pub(super) fn handle_memory_command(&mut self, args: &str) {
        let usage = [
            "memory commands",
            "  /mem                     show summary",
            "  /mem show [n]            show latest n records (default 20)",
            "  /mem find <query>        search memory in this session",
            "  /mem prune [keep]        keep latest N records (default 200)",
            "  /mem clear               clear memory only (keep transcript)",
        ]
        .join("\n");

        let Some(memory) = self.memory.as_ref() else {
            self.push_entry(EntryKind::Error, "memory backend unavailable");
            self.last_status = "memory unavailable".to_string();
            return;
        };

        let mut parts = args.split_whitespace();
        let sub = parts.next().unwrap_or("");

        if sub.is_empty() || sub == "help" {
            match memory.session_message_count(&self.session_id) {
                Ok(count) => {
                    self.push_entry(
                        EntryKind::System,
                        format!("session memory: {} records\n{}", count, usage),
                    );
                    self.last_status = format!("memory {} records", count);
                }
                Err(err) => {
                    self.push_entry(
                        EntryKind::Error,
                        format!("memory read failed: {}", truncate(&err.to_string(), 80)),
                    );
                    self.last_status = "memory error".to_string();
                }
            }
            return;
        }

        match sub {
            "show" => {
                let limit = match parts.next() {
                    Some(raw) => match raw.parse::<usize>() {
                        Ok(v) if v > 0 => v.min(MEM_SHOW_MAX_LIMIT),
                        _ => {
                            self.push_entry(EntryKind::Error, "usage: /mem show [positive-number]");
                            self.last_status = "memory usage".to_string();
                            return;
                        }
                    },
                    None => MEM_SHOW_DEFAULT_LIMIT,
                };
                if parts.next().is_some() {
                    self.push_entry(EntryKind::Error, "usage: /mem show [n]");
                    self.last_status = "memory usage".to_string();
                    return;
                }

                match memory.list_session_lines(&self.session_id, limit) {
                    Ok(lines) => {
                        if lines.is_empty() {
                            self.push_entry(EntryKind::System, "memory is empty");
                            self.last_status = "memory empty".to_string();
                        } else {
                            self.push_entry(
                                EntryKind::System,
                                format!("memory (latest {}):\n{}", lines.len(), lines.join("\n")),
                            );
                            self.last_status = format!("memory show {}", lines.len());
                        }
                    }
                    Err(err) => {
                        self.push_entry(
                            EntryKind::Error,
                            format!("memory read failed: {}", truncate(&err.to_string(), 80)),
                        );
                        self.last_status = "memory error".to_string();
                    }
                }
            }
            "find" => {
                let query = args.strip_prefix("find").map(str::trim).unwrap_or_default();
                if query.is_empty() {
                    self.push_entry(EntryKind::Error, "usage: /mem find <query>");
                    self.last_status = "memory usage".to_string();
                    return;
                }

                match memory.search_session_lines(&self.session_id, query, MEM_FIND_DEFAULT_LIMIT) {
                    Ok(lines) => {
                        if lines.is_empty() {
                            self.push_entry(EntryKind::System, "memory search: no match");
                            self.last_status = "memory no match".to_string();
                        } else {
                            self.push_entry(
                                EntryKind::System,
                                format!(
                                    "memory search results ({}):\n{}",
                                    lines.len(),
                                    lines.join("\n")
                                ),
                            );
                            self.last_status = format!("memory match {}", lines.len());
                        }
                    }
                    Err(err) => {
                        self.push_entry(
                            EntryKind::Error,
                            format!("memory search failed: {}", truncate(&err.to_string(), 80)),
                        );
                        self.last_status = "memory error".to_string();
                    }
                }
            }
            "prune" | "trim" => {
                let keep = match parts.next() {
                    Some(raw) => match raw.parse::<usize>() {
                        Ok(v) => v,
                        Err(_) => {
                            self.push_entry(
                                EntryKind::Error,
                                "usage: /mem prune [non-negative-number]",
                            );
                            self.last_status = "memory usage".to_string();
                            return;
                        }
                    },
                    None => MEM_PRUNE_DEFAULT_KEEP,
                };
                if parts.next().is_some() {
                    self.push_entry(EntryKind::Error, "usage: /mem prune [keep]");
                    self.last_status = "memory usage".to_string();
                    return;
                }

                let prune_result = memory.prune_session_keep_recent(&self.session_id, keep);
                let count_result = memory.session_message_count(&self.session_id);
                match (prune_result, count_result) {
                    (Ok(removed), Ok(remaining)) => {
                        self.push_entry(
                            EntryKind::System,
                            format!(
                                "memory pruned: removed {}, remaining {}",
                                removed, remaining
                            ),
                        );
                        self.last_status = format!("memory pruned {}", removed);
                    }
                    (Err(err), _) | (_, Err(err)) => {
                        self.push_entry(
                            EntryKind::Error,
                            format!("memory prune failed: {}", truncate(&err.to_string(), 80)),
                        );
                        self.last_status = "memory error".to_string();
                    }
                }
            }
            "clear" => match memory.clear_session(&self.session_id) {
                Ok(()) => {
                    self.push_entry(EntryKind::System, "memory cleared for current session");
                    self.last_status = "memory cleared".to_string();
                }
                Err(err) => {
                    self.push_entry(
                        EntryKind::Error,
                        format!("memory clear failed: {}", truncate(&err.to_string(), 80)),
                    );
                    self.last_status = "memory error".to_string();
                }
            },
            _ => {
                self.push_entry(EntryKind::Error, "usage: /mem [show|find|prune|clear]");
                self.last_status = "memory usage".to_string();
            }
        }
    }
}
