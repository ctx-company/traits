//! 0020: schemas are flattened at emit, never composed as `$ref`/`$defs`.
//!
//! `ctx traits preview` renders the same `requested_output_schema` value
//! both drive's provider payload (`--json-schema`) and the in-prompt
//! `<schema>` block are built from (`frame_prompt.rs`), so asserting on the
//! preview prompt text pins the one shared flattening source without
//! needing a live harness. A CDK-authored shared schema referenced from two
//! places must appear fully, independently inlined at each use site — never
//! as a pointer — and a self-referential schema graph must fail at
//! build/validate time with a named, path-bearing error instead of
//! degrading to `{}` at emit time.

use std::fs;
use std::path::Path;

use support::{ScratchRoot, assert_exit_code, git_init, run_ctx, utf8};

const TRAIT_ID: &str = "schema-flatten-fixture";

const PACKAGE_MANIFEST: &str = "[package]\nid = \"schema-flatten-fixture\"\nversion = \"0.1.0\"\nname = \"Schema Flatten Fixture\"\nstatus = \"draft\"\n";

/// A worker step whose output slot is backed by `schema:blocker`, which
/// nests `schema:item` both as a scalar field and inside a list wrapper
/// (`[schema:item]`) — the two ref forms the emit-side walker must inline
/// independently at each use site.
const ACYCLIC_MANIFEST: &str = r#"id = "schema-flatten-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Schema Flatten Fixture"
description = "0020 proof fixture: shared nested schema, no refs at emit."

[[schema]]
id = "item"

[schema.fields.name]
schema = "schema:text"
required = true

[schema.fields.qty]
schema = "schema:integer"
required = true

[[schema]]
id = "blocker"

[schema.fields.item]
schema = "schema:item"
required = true

[schema.fields.items]
schema = "[schema:item]"

[[agent]]
id = "worker"
description = "Fixture worker role."
summary = "Implementation role."

[[slot]]
id = "result"
schema = "schema:blocker"
description = "Fixture output."

[procedure]
description = "One worker step producing a shared-schema output."

[[procedure.sequence]]
id = "produce"
title = "Produce"
agent = "agent:worker"
prompt = "Produce the fixture output."
output = ["slot:result"]
"#;

/// `blocker` and `item` reference each other: `blocker.item -> item`,
/// `item.blocker -> blocker`. Flattening this graph does not terminate.
const CYCLIC_MANIFEST: &str = r#"id = "schema-flatten-fixture"
schema-version = "0.4"
version = "0.1.0"
name = "Schema Flatten Fixture"
description = "0020 proof fixture: mutually recursive schema graph."

[[schema]]
id = "blocker"

[schema.fields.item]
schema = "schema:item"
required = true

[[schema]]
id = "item"

[schema.fields.blocker]
schema = "schema:blocker"
required = true

[[agent]]
id = "worker"
description = "Fixture worker role."
summary = "Implementation role."

[[slot]]
id = "result"
schema = "schema:blocker"
description = "Fixture output."

[procedure]
description = "One worker step producing a recursive-schema output."

[[procedure.sequence]]
id = "produce"
title = "Produce"
agent = "agent:worker"
prompt = "Produce the fixture output."
output = ["slot:result"]
"#;

fn write_fixture(repo: &Path, manifest: &str) {
    let dir = repo.join(format!(".ctx/traits/{TRAIT_ID}/generated"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        repo.join(format!(".ctx/traits/{TRAIT_ID}/trait.toml")),
        PACKAGE_MANIFEST,
    )
    .unwrap();
    fs::write(dir.join("index.toml"), manifest).unwrap();
}

#[test]
fn shared_nested_schema_is_fully_inlined_with_no_ref_or_defs() {
    let scratch = ScratchRoot::new("schema-flatten-acyclic");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    write_fixture(&repo, ACYCLIC_MANIFEST);

    let output = run_ctx(
        &[
            "traits",
            "preview",
            "--file",
            &format!(".ctx/traits/{TRAIT_ID}/generated/index.toml"),
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 0);
    let (stdout, stderr) = utf8(&output);
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!("stdout was not JSON: {error}\nstdout:\n{stdout}\nstderr:\n{stderr}")
    });
    let prompt = report["frames"][0]["prompt"]
        .as_str()
        .unwrap_or_else(|| panic!("no prompt in report: {report}"));

    assert!(
        !prompt.contains("$ref") && !prompt.contains("$defs"),
        "emitted schema must never carry $ref/$defs: {prompt}"
    );

    // `item`'s fields appear fully inlined at BOTH use sites: once as
    // `blocker.item` (a plain object) and once as the element type of
    // `blocker.items` (an array) — never a pointer to a single definition.
    let item_body = "\"name\":{\"type\":\"string\"}";
    let occurrences = prompt.matches(item_body).count();
    assert_eq!(
        occurrences, 2,
        "shared schema `item` must be independently inlined at each of its two use sites, not deduplicated behind a ref: {prompt}"
    );
}

#[test]
fn recursive_schema_graph_fails_at_build_with_a_named_cycle_path() {
    let scratch = ScratchRoot::new("schema-flatten-cyclic");
    let repo = scratch.home().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git_init(&repo);
    write_fixture(&repo, CYCLIC_MANIFEST);

    let output = run_ctx(
        &[
            "traits",
            "preview",
            "--file",
            &format!(".ctx/traits/{TRAIT_ID}/generated/index.toml"),
            "--json",
        ],
        &repo,
        &scratch.home(),
    );
    assert_exit_code(&output, 1);
    let (stdout, stderr) = utf8(&output);
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("recursive") && combined.contains("blocker -> item -> blocker"),
        "recursive schema must fail with a named cycle path, not degrade to {{}} at emit: {combined}"
    );
}
