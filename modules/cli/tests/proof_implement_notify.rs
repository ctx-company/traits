//! Declarative notify narration proofs (owner ruling 2026-09-01,
//! superseding 0273's script-wrapper suite): the basic implement canonical
//! narrates through bare `ctx-notify` argv steps — no shell, no embedded
//! programs — with the review round rendered by a scribe digest whose
//! fields are projected into text slots and fed to the commands verbatim.
//! Structural facts only, read from the regenerated canonical; counts,
//! order, and argv properties — never a whole-file golden. Failure
//! tolerance is explicitly out (MVP ruling): a notifier failure fails its
//! step, so there is no degradation behavior left to prove.

use support::repo_root;

fn basic_canonical() -> toml::Table {
    let path = repo_root().join(".ctx/traits/authored/implement/generated/basic/index.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    text.parse::<toml::Table>()
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn ids(items: &[toml::Value]) -> Vec<&str> {
    items
        .iter()
        .map(|item| {
            item.get("id")
                .and_then(toml::Value::as_str)
                .expect("sequence item id")
        })
        .collect()
}

fn index_of(ids: &[&str], wanted: &str) -> usize {
    ids.iter()
        .position(|id| *id == wanted)
        .unwrap_or_else(|| panic!("id {wanted} not found in {ids:?}"))
}

fn all_pools(canonical: &toml::Table) -> Vec<&Vec<toml::Value>> {
    let mut pools = Vec::new();
    pools.push(
        canonical
            .get("procedure")
            .and_then(|p| p.get("sequence"))
            .and_then(toml::Value::as_array)
            .expect("procedure.sequence"),
    );
    if let Some(named) = canonical.get("sequence").and_then(toml::Value::as_table) {
        for sub in named.values() {
            if let Some(items) = sub.get("sequence").and_then(toml::Value::as_array) {
                pools.push(items);
            }
        }
    }
    pools
}

fn argv_of(canonical: &toml::Table, wanted: &str) -> Vec<String> {
    for pool in all_pools(canonical) {
        for item in pool {
            if item.get("id").and_then(toml::Value::as_str) == Some(wanted) {
                return item
                    .get("command")
                    .and_then(|c| c.get("argv"))
                    .and_then(toml::Value::as_array)
                    .unwrap_or_else(|| panic!("step {wanted} has no command argv"))
                    .iter()
                    .map(|token| token.as_str().expect("argv token").to_string())
                    .collect();
            }
        }
    }
    panic!("step {wanted} not found in any sequence pool");
}

/// The named sequence pool (loop body or branch arm) that holds the step
/// `wanted` — the stage loop nests its work under a stage-open arm, so a
/// site's neighbourhood is found by its step, not by a fixed pool name.
fn pool_containing<'a>(canonical: &'a toml::Table, wanted: &str) -> Vec<&'a str> {
    let named = canonical
        .get("sequence")
        .and_then(toml::Value::as_table)
        .expect("named sequences");
    named
        .values()
        .filter_map(|sub| sub.get("sequence").and_then(toml::Value::as_array))
        .map(|items| ids(items))
        .find(|items| items.contains(&wanted))
        .unwrap_or_else(|| panic!("no named sequence contains {wanted}"))
}

#[test]
fn narration_sits_at_the_contract_sites_in_order() {
    let canonical = basic_canonical();
    let top = canonical
        .get("procedure")
        .and_then(|p| p.get("sequence"))
        .and_then(toml::Value::as_array)
        .expect("procedure.sequence");
    let top_ids = ids(top);

    assert_eq!(
        top_ids[0], "notify-begin",
        "begin opens the run: {top_ids:?}"
    );
    let baseline = index_of(&top_ids, "capture-the-session-base");
    assert_eq!(
        top_ids[baseline - 1],
        "notify-session-base",
        "status announces the step it precedes"
    );
    // The plan is drafted inside the plan-approval loop; the status line
    // announces the loop that contains it.
    let plan = index_of(&top_ids, "plan-approval");
    assert_eq!(top_ids[plan - 1], "notify-plan-drafted");
    let review_baseline = index_of(&top_ids, "review-the-tree-before-any-work");
    assert_eq!(top_ids[review_baseline - 1], "notify-baseline-review");
    assert!(index_of(&top_ids, "stage-loop") > review_baseline);
    let finish = index_of(&top_ids, "notify-finish");
    assert!(finish > index_of(&top_ids, "maybe-commit"));
    assert_eq!(
        finish,
        top_ids.len() - 1,
        "finish closes the run: {top_ids:?}"
    );

    // The stage-open arm: one status line announces the work loop, and the
    // review round's narration follows the review step in contract order,
    // before the annotate gate and the ruling branch.
    let arm_ids = pool_containing(&canonical, "review-the-claim");
    let work = index_of(&arm_ids, "work-the-stage");
    assert_eq!(
        arm_ids[work - 1],
        "notify-working-the-stage",
        "status announces the work loop it precedes"
    );
    let review = index_of(&arm_ids, "review-the-claim");
    assert_eq!(arm_ids[review + 1], "notify-digest-verdict");
    assert_eq!(arm_ids[review + 2], "notify-carry-digest");
    assert_eq!(arm_ids[review + 3], "notify-review-update");
    assert_eq!(arm_ids[review + 4], "notify-review-journal");
    assert_eq!(arm_ids[review + 5], "notify-awaiting-annotations");
    assert_eq!(arm_ids[review + 6], "gate-carry-surface");
    assert_eq!(arm_ids[review + 7], "owner-verdict-gate");
    assert_eq!(arm_ids[review + 8], "record-the-owner-ruling");
    assert_eq!(arm_ids[review + 9], "notify-gate-result");
    assert!(
        index_of(&arm_ids, "owner-ruling") > review + 9,
        "narration and the annotate gate precede the ruling branch"
    );

    // Both commit phases are announced on entry: the stage commit right
    // after the closed stage is carried, the final commit first thing.
    let stage_commit = pool_containing(&canonical, "commit-the-stage");
    assert_eq!(
        stage_commit[0], "carry-closed-stage",
        "stage commit arm: {stage_commit:?}"
    );
    assert_eq!(stage_commit[1], "notify-stage-committed");
    let commit_arm = pool_containing(&canonical, "commit-the-work");
    assert_eq!(
        commit_arm[0], "notify-committed",
        "commit phase announced on entry: {commit_arm:?}"
    );
}

#[test]
fn every_notification_is_one_bare_argv_with_no_embedded_program() {
    let canonical = basic_canonical();

    assert_eq!(
        argv_of(&canonical, "notify-begin"),
        vec!["ctx-notify", "begin", "{port:task}"],
        "begin prints the bare id straight into the id slot"
    );
    for update_id in [
        "notify-session-base",
        "notify-plan-drafted",
        "notify-baseline-review",
        "notify-working-the-stage",
        "notify-awaiting-annotations",
        "notify-stage-committed",
        "notify-committed",
    ] {
        let argv = argv_of(&canonical, update_id);
        assert_eq!(argv[0], "ctx-notify");
        assert_eq!(argv[1], "update");
        assert_eq!(argv[2], "{slot:notify-id}");
        assert_eq!(argv[3], "--status");
        assert_eq!(
            argv.len(),
            5,
            "{update_id}: one status literal, nothing else: {argv:?}"
        );
    }
    assert_eq!(
        argv_of(&canonical, "notify-review-update"),
        vec![
            "ctx-notify",
            "update",
            "{slot:notify-id}",
            "--status",
            "{slot:notify-badge}",
            "--summary",
            "{slot:notify-summary}",
        ]
    );
    assert_eq!(
        argv_of(&canonical, "notify-review-journal"),
        vec![
            "ctx-notify",
            "log",
            "{slot:notify-id}",
            "{slot:notify-journal}"
        ]
    );
    assert_eq!(
        argv_of(&canonical, "notify-gate-result"),
        vec![
            "ctx-notify",
            "log",
            "{slot:notify-id}",
            "{slot:gate-answer}"
        ]
    );
    assert_eq!(
        argv_of(&canonical, "notify-finish"),
        vec!["ctx-notify", "finish", "--ok", "{slot:notify-id}"]
    );

    // The verdict gate is the one sanctioned sh step (house gate shape):
    // surface and mode ride as positional argv data, and the script knows
    // only ctx-annotate, the off-mode, and the accepted literal.
    let gate = argv_of(&canonical, "owner-verdict-gate");
    assert_eq!(&gate[..2], ["sh", "-c"]);
    assert_eq!(
        &gate[3..],
        ["_", "{slot:gate-surface}", "{port:owner-gate}"]
    );
    assert!(gate[2].contains("ctx-annotate --stdin"));
    assert!(gate[2].contains(r#""annotations":[]"#));
    assert!(gate[2].contains("printf accepted"));

    // The declarative guarantee itself: no notification step smuggles a
    // program — no shell, no interpreter, anywhere near a notifier argv.
    for pool in all_pools(&canonical) {
        for item in pool {
            let Some(argv) = item
                .get("command")
                .and_then(|c| c.get("argv"))
                .and_then(toml::Value::as_array)
            else {
                continue;
            };
            let head = argv
                .first()
                .and_then(toml::Value::as_str)
                .unwrap_or_default();
            if head == "ctx-notify" {
                for token in argv {
                    let token = token.as_str().unwrap_or_default();
                    assert!(
                        !token.contains("python3") && !token.contains("subprocess"),
                        "bare notification argv only: {token}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_digest_seat_carries_its_fields_to_text_slots() {
    let canonical = basic_canonical();
    for pool in all_pools(&canonical) {
        for item in pool {
            if item.get("id").and_then(toml::Value::as_str) == Some("notify-carry-digest") {
                let projections = item
                    .get("projection")
                    .and_then(toml::Value::as_array)
                    .expect("carry step projection list");
                let carried: Vec<(&str, &str)> = projections
                    .iter()
                    .map(|projection| {
                        (
                            projection
                                .get("field")
                                .and_then(toml::Value::as_str)
                                .expect("field"),
                            projection
                                .get("destination")
                                .and_then(toml::Value::as_str)
                                .expect("destination"),
                        )
                    })
                    .collect();
                assert_eq!(
                    carried,
                    vec![
                        ("badge", "slot:notify-badge"),
                        ("summary", "slot:notify-summary"),
                        ("journal", "slot:notify-journal"),
                    ]
                );
                return;
            }
        }
    }
    panic!("notify-carry-digest not found");
}

#[test]
fn quick_and_complex_canonicals_stay_silent() {
    for variant in ["quick", "complex"] {
        let path = repo_root().join(format!(
            ".ctx/traits/authored/implement/generated/{variant}/index.toml"
        ));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        assert!(
            !text.contains("ctx-notify"),
            "{variant} canonical must carry no notifier invocation"
        );
    }
}
