//! Real-center proof for the desktop create transport. It deliberately uses
//! two independent desktop subscriptions: the request response is not treated
//! as a board update, so both links must receive the later BoardChanged.

mod support;

use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use ctx_traits_desktop::center_link::LinkUpdate;
use ctx_traits_desktop::shell::Shell;
use gpui::AppContext;

fn await_socket(socket: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match UnixStream::connect(socket) {
            Ok(_) => return,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => panic!("center did not become ready at {socket:?}: {error}"),
        }
    }
}

fn recv_snapshot(updates: &async_channel::Receiver<LinkUpdate>) -> (String, String) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Snapshot(rows)) => {
                return (rows[0].repo_key.clone(), rows[0].repo_path.clone());
            }
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no center snapshot arrived: {error}"),
        }
    }
}

fn recv_board(updates: &async_channel::Receiver<LinkUpdate>, repo_key: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match updates.try_recv() {
            Ok(LinkUpdate::Board {
                repo_key: received,
                board,
            }) if received == repo_key => {
                assert!(
                    board
                        .resolution
                        .rows
                        .iter()
                        .any(|row| row.summary.title == "real center title"),
                    "the created row must be carried by BoardChanged, not the create response"
                );
                return;
            }
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("no board update arrived: {error}"),
        }
    }
}

#[gpui::test]
fn creation_broadcasts_to_the_shell_reducer_and_a_second_link_and_whitespace_is_visible(
    cx: &mut gpui::TestAppContext,
) {
    let guard = support::scratch("new-task-real-center");
    let root = guard.0.clone();
    let socket = unsafe { support::install_center_env(&root) };
    unsafe {
        std::env::set_var("CTX_CENTER_SCAN_MS", "20");
        std::env::set_var("CTX_CENTER_IDLE_MS", "30000");
        std::env::set_var("HOME", &root);
    }

    let repository = root.join("repository");
    std::fs::create_dir_all(&repository).expect("create scratch repository");
    let status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repository)
        .status()
        .expect("start git init");
    assert!(status.success(), "scratch repository must initialize");
    std::fs::create_dir_all(repository.join(".internal/tasks")).expect("create empty board");
    let ledger = Utf8PathBuf::from_path_buf(repository.join("session.json")).expect("UTF-8 ledger");
    let session = support::write_session_ledger(&ledger, "create-session", "create-run", true);
    let mut session_json = serde_json::to_value(session).expect("serialize fixture session");
    session_json["provenance"]["worktree"] = serde_json::json!({
        "id": "create-worktree",
        "branch": "ctx/test/create",
        "path": repository,
    });
    let session = serde_json::from_value(session_json).expect("fixture session with repository");
    ctx_traits_io::run_session::write_run_session(&ledger, &session).expect("rewrite fixture");
    std::env::set_current_dir(&repository).expect("enter scratch repository");
    let indexed_key = ctx_traits_io::state::touch_repo_index().expect("index scratch repository");

    std::thread::spawn(|| {
        let _ = ctx_traits_io::center::run_server();
    });
    await_socket(&socket);

    let second = ctx_traits_desktop::center_link::start(None);
    let (_snapshot_key, repo_path) = recv_snapshot(&second);
    assert!(
        repo_path.is_empty(),
        "the fixture row has no desktop-owned path"
    );
    let repo_key = indexed_key;
    let window = cx
        .update(|cx| cx.open_window(Default::default(), |_, cx| cx.new(Shell::new)))
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _window, cx| {
            shell.set_board_for_test(
                repo_key.clone(),
                repository.to_string_lossy().to_string(),
                ctx_traits_io::center::BoardWireResult {
                    resolution: ctx_traits_io::task_files::BoardResolution {
                        presence: ctx_traits_io::task_files::BoardPresence::Empty,
                        digest: None,
                        rows: Vec::new(),
                        sync_report: Default::default(),
                        resolved_at: 0,
                    },
                    joined_runs: Default::default(),
                    sections: Default::default(),
                },
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let action_bounds = visual
        .debug_bounds("bottom-bar-action-NewTask")
        .expect("the rendered NewTask action is available");
    visual.simulate_click(
        gpui::point(
            action_bounds.origin.x + action_bounds.size.width / 2.,
            action_bounds.origin.y + action_bounds.size.height / 2.,
        ),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    for key in [
        "r", "e", "a", "l", "space", "c", "e", "n", "t", "e", "r", "space", "t", "i", "t", "l",
        "e", "enter",
    ] {
        cx.dispatch_keystroke(*window, gpui::Keystroke::parse(key).unwrap());
        cx.run_until_parked();
    }

    recv_board(&second, &repo_key);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if window
            .update(cx, |shell, _window, _cx| {
                shell.board_has_task_for_test("real center title")
            })
            .unwrap()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the Shell reducer did not accept the BoardChanged answer"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    assert!(
        visual.debug_bounds("tasks-section-Draft").is_some(),
        "the accepted board must render its Draft section"
    );
    assert!(
        visual.debug_bounds("task-status-draft").is_some(),
        "the created row must render its Draft status"
    );

    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let action_bounds = visual
        .debug_bounds("bottom-bar-action-NewTask")
        .expect("the rendered NewTask action is available after settlement");
    visual.simulate_click(
        gpui::point(
            action_bounds.origin.x + action_bounds.size.width / 2.,
            action_bounds.origin.y + action_bounds.size.height / 2.,
        ),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    for key in ["space", "space", "space", "enter"] {
        cx.dispatch_keystroke(*window, gpui::Keystroke::parse(key).unwrap());
        cx.run_until_parked();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        let label = window
            .update(cx, |shell, _window, _cx| {
                shell.new_task_action_for_test().label
            })
            .unwrap();
        if label == "refused: invalid title: title must not be empty or whitespace-only" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "whitespace-only title must render the provider InvalidField refusal, got {label:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
