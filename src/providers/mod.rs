use std::sync::{Arc, Mutex};

use crossbeam_channel::Sender;

use crate::app::{Provider, WorkerEvent};

pub(crate) mod claude;
pub(crate) mod codex;

pub(crate) fn run_provider_stream(
    provider: Provider,
    prompt: &str,
    tx: &Sender<WorkerEvent>,
    child_pids: &Arc<Mutex<Vec<u32>>>,
) -> std::result::Result<String, String> {
    match provider {
        Provider::Claude => claude::run_stream(provider, prompt, tx, child_pids),
        Provider::Codex => codex::run_stream(provider, prompt, tx, child_pids),
    }
}

pub(crate) fn pick_promoted_provider(
    current: Provider,
    available_providers: &[Provider],
    err: &str,
) -> Option<Provider> {
    if current == Provider::Claude
        && claude::is_quota_error_text(err)
        && available_providers.contains(&Provider::Codex)
    {
        return Some(Provider::Codex);
    }
    None
}
