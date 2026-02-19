use super::*;

pub(crate) fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let mut app = App::new();
    const ACTIVE_POLL_MS: u64 = 33;
    const IDLE_POLL_MS: u64 = 100;
    const SPINNER_TICK_MS: u64 = 120;
    const RUNNING_DRAW_INTERVAL_MS: u64 = 33;
    const MAX_EVENTS_PER_FRAME: u16 = 64;
    let mut last_spinner_tick = Instant::now();
    let mut last_draw_at = Instant::now()
        .checked_sub(Duration::from_millis(RUNNING_DRAW_INTERVAL_MS))
        .unwrap_or_else(Instant::now);
    let mut needs_draw = true;
    let mut flushed_log_lines: Vec<Line<'static>> = Vec::new();
    let mut last_flush_was_running = false;

    loop {
        let mut state_changed = false;
        if app.poll_worker() {
            state_changed = true;
        }
        if app.is_running() && last_spinner_tick.elapsed() >= Duration::from_millis(SPINNER_TICK_MS) {
            app.spinner_idx = (app.spinner_idx + 1) % 8;
            last_spinner_tick = Instant::now();
            state_changed = true;
        }
        if state_changed {
            needs_draw = true;
        }

        if app.needs_screen_clear {
            app.needs_screen_clear = false;
            flushed_log_lines.clear();
            last_flush_was_running = false;
            // Clear the entire terminal including scrollback, not just the ratatui viewport.
            crossterm::execute!(
                std::io::stdout(),
                TermClear(ClearType::Purge),
                crossterm::cursor::MoveTo(0, 0)
            )?;
            terminal.clear()?;
            needs_draw = true;
        }

        if needs_draw {
            if app.is_running()
                && last_draw_at.elapsed() < Duration::from_millis(RUNNING_DRAW_INTERVAL_MS)
            {
                // Hold briefly to batch incoming chunks and avoid per-frame flashing.
            } else {
                if let Ok(area) = terminal.size() {
                    app.update_viewport(area.width, area.height);
                }
                app.ensure_render_cache();
                flush_new_log_lines(
                    terminal,
                    &app,
                    &mut flushed_log_lines,
                    &mut last_flush_was_running,
                )?;
                terminal.draw(|f| ui::draw(f, &app))?;
                last_draw_at = Instant::now();
                needs_draw = false;
            }
        }

        if app.should_quit {
            break;
        }

        let timeout = if app.is_running() {
            Duration::from_millis(ACTIVE_POLL_MS)
        } else {
            Duration::from_millis(IDLE_POLL_MS)
        };
        if !event::poll(timeout).context("event poll")? {
            continue;
        }

        let mut wheel_delta: i32 = 0;
        let mut drained_events: u16 = 0;
        let mut input_changed = false;

        loop {
            match event::read().context("event read")? {
                Event::Key(key) => {
                    if !matches!(key.kind, KeyEventKind::Release) {
                        app.handle_key(key);
                        input_changed = true;
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => wheel_delta -= 1,
                    MouseEventKind::ScrollDown => wheel_delta += 1,
                    _ => {}
                },
                Event::Paste(text) => {
                    app.handle_paste_event(&text);
                    input_changed = true;
                }
                Event::Resize(_, _) => {
                    input_changed = true;
                }
                _ => {}
            }

            drained_events = drained_events.saturating_add(1);
            if drained_events >= MAX_EVENTS_PER_FRAME {
                break;
            }
            if !event::poll(Duration::from_millis(0)).context("event poll drain")? {
                break;
            }
        }

        if wheel_delta < 0 {
            app.scroll_up(wheel_delta.abs().min(64) as u16);
            input_changed = true;
        } else if wheel_delta > 0 {
            app.scroll_down(wheel_delta.min(64) as u16);
            input_changed = true;
        }

        if input_changed {
            needs_draw = true;
        }
    }

    app.persist_session();

    // Clear input/status bars while keeping transcript in terminal scrollback.
    terminal.draw(|f| ui::draw_exit(f, &app))?;
    Ok(())
}

fn flush_new_log_lines(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &App,
    flushed_log_lines: &mut Vec<Line<'static>>,
    last_flush_was_running: &mut bool,
) -> Result<()> {
    // While running we still flush streaming rows so work-in-progress text
    // remains visible in transcript scrollback.
    let lines: &[Line<'static>];
    let running_buf;
    if app.is_running() {
        let w = terminal.size().map(|s| s.width).unwrap_or(80).max(1);
        running_buf = app.running_flush_log_lines(w);
        lines = &running_buf;
    } else {
        lines = app.cached_log_lines();
    }
    if lines.is_empty() {
        flushed_log_lines.clear();
        *last_flush_was_running = false;
        return Ok(());
    }

    // Compare using plain text to avoid false mismatches caused by style-only updates
    // (for example, running -> done color changes).
    let flushed_plain = flatten_lines_to_plain(flushed_log_lines);
    let current_plain = flatten_lines_to_plain(lines);

    let append_ranges = compute_flush_append_ranges(
        &flushed_plain,
        &current_plain,
        app.is_running(),
        *last_flush_was_running,
    );
    if append_ranges.is_empty() {
        *flushed_log_lines = lines.to_vec();
        *last_flush_was_running = app.is_running();
        return Ok(());
    }
    let mut new_lines = Vec::new();
    for (start, end) in append_ranges {
        new_lines.extend(lines[start..end].iter().cloned());
    }
    let width = terminal
        .size()
        .context("terminal size for insert")?
        .width
        .max(1);
    let probe = Paragraph::new(Text::from(new_lines.clone())).wrap(Wrap { trim: false });
    let rendered_lines = probe.line_count(width).min(u16::MAX as usize);
    let height = rendered_lines as u16;
    if height == 0 {
        return Ok(());
    }

    let insert_result = catch_unwind(AssertUnwindSafe(|| {
        terminal.insert_before(height, |buf| {
            let paragraph = Paragraph::new(Text::from(new_lines)).wrap(Wrap { trim: false });
            paragraph.render(buf.area, buf);
        })
    }));
    match insert_result {
        Ok(res) => {
            res.context("insert transcript lines")?;
        }
        Err(_) => {
            // Fallback: force a replay on next frame instead of silently dropping updates.
            flushed_log_lines.clear();
            return Ok(());
        }
    }

    *flushed_log_lines = lines.to_vec();
    *last_flush_was_running = app.is_running();
    Ok(())
}

pub(super) fn flatten_lines_to_plain(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(flatten_line_to_plain).collect()
}

pub(super) fn flatten_line_to_plain(line: &Line<'static>) -> String {
    let mut out = String::new();
    for span in &line.spans {
        out.push_str(span.content.as_ref());
    }
    out
}

pub(super) fn compute_append_ranges(
    flushed_plain: &[String],
    current_plain: &[String],
) -> Vec<(usize, usize)> {
    if flushed_plain == current_plain {
        return Vec::new();
    }
    if flushed_plain.len() > current_plain.len() {
        return Vec::new();
    }
    if current_plain[..flushed_plain.len()] == *flushed_plain {
        return vec![(flushed_plain.len(), current_plain.len())];
    }
    // Existing rows changed in place (including middle insertions).
    // With append-only scrollback writes we cannot patch these safely.
    Vec::new()
}

pub(super) fn compute_running_append_ranges(
    flushed_plain: &[String],
    current_plain: &[String],
) -> Vec<(usize, usize)> {
    if flushed_plain == current_plain {
        return Vec::new();
    }
    if flushed_plain.len() > current_plain.len() {
        return Vec::new();
    }
    if current_plain[..flushed_plain.len()] == *flushed_plain {
        return vec![(flushed_plain.len(), current_plain.len())];
    }
    let common_prefix = flushed_plain
        .iter()
        .zip(current_plain.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if common_prefix >= current_plain.len() {
        return Vec::new();
    }
    vec![(common_prefix, current_plain.len())]
}

pub(super) fn compute_flush_append_ranges(
    flushed_plain: &[String],
    current_plain: &[String],
    is_running: bool,
    last_flush_was_running: bool,
) -> Vec<(usize, usize)> {
    if is_running || last_flush_was_running {
        compute_running_append_ranges(flushed_plain, current_plain)
    } else {
        compute_append_ranges(flushed_plain, current_plain)
    }
}
