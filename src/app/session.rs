use super::*;
use std::fs;
use std::path::PathBuf;

impl App {
    pub(super) fn build_contextual_prompt(&self, prompt: &str) -> String {
        if let Some(memory) = &self.memory {
            if let Ok(text) = memory.build_context(&self.session_id, prompt) {
                return text;
            }
        }
        self.build_contextual_prompt_from_entries(prompt)
    }

    pub(super) fn build_contextual_prompt_from_entries(&self, prompt: &str) -> String {
        const CONTEXT_ENTRY_LIMIT: usize = 18;
        const CONTEXT_CHAR_LIMIT: usize = 6000;

        let mut lines = Vec::<String>::new();
        let mut skipped_current_prompt = false;
        for entry in self.entries.iter().rev() {
            if lines.len() >= CONTEXT_ENTRY_LIMIT {
                break;
            }
            match entry.kind {
                EntryKind::User => {
                    let text = entry.text.trim();
                    if !skipped_current_prompt && text == prompt.trim() {
                        skipped_current_prompt = true;
                        continue;
                    }
                    if !text.is_empty() {
                        lines.push(format!("user: {}", text));
                    }
                }
                EntryKind::Assistant => {
                    let text = cleaned_assistant_text_for_model(entry);
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    let actor = extract_agent_name(&entry.text)
                        .map(|name| format!("assistant({name})"))
                        .unwrap_or_else(|| "assistant".to_string());
                    lines.push(format!("{actor}: {text}"));
                }
                _ => {}
            }
        }

        if lines.is_empty() {
            return prompt.to_string();
        }

        lines.reverse();
        let mut selected = Vec::<String>::new();
        let mut used = 0usize;
        for line in lines.into_iter().rev() {
            let delta = line.len().saturating_add(1);
            if used + delta > CONTEXT_CHAR_LIMIT && !selected.is_empty() {
                break;
            }
            used += delta;
            selected.push(line);
        }
        selected.reverse();

        format!(
            "Conversation context from this DAgent session:\n{}\n\nCurrent user request:\n{}",
            selected.join("\n"),
            prompt
        )
    }

    pub(super) fn session_file_path() -> PathBuf {
        if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(".dagent").join("session.json")
        } else {
            PathBuf::from(".dagent").join("session.json")
        }
    }

    pub(super) fn restore_session(&mut self) {
        let path = Self::session_file_path();
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&raw) else {
            return;
        };

        if self
            .available_providers
            .contains(&snapshot.primary_provider)
        {
            self.primary_provider = snapshot.primary_provider;
        }
        self.theme = snapshot.theme;
        if restore_transcript_on_start(self.memory.is_some()) {
            self.entries = snapshot.entries;
        } else {
            self.entries = Vec::new();
        }
        self.history = snapshot.history;
        self.session_id = if snapshot.session_id.trim().is_empty() {
            default_session_id()
        } else {
            snapshot.session_id
        };
        self.history_pos = None;
        self.autoscroll = true;
        self.scroll = self.scroll_max();
        if !self.entries.is_empty() {
            self.last_status = "resumed previous session".to_string();
        }
    }

    pub(super) fn persist_session(&self) {
        const MAX_PERSISTED_ENTRIES: usize = 600;
        const MAX_PERSISTED_HISTORY: usize = 400;

        let path = Self::session_file_path();
        if let Some(parent) = path.parent() {
            if fs::create_dir_all(parent).is_err() {
                return;
            }
        }

        let entries = if self.entries.len() > MAX_PERSISTED_ENTRIES {
            self.entries[self.entries.len().saturating_sub(MAX_PERSISTED_ENTRIES)..].to_vec()
        } else {
            self.entries.clone()
        };
        let history = if self.history.len() > MAX_PERSISTED_HISTORY {
            self.history[self.history.len().saturating_sub(MAX_PERSISTED_HISTORY)..].to_vec()
        } else {
            self.history.clone()
        };
        let snapshot = SessionSnapshot {
            primary_provider: self.primary_provider,
            theme: self.theme,
            entries,
            history,
            session_id: self.session_id.clone(),
        };

        let Ok(serialized) = serde_json::to_string_pretty(&snapshot) else {
            return;
        };
        let _ = fs::write(path, serialized);
    }
}
