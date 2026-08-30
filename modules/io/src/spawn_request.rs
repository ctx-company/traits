//! Face-independent half of spawn-argument validation, shared by every
//! caller of the center's spawn capability ([`crate::center::start_trait`],
//! [`crate::center::start_trait_existing`]). Parses one-argument-per-line
//! spawn text into trimmed user arguments and rejects flags the center's
//! own `run_start` already owns — it injects `--progress none` and owns
//! detached process/log setup, so a caller supplying one of these itself
//! would either collide with or bypass that behavior.

const FORBIDDEN: &[&str] = &[
    "--no-drive",
    "--ephemeral",
    "--out",
    "--session-store",
    "--json",
    "--progress",
];

/// Parse one-argument-per-line spawn text into user arguments, dropping
/// blank lines and `#`-prefixed comments. Rejects empty input and any
/// forbidden flag, matched by name before `=`.
pub fn parse_spawn_args(text: &str) -> Result<Vec<String>, String> {
    let user_args: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    if user_args.is_empty() {
        return Err("spawn request was empty".to_string());
    }
    for arg in &user_args {
        let flag = arg.split('=').next().unwrap_or(arg);
        if FORBIDDEN.contains(&flag) {
            return Err(format!(
                "{flag} is not permitted in a dashboard spawn request"
            ));
        }
    }
    Ok(user_args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_spawn_args_rejects_empty_or_comment_only_text() {
        assert_eq!(
            parse_spawn_args(""),
            Err("spawn request was empty".to_string())
        );
        assert_eq!(
            parse_spawn_args("# just a comment\n\n   \n"),
            Err("spawn request was empty".to_string())
        );
    }

    #[test]
    fn parse_spawn_args_rejects_each_forbidden_flag_by_name() {
        for flag in [
            "--no-drive",
            "--ephemeral",
            "--out",
            "--session-store",
            "--json",
            "--progress",
        ] {
            let error = parse_spawn_args(&format!("fixture\n{flag}"))
                .expect_err("forbidden flag must be rejected");
            assert!(error.contains(flag), "refusal must name {flag}: {error}");

            let with_value = parse_spawn_args(&format!("fixture\n{flag}=value"))
                .expect_err("forbidden flag=value form must be rejected");
            assert!(
                with_value.contains(flag),
                "refusal must name {flag}: {with_value}"
            );
        }
    }

    #[test]
    fn parse_spawn_args_returns_user_args_verbatim_with_no_injected_flags() {
        let args = parse_spawn_args("fixture\n--set\nanswer=true").expect("valid request");
        assert_eq!(args, ["fixture", "--set", "answer=true"]);
        assert!(!args.iter().any(|arg| arg == "--progress"));
    }

    #[test]
    fn parse_spawn_args_trims_and_drops_comments_and_blank_lines() {
        let args = parse_spawn_args("  fixture  \n# comment\n\n--set\n").expect("valid request");
        assert_eq!(args, ["fixture", "--set"]);
    }
}
