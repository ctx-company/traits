//! Pure, unit-testable pieces of mouse-drag copy-and-deselect (task 0023):
//! terminal-style linear-region math, buffer-row text extraction (wide
//! Unicode aware, trailing whitespace trimmed), an OSC 52 clipboard payload
//! builder with a size cap, and the per-frame ellipsized-text ledger that
//! [`super::tui::truncate_display_width_end_recording`] feeds and
//! [`substitute_ledger`] consults. Nothing here touches a real terminal —
//! [`super::tui_ratatui`] is the only caller that does.

use std::cell::RefCell;

use base64::Engine;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::tui::display_width;

/// One contiguous run of columns selected on a single buffer row. `end_col`
/// is exclusive, matching `Buffer`'s own `[x, x+width)` cell convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RowSpan {
    pub(crate) row: u16,
    pub(crate) start_col: u16,
    pub(crate) end_col: u16,
}

/// Terminal-style linear selection: order `anchor`/`current` into
/// (start, end) by row-major position (so a drag that moves *up* the screen
/// — end before anchor — still produces a forward selection), then span the
/// first row from `start`'s column to the row's right edge, every row in
/// between in full, and the last row from the area's left edge to `end`'s
/// column. A single-row selection spans only the columns between the two
/// points. Rows outside `area` are dropped; a selection entirely outside
/// `area` yields no spans.
pub(crate) fn linear_region(anchor: (u16, u16), current: (u16, u16), area: Rect) -> Vec<RowSpan> {
    let (start, end) = if (anchor.1, anchor.0) <= (current.1, current.0) {
        (anchor, current)
    } else {
        (current, anchor)
    };
    let (start_col, start_row) = start;
    let (end_col, end_row) = end;
    let right = area.x.saturating_add(area.width);
    let bottom = area.y.saturating_add(area.height);
    if right <= area.x || bottom <= area.y {
        return Vec::new();
    }
    let first_row = start_row.max(area.y);
    let last_row = end_row.min(bottom - 1);
    let mut spans = Vec::new();
    let mut row = first_row;
    while row <= last_row {
        let (row_start, row_end) = if start_row == end_row {
            (start_col, end_col.saturating_add(1))
        } else if row == start_row {
            (start_col, right)
        } else if row == end_row {
            (area.x, end_col.saturating_add(1))
        } else {
            (area.x, right)
        };
        let row_start = row_start.clamp(area.x, right);
        let row_end = row_end.clamp(area.x, right);
        if row_end > row_start {
            spans.push(RowSpan {
                row,
                start_col: row_start,
                end_col: row_end,
            });
        }
        if row == u16::MAX {
            break;
        }
        row += 1;
    }
    spans
}

/// Reads the last-drawn buffer cell symbols across `spans`, row by row,
/// trims trailing whitespace per row, and joins rows with `\n` — the same
/// shape terminal-native select-and-copy produces. A wide-Unicode cell's
/// continuation column (ratatui always renders it as a plain space with
/// `width() == 1`, never `skip`) is never visited directly: each cell's own
/// display width advances the column cursor, so the continuation cell is
/// stepped over rather than emitted as a stray space in the middle of text.
pub(crate) fn extract_text(buffer: &Buffer, spans: &[RowSpan]) -> String {
    spans
        .iter()
        .map(|span| {
            let mut text = String::new();
            let mut col = span.start_col;
            while col < span.end_col {
                let Some(cell) = buffer.cell((col, span.row)) else {
                    break;
                };
                let symbol = cell.symbol();
                let width = display_width(symbol).max(1) as u16;
                text.push_str(symbol);
                col = col.saturating_add(width);
            }
            text.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The common terminal OSC 52 payload limit. A selection larger than this
/// (raw text, before base64 expansion) is truncated at a char boundary
/// rather than rejected outright — a degraded copy beats none, and the
/// terminals that even honor OSC 52 already treat an oversized payload as a
/// no-op, so silently capping here is strictly better than sending garbage
/// they'd drop anyway. No log call: this runs mid-frame with stderr owned by
/// the active alternate-screen pane, and writing anything else to it here
/// would corrupt the very screen being drawn.
const OSC52_MAX_PAYLOAD_BYTES: usize = 100_000;

fn cap_for_osc52(text: &str) -> &str {
    let max_raw_bytes = OSC52_MAX_PAYLOAD_BYTES * 3 / 4;
    if text.len() <= max_raw_bytes {
        return text;
    }
    let mut end = max_raw_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Builds the OSC 52 escape sequence (`\x1b]52;c;<base64>\x07`) that copies
/// `text` to the system clipboard on terminals that honor it (iTerm2, kitty,
/// tmux with `set-clipboard on`); ignored outright by terminals that don't
/// (Terminal.app by default) — a silent no-op there, not an error.
pub(crate) fn osc52_sequence(text: &str) -> String {
    let capped = cap_for_osc52(text);
    let encoded = base64::engine::general_purpose::STANDARD.encode(capped.as_bytes());
    format!("\x1b]52;c;{encoded}\x07")
}

thread_local! {
    /// Per-frame ellipsized-text provenance: `(rendered_truncated_text,
    /// full_source_text)` pairs recorded by each truncation call site during
    /// one render, consulted once after that same frame's `terminal.draw`
    /// call — both happen on the same (single-threaded) render path, so a
    /// thread-local is a per-frame ledger in practice. Keyed by rendered-tail
    /// CONTENT, not screen position (position isn't cheaply knowable at the
    /// truncation call sites): two different full values that happen to
    /// truncate to identical rendered text in the same frame could
    /// substitute the wrong one. Vanishingly rare given the `HH:MM:SS`
    /// prefixes on event rows, but a known approximation, not a guarantee.
    static LEDGER: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Clears the ledger; called once per frame, before that frame's truncation
/// call sites run.
pub(crate) fn clear_ledger() {
    LEDGER.with(|ledger| ledger.borrow_mut().clear());
}

/// Records that `rendered` (what actually ended up on screen, ellipsis and
/// all) stands in for `full` (the untruncated source). A no-op when the two
/// are equal — nothing was actually truncated, so there is nothing to
/// substitute later.
pub(crate) fn record_truncation(rendered: &str, full: &str) {
    if rendered == full {
        return;
    }
    LEDGER.with(|ledger| {
        ledger
            .borrow_mut()
            .push((rendered.to_string(), full.to_string()));
    });
}

/// Expands every recorded truncated tail found in `text` to its full source
/// text. Applied to buffer-extracted selection text before it is copied, so
/// a selection that reached an ellipsis copies the whole value it stands
/// for rather than the `…`-truncated cells that were actually on screen.
pub(crate) fn substitute_ledger(text: &str) -> String {
    LEDGER.with(|ledger| {
        let mut result = text.to_string();
        for (rendered, full) in ledger.borrow().iter() {
            if result.contains(rendered.as_str()) {
                result = result.replace(rendered.as_str(), full.as_str());
            }
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn area(x: u16, y: u16, width: u16, height: u16) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn single_cell_selection_spans_exactly_one_column() {
        let spans = linear_region((3, 1), (3, 1), area(0, 0, 10, 5));
        assert_eq!(
            spans,
            vec![RowSpan {
                row: 1,
                start_col: 3,
                end_col: 4
            }]
        );
    }

    #[test]
    fn same_row_selection_spans_only_the_dragged_columns() {
        let spans = linear_region((2, 0), (6, 0), area(0, 0, 10, 5));
        assert_eq!(
            spans,
            vec![RowSpan {
                row: 0,
                start_col: 2,
                end_col: 7
            }]
        );
    }

    #[test]
    fn multi_row_drag_down_spans_first_row_to_edge_middle_full_last_row_from_start() {
        let spans = linear_region((5, 0), (2, 2), area(0, 0, 10, 5));
        assert_eq!(
            spans,
            vec![
                RowSpan {
                    row: 0,
                    start_col: 5,
                    end_col: 10
                },
                RowSpan {
                    row: 1,
                    start_col: 0,
                    end_col: 10
                },
                RowSpan {
                    row: 2,
                    start_col: 0,
                    end_col: 3
                },
            ]
        );
    }

    #[test]
    fn reversed_drag_up_produces_the_same_region_as_the_equivalent_forward_drag() {
        let up = linear_region((2, 2), (5, 0), area(0, 0, 10, 5));
        let down = linear_region((5, 0), (2, 2), area(0, 0, 10, 5));
        assert_eq!(up, down);
    }

    #[test]
    fn selection_clamped_to_area_bounds() {
        let spans = linear_region((0, 0), (20, 20), area(2, 1, 5, 3));
        assert_eq!(
            spans,
            vec![
                RowSpan {
                    row: 1,
                    start_col: 2,
                    end_col: 7
                },
                RowSpan {
                    row: 2,
                    start_col: 2,
                    end_col: 7
                },
                RowSpan {
                    row: 3,
                    start_col: 2,
                    end_col: 7
                },
            ]
        );
    }

    #[test]
    fn selection_entirely_outside_area_yields_no_spans() {
        let spans = linear_region((0, 10), (0, 12), area(0, 0, 10, 5));
        assert!(spans.is_empty());
    }

    fn buffer_from_rows(rows: &[&str]) -> Buffer {
        let width = rows.iter().map(|row| display_width(row)).max().unwrap_or(0) as u16;
        let height = rows.len() as u16;
        let area = Rect::new(0, 0, width.max(1), height.max(1));
        let mut buffer = Buffer::empty(area);
        for (y, row) in rows.iter().enumerate() {
            buffer.set_string(0, y as u16, row, Style::default());
        }
        buffer
    }

    #[test]
    fn extraction_trims_trailing_whitespace_per_row_and_joins_with_newline() {
        let buffer = buffer_from_rows(&["hello   ", "world   "]);
        let spans = vec![
            RowSpan {
                row: 0,
                start_col: 0,
                end_col: 8,
            },
            RowSpan {
                row: 1,
                start_col: 0,
                end_col: 8,
            },
        ];
        assert_eq!(extract_text(&buffer, &spans), "hello\nworld");
    }

    #[test]
    fn extraction_handles_wide_unicode_without_duplicated_or_stray_cells() {
        let buffer = buffer_from_rows(&["文字ab  "]);
        let spans = vec![RowSpan {
            row: 0,
            start_col: 0,
            end_col: 8,
        }];
        assert_eq!(extract_text(&buffer, &spans), "文字ab");
    }

    #[test]
    fn osc52_sequence_wraps_base64_of_the_text_in_the_copy_escape() {
        let sequence = osc52_sequence("hi");
        assert_eq!(sequence, "\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn osc52_sequence_caps_oversized_payloads_at_a_char_boundary() {
        let huge = "文".repeat(60_000);
        let sequence = osc52_sequence(&huge);
        assert!(sequence.len() < huge.len() * 3);
        assert!(sequence.starts_with("\x1b]52;c;"));
        assert!(sequence.ends_with('\x07'));
    }

    #[test]
    fn ledger_round_trips_a_single_recorded_truncation() {
        clear_ledger();
        record_truncation(
            "00:00:05 a very long p...",
            "00:00:05 a very long path/to/file.rs",
        );
        let selected = "00:00:05 a very long p...";
        assert_eq!(
            substitute_ledger(selected),
            "00:00:05 a very long path/to/file.rs"
        );
        clear_ledger();
    }

    #[test]
    fn ledger_ignores_a_no_op_record_where_rendered_equals_full() {
        clear_ledger();
        record_truncation("short", "short");
        assert_eq!(substitute_ledger("short"), "short");
        clear_ledger();
    }

    #[test]
    fn ledger_leaves_unrelated_text_untouched() {
        clear_ledger();
        record_truncation("truncated...", "truncated full value");
        assert_eq!(substitute_ledger("unrelated text"), "unrelated text");
        clear_ledger();
    }

    #[test]
    fn clear_ledger_drops_prior_frame_recordings() {
        clear_ledger();
        record_truncation("a...", "a full value");
        clear_ledger();
        assert_eq!(substitute_ledger("a..."), "a...");
    }
}
