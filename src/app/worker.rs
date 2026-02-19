use super::*;
use std::time::Instant;

const COORDINATION_TRACE_PREFIX: &str = "[coord] ";

impl App {
    fn should_emit_collaboration_trace(&self) -> bool {
        self.agent_entries.len() >= 2
    }

    fn push_coordination_trace(&mut self, msg: &str) {
        let msg = msg.trim();
        if msg.is_empty() {
            return;
        }
        let text = if msg.starts_with(COORDINATION_TRACE_PREFIX) {
            msg.to_string()
        } else {
            format!("{COORDINATION_TRACE_PREFIX}{msg}")
        };
        if !self.last_system_entry_is(&text) {
            self.push_entry(EntryKind::System, text);
        }
    }

    fn append_agent_progress_line(&mut self, provider: Provider, msg: &str) {
        let msg = msg.trim();
        if msg.is_empty() {
            return;
        }
        let progress_line = format!("{TRANSCRIPT_PROGRESS_PREFIX}{msg}");
        if let Some(i) = self.agent_entries.get(&provider).copied() {
            if let Some(entry) = self.entries.get_mut(i) {
                let already_last_line = entry
                    .text
                    .lines()
                    .last()
                    .is_some_and(|line| line.trim_end() == progress_line);
                if already_last_line {
                    return;
                }
                if !entry.text.ends_with('\n') {
                    entry.text.push('\n');
                }
                entry.text.push_str(&progress_line);
            }
        }
    }

    pub(super) fn interrupt_running_task(&mut self, reason: &str) {
        if !self.running {
            return;
        }

        if let Ok(mut pids) = self.child_pids.lock() {
            for &pid in pids.iter() {
                kill_pid(pid);
            }
            pids.clear();
        }

        for &idx in self.agent_entries.values() {
            if let Some(entry) = self.entries.get_mut(idx) {
                if entry.text.contains(WORKING_PLACEHOLDER) {
                    entry.text = entry.text.replacen(WORKING_PLACEHOLDER, "(interrupted)", 1);
                } else if entry.text.trim().is_empty() {
                    entry.text = "(interrupted)".to_string();
                }
            }
        }
        if let Some(i) = self.assistant_idx {
            if let Some(entry) = self.entries.get_mut(i) {
                if entry.text.contains(WORKING_PLACEHOLDER) {
                    entry.text = entry.text.replacen(WORKING_PLACEHOLDER, "(interrupted)", 1);
                } else if entry.text.trim().is_empty() {
                    entry.text = "(interrupted)".to_string();
                }
            }
        }

        self.clear_running_state();
        self.last_tool_event = "task interrupted".to_string();
        self.last_status = "interrupted".to_string();
        self.push_entry(EntryKind::System, reason.to_string());
    }

    pub(super) fn poll_worker(&mut self) -> bool {
        if let Some(rx) = self.rx.clone() {
            let mut processed_any = false;
            let mut render_changed = false;
            loop {
                match rx.try_recv() {
                    Ok(WorkerEvent::AgentStart(provider)) => {
                        processed_any = true;
                        self.active_provider = Some(provider);
                        self.agent_chars.insert(provider, 0);
                        let seed = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos() as usize)
                            .unwrap_or(self.spinner_idx);
                        self.agent_verb_idx.insert(provider, seed % 12);
                        self.agent_started_at.insert(provider, Instant::now());
                        let event_msg = format!("agent {} started", provider.as_str());
                        self.agent_tool_event.insert(provider, event_msg.clone());
                        self.last_tool_event = event_msg;
                        self.last_status = format!("{} working", provider.as_str());
                        if self.should_emit_collaboration_trace() {
                            self.append_agent_progress_line(provider, "started");
                            render_changed = true;
                        }
                    }
                    Ok(WorkerEvent::AgentChunk { provider, chunk }) => {
                        processed_any = true;
                        let chunk = sanitize_runtime_text(&chunk);
                        if chunk.trim().is_empty() {
                            continue;
                        }
                        self.stream_had_chunk = true;
                        *self.agent_chars.entry(provider).or_insert(0) += chunk.len();
                        if let Some(i) = self.agent_entries.get(&provider).copied() {
                            if let Some(entry) = self.entries.get_mut(i) {
                                let had_chunk = self
                                    .agent_had_chunk
                                    .get(&provider)
                                    .copied()
                                    .unwrap_or(false);
                                if !had_chunk && entry.text.contains(WORKING_PLACEHOLDER) {
                                    entry.text = entry.text.replacen(WORKING_PLACEHOLDER, "", 1);
                                }
                                if provider == Provider::Codex
                                    && had_chunk
                                    && !entry.text.ends_with('\n')
                                    && !chunk.starts_with('\n')
                                {
                                    entry.text.push('\n');
                                }
                                entry.text.push_str(&chunk);
                                self.agent_had_chunk.insert(provider, true);
                                render_changed = true;
                            }
                        }
                        self.last_status = format!("{} streaming", provider.as_str());
                    }
                    Ok(WorkerEvent::AgentDone(provider)) => {
                        processed_any = true;
                        render_changed = true;
                        let elapsed_secs = self.running_elapsed_secs();
                        if let Some(i) = self.agent_entries.get(&provider).copied() {
                            let had_chunk = self
                                .agent_had_chunk
                                .get(&provider)
                                .copied()
                                .unwrap_or(false);
                            if let Some(entry) = self.entries.get_mut(i) {
                                entry.elapsed_secs = Some(elapsed_secs);
                                if !had_chunk {
                                    if entry.text.contains(WORKING_PLACEHOLDER) {
                                        entry.text = entry.text.replacen(
                                            WORKING_PLACEHOLDER,
                                            "(no output)",
                                            1,
                                        );
                                    } else if entry.text.trim().is_empty() {
                                        entry.text = "(no output)".to_string();
                                    }
                                }
                            }
                        }
                        if let Some(i) = self.agent_entries.get(&provider).copied() {
                            if let Some(entry) = self.entries.get(i) {
                                let text =
                                    cleaned_assistant_text_for_model(entry).trim().to_string();
                                if !text.is_empty()
                                    && text != "(no output)"
                                    && text != "(failed)"
                                    && text != "(interrupted)"
                                    && text != "(cancelled)"
                                    && text != "(disconnected)"
                                {
                                    if let Some(memory) = &self.memory {
                                        if let Err(err) = memory.append_message(
                                            &self.session_id,
                                            "assistant",
                                            Some(provider.as_str()),
                                            &text,
                                        ) {
                                            self.push_entry(
                                                EntryKind::System,
                                                format!(
                                                    "memory write failed: {}",
                                                    truncate(&err.to_string(), 80)
                                                ),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        if self.active_provider == Some(provider) {
                            self.active_provider = None;
                        }
                        let event_msg = format!(
                            "agent {} completed ({:02}:{:02})",
                            provider.as_str(),
                            elapsed_secs / 60,
                            elapsed_secs % 60
                        );
                        self.agent_tool_event.insert(provider, event_msg.clone());
                        self.last_tool_event = event_msg;
                        self.last_status = format!("{} done", provider.as_str());
                        if self.should_emit_collaboration_trace() {
                            let progress = format!(
                                "completed ({:02}:{:02})",
                                elapsed_secs / 60,
                                elapsed_secs % 60
                            );
                            self.append_agent_progress_line(provider, &progress);
                        }
                    }
                    Ok(WorkerEvent::Done(final_text)) => {
                        processed_any = true;
                        render_changed = true;
                        let elapsed_secs = self.running_elapsed_secs();
                        self.finished_elapsed_secs = elapsed_secs;
                        self.finished_provider_name = if self.run_target.trim().is_empty() {
                            self.primary_provider.as_str().to_string()
                        } else {
                            self.run_target.clone()
                        };
                        if self.assistant_idx.is_some() {
                            if !self.stream_had_chunk {
                                let final_text = final_text.trim();
                                if let Some(i) = self.assistant_idx {
                                    if let Some(entry) = self.entries.get_mut(i) {
                                        if entry.elapsed_secs.is_none() {
                                            entry.elapsed_secs = Some(elapsed_secs);
                                        }
                                        if final_text.is_empty() {
                                            if entry.text.trim().is_empty()
                                                || entry.text.trim() == WORKING_PLACEHOLDER
                                            {
                                                entry.text = "(no output)".to_string();
                                            }
                                        } else if entry.text.trim() == WORKING_PLACEHOLDER {
                                            entry.text = final_text.to_string();
                                        } else {
                                            entry.text.push_str(final_text);
                                        }
                                    }
                                }
                            } else if let Some(i) = self.assistant_idx {
                                if let Some(entry) = self.entries.get_mut(i) {
                                    if entry.elapsed_secs.is_none() {
                                        entry.elapsed_secs = Some(elapsed_secs);
                                    }
                                    if entry.text.trim().is_empty() {
                                        entry.text = "(no output)".to_string();
                                    }
                                }
                            }
                        } else if !final_text.trim().is_empty() {
                            if let Some(primary_idx) =
                                self.agent_entries.get(&self.primary_provider).copied()
                            {
                                if let Some(entry) = self.entries.get_mut(primary_idx) {
                                    if entry.elapsed_secs.is_none() {
                                        entry.elapsed_secs = Some(elapsed_secs);
                                    }
                                    if entry.text.contains(WORKING_PLACEHOLDER) {
                                        entry.text =
                                            entry.text.replacen(WORKING_PLACEHOLDER, "", 1);
                                    }
                                    entry.text.push_str(final_text.trim());
                                }
                            }
                        }
                        self.clear_running_state();
                        self.finished_at = Some(Instant::now());
                        if self.last_tool_event.is_empty() {
                            self.last_tool_event = "run completed".to_string();
                        }
                        self.last_status = "done".to_string();
                        break;
                    }
                    Ok(WorkerEvent::Tool { provider, msg }) => {
                        processed_any = true;
                        let msg = sanitize_runtime_text(&msg);
                        if msg.trim().is_empty() {
                            continue;
                        }
                        if let Some(ap) = provider.or(self.active_provider) {
                            self.agent_tool_event.insert(ap, msg.clone());
                        }
                        self.last_tool_event = msg.clone();
                        self.last_status = format!("tool: {}", truncate(&msg, 48));
                        // Tool events only go to the live activity area (not transcript).
                        self.activity_log.push_back(msg.clone());
                        while self.activity_log.len() > MAX_ACTIVITY_LOG_LINES {
                            self.activity_log.pop_front();
                        }
                        if provider.is_none() && self.should_emit_collaboration_trace() {
                            self.push_coordination_trace(&msg);
                        }
                        render_changed = true;
                    }
                    Ok(WorkerEvent::Progress { provider, msg }) => {
                        processed_any = true;
                        let msg = sanitize_runtime_text(&msg);
                        if msg.trim().is_empty() {
                            continue;
                        }
                        let progress_unchanged = self
                            .agent_tool_event
                            .get(&provider)
                            .is_some_and(|prev| prev == &msg);
                        self.agent_tool_event.insert(provider, msg.clone());
                        self.last_tool_event = msg.clone();
                        // Add progress to activity log.
                        self.activity_log.push_back(msg.clone());
                        while self.activity_log.len() > MAX_ACTIVITY_LOG_LINES {
                            self.activity_log.pop_front();
                        }
                        self.last_status =
                            format!("progress {}: {}", provider.as_str(), truncate(&msg, 42));
                        if self.should_emit_collaboration_trace() && !progress_unchanged {
                            self.append_agent_progress_line(provider, &msg);
                            render_changed = true;
                        }
                    }
                    Ok(WorkerEvent::PromotePrimary { to, reason }) => {
                        processed_any = true;
                        if self.primary_provider != to {
                            self.primary_provider = to;
                            self.push_entry(
                                EntryKind::System,
                                format!("primary auto-switched to {} ({})", to.as_str(), reason),
                            );
                            self.last_status = format!("primary -> {}", to.as_str());
                        }
                    }
                    Ok(WorkerEvent::Error(err)) => {
                        processed_any = true;
                        render_changed = true;
                        if let Some(provider) = self.active_provider {
                            if let Some(i) = self.agent_entries.get(&provider).copied() {
                                if let Some(entry) = self.entries.get_mut(i) {
                                    if entry.text.contains(WORKING_PLACEHOLDER) {
                                        entry.text =
                                            entry.text.replacen(WORKING_PLACEHOLDER, "(failed)", 1);
                                    }
                                }
                            }
                        } else if let Some(i) = self.assistant_idx {
                            if let Some(entry) = self.entries.get_mut(i) {
                                if entry.text.trim().is_empty()
                                    || entry.text.trim() == WORKING_PLACEHOLDER
                                {
                                    entry.text = "(failed)".to_string();
                                }
                            }
                        }
                        self.push_entry(EntryKind::Error, err);
                        self.clear_running_state();
                        self.last_tool_event = "run failed".to_string();
                        self.last_status = "error".to_string();
                        break;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        processed_any = true;
                        render_changed = true;
                        if let Some(provider) = self.active_provider {
                            if let Some(i) = self.agent_entries.get(&provider).copied() {
                                if let Some(entry) = self.entries.get_mut(i) {
                                    if entry.text.contains(WORKING_PLACEHOLDER) {
                                        entry.text = entry.text.replacen(
                                            WORKING_PLACEHOLDER,
                                            "(disconnected)",
                                            1,
                                        );
                                    }
                                }
                            }
                        } else if let Some(i) = self.assistant_idx {
                            if let Some(entry) = self.entries.get_mut(i) {
                                if entry.text.trim().is_empty()
                                    || entry.text.trim() == WORKING_PLACEHOLDER
                                {
                                    entry.text = "(disconnected)".to_string();
                                }
                            }
                        }
                        self.clear_running_state();
                        self.last_tool_event = "worker disconnected".to_string();
                        break;
                    }
                }
            }
            if render_changed {
                self.follow_scroll();
            }
            processed_any
        } else {
            false
        }
    }
}
