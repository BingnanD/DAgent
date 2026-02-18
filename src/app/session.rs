use super::*;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

impl App {
    pub(super) fn build_contextual_prompt(&self, prompt: &str) -> String {
        let base = if let Some(memory) = &self.memory {
            if let Ok(text) = memory.build_context(&self.session_id, prompt) {
                text
            } else {
                self.build_contextual_prompt_from_entries(prompt)
            }
        } else {
            self.build_contextual_prompt_from_entries(prompt)
        };
        self.inject_skill_context(prompt, &base)
    }

    fn inject_skill_context(&self, user_prompt: &str, base_prompt: &str) -> String {
        const SKILL_MAX_COUNT: usize = 3;
        const SKILL_MAX_CHARS_PER_ITEM: usize = 1400;
        const SKILL_MAX_TOTAL_CHARS: usize = 3600;

        let Some(store) = &self.skills else {
            return base_prompt.to_string();
        };

        let mut selected = Vec::new();
        let mut seen = HashSet::new();

        if let Ok(explicit) = store.resolve_explicit_refs(user_prompt, SKILL_MAX_COUNT) {
            for skill in explicit {
                if seen.insert(skill.id.clone()) {
                    selected.push(skill);
                }
            }
        }
        if selected.len() < SKILL_MAX_COUNT {
            if let Ok(matched) = store.search_relevant(user_prompt, SKILL_MAX_COUNT) {
                for skill in matched {
                    if selected.len() >= SKILL_MAX_COUNT {
                        break;
                    }
                    if seen.insert(skill.id.clone()) {
                        selected.push(skill);
                    }
                }
            }
        }
        if selected.is_empty() {
            return base_prompt.to_string();
        }

        let mut used = 0usize;
        let mut blocks = Vec::new();
        for skill in selected {
            let remaining = SKILL_MAX_TOTAL_CHARS.saturating_sub(used);
            if remaining < 120 {
                break;
            }
            let hard_limit = SKILL_MAX_CHARS_PER_ITEM.min(remaining);
            let mut content = skill.content.trim().to_string();
            let truncated = content.chars().count() > hard_limit;
            if truncated {
                content = content.chars().take(hard_limit).collect::<String>();
                content.push_str("\n...");
            }
            used += content.chars().count();

            let title = if skill.description.trim().is_empty() {
                format!("[skill:{}] {}", skill.id, skill.name)
            } else {
                format!(
                    "[skill:{}] {} - {}",
                    skill.id, skill.name, skill.description
                )
            };
            blocks.push(format!("{title}\n{content}"));
        }

        if blocks.is_empty() {
            return base_prompt.to_string();
        }

        format!(
            "{base}\n\nRelevant skills loaded by DAgent:\n{skills}\n\nUse these skills when applicable, then solve the user request.",
            base = base_prompt,
            skills = blocks.join("\n\n"),
        )
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
