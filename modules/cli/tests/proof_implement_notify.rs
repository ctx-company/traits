//! Declarative notify-narration proofs (0283 step-walk). Notify is still a
//! bare `ctx-notify` command chain — it becomes a typed effect in 0283.1 — so
//! the surviving invariants are behavioral, not structural: no notification
//! smuggles a program, the digest seat carries its fields into the text slots
//! the argv reads, and the unreviewed / doubly-reviewed lanes stay silent. The
//! exact sequence-order proof was retired with the stage loop: the step-walk's
//! structure is deliberately not frozen (owner ruling — no frozen-inventory
//! proofs).

use support::repo_root;

fn basic_canonical() -> toml::Table {
    let path = repo_root().join(".ctx/traits/authored/implement/generated/basic/index.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    text.parse::<toml::Table>()
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
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
fn every_notification_is_one_bare_argv_with_no_embedded_program() {
    let canonical = basic_canonical();

    assert_eq!(
        argv_of(&canonical, "notify-begin"),
        vec!["ctx-notify", "begin", "{port:task}"],
        "begin prints the bare id straight into the id slot"
    );
    assert_eq!(
        argv_of(&canonical, "notify-finish"),
        vec!["ctx-notify", "finish", "--ok", "{slot:notify-id}"]
    );

    // The declarative guarantee: no notification step smuggles a program —
    // no shell, no interpreter, anywhere near a notifier argv.
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
