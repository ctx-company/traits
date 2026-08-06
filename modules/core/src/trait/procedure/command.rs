// Builds and validates procedure command plans.
// Procedure command definitions.

pub fn command_plan_for_item(
    item: &SequenceItem,
    field_path: &str,
) -> crate::Result<Option<CommandPlan>> {
    match (item.cmd.as_deref(), item.command.as_ref()) {
        (Some(_), Some(_)) => Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "sequence item must not declare both cmd and command".to_string(),
        }
        .into()),
        (Some(cmd), None) => {
            let argv = parse_command_shorthand(cmd, &format!("{field_path}.cmd"))?;
            Ok(Some(CommandPlan {
                argv,
                argv_from: None,
                executable_digest_from: None,
                cwd: None,
                timeout_ms: item.timeout_ms,
                idle_timeout_ms: item.idle_timeout_ms,
                capture_bytes: item.capture_bytes,
                success_exit_code: success_exit_codes(&item.success_exit_code),
            }))
        }
        (None, Some(command)) => {
            match (command.argv.is_empty(), command.argv_from.as_deref()) {
                (false, None) => {
                    validate_command_argv(&command.argv, &format!("{field_path}.command.argv"))?
                }
                (true, Some(_)) => {}
                (false, Some(_)) => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{field_path}.command"),
                        message: "command must declare either argv or argv-from, not both"
                            .to_string(),
                    }
                    .into());
                }
                (true, None) => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("{field_path}.command"),
                        message: "command must declare argv or argv-from".to_string(),
                    }
                    .into());
                }
            }
            validate_command_cwd(command.cwd.as_deref(), &format!("{field_path}.command.cwd"))?;
            Ok(Some(CommandPlan {
                argv: command.argv.clone(),
                argv_from: command.argv_from.clone(),
                executable_digest_from: command.executable_digest_from.clone(),
                cwd: command.cwd.clone(),
                timeout_ms: command.timeout_ms,
                idle_timeout_ms: command.idle_timeout_ms,
                capture_bytes: command.capture_bytes,
                success_exit_code: success_exit_codes(&command.success_exit_code),
            }))
        }
        (None, None) => Ok(None),
    }
}

/// Lower a simple command string to argv without shell interpretation.
///
/// The MVP parser intentionally supports only whitespace-separated tokens. It
/// rejects shell syntax instead of trying to emulate shell quoting or expansion.
pub fn parse_command_shorthand(cmd: &str, field_path: &str) -> crate::Result<Vec<String>> {
    if cmd.trim() != cmd || cmd.is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "command shorthand must be non-empty and trimmed".to_string(),
        }
        .into());
    }
    if cmd.contains('\n') || cmd.contains('\r') {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "command shorthand must not contain newlines".to_string(),
        }
        .into());
    }
    if cmd.chars().any(is_shell_metachar) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "command shorthand contains shell-only syntax; use explicit command.argv"
                .to_string(),
        }
        .into());
    }
    let argv: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
    validate_command_argv(&argv, field_path)?;
    Ok(argv)
}

fn is_shell_metachar(ch: char) -> bool {
    matches!(
        ch,
        '|' | '&'
            | ';'
            | '<'
            | '>'
            | '`'
            | '$'
            | '('
            | ')'
            | '{'
            | '}'
            | '['
            | ']'
            | '*'
            | '?'
            | '~'
    )
}

pub(crate) fn validate_command_argv(argv: &[String], field_path: &str) -> crate::Result<()> {
    validate_resolved_command_argv(argv, &[], field_path)
}

/// Validate a command argv in which the items at `substituted` indices carry
/// runtime-interpolated slot/port VALUES rather than authored text
/// (2026-08-06, run-99dafe1d): a model-written document handed to a step as a
/// positional argument is data, and data may legitimately be multi-line or
/// end in a newline — applying the authored-argv hygiene rule
/// (`trim() != arg`) to it killed the run the moment a draft ended with a
/// newline. Authored items — and the program at argv[0], substituted or not —
/// keep the full authored rules; a substituted non-program item is rejected
/// only for what actually breaks execution or proves a misfire: resolving to
/// an empty value, or carrying NUL. An empty `substituted` list makes this
/// exactly the authored-argv validation.
pub(crate) fn validate_resolved_command_argv(
    argv: &[String],
    substituted: &[usize],
    field_path: &str,
) -> crate::Result<()> {
    if argv.is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "command argv must not be empty".to_string(),
        }
        .into());
    }
    for (i, arg) in argv.iter().enumerate() {
        let data_item = i != 0 && substituted.contains(&i);
        if data_item {
            if arg.is_empty() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{field_path}[{i}]"),
                    message: "argv item resolved from a slot/port must not be empty".to_string(),
                }
                .into());
            }
        } else if arg.trim() != arg || arg.is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}[{i}]"),
                message: "argv item must be non-empty and trimmed".to_string(),
            }
            .into());
        }
        if arg.contains('\0') {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}[{i}]"),
                message: "argv item must not contain NUL".to_string(),
            }
            .into());
        }
    }
    let program = &argv[0];
    if program.split('/').any(|part| part == "..") {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "relative command program path must not contain ..".to_string(),
        }
        .into());
    }
    if program.contains('=') && program.split('=').next().is_some_and(is_env_name) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "environment-assignment command shorthand is unsupported".to_string(),
        }
        .into());
    }
    Ok(())
}

fn is_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn validate_command_cwd(cwd: Option<&str>, field_path: &str) -> crate::Result<()> {
    let Some(cwd) = cwd else {
        return Ok(());
    };
    if cwd != "project-root" {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "only cwd = \"project-root\" is supported for command steps".to_string(),
        }
        .into());
    }
    Ok(())
}

fn success_exit_codes(values: &[i32]) -> Vec<i32> {
    if values.is_empty() {
        vec![0]
    } else {
        let mut values = values.to_vec();
        values.sort_unstable();
        values.dedup();
        values
    }
}

#[cfg(test)]
mod validate_resolved_command_argv_tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn substituted_data_item_may_be_multi_line_and_untrimmed() {
        let argv = argv(&["sh", "-c", r#"printf %s "$1""#, "sh", "# draft\n\nbody\n"]);
        assert!(validate_resolved_command_argv(&argv, &[4], "runtime.command.argv").is_ok());
    }

    #[test]
    fn substituted_data_item_must_not_be_empty() {
        let argv = argv(&["sh", "-c", r#"printf %s "$1""#, "sh", ""]);
        let error = validate_resolved_command_argv(&argv, &[4], "runtime.command.argv")
            .expect_err("empty resolved value must be rejected");
        assert!(error.to_string().contains("resolved from a slot/port"));
    }

    #[test]
    fn substituted_data_item_must_not_contain_nul() {
        let argv = argv(&["sh", "-c", "echo", "sh", "a\0b"]);
        assert!(validate_resolved_command_argv(&argv, &[4], "runtime.command.argv").is_err());
    }

    #[test]
    fn authored_items_keep_the_trimmed_rule() {
        let argv = argv(&["sh", "-c", "echo", "sh", "tail \n"]);
        assert!(validate_resolved_command_argv(&argv, &[], "runtime.command.argv").is_err());
    }

    #[test]
    fn substituted_program_keeps_the_authored_rules() {
        let argv = argv(&["plannotator\n", "annotate"]);
        assert!(validate_resolved_command_argv(&argv, &[0], "runtime.command.argv").is_err());
    }
}
