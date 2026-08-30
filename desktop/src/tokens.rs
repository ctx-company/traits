//! The one entry point for the dark token set of
//! `.internal/docs/design/tokens.md`. Every colour, wash, type size, weight,
//! font-family name and layout constant of that table's dark column is
//! defined exactly once here; siblings that repaint a surface import from
//! this module rather than re-deriving a value. Only the dark column exists
//! — the light column stays recorded in `tokens.md` and unimplemented.
//!
//! Colours are `u32` hex literals (`0xRRGGBB`) rather than `gpui::Rgba`
//! structs: `gpui::rgb`/`gpui::rgba` are not `const fn`, so a `const Rgba`
//! would require hand-computing the float channels, a transcription surface
//! that cannot be diffed against `tokens.md` line-for-line. A call site
//! writes `rgb(tokens::TEXT_MUTED)`.

use gpui::{FontWeight, Pixels, px};

// ---------------------------------------------------------------------
// Fonts — tokens.md:6-11
// ---------------------------------------------------------------------

pub const FONT_SANS: &str = "IBM Plex Sans";
pub const FONT_MONO: &str = "IBM Plex Mono";

// ---------------------------------------------------------------------
// Colors — tokens.md:13-48 (dark column only)
// ---------------------------------------------------------------------

pub const CANVAS: u32 = 0x0c0d10;
pub const CARD: u32 = 0x0e0f12;
pub const SURFACE: u32 = 0x14161b;
pub const SURFACE_RAISED: u32 = 0x16181d;
pub const ROW_OPEN: u32 = 0x101215;
pub const BUTTON: u32 = 0x1c1e24;
pub const BORDER: u32 = 0x1a1c21;
pub const BORDER_SOFT: u32 = 0x23262d;
pub const BORDER_STRONG: u32 = 0x2b2e35;
pub const TEXT_HEADING: u32 = 0xf2f4f7;
pub const TEXT_BRIGHT: u32 = 0xeceef2;
pub const TEXT: u32 = 0xd7dae0;
pub const TEXT_SESSION: u32 = 0xa7abb5;
pub const TEXT_SECONDARY: u32 = 0x8b909c;
pub const TEXT_GHOST: u32 = 0x7f8590;
pub const TEXT_FACT: u32 = 0x6b7078;
pub const TEXT_MUTED: u32 = 0x585d68;
pub const TEXT_FAINT: u32 = 0x4d525c;
pub const TEXT_VERSION: u32 = 0x3f434b;
pub const DOT_IDLE: u32 = 0x33363d;
pub const DOT_DIM: u32 = 0x4a4f5a;
pub const CRUMB: u32 = 0x2c2f36;
/// tokens.md:39 — shares the hex value `#23262d` with [`BORDER_SOFT`] in the
/// dark column; the two roles diverge only in the (unimplemented) light
/// column. Kept as its own name since a dark-only implementation cannot
/// otherwise tell the two roles apart at a call site.
pub const SPINE: u32 = 0x23262d;
pub const ACCENT: u32 = 0x7fb2ff;
pub const ACCENT_BRIGHT: u32 = 0xa8ccff;
pub const ACCENT_DIM: u32 = 0x5778ac;
pub const OK: u32 = 0x7fc79a;
pub const WARN: u32 = 0xd8b46a;
pub const WARN_DIM: u32 = 0x917a4a;
pub const DANGER: u32 = 0xe08b7f;
pub const REVIEW: u32 = 0xc49af0;
pub const REVIEW_DIM: u32 = 0x8469a2;

// ---------------------------------------------------------------------
// Diff row washes — tokens.md:50-51. Literals in the source design, not
// variables: consumed with `gpui::rgba(..)`, not `gpui::rgb(..)`.
// ---------------------------------------------------------------------

pub const WASH_ADDITIONS: u32 = 0x7fc79a14;
pub const WASH_DELETIONS: u32 = 0xe08b7f14;
pub const WASH_SELECTION: u32 = 0x7fb2ff12;

// ---------------------------------------------------------------------
// Type scale — tokens.md:53-65. Named by value, not by role: role
// assignment is `0265.5`'s.
// ---------------------------------------------------------------------

pub const SIZE_16: Pixels = px(16.);
pub const SIZE_14: Pixels = px(14.);
pub const SIZE_13: Pixels = px(13.);
pub const SIZE_12_5: Pixels = px(12.5);
pub const SIZE_12: Pixels = px(12.);
pub const SIZE_11_5: Pixels = px(11.5);
pub const SIZE_11: Pixels = px(11.);
pub const SIZE_10_5: Pixels = px(10.5);
pub const SIZE_10: Pixels = px(10.);

// ---------------------------------------------------------------------
// Weights — tokens.md:68. Only `WEIGHT_NORMAL` is defined: no repainted
// call site in this task uses the medium (500) weight reserved for
// emphasized in-block titles, which is sibling-owned.
// ---------------------------------------------------------------------

pub const WEIGHT_NORMAL: FontWeight = FontWeight::NORMAL;

// ---------------------------------------------------------------------
// Layout constants — tokens.md:71-85
// ---------------------------------------------------------------------

// window
pub const WINDOW_WIDTH: Pixels = px(1440.);
pub const WINDOW_HEIGHT: Pixels = px(900.);
pub const WINDOW_CORNER_RADIUS: Pixels = px(10.);

// title bar
pub const TITLE_BAR_HEIGHT: Pixels = px(36.);
pub const TITLE_BAR_PAD_Y: Pixels = px(0.);
pub const TITLE_BAR_PAD_X: Pixels = px(20.);
pub const TITLE_BAR_TRAFFIC_LIGHT: Pixels = px(12.);
pub const TITLE_BAR_LOGO_WIDTH: Pixels = px(34.);
pub const TITLE_BAR_LOGO_HEIGHT: Pixels = px(18.);
pub const TITLE_BAR_MENU_GAP: Pixels = px(18.);
pub const TITLE_BAR_MENU_SIZE: Pixels = SIZE_12_5;

// rail
pub const RAIL_WIDTH: Pixels = px(260.);
pub const RAIL_PAD_Y: Pixels = px(18.);
pub const RAIL_PAD_X: Pixels = px(0.);
pub const RAIL_HEADING_PAD_TOP: Pixels = px(0.);
pub const RAIL_HEADING_PAD_RIGHT: Pixels = px(18.);
pub const RAIL_HEADING_PAD_BOTTOM: Pixels = px(12.);
pub const RAIL_HEADING_PAD_LEFT: Pixels = px(18.);
pub const RAIL_REPO_ROW_PAD_Y: Pixels = px(0.);
pub const RAIL_REPO_ROW_PAD_X: Pixels = px(10.);
pub const RAIL_REPO_ROW_GAP: Pixels = px(3.);
pub const RAIL_ROW_PAD_Y: Pixels = px(8.);
pub const RAIL_ROW_PAD_X: Pixels = px(10.);

// rail footer
pub const RAIL_FOOTER_PAD_TOP: Pixels = px(14.);
pub const RAIL_FOOTER_PAD_RIGHT: Pixels = px(18.);
pub const RAIL_FOOTER_PAD_BOTTOM: Pixels = px(0.);
pub const RAIL_FOOTER_PAD_LEFT: Pixels = px(18.);
pub const RAIL_FOOTER_GAP: Pixels = px(6.);

// main pane
pub const MAIN_PANE_PAD_TOP: Pixels = px(18.);
pub const MAIN_PANE_PAD_RIGHT: Pixels = px(40.);
pub const MAIN_PANE_PAD_BOTTOM: Pixels = px(20.);
pub const MAIN_PANE_PAD_LEFT: Pixels = px(40.);
pub const MAIN_PANE_GAP: Pixels = px(28.);

// preview column
pub const PREVIEW_COLUMN_WIDTH: Pixels = px(380.);
pub const PREVIEW_COLUMN_PAD_Y: Pixels = px(18.);
pub const PREVIEW_COLUMN_PAD_X: Pixels = px(22.);

// list rows
//
// tokens.md:81 "list rows | pad [6–8, 10–12]" gives a range, not a value, on
// both axes. Both endpoints are named rather than silently collapsed to one;
// the compact endpoint is what a normal row demonstrates, the open endpoint
// is the current/selected row's. See the work summary for the corresponding
// owner correction (a range needs a naming convention ruling before
// siblings inherit it).
pub const LIST_ROW_PAD_Y_COMPACT: Pixels = px(6.);
pub const LIST_ROW_PAD_Y_OPEN: Pixels = px(8.);
pub const LIST_ROW_PAD_X_MIN: Pixels = px(10.);
pub const LIST_ROW_PAD_X_MAX: Pixels = px(12.);
pub const LIST_ROWS_GAP_MIN: Pixels = px(2.);
pub const LIST_ROWS_GAP_MAX: Pixels = px(4.);
pub const LIST_SECTION_GAP: Pixels = px(12.);
pub const LIST_ROW_DOT_SIZE: Pixels = px(5.);
pub const ROW_DOT_TEXT_GAP_MIN: Pixels = px(10.);
pub const ROW_DOT_TEXT_GAP_MAX: Pixels = px(12.);

// blocks
pub const BLOCK_GAP: Pixels = px(8.);
pub const BLOCK_BOTTOM_PADDING: Pixels = px(20.);
pub const BORDERED_BOX_PAD_Y: Pixels = px(8.);
pub const BORDERED_BOX_PAD_X_MIN: Pixels = px(10.);
pub const BORDERED_BOX_PAD_X_MAX: Pixels = px(12.);

// bottom bar
pub const BOTTOM_BAR_PAD_Y: Pixels = px(10.);
pub const BOTTOM_BAR_PAD_X: Pixels = px(14.);
pub const BOTTOM_BAR_BORDER: Pixels = px(1.);
/// `tokens.md:83` records the bar's pad and border but not a gap between its
/// two halves' children; sourced from `sessions.html:579,597` instead. See
/// the work summary for the corresponding owner correction.
pub const BOTTOM_BAR_GAP: Pixels = px(8.);

// bottom fade
pub const BOTTOM_FADE_HEIGHT: Pixels = px(100.);

// rail divider
pub const RAIL_DIVIDER_WIDTH: Pixels = px(32.);
pub const RAIL_DIVIDER_HEIGHT: Pixels = px(1.);
pub const RAIL_DIVIDER_PAD_Y_MIN: Pixels = px(12.);
pub const RAIL_DIVIDER_PAD_Y_MAX: Pixels = px(14.);
pub const RAIL_DIVIDER_PAD_X_MIN: Pixels = px(16.);
pub const RAIL_DIVIDER_PAD_X_MAX: Pixels = px(18.);
