pub(super) fn sanitize_runtime_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_escape = false;
    let mut in_csi = false;

    for ch in text.chars() {
        if in_escape {
            if in_csi {
                // CSI sequence terminates at bytes in range 0x40..0x7E.
                if ('@'..='~').contains(&ch) {
                    in_escape = false;
                    in_csi = false;
                }
                continue;
            }
            if ch == '[' {
                in_csi = true;
                continue;
            }
            in_escape = false;
            continue;
        }

        if ch == '\u{1b}' {
            in_escape = true;
            continue;
        }

        if ch == '\r' {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }

        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }

        out.push(ch);
    }

    out
}
