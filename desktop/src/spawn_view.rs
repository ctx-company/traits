//! Native gpui element tree for a [`SpawnForm`]. Free functions, not `Shell`
//! methods: no `Context`, no `cx.listener` — mirrors `detail_view.rs` so the
//! whole render path is directly callable from a test with no gpui `App`.
//! `Shell` supplies the key/click listeners around the element this module
//! returns.

use gpui::prelude::*;
use gpui::{AnyElement, div};

use crate::spawn_form::{SpawnForm, SpawnStatus};

fn status_text(status: &SpawnStatus) -> String {
    match status {
        SpawnStatus::Idle => String::new(),
        SpawnStatus::Invalid(reason) => format!("invalid: {reason}"),
        SpawnStatus::Requesting => "requesting…".to_string(),
        SpawnStatus::Requested { session_id } => {
            format!("start requested ({session_id}) — waiting for the center")
        }
        SpawnStatus::Rejected(reason) => format!("rejected: {reason}"),
    }
}

/// Build the native element tree for a [`SpawnForm`]'s text/status/hint —
/// everything but the repository picker, which `Shell` renders itself with
/// a click listener. Pure: no `Context`, no `App` required to call it.
/// Submit is disabled — via `status` text alone, since this module has no
/// listener to wire — while `Requesting`.
pub fn spawn_element(form: &SpawnForm) -> AnyElement {
    let requesting = matches!(form.status(), SpawnStatus::Requesting);
    div()
        .id("spawn-form")
        .flex()
        .flex_col()
        .gap_1()
        .child(div().id("spawn-text").child(if form.text().is_empty() {
            "# One argument per line. First non-comment line is the trait id.".to_string()
        } else {
            form.text().to_string()
        }))
        .child(div().id("spawn-status").child(status_text(form.status())))
        .child(div().id("spawn-submit-hint").child(if requesting {
            "submitting…"
        } else {
            "cmd-enter to submit"
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spawn_form::SpawnRepo;

    #[test]
    fn spawn_element_builds_for_every_status_with_no_app() {
        let mut form = SpawnForm::default();
        let _idle: AnyElement = spawn_element(&form);

        form.set_repositories(vec![SpawnRepo {
            repo_key: "repo-a".to_string(),
            repo_path: "/repo-a".to_string(),
            label: "repo-a".to_string(),
        }]);
        let _with_repo: AnyElement = spawn_element(&form);

        form.submit();
        let _invalid: AnyElement = spawn_element(&form);

        form.select_repository("repo-a".to_string());
        form.insert_char('f');
        let request = form.submit().expect("valid submit");
        let _requesting: AnyElement = spawn_element(&form);

        form.settle(
            request.generation,
            crate::spawn_form::SubmitOutcome::Started {
                session_id: "session-1".to_string(),
            },
        );
        let _requested: AnyElement = spawn_element(&form);
    }

    #[test]
    fn spawn_element_builds_for_a_rejected_status_from_either_failure_source() {
        let mut form = SpawnForm::default();
        form.set_repositories(vec![SpawnRepo {
            repo_key: "repo-a".to_string(),
            repo_path: "/repo-a".to_string(),
            label: "repo-a".to_string(),
        }]);
        form.select_repository("repo-a".to_string());
        form.insert_char('f');
        let request = form.submit().expect("valid submit");
        form.settle(
            request.generation,
            crate::spawn_form::SubmitOutcome::Exited {
                code: Some(1),
                stderr: "boom".to_string(),
            },
        );
        let _rejected_exited: AnyElement = spawn_element(&form);
        assert!(matches!(form.status(), SpawnStatus::Rejected(reason) if reason.contains("boom")));

        let mut form = SpawnForm::default();
        form.set_repositories(vec![SpawnRepo {
            repo_key: "repo-a".to_string(),
            repo_path: "/repo-a".to_string(),
            label: "repo-a".to_string(),
        }]);
        form.select_repository("repo-a".to_string());
        form.insert_char('f');
        let request = form.submit().expect("valid submit");
        form.settle(
            request.generation,
            crate::spawn_form::SubmitOutcome::Failed("transport error".to_string()),
        );
        let _rejected_failed: AnyElement = spawn_element(&form);
        assert!(
            matches!(form.status(), SpawnStatus::Rejected(reason) if reason.contains("transport error"))
        );
    }
}
