use super::*;
use crossbeam_channel::unbounded;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn app_with_entries(n: usize) -> App {
    let mut app = App::new();
    for i in 0..n {
        app.push_entry(EntryKind::System, format!("entry {i}"));
    }
    app
}

#[test]
fn pageup_disables_autoscroll_and_moves_up() {
    let mut app = app_with_entries(40);
    let before = app.scroll;
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));

    assert!(!app.autoscroll);
    assert_eq!(app.scroll, before.saturating_sub(5));
}

#[test]
fn pagedown_near_bottom_reenables_autoscroll() {
    let mut app = app_with_entries(40);
    let max = app.scroll_max();
    app.autoscroll = false;
    app.scroll = max.saturating_sub(1);

    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));

    assert_eq!(app.scroll, max);
    assert!(app.autoscroll);
}

#[test]
fn new_entries_do_not_force_scroll_when_autoscroll_off() {
    let mut app = app_with_entries(20);
    app.autoscroll = false;
    app.scroll = 3;
    app.push_entry(EntryKind::System, "extra");

    assert_eq!(app.scroll, 3);
}

#[test]
fn new_entries_follow_bottom_when_autoscroll_on() {
    let mut app = app_with_entries(20);
    app.autoscroll = true;
    app.push_entry(EntryKind::System, "extra");

    let max = app.scroll_max();
    assert_eq!(app.scroll, max);
}

#[test]
fn tool_events_follow_bottom_when_autoscroll_on() {
    let mut app = app_with_entries(30);
    app.autoscroll = true;
    app.scroll = app.scroll_max();
    let before_max = app.scroll_max();

    let (tx, rx) = unbounded::<WorkerEvent>();
    app.push_test_run(vec![], rx);
    tx.send(WorkerEvent::Tool {
        provider: None,
        msg: "invoke bash ls -la".to_string(),
    })
    .expect("send tool event");

    assert!(app.poll_worker());
    let after_max = app.scroll_max();
    assert!(after_max >= before_max);
    assert_eq!(app.scroll, after_max);
}

#[test]
fn tool_events_do_not_enter_transcript_entries() {
    let mut app = App::new();
    app.push_entry(EntryKind::Assistant, "[claude]\nanswer");
    let before_len = app.entries.len();

    let (tx, rx) = unbounded::<WorkerEvent>();
    app.push_test_run(vec![], rx);
    tx.send(WorkerEvent::Tool {
        provider: Some(Provider::Claude),
        msg: "calling tool: Bash".to_string(),
    })
    .expect("send tool event");

    assert!(app.poll_worker());
    assert_eq!(app.entries.len(), before_len);
    assert!(!app.entries[0].text.contains("calling tool: Bash"));
}

#[test]
fn coordination_tool_events_enter_transcript_entries_in_multi_agent_run() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(
        EntryKind::Assistant,
        format!("[claude]\n{}", WORKING_PLACEHOLDER),
    );
    app.push_entry(
        EntryKind::Assistant,
        format!("[codex]\n{}", WORKING_PLACEHOLDER),
    );
    app.agent_entries.insert(Provider::Claude, 0);
    app.agent_entries.insert(Provider::Codex, 1);
    let before_len = app.entries.len();

    let (tx, rx) = unbounded::<WorkerEvent>();
    app.push_test_run(vec![Provider::Claude, Provider::Codex], rx);
    tx.send(WorkerEvent::Tool {
        provider: None,
        msg: "task decomposed, dispatching to agents...".to_string(),
    })
    .expect("send coordination tool event");

    assert!(app.poll_worker());
    assert_eq!(app.entries.len(), before_len + 1);
    let last = app.entries.last().expect("last entry");
    assert!(matches!(last.kind, EntryKind::System));
    assert!(last
        .text
        .contains("[coord] task decomposed, dispatching to agents..."));
}

#[test]
fn progress_events_enter_transcript_entries_for_multi_agent_run() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(
        EntryKind::Assistant,
        format!("[claude]\n{}", WORKING_PLACEHOLDER),
    );
    app.push_entry(
        EntryKind::Assistant,
        format!("[codex]\n{}", WORKING_PLACEHOLDER),
    );
    app.agent_entries.insert(Provider::Claude, 0);
    app.agent_entries.insert(Provider::Codex, 1);

    let (tx, rx) = unbounded::<WorkerEvent>();
    app.push_test_run(vec![Provider::Claude, Provider::Codex], rx);
    tx.send(WorkerEvent::Progress {
        provider: Provider::Claude,
        msg: "thinking...".to_string(),
    })
    .expect("send claude progress");
    tx.send(WorkerEvent::Progress {
        provider: Provider::Codex,
        msg: "drafting response".to_string(),
    })
    .expect("send codex progress");

    assert!(app.poll_worker());
    assert!(app.entries[0].text.contains("[progress] thinking..."));
    assert!(app.entries[1].text.contains("[progress] drafting response"));
    assert!(app.activity_log.iter().any(|item| item == "thinking..."));
    assert!(app
        .activity_log
        .iter()
        .any(|item| item == "drafting response"));
}

#[test]
fn duplicate_progress_events_are_deduped_in_multi_agent_transcript() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(
        EntryKind::Assistant,
        format!("[claude]\n{}", WORKING_PLACEHOLDER),
    );
    app.push_entry(
        EntryKind::Assistant,
        format!("[codex]\n{}", WORKING_PLACEHOLDER),
    );
    app.agent_entries.insert(Provider::Claude, 0);
    app.agent_entries.insert(Provider::Codex, 1);

    let (tx, rx) = unbounded::<WorkerEvent>();
    app.push_test_run(vec![Provider::Claude, Provider::Codex], rx);
    tx.send(WorkerEvent::Progress {
        provider: Provider::Claude,
        msg: "thinking...".to_string(),
    })
    .expect("send first progress");
    tx.send(WorkerEvent::Progress {
        provider: Provider::Claude,
        msg: "thinking...".to_string(),
    })
    .expect("send duplicate progress");

    assert!(app.poll_worker());
    assert_eq!(
        app.entries[0]
            .text
            .matches("[progress] thinking...")
            .count(),
        1
    );
}

#[test]
fn agent_chunks_append_into_owned_agent_panel() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(
        EntryKind::Assistant,
        format!("[claude]\n{}", WORKING_PLACEHOLDER),
    );
    app.push_entry(
        EntryKind::Assistant,
        format!("[codex]\n{}", WORKING_PLACEHOLDER),
    );
    app.agent_entries.insert(Provider::Claude, 0);
    app.agent_entries.insert(Provider::Codex, 1);
    app.agent_had_chunk.insert(Provider::Claude, false);
    app.agent_had_chunk.insert(Provider::Codex, false);

    let (tx, rx) = unbounded::<WorkerEvent>();
    app.push_test_run(vec![Provider::Claude, Provider::Codex], rx);
    tx.send(WorkerEvent::AgentChunk {
        provider: Provider::Claude,
        chunk: "claude process text".to_string(),
    })
    .expect("send claude chunk");
    tx.send(WorkerEvent::AgentChunk {
        provider: Provider::Codex,
        chunk: "codex process text".to_string(),
    })
    .expect("send codex chunk");

    assert!(app.poll_worker());
    assert!(app.entries[0].text.contains("claude process text"));
    assert!(!app.entries[0].text.contains("codex process text"));
    assert!(app.entries[1].text.contains("codex process text"));
    assert!(!app.entries[1].text.contains("claude process text"));
}

#[test]
fn agent_chunk_marks_stream_had_chunk() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(
        EntryKind::Assistant,
        format!("[claude]\n{}", WORKING_PLACEHOLDER),
    );
    app.agent_entries.insert(Provider::Claude, 0);
    app.agent_had_chunk.insert(Provider::Claude, false);
    app.stream_had_chunk = false;

    let (tx, rx) = unbounded::<WorkerEvent>();
    app.push_test_run(vec![Provider::Claude], rx);
    tx.send(WorkerEvent::AgentChunk {
        provider: Provider::Claude,
        chunk: "hello".to_string(),
    })
    .expect("send chunk");

    assert!(app.poll_worker());
    assert!(app.stream_had_chunk);
}

#[test]
fn running_flush_skips_active_streaming_entry() {
    // Active streaming entries (in agent_entries) are always excluded from
    // scrollback flushes while running, regardless of whether chunks have
    // arrived. This prevents append-only scrollback from accumulating
    // duplicate partial-content lines on each frame.
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::User, "test");
    app.push_entry(
        EntryKind::Assistant,
        format!("[claude]\n{}", WORKING_PLACEHOLDER),
    );
    app.agent_entries.insert(Provider::Claude, 1);
    app.agent_had_chunk.insert(Provider::Claude, false);
    app.running_providers.insert(Provider::Claude);

    // Before any chunk: placeholder entry is excluded from flush.
    let before = app
        .running_flush_log_lines(80)
        .into_iter()
        .map(|line| flatten_line_to_plain(&line))
        .collect::<Vec<_>>();
    assert!(!before.iter().any(|line| line.contains(WORKING_PLACEHOLDER)));

    // After first chunk arrives: entry still excluded while actively streaming.
    // Content is visible in the TUI viewport; it will be flushed to scrollback
    // once the run completes and clear_running_state() removes it from agent_entries.
    app.agent_had_chunk.insert(Provider::Claude, true);
    app.entries[1].text = "[claude]\nfirst output line".to_string();

    let after = app
        .running_flush_log_lines(80)
        .into_iter()
        .map(|line| flatten_line_to_plain(&line))
        .collect::<Vec<_>>();
    assert!(!after.iter().any(|line| line.contains("first output line")));

    // Once the run ends (agent_entries cleared), the entry is included.
    app.running_providers.clear();
    app.agent_entries.clear();
    let done = app
        .running_flush_log_lines(80)
        .into_iter()
        .map(|line| flatten_line_to_plain(&line))
        .collect::<Vec<_>>();
    assert!(done.iter().any(|line| line.contains("first output line")));
}

#[test]
fn agent_done_sets_elapsed_only_for_active_entry() {
    let mut app = App::new();
    app.push_entry(EntryKind::Assistant, "[claude]\nold answer");
    app.entries[0].elapsed_secs = Some(7);
    app.push_entry(EntryKind::Assistant, "[claude]\ncurrent answer");
    app.agent_entries.insert(Provider::Claude, 1);
    app.run_started_at = Some(Instant::now());

    let (tx, rx) = unbounded::<WorkerEvent>();
    app.push_test_run(vec![Provider::Claude], rx);
    tx.send(WorkerEvent::AgentDone(Provider::Claude))
        .expect("send agent done event");

    assert!(app.poll_worker());
    assert_eq!(app.entries[0].elapsed_secs, Some(7));
    assert!(app.entries[1].elapsed_secs.is_some());
}

#[test]
fn done_uses_run_target_for_finished_provider_label() {
    let mut app = App::new();
    app.primary_provider = Provider::Codex;
    app.run_started_at = Some(Instant::now());
    app.run_target = "claude".to_string();

    let (tx, rx) = unbounded::<WorkerEvent>();
    app.push_test_run(vec![], rx);
    tx.send(WorkerEvent::Done(String::new()))
        .expect("send done event");

    assert!(app.poll_worker());
    assert_eq!(app.finished_provider_name, "claude");
}

#[test]
fn assistant_header_text_stable_across_running_done_transition() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::Assistant, "[codex]\nanswer");
    app.entries[0].elapsed_secs = Some(12);
    app.agent_entries.insert(Provider::Codex, 0);
    app.running_providers.insert(Provider::Codex);

    let running_header = flatten_line_to_plain(&app.render_entries_lines(80)[0]);

    app.running_providers.clear();
    app.agent_entries.clear();
    let done_header = flatten_line_to_plain(&app.render_entries_lines(80)[0]);

    assert_eq!(running_header, done_header);
    assert_eq!(done_header, "codex  │ answer");
}

#[test]
fn assistant_entry_does_not_render_leading_blank_line_after_marker() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::Assistant, "[codex]\n\nanswer");

    let rendered = flatten_lines_to_plain(&app.render_entries_lines(80));

    assert_eq!(rendered[0], "codex  │ answer");
}

#[test]
fn user_and_assistant_spacing_is_consistent_single_blank_line() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::User, "hello");
    app.push_entry(EntryKind::Assistant, "[codex]\nanswer");

    let rendered = flatten_lines_to_plain(&app.render_entries_lines(80));

    assert_eq!(rendered.len(), 4);
    assert_eq!(rendered[1], "");
    assert_eq!(rendered[2], "codex  │ answer");
}

#[test]
fn consecutive_same_agent_entries_keep_separator_connected() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::Assistant, "[codex]\nfirst");
    app.push_entry(EntryKind::Assistant, "[codex]\nsecond");

    let rendered = flatten_lines_to_plain(&app.render_entries_lines(80));

    assert_eq!(rendered[0], "codex  │ first");
    assert_eq!(rendered[1], "       │");
    assert_eq!(rendered[2], "codex  │ second");
}

#[test]
fn assistant_label_color_stays_provider_colored_while_running() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::Assistant, "[claude]\nstreaming");
    app.agent_entries.insert(Provider::Claude, 0);
    app.running_providers.insert(Provider::Claude);

    let lines = app.render_entries_lines(80);
    let label_fg = lines
        .first()
        .and_then(|line| line.spans.first())
        .and_then(|span| span.style.fg);

    assert_eq!(label_fg, Some(app.theme.palette().claude_label));
}

#[test]
fn wrapped_stream_growth_keeps_tail_when_autoscroll_on() {
    let mut app = App::new();
    app.update_viewport(24, 12);
    app.autoscroll = true;
    app.push_entry(EntryKind::Assistant, "[claude]\nshort line");

    let before = app.scroll_max();
    let idx = app.entries.len().saturating_sub(1);
    if let Some(entry) = app.entries.get_mut(idx) {
        entry
            .text
            .push_str(" this is a long streaming sentence that should wrap across rows");
    }
    app.follow_scroll();

    let after = app.scroll_max();
    assert!(after >= before);
    assert_eq!(app.scroll, after);
}

#[test]
fn startup_banner_renders_as_card() {
    let mut app = App::new();
    app.entries.clear();
    app.maybe_show_startup_banner();

    let rendered = app
        .render_entries_lines(80)
        .into_iter()
        .map(|line| flatten_line_to_plain(&line))
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.starts_with('┌')));
    assert!(rendered.iter().any(|line| line.starts_with("│ ")));
    assert!(rendered.iter().any(|line| line.starts_with('└')));
    assert!(!rendered.iter().any(|line| line.contains("[sys]")));
}

#[test]
fn system_entries_render_multiline_text_line_by_line() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::System, "top line\nmiddle line\nbottom line");

    let rendered = app
        .render_entries_lines(80)
        .into_iter()
        .map(|line| flatten_line_to_plain(&line))
        .collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line == "[sys] top line"));
    assert!(rendered.iter().any(|line| line == "      middle line"));
    assert!(rendered.iter().any(|line| line == "      bottom line"));
}

#[test]
fn assistant_marker_is_not_followed_by_leading_blank_content_line() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::Assistant, "[codex]\n\n   \nFirst line");

    let rendered = app
        .render_entries_lines(80)
        .into_iter()
        .map(|line| flatten_line_to_plain(&line))
        .collect::<Vec<_>>();

    let first_non_empty = rendered
        .iter()
        .find(|line| !line.trim().is_empty())
        .expect("expected at least one rendered line");
    assert_eq!(first_non_empty, "codex  │ First line");
}

#[test]
fn parse_dispatch_override_provider() {
    let parsed = parse_dispatch_override("@claude fix this")
        .expect("parse should succeed")
        .expect("dispatch override should exist");

    assert_eq!(parsed.0, DispatchTarget::Provider(Provider::Claude));
    assert_eq!(parsed.1, "fix this");
}

#[test]
fn parse_dispatch_override_unknown_target_errors() {
    let err = parse_dispatch_override("@foo do work").expect_err("should error");
    assert!(err.contains("unknown dispatch target"));
}

#[test]
fn parse_dispatch_override_multiple_agents() {
    let parsed = parse_dispatch_override("@claude @codex investigate")
        .expect("parse should succeed")
        .expect("dispatch override should exist");
    assert_eq!(
        parsed.0,
        DispatchTarget::Providers(vec![Provider::Claude, Provider::Codex])
    );
    assert_eq!(parsed.1, "investigate");
}

#[test]
fn parse_dispatch_override_all_is_unknown_target() {
    let err = parse_dispatch_override("@claude @all investigate").expect_err("should error");
    assert!(err.contains("unknown dispatch target"));
}

#[test]
fn parse_dispatch_override_mid_sentence_single_agent() {
    let parsed = parse_dispatch_override("please @codex investigate this bug")
        .expect("parse should succeed")
        .expect("dispatch override should exist");
    assert_eq!(parsed.0, DispatchTarget::Provider(Provider::Codex));
    assert_eq!(parsed.1, "please investigate this bug");
}

#[test]
fn parse_dispatch_override_without_mentions_returns_none() {
    let parsed =
        parse_dispatch_override("please investigate this bug").expect("parse should succeed");
    assert!(parsed.is_none());
}

#[test]
fn compute_append_ranges_ignores_style_only_refresh() {
    let old = vec![
        "user".to_string(),
        "assistant".to_string(),
        "tool a".to_string(),
        "tool b".to_string(),
    ];
    let new = vec![
        "user".to_string(),
        "assistant".to_string(),
        "tool a".to_string(),
        "tool b".to_string(),
    ];
    assert_eq!(
        compute_append_ranges(&old, &new),
        Vec::<(usize, usize)>::new()
    );
}

#[test]
fn compute_append_ranges_ignores_replaced_rows_even_with_new_tail() {
    let old = vec![
        "user".to_string(),
        format!("assistant {}", WORKING_PLACEHOLDER),
        "tool start".to_string(),
        "tool progress".to_string(),
    ];
    let new = vec![
        "user".to_string(),
        "assistant final answer".to_string(),
        "tool start".to_string(),
        "tool progress".to_string(),
        "tool done".to_string(),
    ];
    assert_eq!(
        compute_append_ranges(&old, &new),
        Vec::<(usize, usize)>::new()
    );
}

#[test]
fn compute_append_ranges_ignores_middle_insertions() {
    let old = vec!["user".to_string(), "codex line".to_string()];
    let new = vec![
        "user".to_string(),
        "claude line".to_string(),
        "codex line".to_string(),
    ];
    assert_eq!(
        compute_append_ranges(&old, &new),
        Vec::<(usize, usize)>::new()
    );
}

#[test]
fn compute_append_ranges_ignores_middle_reflow_that_would_split_agent_block() {
    let old = vec![
        "claude | previous answer".to_string(),
        "".to_string(),
        " user question ".to_string(),
        "".to_string(),
        "codex  | next answer".to_string(),
        "".to_string(),
    ];
    let new = vec![
        "claude | previous answer".to_string(),
        "       | continued line".to_string(),
        "".to_string(),
        " user question ".to_string(),
        "".to_string(),
        "codex  | next answer".to_string(),
        "".to_string(),
    ];
    assert_eq!(
        compute_append_ranges(&old, &new),
        Vec::<(usize, usize)>::new()
    );
}

#[test]
fn compute_append_ranges_appends_tail_only() {
    // When old is a strict prefix of new, append the tail.
    let old = vec!["user".to_string(), "assistant".to_string()];
    let new = vec![
        "user".to_string(),
        "assistant".to_string(),
        "tool start".to_string(),
        "tool done".to_string(),
    ];
    assert_eq!(compute_append_ranges(&old, &new), vec![(2, 4)]);
}

#[test]
fn compute_append_ranges_ignores_single_line_in_place_growth() {
    let old = vec!["codex  | hello".to_string()];
    let new = vec!["codex  | hello world".to_string()];
    assert_eq!(
        compute_append_ranges(&old, &new),
        Vec::<(usize, usize)>::new()
    );
}

#[test]
fn compute_running_append_ranges_appends_from_first_difference() {
    let old = vec![
        " user question ".to_string(),
        "".to_string(),
        "codex  | hello".to_string(),
    ];
    let new = vec![
        " user question ".to_string(),
        "".to_string(),
        "codex  | hello world".to_string(),
        "       | next line".to_string(),
    ];
    assert_eq!(compute_running_append_ranges(&old, &new), vec![(2, 4)]);
}

#[test]
fn compute_running_append_ranges_keeps_tail_append_behavior() {
    let old = vec!["user".to_string(), "assistant".to_string()];
    let new = vec![
        "user".to_string(),
        "assistant".to_string(),
        "tool start".to_string(),
    ];
    assert_eq!(compute_running_append_ranges(&old, &new), vec![(2, 3)]);
}

#[test]
fn compute_flush_append_ranges_uses_running_diff_on_running_to_done_transition() {
    let old = vec![
        " user question ".to_string(),
        format!("codex  | {}", WORKING_PLACEHOLDER),
    ];
    let new = vec![
        " user question ".to_string(),
        "codex  | final answer".to_string(),
    ];
    assert_eq!(
        compute_flush_append_ranges(&old, &new, false, true),
        vec![(1, 2)]
    );
}

#[test]
fn compute_flush_append_ranges_keeps_stable_done_behavior_after_transition() {
    let old = vec![
        " user question ".to_string(),
        format!("codex  | {}", WORKING_PLACEHOLDER),
    ];
    let new = vec![
        " user question ".to_string(),
        "codex  | final answer".to_string(),
    ];
    assert_eq!(
        compute_flush_append_ranges(&old, &new, false, false),
        Vec::<(usize, usize)>::new()
    );
}

#[test]
fn running_streaming_assistant_omits_trailing_blank_separator() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::Assistant, "[codex]\nstreaming");
    app.agent_entries.insert(Provider::Codex, 0);
    app.running_providers.insert(Provider::Codex);

    let rendered = flatten_lines_to_plain(&app.render_entries_lines(80));

    assert_eq!(rendered, vec!["codex  │ streaming".to_string()]);
}

#[test]
fn assistant_blank_content_line_keeps_divider() {
    let mut app = App::new();
    app.entries.clear();
    app.push_entry(EntryKind::Assistant, "[codex]\nline 1\n\nline 3");

    let rendered = flatten_lines_to_plain(&app.render_entries_lines(80));

    assert_eq!(rendered[0], "codex  │ line 1");
    assert_eq!(rendered[1], "       │  ");
    assert_eq!(rendered[2], "       │ line 3");
}

#[test]
fn large_paste_is_collapsed_and_restored_before_dispatch() {
    let mut app = App::new();
    let payload = "line ".repeat(220);
    app.handle_paste_event(&payload);

    assert!(app.input.starts_with("[Pasted Content "));
    assert_eq!(app.pending_pastes.len(), 1);

    let expanded = app.consume_pending_pastes(&app.input.clone());
    assert_eq!(expanded, payload);
    assert!(app.pending_pastes.is_empty());
}

#[test]
fn short_paste_keeps_plain_text() {
    let mut app = App::new();
    app.handle_paste_event("hello\nworld");
    assert_eq!(app.input, "hello\nworld");
    assert!(app.pending_pastes.is_empty());
}

#[test]
fn mem_command_reports_backend_unavailable_in_tests() {
    let mut app = App::new();
    app.input = "/mem show".to_string();
    app.cursor = app.input.len();

    app.submit_current_line(false);

    let last = app.entries.last().expect("expected memory error entry");
    assert!(matches!(last.kind, EntryKind::Error));
    assert!(last.text.contains("memory backend unavailable"));
}

#[test]
fn running_submit_hint_is_not_duplicated() {
    let mut app = App::new();
    app.entries.clear();
    app.running_providers.insert(app.primary_provider);
    app.input = "hello".to_string();
    app.cursor = app.input.len();

    app.submit_current_line(false);
    app.submit_current_line(false);

    let wait_count = app
        .entries
        .iter()
        .filter(|entry| {
            matches!(entry.kind, EntryKind::System) && entry.text.ends_with("is running, wait...")
        })
        .count();
    assert_eq!(wait_count, 1);
}

#[test]
fn transcript_restore_defaults_to_hidden_when_memory_is_available() {
    assert!(!restore_transcript_on_start(true));
    assert!(restore_transcript_on_start(false));
}
