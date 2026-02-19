use super::*;

pub(super) fn parse_dispatch_override(
    line: &str,
) -> std::result::Result<Option<(DispatchTarget, String)>, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() {
        return Ok(None);
    }

    let mentions: Vec<&str> = tokens
        .iter()
        .copied()
        .filter(|token| token.starts_with("@"))
        .collect();
    if mentions.is_empty() {
        return Ok(None);
    }

    let prompt = tokens
        .iter()
        .copied()
        .filter(|token| !token.starts_with("@"))
        .collect::<Vec<_>>()
        .join(" ");
    if prompt.trim().is_empty() {
        return Err(format!("usage: {} <task>", mentions.join(" ")));
    }

    let mut providers = Vec::new();
    let mut seen = HashSet::new();
    for mention in mentions {
        let name = mention.trim_start_matches("@");
        let Some(provider) = provider_from_name(name) else {
            return Err(format!(
                "unknown dispatch target {}; use @claude or @codex",
                mention
            ));
        };
        if seen.insert(provider) {
            providers.push(provider);
        }
    }

    let target = if providers.len() == 1 {
        DispatchTarget::Provider(providers[0])
    } else {
        DispatchTarget::Providers(providers)
    };

    if matches!(target, DispatchTarget::Providers(ref ps) if ps.is_empty()) {
        return Err("usage: @claude <task> | @codex <task>".to_string());
    }

    Ok(Some((target, prompt)))
}
