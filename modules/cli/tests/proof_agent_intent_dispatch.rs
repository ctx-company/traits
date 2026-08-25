use std::fs;
use std::os::unix::fs::PermissionsExt;

use support::{ScratchRoot, assert_exit_code, git_init, require_success, run_ctx, utf8};

#[test]
fn assigned_agent_intent_dispatch_is_shared_and_legacy_compatible() {
    const ROOT_INTENT: &str = r#"<intent>
  <info>What the finished work is judged against. Each item below carries the group it belongs to; the group says how much it weighs.</info>
  <spec>
    <require>Conditions the finished work has to meet.</require>
    <focus>Where to look. Attention, not acceptance criteria.</focus>
    <avoid>Not acceptable, unless unavoidable.</avoid>
    <block>Unacceptable. If one has happened, correcting it comes before anything else.</block>
  </spec>
  <require id="shared">
    Root replacement text.
  </require>
  <require id="root-only">
    Root remains first.
  </require>
  <require id="root-collision">
    Root collision remains accepted.
  </require>
  <avoid id="root-collision">
    Root collision remains accepted.
  </avoid>
</intent>
"#;
    const LEGACY_CLI_PROMPT: &str = r#"<intent>
  <info>What the finished work is judged against. Each item below carries the group it belongs to; the group says how much it weighs.</info>
  <spec>
    <require>Conditions the finished work has to meet.</require>
    <focus>Where to look. Attention, not acceptance criteria.</focus>
    <avoid>Not acceptable, unless unavoidable.</avoid>
    <block>Unacceptable. If one has happened, correcting it comes before anything else.</block>
  </spec>
  <require id="shared">
    Root replacement text.
  </require>
  <require id="root-only">
    Root remains first.
  </require>
  <require id="root-collision">
    Root collision remains accepted.
  </require>
  <avoid id="root-collision">
    Root collision remains accepted.
  </avoid>
</intent>

<agent>
  <info>Who you are on this step.</info>
  <identity>Assigned worker.</identity>
</agent>

<input>
  <info>The step to do, and the values you have been given to do it with. &lt;spec&gt; says what each value means; &lt;data&gt; carries it.</info>
  <data>
  </data>
  <prompt>
    Produce an answer.
  </prompt>
</input>

<output>
  <info>What to return, and in what shape. The response is validated against this before it is accepted.</info>
  <spec>
    <answer>Answer.</answer>
  </spec>
  <format>{"answer": "string"}</format>
  <budget>Your entire response must fit in 294000 bytes.</budget>
  <response>
    Return ONLY one JSON object matching <format> — no prose before or after it, no code fences, no extra top-level fields. String-typed fields are single strings, never arrays.
  </response>
</output>
"#;

    let scratch = ScratchRoot::new("assigned-agent-intent-dispatch");
    let home = scratch.home();
    let repo = home.join("repo");
    let package = repo.join(".ctx/traits/agent-intent");
    let generated = package.join("generated/index.toml");
    let capture = home.join("capture.txt");
    let marker = home.join("called");
    fs::create_dir_all(generated.parent().unwrap()).unwrap();
    git_init(&repo);

    let script = home.join("capture.sh");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--probe\" ]; then printf 'capture-1.0\\n'; exit 0; fi\nprintf '%s\\n' \"$@\" > {}.args\nfor last; do :; done\nprintf '%s' \"$last\" > {}\ntouch {}\nprintf '{{\"answer\":\"ok\"}}'\n",
            capture.display(),
            capture.display(),
            marker.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();

    fs::write(
        package.join("trait.toml"),
        "[package]\nid = \"agent-intent\"\nversion = \"1.0.0\"\nname = \"Agent intent\"\nstatus = \"draft\"\n",
    )
    .unwrap();
    let canonical = r#"id = "agent-intent"
schema-version = "0.6"
version = "1.0.0"
name = "Agent intent"
description = "Proves assigned intent dispatch."

[intent]
require = [{ id = "shared", summary = "Root replacement text." }, { id = "root-only", summary = "Root remains first." }, { id = "root-collision", summary = "Root collision remains accepted." }]
avoid = [{ id = "root-collision", summary = "Root collision remains accepted." }]

[[agent]]
id = "unassigned"
description = "Must not participate."
system = "UNASSIGNED SYSTEM"
[agent.intent]
require = [{ id = "unassigned-only", summary = "Unassigned guidance." }]

[[agent]]
id = "worker"
description = "Assigned worker."
system = "ASSIGNED SYSTEM"
[agent.intent]
require = [{ id = "shared", summary = "Assigned replacement text." }, { id = "agent-only", summary = "Assigned guidance." }]

[[slot]]
id = "answer"
schema = "schema:text"
description = "Answer."

[procedure]
description = "One assigned step."

[[procedure.sequence]]
id = "work"
title = "Work"
agent = "agent:worker"
prompt = "Produce an answer."
output = ["slot:answer"]
"#;
    let worker_intent = "[agent.intent]\nrequire = [{ id = \"shared\", summary = \"Assigned replacement text.\" }, { id = \"agent-only\", summary = \"Assigned guidance.\" }]";
    fs::write(&generated, canonical).unwrap();

    let runtime = |transport: &str| {
        format!(
            "schema-version = \"0.4\"\n\n[harness.capture]\nkind = \"custom\"\nbin = {:?}\ntransports = [\"cli\", \"mcp\"]\nversion-probe = [\"--probe\"]\n\n[harness.capture.cli]\nargv = []\nprompt-via = \"arg\"\noutput = \"raw-json\"\nsystem-prompt-flag = \"--cli-system\"\n\n[harness.capture.mcp]\nsystem-prompt-flag = \"--mcp-system\"\n\n[agent.role.worker]\nharness = \"capture\"\ntransport = \"{transport}\"\nsession-mode = \"per-frame\"\n",
            script.to_string_lossy(),
        )
    };
    fs::create_dir_all(repo.join(".ctx/traits")).unwrap();
    fs::write(repo.join(".ctx/traits/runtime.toml"), runtime("cli")).unwrap();
    require_success("initialize fixture", &["traits", "init"], &repo, &home);
    require_success(
        "approve fixture",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            generated.to_str().unwrap(),
            "--approve",
        ],
        &repo,
        &home,
    );
    require_success(
        "activate fixture",
        &[
            "traits",
            "state",
            "--active",
            "--file",
            generated.to_str().unwrap(),
        ],
        &repo,
        &home,
    );

    let preview = |label: &str| {
        let output = run_ctx(
            &[
                "traits",
                "internal",
                "preview",
                "--file",
                generated.to_str().unwrap(),
                "--json",
            ],
            &repo,
            &home,
        );
        assert_exit_code(&output, 0);
        let (stdout, stderr) = utf8(&output);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
            panic!("{label} preview was not JSON: {error}\nstdout={stdout}\nstderr={stderr}")
        });
        json["frames"][0]["prompt"]
            .as_str()
            .unwrap_or_else(|| panic!("{label} preview had no prompt: {json}"))
            .to_string()
    };
    let extract_intent = |text: &str| {
        let start = text.find("<intent>\n").expect("intent opening");
        let end = text[start..].find("</intent>\n").expect("intent closing")
            + start
            + "</intent>\n".len();
        text[start..end].to_string()
    };
    let extract_behavior = |text: &str| {
        let start = text.find("<behavior>\n").expect("behavior opening");
        let end = text[start..]
            .find("</behavior>\n")
            .expect("behavior closing")
            + start
            + "</behavior>\n".len();
        text[start..end].to_string()
    };
    let assert_system = |args: &str, flag: &str| {
        let values: Vec<_> = args.lines().collect();
        assert_eq!(
            values
                .windows(2)
                .filter(|pair| *pair == [flag, "ASSIGNED SYSTEM"])
                .count(),
            1
        );
        assert_eq!(
            values
                .iter()
                .filter(|value| **value == "ASSIGNED SYSTEM")
                .count(),
            1
        );
        assert!(!args.contains("UNASSIGNED SYSTEM"));
    };
    let assert_unassigned_absent = |prompt: &str| {
        assert!(!prompt.contains("unassigned-only"));
        assert!(!prompt.contains("ASSIGNED SYSTEM"));
        assert!(!prompt.contains("UNASSIGNED SYSTEM"));
    };
    let run = |transport: &str, output: &str| {
        let _ = fs::remove_file(&capture);
        let _ = fs::remove_file(capture.with_extension("txt.args"));
        let _ = fs::remove_file(&marker);
        fs::write(repo.join(".ctx/traits/runtime.toml"), runtime(transport)).unwrap();
        require_success(
            "approve current fixture",
            &[
                "traits",
                "internal",
                "review",
                "--file",
                generated.to_str().unwrap(),
                "--approve",
            ],
            &repo,
            &home,
        );
        let outcome = run_ctx(
            &[
                "traits",
                "run",
                "--file",
                generated.to_str().unwrap(),
                "--out",
                home.join(output).to_str().unwrap(),
                "--json",
                "--progress",
                "none",
            ],
            &repo,
            &home,
        );
        assert_exit_code(&outcome, 0);
        (
            fs::read_to_string(&capture).expect("prompt capture"),
            fs::read_to_string(capture.with_extension("txt.args")).expect("argv capture"),
        )
    };

    let happy_preview = preview("assigned");
    let happy_intent = extract_intent(&happy_preview);
    assert!(happy_intent.find("root-only").unwrap() < happy_intent.find("shared").unwrap());
    assert!(happy_intent.find("shared").unwrap() < happy_intent.find("agent-only").unwrap());
    assert_eq!(happy_intent.matches("id=\"shared\"").count(), 1);
    assert!(happy_intent.contains("Assigned replacement text."));
    assert_unassigned_absent(&happy_preview);
    let (cli_prompt, cli_args) = run("cli", "assigned-cli.json");
    assert_eq!(extract_intent(&cli_prompt), happy_intent);
    assert_system(&cli_args, "--cli-system");
    assert_unassigned_absent(&cli_prompt);
    let (mcp_prompt, mcp_args) = run("mcp", "assigned-mcp.json");
    assert_eq!(extract_intent(&mcp_prompt), happy_intent);
    assert_system(&mcp_args, "--mcp-system");
    assert_unassigned_absent(&mcp_prompt);

    let root_behavior = r#"
[behavior]
tone = [{ id = "shared-tone", summary = "Root tone." }, { id = "root-tone", summary = "Root tone remains." }]
method = [{ id = "shared-method", summary = "Root method." }, { id = "root-method", summary = "Root method remains." }]
format = [{ id = "shared-format", summary = "Root format." }, { id = "root-format", summary = "Root format remains." }]
verbosity = { id = "root-verbosity", summary = "Root verbosity." }
directness = { id = "root-directness", summary = "Root directness." }
scope-control = { id = "root-scope", summary = "Root scope." }
initiative = { id = "root-initiative", summary = "Root initiative." }
uncertainty = { id = "root-uncertainty", summary = "Root uncertainty." }
"#;
    let worker_behavior = r#"[agent.behavior]
tone = [{ id = "shared-tone", summary = "Agent tone." }, { id = "agent-tone", summary = "Agent tone added." }]
method = [{ id = "shared-method", summary = "Agent method." }, { id = "agent-method", summary = "Agent method added." }]
format = [{ id = "shared-format", summary = "Agent format." }, { id = "agent-format", summary = "Agent format added." }]
verbosity = { id = "agent-verbosity", summary = "Agent verbosity." }
directness = { id = "agent-directness", summary = "Agent directness." }
scope-control = { id = "agent-scope", summary = "Agent scope." }
uncertainty = { id = "agent-uncertainty", summary = "Agent uncertainty." }"#;
    let behavior_canonical = canonical
        .replacen("\n[[agent]]", &format!("{root_behavior}\n[[agent]]"), 1)
        .replace(worker_intent, worker_behavior)
        .replace(
            "[agent.intent]\nrequire = [{ id = \"unassigned-only\", summary = \"Unassigned guidance.\" }]",
            "[agent.intent]\nrequire = [{ id = \"unassigned-only\", summary = \"Unassigned guidance.\" }]\n[agent.behavior]\ntone = [{ id = \"unassigned-tone\", summary = \"Unassigned behavior.\" }]",
        );
    fs::write(&generated, &behavior_canonical).unwrap();
    let behavior_preview = preview("behavior-only assigned");
    let behavior_block = extract_behavior(&behavior_preview);
    for (root, agent) in [
        ("root-tone", "shared-tone"),
        ("root-method", "shared-method"),
        ("root-format", "shared-format"),
    ] {
        assert!(
            behavior_block.find(root).unwrap() < behavior_block.find(agent).unwrap(),
            "{behavior_block}"
        );
    }
    for (id, text) in [
        ("shared-tone", "Agent tone."),
        ("shared-method", "Agent method."),
        ("shared-format", "Agent format."),
    ] {
        assert_eq!(
            behavior_block.matches(&format!("id=\"{id}\"")).count(),
            1,
            "{behavior_block}"
        );
        assert!(behavior_block.contains(text), "{behavior_block}");
    }
    for expected in [
        "agent-verbosity",
        "agent-directness",
        "agent-scope",
        "root-initiative",
        "agent-uncertainty",
    ] {
        assert!(behavior_block.contains(expected), "{behavior_block}");
    }
    assert!(!behavior_block.contains("unassigned-tone"));
    assert!(!behavior_block.contains("ASSIGNED SYSTEM"));
    assert!(!behavior_block.contains("UNASSIGNED SYSTEM"));
    let (behavior_cli, cli_args) = run("cli", "behavior-cli.json");
    assert_eq!(extract_behavior(&behavior_cli), behavior_block);
    assert_system(&cli_args, "--cli-system");
    let (behavior_mcp, mcp_args) = run("mcp", "behavior-mcp.json");
    assert_eq!(extract_behavior(&behavior_mcp), behavior_block);
    assert_system(&mcp_args, "--mcp-system");
    assert_unassigned_absent(&behavior_preview);
    assert_unassigned_absent(&behavior_cli);
    assert_unassigned_absent(&behavior_mcp);

    let root_behavior_block = r#"<behavior>
  <info>How to work and how to write, as distinct from what to produce. Each item carries the axis it sets.</info>
  <spec>
    <tone>How the writing sounds.</tone>
    <method>How the work is approached before it is reported.</method>
    <format>How the answer is laid out.</format>
    <verbosity>How much is said.</verbosity>
    <directness>How much is stated outright rather than hedged.</directness>
    <scope-control>How strictly the answer stays inside what was asked.</scope-control>
    <initiative>How much is done without being asked.</initiative>
    <uncertainty>What happens when the agent does not know.</uncertainty>
  </spec>
  <tone id="shared-tone">
    Root tone.
  </tone>
  <tone id="root-tone">
    Root tone remains.
  </tone>
  <method id="shared-method">
    Root method.
  </method>
  <method id="root-method">
    Root method remains.
  </method>
  <format id="shared-format">
    Root format.
  </format>
  <format id="root-format">
    Root format remains.
  </format>
  <verbosity id="root-verbosity">
    Root verbosity.
  </verbosity>
  <directness id="root-directness">
    Root directness.
  </directness>
  <scope-control id="root-scope">
    Root scope.
  </scope-control>
  <initiative id="root-initiative">
    Root initiative.
  </initiative>
  <uncertainty id="root-uncertainty">
    Root uncertainty.
  </uncertainty>
</behavior>
"#;
    for (name, behavior) in [
        ("behavior-absent", ""),
        ("behavior-empty", "[agent.behavior]\n"),
    ] {
        fs::write(
            &generated,
            behavior_canonical.replace(worker_behavior, behavior),
        )
        .unwrap();
        let preview = preview(name);
        assert_eq!(
            extract_behavior(&preview),
            root_behavior_block,
            "{name} preview behavior changed"
        );
        let (cli, cli_args) = run("cli", &format!("{name}-cli.json"));
        assert_eq!(
            extract_behavior(&cli),
            root_behavior_block,
            "{name} CLI behavior changed"
        );
        assert_system(&cli_args, "--cli-system");
        let (mcp, mcp_args) = run("mcp", &format!("{name}-mcp.json"));
        assert!(
            !mcp.contains("<behavior>\n"),
            "{name} MCP gained root-only behavior"
        );
        assert_system(&mcp_args, "--mcp-system");
    }

    for (name, intent) in [("absent", ""), ("default-empty", "[agent.intent]\n")] {
        fs::write(&generated, canonical.replace(worker_intent, intent)).unwrap();
        let legacy_preview = preview(name);
        assert_eq!(
            legacy_preview, LEGACY_CLI_PROMPT,
            "{name} preview changed from its frozen legacy bytes"
        );
        assert_eq!(extract_intent(&legacy_preview), ROOT_INTENT);
        let (legacy_cli, cli_args) = run("cli", &format!("{name}-cli.json"));
        assert_eq!(
            legacy_cli, LEGACY_CLI_PROMPT,
            "{name} CLI prompt changed from its frozen legacy bytes"
        );
        assert_system(&cli_args, "--cli-system");
        let mcp_output = format!("{name}-mcp.json");
        let (legacy_mcp, mcp_args) = run("mcp", &mcp_output);
        let session = home.join(&mcp_output);
        let expected_mcp = format!(
            "Serve this ctx.traits frame via MCP.\nAgent role: worker\nHarness id: capture\nRun session: {}\nSession store: \n\nRequired steps:\n1. Call ctx_traits_run_next with agent=worker, session={}, and the session-store above when present.\n2. Use the authoritative frame refs/digests from ctx, and use the resolved content below for the actual goal, inputs, and instructions.\n3. Complete only the returned frame.\n4. Submit with ctx_traits_run_set or ctx_traits_run_call, including agent=worker and harness=capture.\n5. Stop after the submit succeeds; do not continue the procedure loop.\n\nFrame title: Work\n\nStep [run 0 / source 0]: Work\nAssigned agent: agent:worker (Assigned worker.)\nAvailable inputs:\nRequested outputs:\n- slot:answer (replace)\n\nResolved prompt instructions:\nProduce an answer.\nResolved input values:\n\n",
            session.display(),
            session.display(),
        );
        assert_eq!(
            legacy_mcp, expected_mcp,
            "{name} MCP prompt changed from its frozen legacy bytes"
        );
        assert_system(&mcp_args, "--mcp-system");
    }

    let conflict = canonical.replace(
        worker_intent,
        "[agent.intent]\navoid = [{ id = \"shared\", summary = \"Assigned conflict.\" }]",
    );
    fs::write(&generated, conflict).unwrap();
    require_success(
        "approve conflict fixture",
        &[
            "traits",
            "internal",
            "review",
            "--file",
            generated.to_str().unwrap(),
            "--approve",
        ],
        &repo,
        &home,
    );
    let _ = fs::remove_file(&marker);
    let conflict = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--out",
            home.join("conflict.json").to_str().unwrap(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert!(
        !conflict.status.success(),
        "cross-layer conflict unexpectedly dispatched"
    );
    let (_, stderr) = utf8(&conflict);
    assert!(
        stderr.contains("effective guidance id \"shared\" cannot appear in both require and avoid")
    );
    assert!(
        !marker.exists(),
        "harness ran before the cross-layer conflict was rejected"
    );
}
