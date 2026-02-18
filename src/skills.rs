use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Skill {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    pub(crate) content: String,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct SkillSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) updated_at: i64,
}

pub(crate) struct SkillStore {
    root: PathBuf,
}

impl SkillStore {
    pub(crate) fn open_default() -> Result<Self> {
        let root = skill_root_path();
        Self::open_at(root)
    }

    pub(crate) fn open_at(path: impl AsRef<Path>) -> Result<Self> {
        let root = path.as_ref().to_path_buf();
        fs::create_dir_all(&root)
            .with_context(|| format!("create skills dir {}", root.display()))?;
        Ok(Self { root })
    }

    pub(crate) fn count(&self) -> Result<usize> {
        Ok(self.list()?.len())
    }

    pub(crate) fn list(&self) -> Result<Vec<SkillSummary>> {
        let mut out = self
            .load_all_skills()?
            .into_iter()
            .map(|skill| SkillSummary {
                id: skill.id,
                name: skill.name,
                description: skill.description,
                updated_at: skill.updated_at,
            })
            .collect::<Vec<_>>();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    pub(crate) fn get(&self, raw_id: &str) -> Result<Option<Skill>> {
        let Some(id) = normalize_skill_id(raw_id) else {
            return Ok(None);
        };
        let path = self.skill_file_path(&id);
        if !path.exists() {
            return Ok(None);
        }
        let raw =
            fs::read_to_string(&path).with_context(|| format!("read skill {}", path.display()))?;
        let skill = serde_json::from_str::<Skill>(&raw).with_context(|| format!("parse {}", id))?;
        Ok(Some(skill))
    }

    pub(crate) fn create(
        &self,
        raw_id: &str,
        name: &str,
        description: &str,
        content: &str,
    ) -> Result<Skill> {
        let id = normalize_skill_id(raw_id)
            .ok_or_else(|| anyhow!("invalid skill id: {}", raw_id.trim()))?;
        let path = self.skill_file_path(&id);
        if path.exists() {
            bail!("skill already exists: {}", id);
        }
        let now = unix_timestamp();
        let skill = Skill {
            id,
            name: sanitize_line(name),
            description: sanitize_line(description),
            content: sanitize_block(content),
            created_at: now,
            updated_at: now,
        };
        validate_skill(&skill)?;
        self.write_skill(&skill)?;
        Ok(skill)
    }

    pub(crate) fn update(
        &self,
        raw_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        content: Option<&str>,
    ) -> Result<Skill> {
        let id = normalize_skill_id(raw_id)
            .ok_or_else(|| anyhow!("invalid skill id: {}", raw_id.trim()))?;
        let Some(mut skill) = self.get(&id)? else {
            bail!("skill not found: {}", id);
        };
        if let Some(v) = name {
            let cleaned = sanitize_line(v);
            if !cleaned.is_empty() {
                skill.name = cleaned;
            }
        }
        if let Some(v) = description {
            skill.description = sanitize_line(v);
        }
        if let Some(v) = content {
            let cleaned = sanitize_block(v);
            if !cleaned.is_empty() {
                skill.content = cleaned;
            }
        }
        skill.updated_at = unix_timestamp();
        validate_skill(&skill)?;
        self.write_skill(&skill)?;
        Ok(skill)
    }

    pub(crate) fn delete(&self, raw_id: &str) -> Result<bool> {
        let Some(id) = normalize_skill_id(raw_id) else {
            return Ok(false);
        };
        let path = self.skill_file_path(&id);
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(&path).with_context(|| format!("remove skill {}", path.display()))?;
        Ok(true)
    }

    pub(crate) fn search_relevant(&self, query: &str, limit: usize) -> Result<Vec<Skill>> {
        let terms = tokenize_terms(query);
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let mut scored = Vec::<(i64, Skill)>::new();
        for skill in self.load_all_skills()? {
            let hay_meta = format!(
                "{} {} {}",
                skill.id,
                skill.name.to_lowercase(),
                skill.description.to_lowercase()
            );
            let hay_content = skill.content.to_lowercase();
            let mut score = 0i64;
            for term in &terms {
                if hay_meta.contains(term) {
                    score += 3;
                }
                if hay_content.contains(term) {
                    score += 1;
                }
            }
            if score > 0 {
                scored.push((score, skill));
            }
        }

        scored.sort_by(|(score_a, skill_a), (score_b, skill_b)| {
            score_b
                .cmp(score_a)
                .then_with(|| skill_b.updated_at.cmp(&skill_a.updated_at))
                .then_with(|| skill_a.id.cmp(&skill_b.id))
        });

        Ok(scored
            .into_iter()
            .take(limit)
            .map(|(_, skill)| skill)
            .collect())
    }

    pub(crate) fn resolve_explicit_refs(&self, prompt: &str, limit: usize) -> Result<Vec<Skill>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let ids = extract_skill_refs(prompt);
        let mut out = Vec::new();
        for id in ids.into_iter().take(limit) {
            if let Some(skill) = self.get(&id)? {
                out.push(skill);
            }
        }
        Ok(out)
    }

    fn load_all_skills(&self) -> Result<Vec<Skill>> {
        let mut out = Vec::new();
        let entries = fs::read_dir(&self.root)
            .with_context(|| format!("read skills dir {}", self.root.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(skill) = serde_json::from_str::<Skill>(&raw) else {
                continue;
            };
            if normalize_skill_id(&skill.id).is_none() {
                continue;
            }
            out.push(skill);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    fn write_skill(&self, skill: &Skill) -> Result<()> {
        let path = self.skill_file_path(&skill.id);
        let serialized = serde_json::to_string_pretty(skill)
            .with_context(|| format!("serialize skill {}", skill.id))?;
        fs::write(&path, serialized).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    fn skill_file_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }
}

pub(crate) fn normalize_skill_id(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
            continue;
        }
        if matches!(ch, '-' | '_' | '.' | ' ' | '/') {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
            continue;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

pub(crate) fn extract_skill_refs(prompt: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in prompt.split_whitespace() {
        let token = raw.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && !matches!(c, ':' | '-' | '_' | '#' | '@')
        });
        for prefix in ["@skill:", "#skill:", "skill:"] {
            if let Some(rest) = token.strip_prefix(prefix) {
                if let Some(id) = normalize_skill_id(rest) {
                    if seen.insert(id.clone()) {
                        out.push(id);
                    }
                }
            }
        }
    }
    out
}

fn validate_skill(skill: &Skill) -> Result<()> {
    if skill.id.trim().is_empty() {
        bail!("skill id cannot be empty");
    }
    if skill.name.trim().is_empty() {
        bail!("skill name cannot be empty");
    }
    if skill.content.trim().is_empty() {
        bail!("skill content cannot be empty");
    }
    Ok(())
}

fn sanitize_line(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sanitize_block(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut out = Vec::new();
    for line in trimmed.lines() {
        out.push(line.trim_end());
    }
    out.join("\n").trim().to_string()
}

fn tokenize_terms(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for token in input
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(|s| s.trim().to_lowercase())
    {
        if token.len() < 2 {
            continue;
        }
        if seen.insert(token.clone()) {
            out.push(token);
        }
        if out.len() >= 10 {
            break;
        }
    }
    out
}

fn skill_root_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".dagent").join("skills")
    } else {
        PathBuf::from(".dagent").join("skills")
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_skill_id_compacts_symbols() {
        assert_eq!(
            normalize_skill_id("  Code Review / Rust "),
            Some("code-review-rust".to_string())
        );
        assert_eq!(normalize_skill_id("___"), None);
    }

    #[test]
    fn extract_skill_refs_reads_multiple_markers() {
        let refs = extract_skill_refs("try @skill:code-review and [#skill:api-design]");
        assert_eq!(
            refs,
            vec!["code-review".to_string(), "api-design".to_string()]
        );
    }

    #[test]
    fn skill_store_crud_roundtrip() {
        let root = std::env::temp_dir().join(format!(
            "dagent_skill_test_{}_{}",
            std::process::id(),
            unix_timestamp()
        ));
        let store = SkillStore::open_at(&root).expect("open skill store");

        let created = store
            .create(
                "api-review",
                "API Review",
                "Review API specs",
                "Check HTTP status codes.\nValidate error models.",
            )
            .expect("create skill");
        assert_eq!(created.id, "api-review");
        assert_eq!(store.count().expect("count"), 1);

        let fetched = store
            .get("api-review")
            .expect("get skill")
            .expect("skill exists");
        assert_eq!(fetched.name, "API Review");

        let updated = store
            .update(
                "api-review",
                Some("API Contract Review"),
                Some("Review API contracts"),
                Some("Validate OpenAPI schema."),
            )
            .expect("update skill");
        assert_eq!(updated.name, "API Contract Review");
        assert!(updated.updated_at >= updated.created_at);

        let deleted = store.delete("api-review").expect("delete skill");
        assert!(deleted);
        assert_eq!(store.count().expect("count"), 0);

        fs::remove_dir_all(root).ok();
    }
}
