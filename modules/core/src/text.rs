//! Shared text-sanitizing helpers for any surface that echoes raw model or
//! tool text into a UI: ANSI/control-code stripping and a one-line quoted
//! preview. Moved here from `modules/cli/src/app/tui.rs` verbatim so the
//! desktop can consume the same sanitizing rules the CLI's live output panel
//! already established, instead of forking a second copy.

pub const PASSTHROUGH_PREVIEW_CHARS: usize = 100;

/// Wrap a raw model text delta as a one-line quoted preview for the passthrough
/// panel: control characters collapsed to spaces, head-truncated with an ellipsis.
pub fn quote_line(text: &str) -> String {
    let cleaned = clean_live_text(text);
    let trimmed = cleaned.trim();
    let preview: String = trimmed.chars().take(PASSTHROUGH_PREVIEW_CHARS).collect();
    if preview.chars().count() < trimmed.chars().count() {
        format!("\"{preview}…\"")
    } else {
        format!("\"{preview}\"")
    }
}

pub fn clean_live_text(line: &str) -> String {
    strip_ansi_sequences(line)
        .chars()
        .map(|ch| {
            if ch.is_control() || is_bidi_format_control(ch) {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

/// True for Unicode `Cf`-category bidirectional-formatting controls (the
/// explicit embedding/override/isolate marks and the directional marks).
/// These are not C0/C1 control codes and are not touched by
/// `char::is_control`, but a terminal still honors them and can use them to
/// visually reorder or mask surrounding text (e.g. `U+202E RIGHT-TO-LEFT
/// OVERRIDE`), so any text reaching a terminal via [`clean_live_text`] must
/// have them stripped alongside ANSI/control sequences.
pub fn is_bidi_format_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{200E}' | '\u{200F}' // LRM, RLM
            | '\u{061C}' // ALM (Arabic Letter Mark)
            | '\u{202A}'..='\u{202E}' // LRE, RLE, PDF, LRO, RLO
            | '\u{2066}'..='\u{2069}' // LRI, RLI, FSI, PDI
    )
}

pub fn strip_ansi_sequences(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            let Some(ch) = input[index..].chars().next() else {
                break;
            };
            output.push(ch);
            index += ch.len_utf8();
            continue;
        }
        index += 1;
        match bytes.get(index).copied() {
            Some(b'[') => {
                index += 1;
                while index < bytes.len() {
                    let byte = bytes[index];
                    index += 1;
                    if (0x40..=0x7e).contains(&byte) {
                        break;
                    }
                }
            }
            Some(b']') => {
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        0x07 => {
                            index += 1;
                            break;
                        }
                        0x1b if bytes.get(index + 1) == Some(&b'\\') => {
                            index += 2;
                            break;
                        }
                        _ => index += 1,
                    }
                }
            }
            Some(b'P' | b'^' | b'_') => {
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            Some(_) => index += 1,
            None => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_line_truncates_and_strips_control_chars() {
        assert_eq!(quote_line("hello\x1b[31mworld"), "\"helloworld\"");
        let long = "a".repeat(PASSTHROUGH_PREVIEW_CHARS + 10);
        assert_eq!(
            quote_line(&long),
            format!("\"{}…\"", "a".repeat(PASSTHROUGH_PREVIEW_CHARS))
        );
    }

    #[test]
    fn clean_live_text_strips_bidi_overrides() {
        assert_eq!(clean_live_text("a\u{202E}b"), "a b");
    }
}
