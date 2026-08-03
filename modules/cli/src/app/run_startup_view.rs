//! Inline startup progress shown before a driven run has a session panel.

use std::sync::{Arc, Mutex};

use ratatui::layout::{Constraint, Direction};

use crate::app::tui::{Line, Tone};
use crate::app::tui_panes::{self, PaneTree};
use crate::app::tui_ratatui::{self, RatatuiPane};

use ctx_traits_io::run::{StartupObserver, StartupStage, StartupStageState, StartupUpdate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pending,
    Running,
    Done,
    Failed,
}

struct Row {
    stage: StartupStage,
    label: &'static str,
    state: State,
    detail: String,
}

struct Inner {
    pane: Option<RatatuiPane>,
    rows: Vec<Row>,
    interrupted: bool,
}

/// A shared, synchronous observer plus the one inline terminal owner. The
/// owner is consumed by the live panel only after session startup succeeds.
pub(crate) struct StartupView {
    inner: Arc<Mutex<Inner>>,
}

impl StartupView {
    pub(crate) fn new() -> std::io::Result<Self> {
        let rows = [
            (StartupStage::Initialization, "Initialization"),
            (StartupStage::Trust, "Trust gate"),
            (StartupStage::Harness, "Harness probe"),
            (StartupStage::Worktree, "Worktree"),
            (StartupStage::Seeding, "Seeding"),
            (StartupStage::Warm, "Warm"),
        ]
        .into_iter()
        .map(|(stage, label)| Row {
            stage,
            label,
            state: State::Pending,
            detail: "pending".to_string(),
        })
        .collect();
        let view = Self {
            inner: Arc::new(Mutex::new(Inner {
                pane: Some(RatatuiPane::new_inline()?),
                rows,
                interrupted: false,
            })),
        };
        let inner = Arc::clone(&view.inner);
        if let Ok(mut locked) = view.inner.lock()
            && let Some(pane) = locked.pane.as_mut()
        {
            pane.install_input_wake(Arc::new(move || Self::service_input(&inner)));
        }
        view.apply(StartupUpdate {
            stage: StartupStage::Initialization,
            state: StartupStageState::Running,
            detail: "starting".to_string(),
        });
        Ok(view)
    }

    pub(crate) fn observer(&self) -> StartupObserver {
        let inner = Arc::clone(&self.inner);
        Arc::new(move |update| Self::apply_inner(&inner, update))
    }

    pub(crate) fn interrupted(&self) -> bool {
        self.inner.lock().is_ok_and(|inner| inner.interrupted)
    }

    pub(crate) fn fail(&self, detail: impl Into<String>) {
        let detail = detail.into();
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.rows.iter().any(|row| row.state == State::Failed) {
            return;
        }
        let index = inner
            .rows
            .iter()
            .rev()
            .position(|row| row.state == State::Running)
            .map(|offset| inner.rows.len() - offset - 1)
            .or_else(|| {
                inner
                    .rows
                    .iter()
                    .position(|row| row.stage == StartupStage::Initialization)
            });
        if let Some(row) = index.and_then(|index| inner.rows.get_mut(index)) {
            row.state = State::Failed;
            row.detail = detail;
        }
        Self::render(&mut inner);
    }

    fn lines(rows: &[Row]) -> Vec<Line> {
        rows.iter()
            .map(|row| {
                let (marker, tone) = match row.state {
                    State::Pending => ("[ ]", Tone::Muted),
                    State::Running => ("[~]", Tone::Warn),
                    State::Done => ("[+]", Tone::Pass),
                    State::Failed => ("[x]", Tone::Fail),
                };
                let mut line = Line::blank();
                line.push(format!("{marker} {:<16} ", row.label), tone);
                line.push(
                    &row.detail,
                    if row.state == State::Failed {
                        Tone::Fail
                    } else {
                        Tone::Muted
                    },
                );
                line
            })
            .collect()
    }

    fn render(inner: &mut Inner) {
        let rows = Self::lines(&inner.rows);
        let detail = inner
            .rows
            .iter()
            .rev()
            .find(|row| row.state == State::Running || row.state == State::Failed)
            .map(|row| row.detail.clone())
            .unwrap_or_else(|| "Preparing run".to_string());
        if let Some(pane) = inner.pane.as_mut() {
            let _ = pane.draw(|frame| {
                let tree = PaneTree::Split {
                    dir: Direction::Horizontal,
                    children: vec![
                        (
                            Constraint::Percentage(70),
                            PaneTree::Leaf {
                                id: "startup-stages",
                                title: "Run startup".to_string(),
                            },
                        ),
                        (
                            Constraint::Percentage(30),
                            PaneTree::Leaf {
                                id: "startup-detail",
                                title: "Detail".to_string(),
                            },
                        ),
                    ],
                };
                let layout = tree.resolve(frame.area());
                for id in tree.leaf_ids() {
                    if let Some(rect) = layout.rect(id) {
                        let area = tui_panes::render_pane(
                            frame,
                            rect,
                            tree.title(id).unwrap_or(""),
                            id == "startup-stages",
                        );
                        let lines = if id == "startup-stages" {
                            rows.iter().map(tui_ratatui::render_line).collect()
                        } else {
                            vec![tui_ratatui::render_line(&{
                                let mut line = Line::blank();
                                line.push(detail.clone(), Tone::Default);
                                line
                            })]
                        };
                        tui_panes::render_lines_pane(frame, area, &lines, Default::default());
                    }
                }
            });
        }
    }

    fn service_input(shared: &Arc<Mutex<Inner>>) {
        let Ok(mut inner) = shared.lock() else {
            return;
        };
        let mut redraw = false;
        let mut retry_resize = false;
        let mut interrupted = false;
        if let Some(pane) = inner.pane.as_mut() {
            redraw = pane.apply_resize();
            retry_resize = pane.resize_pending() && !redraw;
            interrupted = pane.poll_startup_interrupt();
        }
        if interrupted {
            inner.interrupted = true;
            if let Some(row) = inner
                .rows
                .iter_mut()
                .rev()
                .find(|row| row.state == State::Running)
            {
                row.state = State::Failed;
                row.detail = "interrupted".to_string();
            }
            // A synchronous startup operation (notably config loading) may
            // be blocked indefinitely. Commit and restore from the input
            // pump itself so Ctrl-C never strands raw mode waiting for that
            // operation to return.
            let lines = Self::lines(&inner.rows);
            if let Some(pane) = inner.pane.as_mut() {
                let _ = pane.commit_inline_scrollback(&lines);
                pane.quit();
            }
            eprintln!("run startup interrupted; terminal restored");
            std::process::exit(130);
        }
        if redraw {
            Self::render(&mut inner);
        }
        drop(inner);
        if retry_resize {
            let inner = Arc::clone(shared);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(25));
                Self::service_input(&inner);
            });
        }
    }

    pub(crate) fn into_pane(self) -> Option<RatatuiPane> {
        self.inner.lock().ok()?.pane.take()
    }

    fn apply(&self, update: StartupUpdate) {
        Self::apply_inner(&self.inner, update);
    }

    fn apply_inner(inner: &Arc<Mutex<Inner>>, update: StartupUpdate) {
        let Ok(mut inner) = inner.lock() else {
            return;
        };
        // Startup initialization is synchronous, so an observer can still
        // complete work after Ctrl-C was received on the pane input thread.
        // The interrupted row is the terminal disposition and must survive
        // until Drop projects it into inline scrollback.
        if inner.interrupted {
            return;
        }
        if let Some(row) = inner.rows.iter_mut().find(|row| row.stage == update.stage) {
            // Observer callbacks can report a late setup detail after a
            // substage completed. Terminal states are monotonic: accept a
            // Done detail refresh, but never reopen completion or overwrite a
            // concrete failure.
            if row.state == State::Failed
                || (row.state == State::Done && update.state == StartupStageState::Running)
            {
                return;
            }
            row.state = match update.state {
                StartupStageState::Running => State::Running,
                StartupStageState::Done => State::Done,
                StartupStageState::Failed => State::Failed,
            };
            row.detail = update.detail;
        }
        Self::render(&mut inner);
    }
}

impl Drop for StartupView {
    fn drop(&mut self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if !inner.interrupted
            && !inner.rows.iter().any(|row| row.state == State::Failed)
            && let Some(row) = inner
                .rows
                .iter_mut()
                .rev()
                .find(|row| row.state == State::Running)
        {
            row.state = State::Failed;
            row.detail = "startup did not complete".to_string();
        }
        let lines = Self::lines(&inner.rows);
        if let Some(mut pane) = inner.pane.take() {
            // Decode warnings originate in the trait document. Do not project
            // them into the terminal until the trust row has completed.
            if !inner
                .rows
                .iter()
                .any(|row| row.stage == StartupStage::Trust && row.state == State::Done)
            {
                let _ = ctx_traits_io::decode_diagnostics::end_capture();
            }
            let _ = pane.commit_inline_scrollback(&lines);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_failure_marks_the_active_stage_once() {
        let view = StartupView {
            inner: Arc::new(Mutex::new(Inner {
                pane: None,
                interrupted: false,
                rows: vec![
                    Row {
                        stage: StartupStage::Initialization,
                        label: "Initialization",
                        state: State::Done,
                        detail: "ready".to_string(),
                    },
                    Row {
                        stage: StartupStage::Harness,
                        label: "Harness probe",
                        state: State::Running,
                        detail: "probing".to_string(),
                    },
                ],
            })),
        };
        view.fail("probe refused");
        view.fail("later wrapper error");

        let inner = view.inner.lock().unwrap();
        assert_eq!(inner.rows[0].state, State::Done);
        assert_eq!(inner.rows[1].state, State::Failed);
        assert_eq!(inner.rows[1].detail, "probe refused");
    }

    #[test]
    fn updates_replace_detail_and_preserve_stage_order() {
        let view = StartupView {
            inner: Arc::new(Mutex::new(Inner {
                pane: None,
                interrupted: false,
                rows: [
                    (StartupStage::Initialization, "Initialization"),
                    (StartupStage::Trust, "Trust gate"),
                ]
                .into_iter()
                .map(|(stage, label)| Row {
                    stage,
                    label,
                    state: State::Pending,
                    detail: "pending".to_string(),
                })
                .collect(),
            })),
        };
        view.apply(StartupUpdate {
            stage: StartupStage::Trust,
            state: StartupStageState::Running,
            detail: "checking".to_string(),
        });
        view.apply(StartupUpdate {
            stage: StartupStage::Trust,
            state: StartupStageState::Done,
            detail: "approved".to_string(),
        });

        let inner = view.inner.lock().unwrap();
        assert_eq!(inner.rows[0].state, State::Pending);
        assert_eq!(inner.rows[1].state, State::Done);
        assert_eq!(inner.rows[1].detail, "approved");
    }

    #[test]
    fn completed_rows_are_not_reopened_by_stale_updates() {
        let view = StartupView {
            inner: Arc::new(Mutex::new(Inner {
                pane: None,
                interrupted: false,
                rows: vec![Row {
                    stage: StartupStage::Worktree,
                    label: "Worktree",
                    state: State::Done,
                    detail: "prepared".to_string(),
                }],
            })),
        };
        view.apply(StartupUpdate {
            stage: StartupStage::Worktree,
            state: StartupStageState::Running,
            detail: "stale setup detail".to_string(),
        });

        let inner = view.inner.lock().unwrap();
        assert_eq!(inner.rows[0].state, State::Done);
        assert_eq!(inner.rows[0].detail, "prepared");
    }

    #[test]
    fn interruption_prevents_later_observer_updates_from_erasing_failure() {
        let view = StartupView {
            inner: Arc::new(Mutex::new(Inner {
                pane: None,
                interrupted: true,
                rows: vec![Row {
                    stage: StartupStage::Trust,
                    label: "Trust gate",
                    state: State::Failed,
                    detail: "interrupted".to_string(),
                }],
            })),
        };
        view.apply(StartupUpdate {
            stage: StartupStage::Trust,
            state: StartupStageState::Done,
            detail: "approved".to_string(),
        });

        let inner = view.inner.lock().unwrap();
        assert_eq!(inner.rows[0].state, State::Failed);
        assert_eq!(inner.rows[0].detail, "interrupted");
    }

    #[test]
    fn failure_after_harness_completion_targets_initialization() {
        let view = StartupView {
            inner: Arc::new(Mutex::new(Inner {
                pane: None,
                interrupted: false,
                rows: vec![
                    Row {
                        stage: StartupStage::Initialization,
                        label: "Initialization",
                        state: State::Running,
                        detail: "writing session".to_string(),
                    },
                    Row {
                        stage: StartupStage::Harness,
                        label: "Harness probe",
                        state: State::Done,
                        detail: "ready".to_string(),
                    },
                ],
            })),
        };
        view.apply(StartupUpdate {
            stage: StartupStage::Initialization,
            state: StartupStageState::Failed,
            detail: "could not write session".to_string(),
        });

        let inner = view.inner.lock().unwrap();
        assert_eq!(inner.rows[0].state, State::Failed);
        assert_eq!(inner.rows[1].state, State::Done);
    }
}
