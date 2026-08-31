//! The shared start-clock presentation: a UNIX epoch plus the reader's UTC
//! offset *at that stamp* (not "now"), decomposed into either an `HH:MM:SS`
//! or `HH:MM` clock string. Moved here from `ctx_traits_cli::app::run_view`
//! (`0265.10`) so the desktop gets byte-identical presentation without a
//! `ctx-traits-cli` dependency; the CLI stays the retained non-GUI consumer.
//! `libc` is already a direct dependency of this crate; `core` has none,
//! which is why this owns the clock rather than sitting beside
//! `procedure::activity::compact_elapsed_text`.

/// The reader's UTC offset, in seconds, at the moment `epoch` occurred (not
/// "now" — DST-correct for the stamp being rendered). `None` if the C
/// library cannot resolve it, in which case the caller falls back to a
/// labelled UTC display. No `chrono`/`time` dependency exists in this
/// workspace; one `localtime_r` call (which applies the environment's `TZ`
/// itself, POSIX-equivalent to a `tzset` call) rather than TZif parsing.
pub fn local_utc_offset_seconds(epoch: u64) -> Option<i32> {
    let epoch_time = epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::localtime_r(&epoch_time, &mut tm) };
    if result.is_null() {
        None
    } else {
        Some(tm.tm_gmtoff as i32)
    }
}

/// The one decomposition both clock formats share, so the `UTC` suffix rule
/// and the `rem_euclid` day wrap cannot drift between them.
fn decompose(epoch: u64, utc_offset_seconds: Option<i32>) -> (i64, i64, i64, &'static str) {
    let (offset, suffix) = match utc_offset_seconds {
        Some(offset) => (offset as i64, ""),
        None => (0, " UTC"),
    };
    let seconds_of_day = (epoch as i64 + offset).rem_euclid(86_400);
    let hours = seconds_of_day / 3_600;
    let minutes = (seconds_of_day % 3_600) / 60;
    let seconds = seconds_of_day % 60;
    (hours, minutes, seconds, suffix)
}

/// Pure `HH:MM:SS` decomposition of a UNIX epoch, shifted by
/// `utc_offset_seconds`. `None` renders the UTC fallback labelled `UTC`;
/// `Some(0)` renders unlabelled (a genuinely-UTC locale is local time, not a
/// degradation).
pub fn epoch_clock(epoch: u64, utc_offset_seconds: Option<i32>) -> String {
    let (hours, minutes, seconds, suffix) = decompose(epoch, utc_offset_seconds);
    format!("{hours:02}:{minutes:02}:{seconds:02}{suffix}")
}

/// Minute-precision `HH:MM` decomposition, same offset+fallback policy as
/// [`epoch_clock`].
pub fn epoch_clock_minutes(epoch: u64, utc_offset_seconds: Option<i32>) -> String {
    let (hours, minutes, _seconds, suffix) = decompose(epoch, utc_offset_seconds);
    format!("{hours:02}:{minutes:02}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_clock_wraps_seconds_of_day() {
        assert_eq!(epoch_clock(3_723, Some(0)), "01:02:03");
        assert_eq!(epoch_clock(86_400, Some(0)), "00:00:00");
    }

    #[test]
    fn epoch_clock_applies_offset() {
        assert_eq!(epoch_clock(3_723, Some(-2 * 3_600)), "23:02:03");
        assert_eq!(epoch_clock(3_723, Some(5 * 3_600 + 45 * 60)), "06:47:03");
        assert_eq!(epoch_clock(3_723, None), "01:02:03 UTC");
    }

    #[test]
    fn epoch_clock_minutes_matches_epoch_clock_without_seconds() {
        assert_eq!(epoch_clock_minutes(3_723, Some(0)), "01:02");
        assert_eq!(epoch_clock_minutes(3_723, None), "01:02 UTC");
        assert_eq!(epoch_clock_minutes(3_723, Some(-2 * 3_600)), "23:02");
    }
}
