//! 0253.3 run/replay proof: a step's bare-schema `output:` (virtual slot)
//! lowers to an ordinary auto-named slot a run writes and replays, and two
//! command steps each writing `.with(operation.Append)` to a list slot
//! accumulate both entries, in order — a scratch CDK source, built through
//! `ctx traits build`, run to completion, its persisted ledger re-read off
//! disk to confirm the file on disk agrees with what the run reported.

use std::fs;

use support::{ScratchRoot, git_init, require_success, symlink_node_modules};

fn fixture_source() -> &'static str {
    "import { operation, procedure, schema, sequence, slot, trait } from \"@ctx-traits/cdk\";\n\
\n\
const decisions = slot.list(schema.text(), \"owner-decisions\");\n\
\n\
const write = sequence.command(\"write\", {\n\
  cmd: \"echo hello\",\n\
  output: schema.text(),\n\
});\n\
\n\
const appendOne = sequence.command(\"append-one\", {\n\
  cmd: \"echo first\",\n\
  output: decisions.with(operation.Append),\n\
});\n\
\n\
const appendTwo = sequence.command(\"append-two\", {\n\
  cmd: \"echo second\",\n\
  output: decisions.with(operation.Append),\n\
});\n\
\n\
export const draft = trait({\n\
  id: \"vslot-fixture\",\n\
  name: \"vslot-fixture\",\n\
  description: \"Virtual slot + append run/replay fixture.\",\n\
  slot: [decisions],\n\
  procedure: procedure({\n\
    description: \"Write a virtual slot, then append two entries to a list slot.\",\n\
    sequence: [write, appendOne, appendTwo],\n\
  }),\n\
});\n"
}

/// `ctx traits run --json` prints a `command step ... running (waiting for
/// it to finish)` status line ahead of its JSON envelope whenever no TUI
/// panel is active (`command_started_event`, `app/drive.rs`) — `--json`
/// silences the panel, not that line. Skip to the first `{` the same way
/// `proof_drive_retries.rs`'s `value_json` does.
fn value_json(stdout: &str) -> serde_json::Value {
    let start = stdout
        .lines()
        .position(|line| line.trim_start().starts_with('{'));
    let json_text = match start {
        Some(index) => stdout.lines().skip(index).collect::<Vec<_>>().join("\n"),
        None => stdout.to_string(),
    };
    serde_json::from_str(&json_text).unwrap_or_else(|error| {
        panic!("stdout was not a JSON envelope: {error}\nstdout:\n{stdout}")
    })
}

/// Build, trust-approve, activate, and run `vslot-fixture`'s CDK source to
/// completion, returning the run's `--json` stdout and the ledger file
/// written by `--out`.
fn run_fixture() -> (serde_json::Value, serde_json::Value) {
    let scratch = ScratchRoot::new("cdk-virtual-slot-append");
    let home = scratch.home();
    let proj = home.join("repo");
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    symlink_node_modules(&proj);

    let trait_id = "vslot-fixture";
    require_success(
        &format!("`ctx traits init {trait_id}`"),
        &["traits", "init", trait_id],
        &proj,
        &home,
    );

    let source_path = proj.join(format!(".ctx/traits/authored/{trait_id}/source/index.ts"));
    fs::write(&source_path, fixture_source())
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", source_path.display()));

    require_success(
        &format!("`ctx traits build` for {trait_id}"),
        &[
            "traits",
            "build",
            &format!(".ctx/traits/authored/{trait_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );

    let canonical_path = format!(".ctx/traits/authored/{trait_id}/generated/index.toml");
    let canonical_text = fs::read_to_string(proj.join(&canonical_path)).unwrap();
    assert!(
        canonical_text.contains("id = \"write\"\nschema = \"schema:text\""),
        "expected the virtual slot's output: schema.text() to lower to an auto-named slot \
         `write` sharing the step's id, got:\n{canonical_text}"
    );
    assert!(
        canonical_text.matches("operation = \"append\"").count() == 2,
        "expected both append steps to lower to operation = \"append\", got:\n{canonical_text}"
    );

    require_success(
        &format!("`ctx traits trust --approved` for {trait_id}"),
        &["traits", "trust", "--approved", &canonical_path],
        &proj,
        &home,
    );
    require_success(
        &format!("`ctx traits state --active {trait_id}`"),
        &["traits", "state", "--active", trait_id],
        &proj,
        &home,
    );

    let ledger_path = proj.join("run.json");
    let run_stdout = require_success(
        "`ctx traits run --json`",
        &[
            "traits",
            "run",
            "--file",
            &canonical_path,
            "--json",
            "--progress",
            "none",
            "--out",
            ledger_path.to_str().unwrap(),
        ],
        &proj,
        &home,
    );
    let run_json = value_json(&run_stdout);

    let ledger_text = fs::read_to_string(&ledger_path).unwrap_or_else(|error| {
        panic!(
            "cannot read persisted ledger {}: {error}",
            ledger_path.display()
        )
    });
    let ledger_json: serde_json::Value =
        serde_json::from_str(&ledger_text).unwrap_or_else(|error| {
            panic!(
                "persisted ledger {} was not JSON: {error}",
                ledger_path.display()
            )
        });

    (run_json, ledger_json)
}

fn accepted_owner_decisions(session: &serde_json::Value) -> Vec<String> {
    session["accepted-slot-values"]
        .as_array()
        .unwrap_or_else(|| panic!("no accepted-slot-values in {session}"))
        .iter()
        .find(|entry| entry["ref-text"] == "slot:owner-decisions")
        .unwrap_or_else(|| panic!("no slot:owner-decisions in accepted-slot-values: {session}"))["value"]
        .as_array()
        .unwrap_or_else(|| panic!("slot:owner-decisions value was not an array: {session}"))
        .iter()
        .map(|entry| entry.as_str().unwrap_or_else(|| panic!("non-string entry in {entry}")).to_string())
        .collect()
}

fn accepted_virtual_slot(session: &serde_json::Value) -> String {
    session["accepted-slot-values"]
        .as_array()
        .unwrap_or_else(|| panic!("no accepted-slot-values in {session}"))
        .iter()
        .find(|entry| entry["ref-text"] == "slot:write")
        .unwrap_or_else(|| panic!("no slot:write (the virtual slot's auto-named slot) in accepted-slot-values: {session}"))
        ["value"]
        .as_str()
        .unwrap_or_else(|| panic!("slot:write value was not a string: {session}"))
        .to_string()
}

/// A bare-schema `output:` (virtual slot) lowers to an ordinary auto-named
/// slot a run writes; two steps each appending to a list slot accumulate
/// both entries, in order — and the ledger persisted to disk (`--out`)
/// agrees with what the run reported live.
#[test]
fn virtual_slot_and_append_run_and_replay_agree() {
    let (run_json, ledger_json) = run_fixture();

    let run_session = &run_json["value"]["session"];
    assert_eq!(
        run_session["status"], "completed",
        "expected the fixture run to complete: {run_json}"
    );
    assert_eq!(
        accepted_virtual_slot(run_session),
        "hello\n",
        "virtual slot's auto-named slot should carry the write step's output"
    );
    assert_eq!(
        accepted_owner_decisions(run_session),
        vec!["first".to_string(), "second".to_string()],
        "two Append steps should accumulate both entries, in order, on the live run's report"
    );

    // The persisted ledger (re-read off disk, not the live process's stdout)
    // must agree: same virtual-slot value, same append order.
    let ledger_session = if ledger_json.get("session").is_some() {
        &ledger_json["session"]
    } else {
        &ledger_json
    };
    assert_eq!(
        accepted_virtual_slot(ledger_session),
        "hello\n",
        "replayed ledger disagreed with the live run on the virtual slot's value"
    );
    assert_eq!(
        accepted_owner_decisions(ledger_session),
        vec!["first".to_string(), "second".to_string()],
        "replayed ledger disagreed with the live run on append order"
    );
}
