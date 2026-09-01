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
    assert_eq!(top_ids[baseline + 1], "notify-session-base");
    let draft = index_of(&top_ids, "draft-the-implementation-plan");
    assert_eq!(top_ids[draft + 1], "notify-plan-drafted");
    let finish = index_of(&top_ids, "notify-finish");
    assert!(finish > index_of(&top_ids, "maybe-commit"));
    assert_eq!(
        finish,
        top_ids.len() - 1,
        "finish closes the run: {top_ids:?}"
    );

    let body = canonical
        .get("sequence")
        .and_then(|s| s.get("reviewed-refinement-body"))
        .and_then(|s| s.get("sequence"))
        .and_then(toml::Value::as_array)
        .expect("reviewed-refinement-body.sequence");
    let body_ids = ids(body);
    let implement = index_of(&body_ids, "implement-the-task");
    assert_eq!(body_ids[implement + 1], "notify-implement-pass");
    let review = index_of(&body_ids, "review-the-implementation");
    assert_eq!(body_ids[review + 1], "notify-digest-verdict");
    assert_eq!(body_ids[review + 2], "notify-carry-digest");
    assert_eq!(body_ids[review + 3], "notify-review-update");
    assert_eq!(body_ids[review + 4], "notify-review-journal");
    assert!(
        index_of(&body_ids, "owner-ruling") > review + 4,
        "review narration precedes the ruling branch"
    );

    let named = canonical
        .get("sequence")
        .and_then(toml::Value::as_table)
        .expect("named sequences");
    let commit_arm = named
        .values()
        .filter_map(|sub| sub.get("sequence").and_then(toml::Value::as_array))
        .find(|items| ids(items).contains(&"commit-the-work"))
        .expect("an arm containing commit-the-work");
    let arm_ids = ids(commit_arm);
    assert_eq!(
        arm_ids[index_of(&arm_ids, "commit-the-work") + 1],
        "notify-committed"
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
        "notify-implement-pass",
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
        argv_of(&canonical, "notify-finish"),
        vec!["ctx-notify", "finish", "--ok", "{slot:notify-id}"]
    );

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
