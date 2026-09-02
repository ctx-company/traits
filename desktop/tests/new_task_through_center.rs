//! Wire-level proof for the Tasks bar's create dispatcher.

mod support;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ctx_traits_core::task::TaskStatus;
use ctx_traits_core::task::graph::DerivedStatus;
use ctx_traits_core::task::provider::TaskSummary;
use ctx_traits_desktop::board::{CreateOutcome, CreateRequest, NewTaskEntry, dispatch_create};
use ctx_traits_desktop::shell::Shell;
use ctx_traits_io::center::CreateTaskWireResult;
use ctx_traits_io::task_files::{BoardPresence, BoardResolution, BoardRow};
use gpui::AppContext;

static CENTER_ENV_LOCK: Mutex<()> = Mutex::new(());

fn empty_board() -> ctx_traits_io::center::BoardWireResult {
    ctx_traits_io::center::BoardWireResult {
        resolution: BoardResolution {
            presence: BoardPresence::Empty,
            digest: Some("sha256:fixture".to_string()),
            rows: Vec::new(),
            sync_report: Default::default(),
            resolved_at: 0,
        },
        joined_runs: Default::default(),
        sections: Default::default(),
    }
}

fn board_with_titles(titles: &[&str]) -> ctx_traits_io::center::BoardWireResult {
    let rows = titles
        .iter()
        .enumerate()
        .map(|(index, title)| BoardRow {
            summary: TaskSummary {
                key: format!("draft-{}", index + 1),
                title: (*title).to_string(),
                stored_status: Some(TaskStatus::Draft),
                derived_status: DerivedStatus::Ready,
                archived: false,
            },
            relations: Default::default(),
            unmet_dependencies: Vec::new(),
            digest: format!("sha256:{index}"),
            short_description: String::new(),
        })
        .collect::<Vec<_>>();
    let sections = rows
        .iter()
        .map(|row| {
            (
                row.summary.key.clone(),
                Some(ctx_traits_core::task::provider::BoardSection::Draft),
            )
        })
        .collect::<BTreeMap<_, _>>();
    ctx_traits_io::center::BoardWireResult {
        resolution: BoardResolution {
            presence: BoardPresence::Loaded,
            digest: Some("sha256:changed".to_string()),
            rows,
            sync_report: Default::default(),
            resolved_at: 1,
        },
        joined_runs: Default::default(),
        sections,
    }
}

enum CreateDriverCommand {
    Reply(CreateTaskWireResult),
    BoardThenHoldReply(ctx_traits_io::center::BoardWireResult, CreateTaskWireResult),
    ReleaseReply,
    Close,
}

enum CreateDriverStart {
    Serve,
    Stop,
}

fn activate_and_submit(
    visual: &mut gpui::VisualTestContext,
    window: gpui::AnyWindowHandle,
    cx: &mut gpui::TestAppContext,
    title: &str,
) {
    let bounds = visual
        .debug_bounds("bottom-bar-action-NewTask")
        .expect("the NewTask action is rendered");
    visual.simulate_click(
        gpui::point(
            bounds.origin.x + bounds.size.width / 2.,
            bounds.origin.y + bounds.size.height / 2.,
        ),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    for character in title.chars() {
        let key = match character {
            ' ' => "space".to_string(),
            character => character.to_string(),
        };
        cx.dispatch_keystroke(window, gpui::Keystroke::parse(&key).unwrap());
        cx.run_until_parked();
    }
    cx.dispatch_keystroke(window, gpui::Keystroke::parse("enter").unwrap());
}

fn wait_for(
    shell: &gpui::Entity<Shell>,
    cx: &mut gpui::TestAppContext,
    predicate: impl Fn(&Shell) -> bool,
    expectation: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.background_executor.tick();
        if shell.read_with(cx, |shell, _cx| predicate(shell)) {
            return;
        }
        assert!(Instant::now() < deadline, "{expectation}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui::test]
fn rendered_action_opens_entry_and_escape_cancels(cx: &mut gpui::TestAppContext) {
    let shell = cx.update(|cx| cx.new(Shell::new_for_test));
    let window = cx
        .update(|cx| {
            let shell = shell.clone();
            cx.open_window(Default::default(), move |_, _| shell)
        })
        .unwrap();
    window
        .update(cx, |shell, _window, cx| {
            shell.set_board_for_test(
                "repo-a".to_string(),
                "/repo-a".to_string(),
                empty_board(),
                cx,
            );
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let bounds = visual.debug_bounds("bottom-bar-action-NewTask").unwrap();
    visual.simulate_click(
        gpui::point(
            bounds.origin.x + bounds.size.width / 2.,
            bounds.origin.y + bounds.size.height / 2.,
        ),
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    for key in ["o", "w", "n", "e", "r", "escape"] {
        cx.dispatch_keystroke(*window, gpui::Keystroke::parse(key).unwrap());
        cx.run_until_parked();
    }
    assert_eq!(
        window
            .update(cx, |shell, _window, _cx| shell
                .new_task_action_for_test()
                .label)
            .unwrap(),
        "new task"
    );
}

#[test]
fn create_dispatch_preserves_owner_payload_and_typed_refusals() {
    let _environment = CENTER_ENV_LOCK.lock().expect("lock center environment");
    let guard = support::scratch("new-task-through-center");
    let socket = unsafe { support::install_center_env(&guard.0) };
    let peer = support::FakePeer::bind(&socket);

    let request = CreateRequest {
        repo_key: "repo-a".to_string(),
        title: "owner title".to_string(),
        generation: 1,
        board_scope_generation: 1,
    };
    let create = std::thread::spawn(move || dispatch_create(request));
    let mut connection = peer.accept();
    let (id, repo_key, task) = connection.read_create_task_request();
    assert_eq!(repo_key, "repo-a");
    assert_eq!(task["title"], "owner title");
    assert_eq!(task["status"], "draft");
    assert!(task["parent"].is_null());
    connection.send_create_task_result(&id, &CreateTaskWireResult::Occupied);

    let mut entry = NewTaskEntry::default();
    entry.activate();
    entry.insert_char('x');
    let pending = entry.submit("repo-a".to_string(), 1, 1).unwrap();
    assert!(entry.settle(
        &pending.repo_key,
        pending.generation,
        pending.board_scope_generation,
        create.join().expect("create thread"),
    ));
    assert!(entry.action().label.starts_with("refused: "));
}

#[test]
fn each_typed_refusal_and_transport_failure_stays_distinct() {
    let refusals = [
        CreateTaskWireResult::Occupied,
        CreateTaskWireResult::InvalidField {
            field: "title".to_string(),
            reason: "blank".to_string(),
        },
        CreateTaskWireResult::UnknownParent {
            parent: "0001".to_string(),
        },
        CreateTaskWireResult::AmbiguousParent {
            parent: "0001".to_string(),
        },
        CreateTaskWireResult::BoardAbsent,
        CreateTaskWireResult::BoardUnreadable {
            reason: "bad toml".to_string(),
        },
    ];
    let mut messages = Vec::new();
    for refusal in refusals {
        let mut entry = NewTaskEntry::default();
        entry.activate();
        entry.insert_char('x');
        let request = entry.submit("repo-a".to_string(), 1, 1).unwrap();
        assert!(entry.settle(
            &request.repo_key,
            request.generation,
            request.board_scope_generation,
            CreateOutcome::Result(refusal),
        ));
        messages.push(entry.action().label);
    }
    let mut entry = NewTaskEntry::default();
    entry.activate();
    entry.insert_char('x');
    let request = entry.submit("repo-a".to_string(), 1, 1).unwrap();
    assert!(entry.settle(
        &request.repo_key,
        request.generation,
        request.board_scope_generation,
        CreateOutcome::Failed("connection closed".to_string()),
    ));
    messages.push(entry.action().label);
    messages.sort();
    messages.dedup();
    assert_eq!(messages.len(), 7);
}

#[gpui::test]
fn rendered_shell_dispatch_preserves_create_contract_across_ordering_refusals_and_repo_switches(
    cx: &mut gpui::TestAppContext,
) {
    let _environment = CENTER_ENV_LOCK.lock().expect("lock center environment");
    let guard = support::scratch("new-task-shell-contract");
    let socket = unsafe { support::install_center_env(&guard.0) };
    let peer = support::FakePeer::bind(&socket);
    let shell = cx.update(|cx| cx.new(Shell::new));
    let window = cx
        .update(|cx| {
            let shell = shell.clone();
            cx.open_window(Default::default(), move |_, _| shell)
        })
        .unwrap();
    let mut subscription = peer.accept();
    let subscription_id = subscription.read_subscribe_id();
    subscription.serve_snapshot(&subscription_id, &[]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _window, cx| {
            shell.set_board_for_test(
                "repo-a".to_string(),
                "/repo-a".to_string(),
                empty_board(),
                cx,
            );
        })
        .unwrap();

    // Keep the subscription in this test thread while a dedicated driver owns
    // each request socket. This lets the UI keep running while a reply waits.
    let (requests_tx, requests_rx) = mpsc::channel();
    let (starts_tx, starts_rx) = mpsc::channel();
    let (commands_tx, commands_rx) = mpsc::channel();
    let (board_sent_tx, board_sent_rx) = mpsc::channel();
    let driver = std::thread::spawn(move || {
        while matches!(starts_rx.recv(), Ok(CreateDriverStart::Serve)) {
            let mut connection = peer.accept();
            let (id, repo_key, task) = connection.read_create_task_request();
            requests_tx
                .send((repo_key.clone(), task))
                .expect("test receives create payload");
            match commands_rx.recv().expect("test supplies create response") {
                CreateDriverCommand::Reply(result) => {
                    connection.send_create_task_result(&id, &result)
                }
                CreateDriverCommand::BoardThenHoldReply(board, result) => {
                    subscription.send_board_changed(&repo_key, &board);
                    board_sent_tx
                        .send(())
                        .expect("test observes subscription BoardChanged");
                    match commands_rx.recv().expect("test releases create response") {
                        CreateDriverCommand::ReleaseReply => {
                            connection.send_create_task_result(&id, &result);
                        }
                        _ => panic!("the held create response must be released explicitly"),
                    }
                }
                CreateDriverCommand::Close => connection.shutdown(),
                CreateDriverCommand::ReleaseReply => {
                    panic!("a response release is valid only after BoardChanged")
                }
            }
        }
    });

    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);

    // The response settles first, but does not create an optimistic board row.
    starts_tx.send(CreateDriverStart::Serve).unwrap();
    activate_and_submit(&mut visual, *window, cx, "owner title");
    // A second Enter while the first request has no reply must not open a
    // second request socket.
    cx.dispatch_keystroke(*window, gpui::Keystroke::parse("enter").unwrap());
    commands_tx
        .send(CreateDriverCommand::Reply(CreateTaskWireResult::Created(
            TaskSummary {
                key: "draft-1".to_string(),
                title: "owner title".to_string(),
                stored_status: Some(TaskStatus::Draft),
                derived_status: DerivedStatus::Ready,
                archived: false,
            },
        )))
        .unwrap();
    wait_for(
        &shell,
        cx,
        |shell| shell.new_task_action_for_test().label == "new task",
        "the successful reply settles the entry",
    );
    assert_eq!(
        requests_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first create request"),
        (
            "repo-a".to_string(),
            serde_json::json!({
                "title": "owner title",
                "content": "",
                "status": "draft",
                "depends-on": [],
                "parent": null,
                "validation": "",
                "steps": [],
            })
        ),
        "the rendered binding sends exactly the owner title, active identity, and canonical Draft payload"
    );
    assert!(
        requests_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "two rendered Enter keys while unresolved permit exactly one wire request"
    );
    assert!(
        !window
            .update(cx, |shell, _window, _cx| shell
                .board_has_task_for_test("owner title"))
            .unwrap(),
        "the create response itself must not change the accepted board"
    );

    // Hold the reply after the actual subscription write so this asserts the
    // reducer ordering rather than merely the eventual state after two
    // independently scheduled writes.
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    starts_tx.send(CreateDriverStart::Serve).unwrap();
    activate_and_submit(&mut visual, *window, cx, "broadcast first");
    assert_eq!(
        window
            .update(cx, |shell, _window, _cx| shell
                .new_task_action_for_test()
                .label)
            .unwrap(),
        "creating",
        "the rendered handler must install its pending affordance before BoardChanged"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let broadcast_request = loop {
        match requests_rx.try_recv() {
            Ok(request) => break request,
            Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                cx.background_executor.tick();
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("second create request did not arrive: {error}"),
        }
    };
    commands_tx
        .send(CreateDriverCommand::BoardThenHoldReply(
            board_with_titles(&["owner title", "broadcast first"]),
            CreateTaskWireResult::Created(TaskSummary {
                key: "draft-2".to_string(),
                title: "broadcast first".to_string(),
                stored_status: Some(TaskStatus::Draft),
                derived_status: DerivedStatus::Ready,
                archived: false,
            }),
        ))
        .unwrap();
    board_sent_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("driver sends BoardChanged through the retained subscription");
    wait_for(
        &shell,
        cx,
        |shell| {
            shell.board_has_task_for_test("broadcast first")
                && shell.new_task_action_for_test().label == "new task"
        },
        "the Shell must accept subscription BoardChanged and clear pending before the held reply runs",
    );
    assert!(
        shell.read_with(cx, |shell, _cx| shell.new_task_awaits_result_for_test()),
        "the held response must still be correlated after BoardChanged clears the affordance"
    );
    let board_before_reply = shell.read_with(cx, |shell, _cx| {
        (
            shell.board_answer_for_test(),
            shell.new_task_action_for_test().label,
        )
    });
    commands_tx.send(CreateDriverCommand::ReleaseReply).unwrap();
    wait_for(
        &shell,
        cx,
        |shell| !shell.new_task_awaits_result_for_test(),
        "the late success reply must complete its Shell handoff",
    );
    assert_eq!(
        shell.read_with(cx, |shell, _cx| {
            (
                shell.board_answer_for_test(),
                shell.new_task_action_for_test().label,
            )
        }),
        board_before_reply,
        "the late success reply must not regress the board accepted from BoardChanged"
    );
    let (repo_key, task) = broadcast_request;
    assert_eq!(repo_key, "repo-a");
    assert_eq!(task["title"], "broadcast first");

    let refusals = [
        (
            CreateTaskWireResult::Occupied,
            "another task creation is in progress",
        ),
        (
            CreateTaskWireResult::InvalidField {
                field: "title".to_string(),
                reason: "blank".to_string(),
            },
            "invalid title: blank",
        ),
        (
            CreateTaskWireResult::UnknownParent {
                parent: "0001".to_string(),
            },
            "no task \"0001\" in the board",
        ),
        (
            CreateTaskWireResult::AmbiguousParent {
                parent: "0001".to_string(),
            },
            "parent \"0001\" is ambiguous",
        ),
        (CreateTaskWireResult::BoardAbsent, "task board is absent"),
        (
            CreateTaskWireResult::BoardUnreadable {
                reason: "bad toml".to_string(),
            },
            "task board is unreadable: bad toml",
        ),
    ];
    for (index, (refusal, message)) in refusals.into_iter().enumerate() {
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        starts_tx.send(CreateDriverStart::Serve).unwrap();
        activate_and_submit(&mut visual, *window, cx, &format!("refusal {index}"));
        commands_tx
            .send(CreateDriverCommand::Reply(refusal))
            .unwrap();
        wait_for(
            &shell,
            cx,
            |shell| shell.new_task_action_for_test().label == format!("refused: {message}"),
            "each typed refusal must be visible and distinct through the Shell path",
        );
        requests_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("refusal create request");
    }

    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    starts_tx.send(CreateDriverStart::Serve).unwrap();
    activate_and_submit(&mut visual, *window, cx, "transport failure");
    commands_tx.send(CreateDriverCommand::Close).unwrap();
    wait_for(
        &shell,
        cx,
        |shell| {
            shell
                .new_task_action_for_test()
                .label
                .starts_with("refused: ")
        },
        "a closed create connection must render the outer client failure",
    );
    requests_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("transport-failure create request");

    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    starts_tx.send(CreateDriverStart::Serve).unwrap();
    activate_and_submit(&mut visual, *window, cx, "old repository");
    let deadline = Instant::now() + Duration::from_secs(5);
    let old_repository_request = loop {
        match requests_rx.try_recv() {
            Ok(request) => break request,
            Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                cx.background_executor.tick();
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("old-repository create request did not arrive: {error}"),
        }
    };
    window
        .update(cx, |shell, _window, cx| {
            shell.set_board_for_test(
                "repo-b".to_string(),
                "/repo-b".to_string(),
                empty_board(),
                cx,
            );
        })
        .unwrap();
    commands_tx
        .send(CreateDriverCommand::Reply(CreateTaskWireResult::Occupied))
        .unwrap();
    wait_for(
        &shell,
        cx,
        |shell| shell.new_task_action_for_test().label == "new task",
        "a reply for the old repository must not surface on the active board",
    );
    assert!(
        !window
            .update(cx, |shell, _window, _cx| shell
                .board_has_task_for_test("old repository"))
            .unwrap(),
        "a late old-repository result must not alter the active board"
    );
    assert_eq!(old_repository_request.0, "repo-a");
    starts_tx.send(CreateDriverStart::Stop).unwrap();
    driver.join().expect("create driver exits");
}
