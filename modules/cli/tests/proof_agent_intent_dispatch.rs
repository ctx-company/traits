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
    // The response key/value is derived from the frame's own `<format>{"<key>": ...}`
    // line rather than hardcoded, so a step whose declared output is not
    // `slot:answer` (the branch-guard decision leaves added below) can drive its
    // own harness answer via a `CTX_TEST_<KEY>` env var while every existing
    // `slot:answer` step keeps its prior hardcoded "ok" behavior unchanged (the
    // `*) value=ok` default).
    fs::write(
        &script,
        r#"#!/bin/sh
if [ "$1" = "--probe" ]; then printf 'capture-1.0\n'; exit 0; fi
printf '%s\n' "$@" > __CAPTURE__.args
ctx=
for arg; do
  case "$arg" in
    *'"command":'*) ctx=$(printf '%s' "$arg" | sed -n 's/.*"command":"\([^"]*\)".*/\1/p') ;;
  esac
done
for last; do :; done
printf '%s' "$last" > __CAPTURE__
if [ -f __CAPTURE__.calls ]; then n=$(($(wc -l < __CAPTURE__.calls))); else n=0; fi
printf '%s' "$last" > __CAPTURE__.$n
printf '%s\n' "$@" > __CAPTURE__.args.$n
printf 'x\n' >> __CAPTURE__.calls
touch __MARKER__
key=$(printf '%s\n' "$last" | sed -n 's/.*<format>{"\([a-zA-Z0-9_-]*\)".*/\1/p')
if [ -z "$key" ]; then
  key=$(printf '%s\n' "$last" | sed -n 's/^- slot:\([a-zA-Z0-9_-]*\) (replace)$/\1/p' | head -n1)
fi
case "$key" in
  choice) value=${CTX_TEST_CHOICE:-ok} ;;
   nested-choice) value=${CTX_TEST_NESTED_CHOICE:-ok} ;;
   loop-choice) value=${CTX_TEST_LOOP_CHOICE:-ok} ;;
   loop-verdict)
     if [ -f __CAPTURE__.verdict-calls ]; then n=$(cat __CAPTURE__.verdict-calls); else n=0; fi
     n=$((n + 1))
     printf '%s' "$n" > __CAPTURE__.verdict-calls
     if [ "$n" -eq 1 ]; then value=continue; else value=done; fi
     ;;
  *) value=ok ;;
esac
session=$(printf '%s\n' "$last" | sed -n 's/^Run session: //p')
if [ -n "$session" ]; then
  printf '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ctx_traits_run_set","arguments":{"session":"%s","target":"slot:%s","value":"%s","agent":"worker","harness":"capture"}}}\n' "$session" "$key" "$value" | "$ctx" traits internal mcp >/dev/null || exit 1
fi
printf '{"%s":"%s"}' "$key" "$value"
"#
        .replace("__CAPTURE__", &capture.display().to_string())
        .replace("__MARKER__", &marker.display().to_string()),
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
            "schema-version = \"0.4\"\n\n[harness.capture]\nkind = \"custom\"\nbin = {:?}\ntransports = [\"cli\", \"mcp\"]\nversion-probe = [\"--probe\"]\n\n[harness.capture.cli]\nargv = []\nprompt-via = \"arg\"\noutput = \"raw-json\"\nsystem-prompt-flag = \"--cli-system\"\n\n[harness.capture.mcp]\nmcp-config-flag = \"--mcp-config\"\nsystem-prompt-flag = \"--mcp-system\"\n\n[agent.role.worker]\nharness = \"capture\"\ntransport = \"{transport}\"\nsession-mode = \"per-frame\"\n",
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
    let expected_mcp_prompt = |session: &std::path::Path| {
        format!(
            "Serve this ctx.traits frame via MCP.\nAgent role: worker\nHarness id: capture\nRun session: {}\nSession store: \n\nRequired steps:\n1. Call ctx_traits_run_next with agent=worker, session={}, and the session-store above when present.\n2. Use the authoritative frame refs/digests from ctx, and use the resolved content below for the actual goal, inputs, and instructions.\n3. Complete only the returned frame.\n4. Submit with ctx_traits_run_set or ctx_traits_run_call, including agent=worker and harness=capture.\n5. Stop after the submit succeeds; do not continue the procedure loop.\n\nFrame title: Work\n\nStep [run 0 / source 0]: Work\nAssigned agent: agent:worker (Assigned worker.)\nAvailable inputs:\nRequested outputs:\n- slot:answer (replace)\n\nResolved prompt instructions:\nProduce an answer.\nResolved input values:\n\n",
            session.display(),
            session.display(),
        )
    };
    let run_with_env = |transport: &str, output: &str, extra_env: &[(&str, &str)]| {
        let _ = fs::remove_file(&capture);
        let _ = fs::remove_file(capture.with_extension("txt.args"));
        for index in 0..12 {
            let _ = fs::remove_file(capture.with_extension(format!("txt.{index}")));
            let _ = fs::remove_file(capture.with_extension(format!("txt.args.{index}")));
        }
        let _ = fs::remove_file(capture.with_extension("txt.calls"));
        let _ = fs::remove_file(capture.with_extension("txt.verdict-calls"));
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
        let outcome = support::run_ctx_with_env(
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
            extra_env,
        );
        assert_exit_code(&outcome, 0);
        (
            fs::read_to_string(&capture).expect("prompt capture"),
            fs::read_to_string(capture.with_extension("txt.args")).expect("argv capture"),
        )
    };
    let run = |transport: &str, output: &str| run_with_env(transport, output, &[]);

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
        "agent-tone",
        "agent-method",
        "agent-format",
        "agent-verbosity",
        "agent-directness",
        "agent-scope",
        "root-initiative",
        "agent-uncertainty",
    ] {
        assert!(behavior_block.contains(expected), "{behavior_block}");
    }
    for overridden_root in [
        "root-verbosity",
        "root-directness",
        "root-scope",
        "root-uncertainty",
    ] {
        assert!(
            !behavior_block.contains(overridden_root),
            "{behavior_block}"
        );
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
        let session = home.join(format!("{name}-mcp.json"));
        assert_eq!(
            mcp,
            expected_mcp_prompt(&session),
            "{name} MCP prompt changed from its frozen legacy bytes"
        );
        assert_system(&mcp_args, "--mcp-system");
    }

    let prompt_behavior_canonical = behavior_canonical.replace(
        "prompt = \"Produce an answer.\"\noutput = [\"slot:answer\"]",
        "prompt = \"Produce first answer.\"\nbehavior = { tone = [{ id = \"shared-tone\", summary = \"Prompt tone.\" }, { id = \"current-tone\", summary = \"Current prompt tone.\" }], verbosity = { id = \"current-verbosity\", summary = \"Current prompt verbosity.\" } }\noutput = [\"slot:answer\"]\n\n[[procedure.sequence]]\nid = \"next\"\ntitle = \"Next\"\nagent = \"agent:worker\"\nprompt = \"Produce second answer.\"\nbehavior = { method = [{ id = \"next-method\", summary = \"Next prompt method.\" }] }\ninput = [\"slot:answer\"]\noutput = [\"slot:answer\"]",
    );
    fs::write(&generated, &prompt_behavior_canonical).unwrap();
    let behavior_previews = run_ctx(
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
    assert_exit_code(&behavior_previews, 0);
    let (stdout, _) = utf8(&behavior_previews);
    let behavior_previews: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let first_behavior_preview = behavior_previews["frames"][0]["prompt"].as_str().unwrap();
    let second_behavior_preview = behavior_previews["frames"][1]["prompt"].as_str().unwrap();
    let first_behavior = extract_behavior(first_behavior_preview);
    let second_behavior = extract_behavior(second_behavior_preview);

    assert!(
        first_behavior.find("root-tone").unwrap() < first_behavior.find("agent-tone").unwrap(),
        "{first_behavior}"
    );
    assert!(
        first_behavior.find("agent-tone").unwrap() < first_behavior.find("shared-tone").unwrap(),
        "{first_behavior}"
    );
    assert!(
        first_behavior.find("shared-tone").unwrap() < first_behavior.find("current-tone").unwrap(),
        "{first_behavior}"
    );
    assert_eq!(
        first_behavior.matches("id=\"shared-tone\"").count(),
        1,
        "{first_behavior}"
    );
    assert!(first_behavior.contains("Prompt tone."), "{first_behavior}");
    assert!(!first_behavior.contains("Agent tone."), "{first_behavior}");
    assert!(!first_behavior.contains("Root tone."), "{first_behavior}");
    assert!(
        first_behavior.contains("current-verbosity"),
        "{first_behavior}"
    );
    assert!(
        !first_behavior.contains("agent-verbosity"),
        "{first_behavior}"
    );
    assert!(
        !first_behavior.contains("root-verbosity"),
        "{first_behavior}"
    );
    for unchanged in [
        "agent-directness",
        "agent-scope",
        "root-initiative",
        "agent-uncertainty",
    ] {
        assert!(first_behavior.contains(unchanged), "{first_behavior}");
    }
    assert!(!first_behavior.contains("next-method"), "{first_behavior}");
    assert!(
        !first_behavior.contains("unassigned-tone"),
        "{first_behavior}"
    );

    assert!(
        second_behavior.find("root-method").unwrap()
            < second_behavior.find("shared-method").unwrap(),
        "{second_behavior}"
    );
    assert!(
        second_behavior.find("shared-method").unwrap()
            < second_behavior.find("agent-method").unwrap(),
        "{second_behavior}"
    );
    assert!(
        second_behavior.find("agent-method").unwrap()
            < second_behavior.find("next-method").unwrap(),
        "{second_behavior}"
    );
    assert_eq!(
        second_behavior.matches("id=\"shared-method\"").count(),
        1,
        "{second_behavior}"
    );
    assert!(
        second_behavior.contains("Agent method."),
        "{second_behavior}"
    );
    assert!(
        !second_behavior.contains("Root method."),
        "{second_behavior}"
    );
    assert!(second_behavior.contains("Agent tone."), "{second_behavior}");
    assert!(
        !second_behavior.contains("Prompt tone."),
        "{second_behavior}"
    );
    assert!(
        !second_behavior.contains("current-tone"),
        "{second_behavior}"
    );
    assert!(
        !second_behavior.contains("current-verbosity"),
        "{second_behavior}"
    );
    assert!(
        second_behavior.contains("agent-verbosity"),
        "{second_behavior}"
    );
    assert!(
        !second_behavior.contains("unassigned-tone"),
        "{second_behavior}"
    );
    assert_unassigned_absent(first_behavior_preview);
    assert_unassigned_absent(second_behavior_preview);

    let (_, cli_args) = run("cli", "behavior-prompt-cli.json");
    let first_behavior_cli = fs::read_to_string(capture.with_extension("txt.0")).unwrap();
    let second_behavior_cli = fs::read_to_string(capture.with_extension("txt.1")).unwrap();
    assert_eq!(extract_behavior(&first_behavior_cli), first_behavior);
    assert_eq!(extract_behavior(&second_behavior_cli), second_behavior);
    assert_system(&cli_args, "--cli-system");
    assert_unassigned_absent(&first_behavior_cli);
    assert_unassigned_absent(&second_behavior_cli);
    let (_, mcp_args) = run("mcp", "behavior-prompt-mcp.json");
    let first_behavior_mcp = fs::read_to_string(capture.with_extension("txt.0")).unwrap();
    let second_behavior_mcp = fs::read_to_string(capture.with_extension("txt.1")).unwrap();
    assert_eq!(extract_behavior(&first_behavior_mcp), first_behavior);
    assert_eq!(extract_behavior(&second_behavior_mcp), second_behavior);
    assert_system(&mcp_args, "--mcp-system");
    assert_unassigned_absent(&first_behavior_mcp);
    assert_unassigned_absent(&second_behavior_mcp);

    let behavior_prompt_only = canonical.replace(
        "prompt = \"Produce an answer.\"",
        "prompt = \"Produce an answer.\"\nbehavior = { tone = [{ id = \"prompt-only-tone\", summary = \"Prompt-only tone.\" }] }",
    );
    fs::write(&generated, &behavior_prompt_only).unwrap();
    let (_, _) = run("mcp", "behavior-prompt-only-mcp.json");
    let behavior_prompt_only_mcp = fs::read_to_string(capture.with_extension("txt.0")).unwrap();
    let behavior_prompt_only_behavior = extract_behavior(&behavior_prompt_only_mcp);
    assert!(
        behavior_prompt_only_behavior.contains("prompt-only-tone"),
        "{behavior_prompt_only_behavior}"
    );
    assert!(
        behavior_prompt_only_behavior.contains("Prompt-only tone."),
        "{behavior_prompt_only_behavior}"
    );
    assert!(
        !behavior_prompt_only_behavior.contains("agent-only"),
        "{behavior_prompt_only_behavior}"
    );

    let mut prompt_behavior_compatibility = None;
    for (name, prompt_behavior) in [
        ("prompt-behavior-absent", ""),
        ("prompt-behavior-empty", "behavior = {}\n"),
    ] {
        fs::write(
            &generated,
            behavior_canonical.replace(
                "prompt = \"Produce an answer.\"",
                &format!("{prompt_behavior}prompt = \"Produce an answer.\""),
            ),
        )
        .unwrap();
        let preview = preview(name);
        let (cli, cli_args) = run("cli", "prompt-behavior-compat-cli.json");
        let (mcp, mcp_args) = run("mcp", "prompt-behavior-compat-mcp.json");
        assert_system(&cli_args, "--cli-system");
        assert_system(&mcp_args, "--mcp-system");
        if let Some((baseline_preview, baseline_cli, baseline_mcp)) = &prompt_behavior_compatibility
        {
            assert_eq!(&preview, baseline_preview, "{name} preview changed");
            assert_eq!(&cli, baseline_cli, "{name} CLI prompt changed");
            assert_eq!(&mcp, baseline_mcp, "{name} MCP prompt changed");
        } else {
            prompt_behavior_compatibility = Some((preview, cli, mcp));
        }
    }
    let baseline_behavior_preview = &prompt_behavior_compatibility.as_ref().unwrap().0;
    let baseline_behavior = extract_behavior(baseline_behavior_preview);
    assert!(
        baseline_behavior.contains("agent-tone"),
        "{baseline_behavior}"
    );
    assert!(
        baseline_behavior.contains("root-tone"),
        "{baseline_behavior}"
    );

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
        assert_eq!(
            legacy_mcp,
            expected_mcp_prompt(&session),
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

    let prompt_canonical = canonical.replace(
        "prompt = \"Produce an answer.\"\noutput = [\"slot:answer\"]",
        "prompt = \"Produce first answer.\"\nintent = { require = [{ id = \"shared\", summary = \"Prompt replacement text.\" }, { id = \"current-only\", summary = \"Current prompt guidance.\" }] }\noutput = [\"slot:answer\"]\n\n[[procedure.sequence]]\nid = \"next\"\ntitle = \"Next\"\nagent = \"agent:worker\"\nprompt = \"Produce second answer.\"\nintent = { require = [{ id = \"next-only\", summary = \"Next prompt guidance.\" }] }\ninput = [\"slot:answer\"]\noutput = [\"slot:answer\"]",
    );
    fs::write(&generated, &prompt_canonical).unwrap();
    let previews = run_ctx(
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
    assert_exit_code(&previews, 0);
    let (stdout, _) = utf8(&previews);
    let previews: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let first_preview = previews["frames"][0]["prompt"].as_str().unwrap();
    let second_preview = previews["frames"][1]["prompt"].as_str().unwrap();
    let first_intent = extract_intent(first_preview);
    let second_intent = extract_intent(second_preview);
    assert!(first_intent.find("root-only").unwrap() < first_intent.find("agent-only").unwrap());
    assert!(first_intent.find("agent-only").unwrap() < first_intent.find("shared").unwrap());
    assert!(first_intent.find("shared").unwrap() < first_intent.find("current-only").unwrap());
    assert_eq!(first_intent.matches("id=\"shared\"").count(), 1);
    assert!(first_intent.contains("Prompt replacement text."));
    assert!(!first_intent.contains("Assigned replacement text."));
    assert!(!first_intent.contains("next-only"));
    assert!(second_intent.contains("next-only"));
    assert!(second_intent.find("root-only").unwrap() < second_intent.find("shared").unwrap());
    assert!(second_intent.find("shared").unwrap() < second_intent.find("agent-only").unwrap());
    assert!(second_intent.find("agent-only").unwrap() < second_intent.find("next-only").unwrap());
    assert_eq!(second_intent.matches("id=\"shared\"").count(), 1);
    assert!(!second_intent.contains("current-only"));
    assert_unassigned_absent(first_preview);
    assert_unassigned_absent(second_preview);
    let (_, cli_args) = run("cli", "prompt-cli.json");
    let first_cli = fs::read_to_string(capture.with_extension("txt.0")).unwrap();
    let second_cli = fs::read_to_string(capture.with_extension("txt.1")).unwrap();
    assert_eq!(extract_intent(&first_cli), first_intent);
    assert_eq!(extract_intent(&second_cli), second_intent);
    assert_system(&cli_args, "--cli-system");
    assert_unassigned_absent(&first_cli);
    assert_unassigned_absent(&second_cli);
    let (_, mcp_args) = run("mcp", "prompt-mcp.json");
    let first_mcp = fs::read_to_string(capture.with_extension("txt.0")).unwrap();
    let second_mcp = fs::read_to_string(capture.with_extension("txt.1")).unwrap();
    assert_eq!(extract_intent(&first_mcp), first_intent);
    assert_eq!(extract_intent(&second_mcp), second_intent);
    assert_system(&mcp_args, "--mcp-system");
    assert_unassigned_absent(&first_mcp);
    assert_unassigned_absent(&second_mcp);

    let prompt_only = canonical.replace(worker_intent, "").replace(
        "prompt = \"Produce an answer.\"",
        "prompt = \"Produce an answer.\"\nintent = { require = [{ id = \"prompt-only\", summary = \"Prompt-only guidance.\" }] }",
    );
    fs::write(&generated, &prompt_only).unwrap();
    let (_, _) = run("mcp", "prompt-only-mcp.json");
    let prompt_only_mcp = fs::read_to_string(capture.with_extension("txt.0")).unwrap();
    let prompt_only_intent = extract_intent(&prompt_only_mcp);
    assert!(prompt_only_intent.contains("Prompt-only guidance."));
    assert!(prompt_only_intent.contains("prompt-only"));
    assert!(!prompt_only_intent.contains("agent-only"));

    let prompt_conflict = canonical.replace(
        "prompt = \"Produce an answer.\"",
        "prompt = \"Produce an answer.\"\nintent = { avoid = [{ id = \"shared\", summary = \"Prompt conflict.\" }] }",
    );
    fs::write(&generated, prompt_conflict).unwrap();
    require_success(
        "approve prompt conflict fixture",
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
            home.join("prompt-conflict.json").to_str().unwrap(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert!(
        !conflict.status.success(),
        "prompt conflict unexpectedly dispatched"
    );
    let (_, stderr) = utf8(&conflict);
    assert!(
        stderr.contains("effective guidance id \"shared\" cannot appear in both require and avoid when ready-prompt intent participates")
    );
    assert!(
        !marker.exists(),
        "harness ran before the prompt conflict was rejected"
    );

    let mut prompt_compatibility = None;
    for (name, prompt_intent) in [("prompt-absent", ""), ("prompt-empty", "intent = {}\n")] {
        fs::write(
            &generated,
            canonical.replace(
                "prompt = \"Produce an answer.\"",
                &format!("{prompt_intent}prompt = \"Produce an answer.\""),
            ),
        )
        .unwrap();
        let preview = preview(name);
        let (cli, cli_args) = run("cli", "prompt-compat-cli.json");
        let (mcp, mcp_args) = run("mcp", "prompt-compat-mcp.json");
        assert_system(&cli_args, "--cli-system");
        assert_system(&mcp_args, "--mcp-system");
        if let Some((baseline_preview, baseline_cli, baseline_mcp)) = &prompt_compatibility {
            assert_eq!(&preview, baseline_preview, "{name} preview changed");
            assert_eq!(&cli, baseline_cli, "{name} CLI prompt changed");
            assert_eq!(&mcp, baseline_mcp, "{name} MCP prompt changed");
        } else {
            prompt_compatibility = Some((preview, cli, mcp));
        }
    }

    // 0255.9: a nested named-sequence chain (procedure -> outer-ref ->
    // outer.sequence -> outer-inner-ref -> inner.sequence). `outer-ref` and
    // `outer-inner-ref` are pure grouping containers, never dispatch targets.
    // Sequence-local indices deliberately collide across distinct owners —
    // index 0: `outer-direct` (in `outer`) vs `inner-absent` (in `inner`);
    // index 1: `outer-inner-ref` (in `outer`) vs `inner-guided` (in `inner`)
    // vs `later` (top-level, after the container) — so a lookup keyed on
    // index or item id alone, rather than full structural position, would
    // select the wrong declaration.
    // Named leaves must prove the same root->agent->prompt precedence used by
    // the top-level behavior fixture above, so this base carries the same
    // root/agent behavior alongside the existing root/agent intent — reusing
    // `root_behavior`/`worker_behavior` rather than duplicating them.
    let nested_canonical_base = canonical
        .replacen("\n[[agent]]", &format!("{root_behavior}\n[[agent]]"), 1)
        .replace(worker_intent, &format!("{worker_intent}\n{worker_behavior}"))
        .replace(
            "[agent.intent]\nrequire = [{ id = \"unassigned-only\", summary = \"Unassigned guidance.\" }]",
            "[agent.intent]\nrequire = [{ id = \"unassigned-only\", summary = \"Unassigned guidance.\" }]\n[agent.behavior]\ntone = [{ id = \"unassigned-tone\", summary = \"Unassigned behavior.\" }]",
        );
    let nested_header = nested_canonical_base.split_once("[procedure]").unwrap().0;
    let nested_fixture = |inner_absent_guidance: &str| {
        format!(
            r#"{nested_header}[[sequence.inner.sequence]]
id = "inner-absent"
title = "Inner absent"
agent = "agent:worker"
prompt = "Produce inner-absent answer."
{inner_absent_guidance}output = ["slot:answer"]

[[sequence.inner.sequence]]
id = "inner-guided"
title = "Inner guided"
agent = "agent:worker"
prompt = "Produce inner-guided answer."
intent = {{ require = [{{ id = "shared", summary = "Inner guided replacement text." }}, {{ id = "inner-guided-marker", summary = "Inner guided leaf marker." }}] }}
behavior = {{ tone = [{{ id = "shared-tone", summary = "Inner guided tone." }}, {{ id = "inner-guided-tone-marker", summary = "Inner guided tone marker." }}], verbosity = {{ id = "inner-guided-verbosity-marker", summary = "Inner guided verbosity marker." }} }}
output = ["slot:answer"]

[[sequence.outer.sequence]]
id = "outer-direct"
title = "Outer direct"
agent = "agent:worker"
prompt = "Produce outer-direct answer."
intent = {{ require = [{{ id = "outer-direct-marker", summary = "Outer direct sibling marker." }}] }}
behavior = {{ tone = [{{ id = "outer-direct-tone-marker", summary = "Outer direct sibling tone marker." }}] }}
output = ["slot:answer"]

[[sequence.outer.sequence]]
id = "outer-inner-ref"
title = "Outer inner ref"
kind = "sequence"
sequence = "sequence:inner"

[procedure]
description = "Nested named-sequence chain proving structural leaf selection."

[[procedure.sequence]]
id = "outer-ref"
title = "Outer ref"
kind = "sequence"
sequence = "sequence:outer"

[[procedure.sequence]]
id = "later"
title = "Later"
agent = "agent:worker"
prompt = "Produce later answer."
intent = {{ require = [{{ id = "later-prompt-marker", summary = "Later top-level prompt marker." }}] }}
behavior = {{ tone = [{{ id = "later-tone-marker", summary = "Later top-level prompt tone marker." }}] }}
output = ["slot:answer"]
"#
        )
    };

    let preview_all = |label: &str| -> Vec<String> {
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
        json["frames"]
            .as_array()
            .unwrap_or_else(|| panic!("{label} preview had no frames array: {json}"))
            .iter()
            .map(|frame| {
                frame["prompt"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{label} preview frame had no prompt: {json}"))
                    .to_string()
            })
            .collect()
    };

    let assert_nested_markers = |label: &str, prompts: &[String]| {
        assert_eq!(prompts.len(), 4, "{label} frame count: {prompts:?}");
        let outer_direct = &prompts[0];
        let inner_absent = &prompts[1];
        let inner_guided = &prompts[2];
        let later = &prompts[3];

        assert!(
            outer_direct.contains("outer-direct-marker"),
            "{label} outer-direct missing its own marker: {outer_direct}"
        );
        assert!(
            outer_direct.contains("outer-direct-tone-marker"),
            "{label} outer-direct missing its own behavior marker: {outer_direct}"
        );
        assert!(
            !outer_direct.contains("inner-guided-marker")
                && !outer_direct.contains("later-prompt-marker")
                && !outer_direct.contains("inner-guided-tone-marker")
                && !outer_direct.contains("later-tone-marker"),
            "{label} outer-direct leaked a sibling/later marker: {outer_direct}"
        );

        assert!(
            !inner_absent.contains("outer-direct-marker")
                && !inner_absent.contains("inner-guided-marker")
                && !inner_absent.contains("later-prompt-marker")
                && !inner_absent.contains("outer-direct-tone-marker")
                && !inner_absent.contains("inner-guided-tone-marker")
                && !inner_absent.contains("later-tone-marker"),
            "{label} inner-absent leaked a container/sibling/later marker: {inner_absent}"
        );

        assert!(
            inner_guided.contains("inner-guided-marker"),
            "{label} inner-guided missing its own marker: {inner_guided}"
        );
        assert!(
            !inner_guided.contains("outer-direct-marker")
                && !inner_guided.contains("later-prompt-marker")
                && !inner_guided.contains("outer-direct-tone-marker")
                && !inner_guided.contains("later-tone-marker"),
            "{label} inner-guided leaked a stale/previous/later marker: {inner_guided}"
        );
        assert_eq!(
            inner_guided.matches("id=\"shared\"").count(),
            1,
            "{label} inner-guided: {inner_guided}"
        );
        assert!(
            inner_guided.find("root-only").unwrap() < inner_guided.find("agent-only").unwrap(),
            "{label} inner-guided root/agent ordering: {inner_guided}"
        );
        assert!(
            inner_guided.find("agent-only").unwrap() < inner_guided.find("shared").unwrap(),
            "{label} inner-guided agent/shared ordering: {inner_guided}"
        );
        assert!(
            inner_guided.find("shared").unwrap()
                < inner_guided.find("inner-guided-marker").unwrap(),
            "{label} inner-guided shared/prompt ordering: {inner_guided}"
        );
        assert!(
            inner_guided.contains("Inner guided replacement text."),
            "{label} inner-guided scalar precedence: {inner_guided}"
        );
        assert!(
            !inner_guided.contains("Assigned replacement text."),
            "{label} inner-guided stale agent text leaked: {inner_guided}"
        );

        // Behavior mirrors the intent proof above: root->agent->named-leaf
        // ordering, same-axis (tone) replacement, leaf scalar (verbosity)
        // precedence over the agent scalar, and broader-axis (initiative)
        // fallback all the way to root when neither agent nor leaf sets it.
        assert!(
            inner_guided.contains("inner-guided-tone-marker"),
            "{label} inner-guided missing its own behavior marker: {inner_guided}"
        );
        assert_eq!(
            inner_guided.matches("id=\"shared-tone\"").count(),
            1,
            "{label} inner-guided: {inner_guided}"
        );
        assert!(
            inner_guided.find("root-tone").unwrap() < inner_guided.find("agent-tone").unwrap(),
            "{label} inner-guided root/agent tone ordering: {inner_guided}"
        );
        assert!(
            inner_guided.find("agent-tone").unwrap() < inner_guided.find("shared-tone").unwrap(),
            "{label} inner-guided agent/shared tone ordering: {inner_guided}"
        );
        assert!(
            inner_guided.find("shared-tone").unwrap()
                < inner_guided.find("inner-guided-tone-marker").unwrap(),
            "{label} inner-guided shared/leaf tone ordering: {inner_guided}"
        );
        assert!(
            inner_guided.contains("Inner guided tone."),
            "{label} inner-guided tone scalar precedence: {inner_guided}"
        );
        assert!(
            !inner_guided.contains("Agent tone."),
            "{label} inner-guided stale agent tone text leaked: {inner_guided}"
        );
        assert!(
            inner_guided.contains("inner-guided-verbosity-marker")
                && !inner_guided.contains("agent-verbosity"),
            "{label} inner-guided verbosity scalar precedence: {inner_guided}"
        );
        assert!(
            inner_guided.contains("root-initiative"),
            "{label} inner-guided broader-axis fallback to root: {inner_guided}"
        );
        for unchanged in ["agent-directness", "agent-scope", "agent-uncertainty"] {
            assert!(
                inner_guided.contains(unchanged),
                "{label} inner-guided agent-scalar fallback: {inner_guided}"
            );
        }

        assert!(
            later.contains("later-prompt-marker"),
            "{label} later missing its own marker: {later}"
        );
        assert!(
            later.contains("later-tone-marker"),
            "{label} later missing its own behavior marker: {later}"
        );
        assert!(
            !later.contains("outer-direct-marker")
                && !later.contains("inner-guided-marker")
                && !later.contains("outer-direct-tone-marker")
                && !later.contains("inner-guided-tone-marker"),
            "{label} later leaked a container/sibling marker: {later}"
        );

        for prompt in prompts {
            assert_unassigned_absent(prompt);
            assert!(
                !prompt.contains("unassigned-tone"),
                "{label} leaked unassigned-agent behavior: {prompt}"
            );
            // `outer-ref`/`outer-inner-ref` are pure grouping containers that
            // schema validation forbids from ever carrying intent/behavior
            // (`ln and behavior are valid only on prompt sequence items`), so
            // the only leak they could cause is their own structural
            // identifiers bleeding into a leaf's resolved guidance.
            for container_id in [
                "outer-ref",
                "outer-inner-ref",
                "sequence:outer",
                "sequence:inner",
            ] {
                assert!(
                    !prompt.contains(container_id),
                    "{label} leaked a container identifier {container_id}: {prompt}"
                );
            }
            assert!(
                !prompt.contains("source=\""),
                "{label} leaked static declaration source attribution: {prompt}"
            );
        }
    };

    let mut nested_baseline: Option<(Vec<String>, Vec<String>, Vec<String>)> = None;
    for (name, inner_absent_guidance) in [
        ("nested-absent", ""),
        ("nested-empty", "intent = {}\n"),
        ("nested-behavior-empty", "behavior = {}\n"),
    ] {
        fs::write(&generated, nested_fixture(inner_absent_guidance)).unwrap();
        require_success(
            "approve nested fixture",
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

        let preview_prompts = preview_all(name);
        assert_nested_markers(&format!("{name} preview"), &preview_prompts);

        let (_, cli_args) = run("cli", "nested-cli.json");
        assert_system(&cli_args, "--cli-system");
        let cli_prompts: Vec<String> = (0..4)
            .map(|index| {
                fs::read_to_string(capture.with_extension(format!("txt.{index}"))).unwrap()
            })
            .collect();
        assert_nested_markers(&format!("{name} cli"), &cli_prompts);

        let (_, mcp_args) = run("mcp", "nested-mcp.json");
        assert_system(&mcp_args, "--mcp-system");
        let mcp_prompts: Vec<String> = (0..4)
            .map(|index| {
                fs::read_to_string(capture.with_extension(format!("txt.{index}"))).unwrap()
            })
            .collect();
        assert_nested_markers(&format!("{name} mcp"), &mcp_prompts);

        // The direct named leaf, the recursively nested leaf, and the
        // post-sequence top-level leaf must all select the same structurally
        // current declaration regardless of transport: complete intent and
        // behavior blocks are byte-identical across preview, CLI, and MCP.
        for index in [0usize, 2, 3] {
            let preview_intent = extract_intent(&preview_prompts[index]);
            let preview_behavior = extract_behavior(&preview_prompts[index]);
            assert_eq!(
                extract_intent(&cli_prompts[index]),
                preview_intent,
                "{name} frame {index} intent differs between preview and CLI"
            );
            assert_eq!(
                extract_intent(&mcp_prompts[index]),
                preview_intent,
                "{name} frame {index} intent differs between preview and MCP"
            );
            assert_eq!(
                extract_behavior(&cli_prompts[index]),
                preview_behavior,
                "{name} frame {index} behavior differs between preview and CLI"
            );
            assert_eq!(
                extract_behavior(&mcp_prompts[index]),
                preview_behavior,
                "{name} frame {index} behavior differs between preview and MCP"
            );
        }

        // A named leaf with no local guidance must compose to the exact same
        // root+agent-only guidance as the top-level no-participation oracle
        // established earlier in this test, not merely to a value equal to
        // its own empty-guidance sibling.
        assert_eq!(
            extract_intent(&preview_prompts[1]),
            happy_intent,
            "{name} inner-absent intent must match the established no-participation oracle"
        );
        assert_eq!(
            extract_behavior(&preview_prompts[1]),
            behavior_block,
            "{name} inner-absent behavior must match the established no-participation oracle"
        );

        if let Some((baseline_preview, baseline_cli, baseline_mcp)) = &nested_baseline {
            assert_eq!(&preview_prompts, baseline_preview, "{name} preview changed");
            assert_eq!(&cli_prompts, baseline_cli, "{name} CLI prompts changed");
            assert_eq!(&mcp_prompts, baseline_mcp, "{name} MCP prompts changed");
        } else {
            nested_baseline = Some((preview_prompts, cli_prompts, mcp_prompts));
        }
    }

    // A named leaf's own prompt-scoped guidance must be able to enable
    // categorized MCP participation entirely on its own, with no assigned-
    // agent-scoped guidance declared anywhere in the trait.
    let prompt_only_nested_header = canonical.replace(worker_intent, "");
    let prompt_only_nested_header = prompt_only_nested_header
        .split_once("[procedure]")
        .unwrap()
        .0;
    let prompt_only_nested_fixture = format!(
        r#"{prompt_only_nested_header}[[sequence.outer.sequence]]
id = "outer-prompt-only"
title = "Outer prompt only"
agent = "agent:worker"
prompt = "Produce prompt-only answer."
intent = {{ require = [{{ id = "prompt-only-marker", summary = "Prompt-only nested guidance." }}] }}
behavior = {{ tone = [{{ id = "prompt-only-tone-marker", summary = "Prompt-only nested tone." }}] }}
output = ["slot:answer"]

[procedure]
description = "Nested named-sequence prompt-only participation."

[[procedure.sequence]]
id = "outer-ref"
title = "Outer ref"
kind = "sequence"
sequence = "sequence:outer"
"#
    );
    fs::write(&generated, &prompt_only_nested_fixture).unwrap();
    require_success(
        "approve nested prompt-only fixture",
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
    let (_, _) = run("mcp", "nested-prompt-only-mcp.json");
    let nested_prompt_only_mcp = fs::read_to_string(capture.with_extension("txt.0")).unwrap();
    let nested_prompt_only_intent = extract_intent(&nested_prompt_only_mcp);
    let nested_prompt_only_behavior = extract_behavior(&nested_prompt_only_mcp);
    assert!(
        nested_prompt_only_intent.contains("prompt-only-marker"),
        "{nested_prompt_only_intent}"
    );
    assert!(
        !nested_prompt_only_intent.contains("agent-only"),
        "{nested_prompt_only_intent}"
    );
    assert!(
        nested_prompt_only_behavior.contains("prompt-only-tone-marker"),
        "{nested_prompt_only_behavior}"
    );
    assert!(
        !nested_prompt_only_behavior.contains("agent-tone"),
        "{nested_prompt_only_behavior}"
    );

    // A nested (non-top-level) leaf's require/avoid conflict must still fail
    // before the harness ever runs — the structural admission broadening
    // must not move conflict detection later than dispatch.
    let nested_conflict_fixture = format!(
        r#"{nested_header}[[sequence.outer.sequence]]
id = "outer-conflict"
title = "Outer conflict"
agent = "agent:worker"
prompt = "Produce an answer."
intent = {{ avoid = [{{ id = "shared", summary = "Prompt conflict." }}] }}
output = ["slot:answer"]

[procedure]
description = "Nested named-sequence conflict fixture."

[[procedure.sequence]]
id = "outer-ref"
title = "Outer ref"
kind = "sequence"
sequence = "sequence:outer"
"#
    );
    fs::write(&generated, &nested_conflict_fixture).unwrap();
    require_success(
        "approve nested conflict fixture",
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
    let nested_conflict = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--out",
            home.join("nested-conflict.json").to_str().unwrap(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert!(
        !nested_conflict.status.success(),
        "nested prompt conflict unexpectedly dispatched"
    );
    let (_, stderr) = utf8(&nested_conflict);
    assert!(
        stderr.contains(
            "effective guidance id \"shared\" cannot appear in both require and avoid when ready-prompt intent participates"
        )
    );
    assert!(
        !marker.exists(),
        "harness ran before the nested prompt conflict was rejected"
    );

    // 0255.10: a branch reached through the same named-sequence chain
    // (procedure -> branch-ref -> branch-outer.sequence -> the-branch
    // (branch) -> branch-then/branch-otherwise.sequence), with a second
    // branch nested inside the `then` arm (branch-then.sequence ->
    // nested-branch-ref (branch) -> nested-branch-then.sequence) — a
    // recursively nested branch reached through mixed sequence/branch
    // segments. `nested-branch-ref` deliberately has no `otherwise`, so a
    // false nested choice must emit no leaf frame at all. Local indices
    // collide across every owner (index 0 in `branch-outer`, `branch-then`,
    // `branch-otherwise`, and `nested-branch-then` alike), and `then-leaf`/
    // `nested-then-leaf`/`otherwise-leaf` all reuse the "shared" id to prove
    // the same root->agent->leaf replacement precedence the earlier
    // named-leaf proof established, now across sibling branch arms.
    // `then-leaf` additionally declares its own `verbosity` scalar, proving
    // a branch leaf's own scalar wins over the agent's scalar exactly as
    // `inner_guided`'s verbosity override did for a plain named leaf.
    let branch_header = nested_header.replace(
        "[[slot]]\nid = \"answer\"\nschema = \"schema:text\"\ndescription = \"Answer.\"\n",
        "[[slot]]\nid = \"answer\"\nschema = \"schema:text\"\ndescription = \"Answer.\"\n\n[[slot]]\nid = \"choice\"\nschema = \"schema:text\"\ndescription = \"Outer branch choice.\"\n\n[[slot]]\nid = \"nested-choice\"\nschema = \"schema:text\"\ndescription = \"Nested branch choice.\"\n",
    );
    assert_ne!(branch_header, nested_header, "slot insertion point moved");
    let branch_fixture = format!(
        r#"{branch_header}[[sequence.branch-then.sequence]]
id = "then-leaf"
title = "Then leaf"
agent = "agent:worker"
prompt = "Produce then-leaf answer."
intent = {{ require = [{{ id = "shared", summary = "Then leaf replacement text." }}, {{ id = "then-arm-marker", summary = "Then leaf marker." }}] }}
behavior = {{ tone = [{{ id = "shared-tone", summary = "Then leaf tone." }}, {{ id = "then-arm-tone-marker", summary = "Then leaf tone marker." }}], verbosity = {{ id = "then-arm-verbosity-marker", summary = "Then leaf verbosity marker." }} }}
output = ["slot:answer"]

[[sequence.branch-then.sequence]]
id = "nested-branch-ref"
title = "Nested branch ref"
kind = "branch"
sequence = "sequence:nested-branch-then"
when = {{ slot = "slot:nested-choice", equals = "yes" }}

[[sequence.nested-branch-then.sequence]]
id = "nested-then-leaf"
title = "Nested then leaf"
agent = "agent:worker"
prompt = "Produce nested-then-leaf answer."
intent = {{ require = [{{ id = "shared", summary = "Nested then leaf replacement text." }}, {{ id = "nested-arm-marker", summary = "Nested then leaf marker." }}] }}
behavior = {{ tone = [{{ id = "shared-tone", summary = "Nested then leaf tone." }}, {{ id = "nested-arm-tone-marker", summary = "Nested then leaf tone marker." }}] }}
output = ["slot:answer"]

[[sequence.branch-otherwise.sequence]]
id = "otherwise-leaf"
title = "Otherwise leaf"
agent = "agent:worker"
prompt = "Produce otherwise-leaf answer."
intent = {{ require = [{{ id = "shared", summary = "Otherwise leaf replacement text." }}, {{ id = "otherwise-arm-marker", summary = "Otherwise leaf marker." }}] }}
behavior = {{ tone = [{{ id = "shared-tone", summary = "Otherwise leaf tone." }}, {{ id = "otherwise-arm-tone-marker", summary = "Otherwise leaf tone marker." }}] }}
output = ["slot:answer"]

[[sequence.branch-outer.sequence]]
id = "pre-branch-sibling"
title = "Pre branch sibling"
agent = "agent:worker"
prompt = "Produce pre-branch-sibling answer."
intent = {{ require = [{{ id = "pre-branch-sibling-marker", summary = "Pre branch sibling marker." }}] }}
output = ["slot:answer"]

[[sequence.branch-outer.sequence]]
id = "the-branch"
title = "The branch"
kind = "branch"
sequence = "sequence:branch-then"
otherwise = "sequence:branch-otherwise"
when = {{ slot = "slot:choice", equals = "yes" }}

[[sequence.branch-outer.sequence]]
id = "post-branch-sibling"
title = "Post branch sibling"
agent = "agent:worker"
prompt = "Produce post-branch-sibling answer."
intent = {{ require = [{{ id = "post-branch-sibling-marker", summary = "Post branch sibling marker." }}] }}
output = ["slot:answer"]

[procedure]
description = "Nested named-sequence branch leaf guidance."

[[procedure.sequence]]
id = "decide"
title = "Decide"
agent = "agent:worker"
prompt = "Decide the outer branch."
output = ["slot:choice"]

[[procedure.sequence]]
id = "decide-nested"
title = "Decide nested"
agent = "agent:worker"
prompt = "Decide the nested branch."
output = ["slot:nested-choice"]

[[procedure.sequence]]
id = "branch-ref"
title = "Branch ref"
kind = "sequence"
sequence = "sequence:branch-outer"

[[procedure.sequence]]
id = "branch-later"
title = "Branch later"
agent = "agent:worker"
prompt = "Produce branch-later answer."
intent = {{ require = [{{ id = "branch-later-marker", summary = "Branch later top-level prompt marker." }}] }}
output = ["slot:answer"]
"#
    );
    fs::write(&generated, &branch_fixture).unwrap();
    require_success(
        "approve branch fixture",
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

    // No-session static preview exposes both arms of every branch, including
    // the nested one, regardless of any guard value.
    let branch_preview_prompts = preview_all("branch");
    assert_eq!(
        branch_preview_prompts.len(),
        8,
        "branch preview frame count: {branch_preview_prompts:?}"
    );
    let branch_preview_then = &branch_preview_prompts[3];
    let branch_preview_nested_then = &branch_preview_prompts[4];
    let branch_preview_otherwise = &branch_preview_prompts[5];
    assert!(branch_preview_then.contains("then-arm-marker"));
    assert!(branch_preview_nested_then.contains("nested-arm-marker"));
    assert!(branch_preview_otherwise.contains("otherwise-arm-marker"));
    // Each static declaration renders its own declared prompt text and no
    // other arm's, proving the no-session preview attributes every projected
    // leaf to its own owner path rather than a shared/ambiguous lookup.
    assert!(branch_preview_then.contains("Produce then-leaf answer."));
    assert!(branch_preview_nested_then.contains("Produce nested-then-leaf answer."));
    assert!(branch_preview_otherwise.contains("Produce otherwise-leaf answer."));
    for prompt in &branch_preview_prompts {
        for container_id in ["branch-ref", "the-branch", "nested-branch-ref"] {
            assert!(
                !prompt.contains(container_id),
                "branch preview leaked container identifier {container_id}: {prompt}"
            );
        }
        assert!(
            !prompt.contains("source=\""),
            "branch preview leaked static declaration source attribution: {prompt}"
        );
    }

    // Static declaration rendering (`traits internal export`) attributes
    // every branch arm's own declared guidance to its own structural
    // source — `sequence:<owner>/<item>` — with a source-tagged block that
    // carries only that declaration's own text, never a merged/composed
    // ready-frame view. The render is explicitly labeled static throughout
    // and makes no active/ready/selected claim about any one arm.
    let branch_export_dir = home.join("branch-static-export");
    let branch_export = run_ctx(
        &[
            "traits",
            "internal",
            "export",
            "--file",
            generated.to_str().unwrap(),
            "--profile",
            "agent-skills",
            "--format",
            "compat",
            "--out",
            branch_export_dir.to_str().unwrap(),
        ],
        &repo,
        &home,
    );
    assert_exit_code(&branch_export, 0);
    let branch_skill =
        fs::read_to_string(branch_export_dir.join("agent-intent").join("SKILL.md")).unwrap();
    for (source, own_text) in [
        (
            "sequence:branch-then/then-leaf",
            "Then leaf replacement text.",
        ),
        (
            "sequence:branch-otherwise/otherwise-leaf",
            "Otherwise leaf replacement text.",
        ),
        (
            "sequence:nested-branch-then/nested-then-leaf",
            "Nested then leaf replacement text.",
        ),
    ] {
        assert!(
            branch_skill.contains(&format!("source=\"{source}\"")),
            "static export missing branch-arm source attribution source=\"{source}\": {branch_skill}"
        );
        // The `id="shared"` declaration is the one every arm collides on, so
        // its own-source block is the one that proves attribution names the
        // owning leaf rather than a merged view: search from that specific
        // tag (not the bare `source="..."` above, which also matches the
        // group-info line that carries no text) to the tag's own closing
        // `</intent>`.
        let own_tag = format!("id=\"shared\" source=\"{source}\">");
        let block_start = branch_skill
            .find(&own_tag)
            .unwrap_or_else(|| panic!("static export missing {own_tag}: {branch_skill}"));
        let block_end = branch_skill[block_start..]
            .find("</intent>")
            .map(|offset| block_start + offset)
            .unwrap_or(branch_skill.len());
        assert!(
            branch_skill[block_start..block_end].contains(own_text),
            "static export source block for {own_tag} missing its own text: {branch_skill}"
        );
    }
    // Static export never selects a branch outcome, so it must carry no
    // claim that any one arm is the active/ready one — every arm's own
    // declaration renders side by side instead, explicitly labeled static.
    assert!(
        branch_skill.contains("Static render advisory")
            && branch_skill.contains("Static host note"),
        "static export must remain explicitly labeled static, not an active/ready claim: {branch_skill}"
    );
    for claim in ["active", "ready", "selected", "not-yet-accepted"] {
        assert!(
            !branch_skill.to_ascii_lowercase().contains(claim),
            "static export must not claim any arm is {claim}: {branch_skill}"
        );
    }

    let read_frames = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| {
                fs::read_to_string(capture.with_extension(format!("txt.{index}"))).unwrap()
            })
            .collect()
    };

    // Session-scoped preview (as opposed to the no-session preview above):
    // `--session <path>` alone previews the active (not-yet-accepted) frame;
    // `--session <path> --step <id>` reconstructs a completed frame exactly
    // as it activated, historically. Both must expose only the recorded arm
    // — never the sibling arm a no-session preview projects alongside it.
    let session_preview = |label: &str, session: &std::path::Path, step: Option<&str>| -> String {
        let session_str = session.to_str().unwrap().to_string();
        let mut argv = vec![
            "traits",
            "internal",
            "preview",
            "--file",
            generated.to_str().unwrap(),
            "--session",
            session_str.as_str(),
            "--json",
        ];
        if let Some(step) = step {
            argv.push("--step");
            argv.push(step);
        }
        let output = run_ctx(&argv, &repo, &home);
        assert_exit_code(&output, 0);
        let (stdout, stderr) = utf8(&output);
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
            panic!(
                "{label} session preview was not JSON: {error}\nstdout={stdout}\nstderr={stderr}"
            )
        });
        json["frames"][0]["prompt"]
            .as_str()
            .unwrap_or_else(|| panic!("{label} session preview had no prompt: {json}"))
            .to_string()
    };

    // True outer choice, true nested choice: both branches select their
    // `sequence` (then) arm.
    let (_, tt_cli_args) = run_with_env(
        "cli",
        "branch-tt-cli.json",
        &[
            ("CTX_TEST_CHOICE", "yes"),
            ("CTX_TEST_NESTED_CHOICE", "yes"),
        ],
    );
    assert_system(&tt_cli_args, "--cli-system");
    let tt_frames = read_frames(7);
    let tt_then = &tt_frames[3];
    let tt_nested_then = &tt_frames[4];
    let tt_post_sibling = &tt_frames[5];
    let tt_later = &tt_frames[6];
    assert!(tt_then.contains("then-arm-marker"), "{tt_then}");
    assert!(
        tt_nested_then.contains("nested-arm-marker"),
        "{tt_nested_then}"
    );
    assert!(
        !tt_then.contains("nested-arm-marker") && !tt_then.contains("otherwise-arm-marker"),
        "{tt_then}"
    );
    assert!(
        !tt_nested_then.contains("then-arm-marker")
            && !tt_nested_then.contains("otherwise-arm-marker"),
        "{tt_nested_then}"
    );
    assert_eq!(
        tt_nested_then.matches("id=\"shared\"").count(),
        1,
        "{tt_nested_then}"
    );
    assert!(
        tt_nested_then.contains("Nested then leaf replacement text."),
        "{tt_nested_then}"
    );
    assert!(
        !tt_post_sibling.contains("then-arm-marker")
            && !tt_post_sibling.contains("nested-arm-marker"),
        "{tt_post_sibling}"
    );
    assert!(tt_later.contains("branch-later-marker"), "{tt_later}");
    for container_id in ["branch-ref", "the-branch", "nested-branch-ref"] {
        for prompt in &tt_frames {
            assert!(
                !prompt.contains(container_id),
                "true/true CLI leaked container identifier {container_id}: {prompt}"
            );
        }
    }
    assert_eq!(
        extract_intent(tt_nested_then),
        extract_intent(branch_preview_nested_then),
        "true/true nested-then intent differs between preview and CLI"
    );
    assert_eq!(
        extract_behavior(tt_nested_then),
        extract_behavior(branch_preview_nested_then),
        "true/true nested-then behavior differs between preview and CLI"
    );
    assert_eq!(
        extract_intent(tt_then),
        extract_intent(branch_preview_then),
        "true/true then (direct leaf) intent differs between preview and CLI"
    );
    assert_eq!(
        extract_behavior(tt_then),
        extract_behavior(branch_preview_then),
        "true/true then (direct leaf) behavior differs between preview and CLI"
    );
    let tt_session = home.join("branch-tt-cli.json");

    // True outer choice, false nested choice, over MCP: the nested branch has
    // no `otherwise`, so a false nested choice must emit no leaf frame — only
    // 6 frames total instead of 7, and the `.calls` ledger proves the harness
    // was invoked exactly that many times, not silently skipped.
    let (_, tf_mcp_args) = run_with_env(
        "mcp",
        "branch-tf-mcp.json",
        &[("CTX_TEST_CHOICE", "yes"), ("CTX_TEST_NESTED_CHOICE", "no")],
    );
    assert_system(&tf_mcp_args, "--mcp-system");
    let calls = fs::read_to_string(capture.with_extension("txt.calls")).unwrap();
    assert_eq!(
        calls.lines().count(),
        6,
        "false nested choice without otherwise must not add a leaf frame: {calls:?}"
    );
    let tf_frames = read_frames(6);
    let tf_then = &tf_frames[3];
    let tf_post_sibling = &tf_frames[4];
    let tf_later = &tf_frames[5];
    assert!(tf_then.contains("then-arm-marker"), "{tf_then}");
    assert!(
        !tf_then.contains("nested-arm-marker") && !tf_then.contains("otherwise-arm-marker"),
        "{tf_then}"
    );
    assert!(
        !tf_post_sibling.contains("nested-arm-marker"),
        "{tf_post_sibling}"
    );
    assert!(tf_later.contains("branch-later-marker"), "{tf_later}");
    assert_eq!(
        extract_intent(tf_then),
        extract_intent(branch_preview_then),
        "true/false then intent differs between preview and MCP"
    );
    assert_eq!(
        extract_behavior(tf_then),
        extract_behavior(branch_preview_then),
        "true/false then behavior differs between preview and MCP"
    );
    let tf_session = home.join("branch-tf-mcp.json");

    // False outer choice: the top-level branch selects its `otherwise` arm
    // and the `then`/nested-branch arms never dispatch at all.
    let (_, f_cli_args) = run_with_env(
        "cli",
        "branch-f-cli.json",
        &[("CTX_TEST_CHOICE", "no"), ("CTX_TEST_NESTED_CHOICE", "no")],
    );
    assert_system(&f_cli_args, "--cli-system");
    let f_frames = read_frames(6);
    let f_otherwise = &f_frames[3];
    let f_post_sibling = &f_frames[4];
    let f_later = &f_frames[5];
    assert!(
        f_otherwise.contains("otherwise-arm-marker"),
        "{f_otherwise}"
    );
    assert!(
        !f_otherwise.contains("then-arm-marker") && !f_otherwise.contains("nested-arm-marker"),
        "{f_otherwise}"
    );
    assert!(
        !f_post_sibling.contains("otherwise-arm-marker"),
        "{f_post_sibling}"
    );
    assert!(f_later.contains("branch-later-marker"), "{f_later}");
    assert_eq!(f_frames.len(), 6);
    assert_eq!(
        extract_intent(f_otherwise),
        extract_intent(branch_preview_otherwise),
        "false then otherwise intent differs between preview and CLI"
    );
    assert_eq!(
        extract_behavior(f_otherwise),
        extract_behavior(branch_preview_otherwise),
        "false then otherwise behavior differs between preview and CLI"
    );
    let f_session = home.join("branch-f-cli.json");

    // The direct leaf was reached only via CLI (true/true) and MCP
    // (true/false) above, and the nested/alternate leaves only via CLI —
    // repeat true/true and false over MCP so every one of direct, alternate,
    // and recursively nested selected leaves is exercised through both
    // transports, matching the same no-session preview and CLI counterparts.
    let (_, tt_mcp_args) = run_with_env(
        "mcp",
        "branch-tt-mcp.json",
        &[
            ("CTX_TEST_CHOICE", "yes"),
            ("CTX_TEST_NESTED_CHOICE", "yes"),
        ],
    );
    assert_system(&tt_mcp_args, "--mcp-system");
    let tt_mcp_frames = read_frames(7);
    let tt_mcp_then = &tt_mcp_frames[3];
    let tt_mcp_nested_then = &tt_mcp_frames[4];
    assert!(
        tt_mcp_then.contains("Produce then-leaf answer."),
        "{tt_mcp_then}"
    );
    assert!(
        tt_mcp_nested_then.contains("Produce nested-then-leaf answer."),
        "{tt_mcp_nested_then}"
    );
    assert_eq!(
        extract_intent(tt_mcp_then),
        extract_intent(tt_then),
        "true/true then intent differs between CLI and MCP"
    );
    assert_eq!(
        extract_behavior(tt_mcp_then),
        extract_behavior(tt_then),
        "true/true then behavior differs between CLI and MCP"
    );
    assert_eq!(
        extract_intent(tt_mcp_nested_then),
        extract_intent(tt_nested_then),
        "true/true nested-then intent differs between CLI and MCP"
    );
    assert_eq!(
        extract_behavior(tt_mcp_nested_then),
        extract_behavior(tt_nested_then),
        "true/true nested-then behavior differs between CLI and MCP"
    );
    let tt_mcp_session = home.join("branch-tt-mcp.json");

    let (_, f_mcp_args) = run_with_env(
        "mcp",
        "branch-f-mcp.json",
        &[("CTX_TEST_CHOICE", "no"), ("CTX_TEST_NESTED_CHOICE", "no")],
    );
    assert_system(&f_mcp_args, "--mcp-system");
    let f_mcp_frames = read_frames(6);
    let f_mcp_otherwise = &f_mcp_frames[3];
    assert!(
        f_mcp_otherwise.contains("Produce otherwise-leaf answer."),
        "{f_mcp_otherwise}"
    );
    assert_eq!(
        extract_intent(f_mcp_otherwise),
        extract_intent(f_otherwise),
        "false otherwise intent differs between CLI and MCP"
    );
    assert_eq!(
        extract_behavior(f_mcp_otherwise),
        extract_behavior(f_otherwise),
        "false otherwise behavior differs between CLI and MCP"
    );
    assert_eq!(
        extract_intent(f_mcp_otherwise),
        extract_intent(branch_preview_otherwise),
        "false otherwise intent differs between preview and MCP"
    );
    assert_eq!(
        extract_behavior(f_mcp_otherwise),
        extract_behavior(branch_preview_otherwise),
        "false otherwise behavior differs between preview and MCP"
    );
    let f_mcp_session = home.join("branch-f-mcp.json");

    // Every recorded leaf across the three real dispatches must show the
    // same root->agent->leaf scalar precedence the top-level and named-leaf
    // proofs above established, and must exclude every marker belonging to a
    // sibling, opposite, previous, next, or later declaration — table-driven
    // so the sweep covers the same ground for every arm exactly once.
    let all_branch_markers = [
        "then-arm-marker",
        "then-arm-tone-marker",
        "nested-arm-marker",
        "nested-arm-tone-marker",
        "otherwise-arm-marker",
        "otherwise-arm-tone-marker",
        "pre-branch-sibling-marker",
        "post-branch-sibling-marker",
        "branch-later-marker",
    ];
    let all_branch_prompt_texts = [
        "Produce then-leaf answer.",
        "Produce nested-then-leaf answer.",
        "Produce otherwise-leaf answer.",
    ];
    let assert_leaf = |label: &str,
                       prompt: &str,
                       all_markers: &[&str],
                       all_prompt_texts: &[&str],
                       own_marker: &str,
                       own_tone_marker: &str,
                       own_text: &str,
                       own_tone_text: &str,
                       own_prompt_text: &str,
                       own_verbosity_marker: Option<&str>,
                       foreign_verbosity_marker: &str| {
        let intent = extract_intent(prompt);
        let behavior = extract_behavior(prompt);
        assert!(
            prompt.contains(own_prompt_text),
            "{label} missing its own declared prompt text: {prompt}"
        );
        for prompt_text in all_prompt_texts {
            if *prompt_text == own_prompt_text {
                continue;
            }
            assert!(
                !prompt.contains(prompt_text),
                "{label} leaked sibling declared prompt text {prompt_text}: {prompt}"
            );
        }
        assert!(
            intent.find("root-only").unwrap() < intent.find("agent-only").unwrap(),
            "{label} root/agent intent ordering: {intent}"
        );
        assert!(
            intent.find("agent-only").unwrap() < intent.find("shared").unwrap(),
            "{label} agent/shared intent ordering: {intent}"
        );
        assert!(
            intent.find("shared").unwrap() < intent.find(own_marker).unwrap(),
            "{label} shared/leaf intent ordering: {intent}"
        );
        assert_eq!(
            intent.matches("id=\"shared\"").count(),
            1,
            "{label}: {intent}"
        );
        assert!(
            intent.contains(own_text),
            "{label} scalar precedence: {intent}"
        );
        assert!(
            !intent.contains("Assigned replacement text."),
            "{label} stale agent text leaked: {intent}"
        );
        assert!(
            behavior.find("root-tone").unwrap() < behavior.find("agent-tone").unwrap(),
            "{label} root/agent tone ordering: {behavior}"
        );
        assert!(
            behavior.find("agent-tone").unwrap() < behavior.find("shared-tone").unwrap(),
            "{label} agent/shared tone ordering: {behavior}"
        );
        assert!(
            behavior.find("shared-tone").unwrap() < behavior.find(own_tone_marker).unwrap(),
            "{label} shared/leaf tone ordering: {behavior}"
        );
        assert_eq!(
            behavior.matches("id=\"shared-tone\"").count(),
            1,
            "{label}: {behavior}"
        );
        assert!(
            behavior.contains(own_tone_text),
            "{label} tone scalar precedence: {behavior}"
        );
        // Guided leaves declare `tone`, while only selected leaves additionally
        // declare their own `verbosity` scalar. Every other
        // behavior axis must still fall back through agent to root exactly
        // as the plain named-sequence leaf proof (`inner_guided`)
        // established, proving the shared composer's fallback and
        // prompt-over-agent scalar precedence are unaffected by nested-control
        // admission.
        assert!(
            behavior.contains("root-initiative"),
            "{label} broader-axis fallback to root: {behavior}"
        );
        for unchanged in ["agent-directness", "agent-scope", "agent-uncertainty"] {
            assert!(
                behavior.contains(unchanged),
                "{label} agent-scalar fallback: {behavior}"
            );
        }
        match own_verbosity_marker {
            Some(marker) => {
                assert!(
                    behavior.contains(marker),
                    "{label} own verbosity scalar precedence: {behavior}"
                );
                assert!(
                    !behavior.contains("agent-verbosity"),
                    "{label} stale agent verbosity leaked: {behavior}"
                );
            }
            None => {
                assert!(
                    behavior.contains("agent-verbosity"),
                    "{label} verbosity agent-scalar fallback: {behavior}"
                );
                assert!(
                    !behavior.contains(foreign_verbosity_marker),
                    "{label} leaked another leaf's own verbosity scalar: {behavior}"
                );
            }
        }
        for marker in all_markers {
            if *marker == own_marker || *marker == own_tone_marker {
                continue;
            }
            assert!(
                !prompt.contains(marker),
                "{label} leaked marker {marker}: {prompt}"
            );
        }
        assert_unassigned_absent(prompt);
        assert!(!prompt.contains("source=\""), "{label}: {prompt}");
        assert!(
            !prompt.contains("unassigned-tone"),
            "{label} leaked unassigned behavior: {prompt}"
        );
    };
    assert_leaf(
        "true/true then (CLI)",
        tt_then,
        &all_branch_markers,
        &all_branch_prompt_texts,
        "then-arm-marker",
        "then-arm-tone-marker",
        "Then leaf replacement text.",
        "Then leaf tone.",
        "Produce then-leaf answer.",
        Some("then-arm-verbosity-marker"),
        "then-arm-verbosity-marker",
    );
    assert_leaf(
        "true/true nested-then (CLI)",
        tt_nested_then,
        &all_branch_markers,
        &all_branch_prompt_texts,
        "nested-arm-marker",
        "nested-arm-tone-marker",
        "Nested then leaf replacement text.",
        "Nested then leaf tone.",
        "Produce nested-then-leaf answer.",
        None,
        "then-arm-verbosity-marker",
    );
    assert_leaf(
        "true/false then (MCP)",
        tf_then,
        &all_branch_markers,
        &all_branch_prompt_texts,
        "then-arm-marker",
        "then-arm-tone-marker",
        "Then leaf replacement text.",
        "Then leaf tone.",
        "Produce then-leaf answer.",
        Some("then-arm-verbosity-marker"),
        "then-arm-verbosity-marker",
    );
    assert_leaf(
        "false otherwise (CLI)",
        f_otherwise,
        &all_branch_markers,
        &all_branch_prompt_texts,
        "otherwise-arm-marker",
        "otherwise-arm-tone-marker",
        "Otherwise leaf replacement text.",
        "Otherwise leaf tone.",
        "Produce otherwise-leaf answer.",
        None,
        "then-arm-verbosity-marker",
    );
    assert_leaf(
        "true/true then (MCP)",
        tt_mcp_then,
        &all_branch_markers,
        &all_branch_prompt_texts,
        "then-arm-marker",
        "then-arm-tone-marker",
        "Then leaf replacement text.",
        "Then leaf tone.",
        "Produce then-leaf answer.",
        Some("then-arm-verbosity-marker"),
        "then-arm-verbosity-marker",
    );
    assert_leaf(
        "true/true nested-then (MCP)",
        tt_mcp_nested_then,
        &all_branch_markers,
        &all_branch_prompt_texts,
        "nested-arm-marker",
        "nested-arm-tone-marker",
        "Nested then leaf replacement text.",
        "Nested then leaf tone.",
        "Produce nested-then-leaf answer.",
        None,
        "then-arm-verbosity-marker",
    );
    assert_leaf(
        "false otherwise (MCP)",
        f_mcp_otherwise,
        &all_branch_markers,
        &all_branch_prompt_texts,
        "otherwise-arm-marker",
        "otherwise-arm-tone-marker",
        "Otherwise leaf replacement text.",
        "Otherwise leaf tone.",
        "Produce otherwise-leaf answer.",
        None,
        "then-arm-verbosity-marker",
    );
    assert_ne!(
        extract_intent(tt_then),
        extract_intent(f_otherwise),
        "distinct arm selection must produce distinct effective intent text"
    );
    assert_ne!(
        tt_then, f_otherwise,
        "distinct arm selection must produce distinct effective prompt text"
    );
    assert_ne!(
        tt_then, tt_nested_then,
        "sibling and nested arms must produce distinct effective prompt text"
    );

    // Historical session preview reconstructs each recorded leaf exactly as
    // it activated, from the same completed session the real dispatch above
    // produced — proving `--session <path> --step <id>` exposes only the
    // recorded arm rather than a no-session preview's projection of both.
    for (label, prompt, step_id, session) in [
        ("true/true then", tt_then.as_str(), "then-leaf", &tt_session),
        (
            "true/true nested-then",
            tt_nested_then.as_str(),
            "nested-then-leaf",
            &tt_session,
        ),
        (
            "true/false then",
            tf_then.as_str(),
            "then-leaf",
            &tf_session,
        ),
        (
            "false otherwise",
            f_otherwise.as_str(),
            "otherwise-leaf",
            &f_session,
        ),
        (
            "true/true then (MCP)",
            tt_mcp_then.as_str(),
            "then-leaf",
            &tt_mcp_session,
        ),
        (
            "true/true nested-then (MCP)",
            tt_mcp_nested_then.as_str(),
            "nested-then-leaf",
            &tt_mcp_session,
        ),
        (
            "false otherwise (MCP)",
            f_mcp_otherwise.as_str(),
            "otherwise-leaf",
            &f_mcp_session,
        ),
    ] {
        let historical = session_preview(&format!("{label} historical"), session, Some(step_id));
        assert_eq!(
            extract_intent(&historical),
            extract_intent(prompt),
            "{label} historical intent differs from the recorded dispatch"
        );
        assert_eq!(
            extract_behavior(&historical),
            extract_behavior(prompt),
            "{label} historical behavior differs from the recorded dispatch"
        );
    }

    // Active session preview: pause a fresh session immediately before the
    // `then` leaf activates (3 driven frames: decide, decide-nested,
    // pre-branch-sibling) and confirm the not-yet-accepted frame already
    // exposes only the recorded arm, matching the completed true/true
    // dispatch above byte for byte.
    let active_session = home.join("branch-active-cli.json");
    fs::write(repo.join(".ctx/traits/runtime.toml"), runtime("cli")).unwrap();
    require_success(
        "approve branch fixture for active preview",
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
    let seed = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--no-drive",
            "--out",
            active_session.to_str().unwrap(),
        ],
        &repo,
        &home,
    );
    assert_exit_code(&seed, 0);
    let drive = support::run_ctx_with_env(
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            generated.to_str().unwrap(),
            "--session",
            active_session.to_str().unwrap(),
            "--max-frames",
            "3",
            "--no-worktree",
            "--no-wait",
            "--progress",
            "none",
        ],
        &repo,
        &home,
        &[
            ("CTX_TEST_CHOICE", "yes"),
            ("CTX_TEST_NESTED_CHOICE", "yes"),
        ],
    );
    assert_exit_code(&drive, 0);
    let active = session_preview("true/true then (active)", &active_session, None);
    assert_eq!(
        extract_intent(&active),
        extract_intent(tt_then),
        "active session preview intent differs from the recorded then dispatch"
    );
    assert_eq!(
        extract_behavior(&active),
        extract_behavior(tt_then),
        "active session preview behavior differs from the recorded then dispatch"
    );
    assert!(
        !active.contains("otherwise-arm-marker") && !active.contains("nested-arm-marker"),
        "active session preview leaked a sibling arm: {active}"
    );

    // Active session preview, false outcome: pause a fresh session at the
    // same 3-frame boundary but with the outer choice false, so the
    // not-yet-accepted frame is the `otherwise` arm instead of `then`.
    let active_false_session = home.join("branch-active-false-cli.json");
    let seed_false = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--no-drive",
            "--out",
            active_false_session.to_str().unwrap(),
        ],
        &repo,
        &home,
    );
    assert_exit_code(&seed_false, 0);
    let drive_false = support::run_ctx_with_env(
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            generated.to_str().unwrap(),
            "--session",
            active_false_session.to_str().unwrap(),
            "--max-frames",
            "3",
            "--no-worktree",
            "--no-wait",
            "--progress",
            "none",
        ],
        &repo,
        &home,
        &[("CTX_TEST_CHOICE", "no"), ("CTX_TEST_NESTED_CHOICE", "no")],
    );
    assert_exit_code(&drive_false, 0);
    let active_false = session_preview("false otherwise (active)", &active_false_session, None);
    assert_eq!(
        extract_intent(&active_false),
        extract_intent(f_otherwise),
        "active session preview intent differs from the recorded otherwise dispatch"
    );
    assert_eq!(
        extract_behavior(&active_false),
        extract_behavior(f_otherwise),
        "active session preview behavior differs from the recorded otherwise dispatch"
    );
    assert!(
        !active_false.contains("then-arm-marker") && !active_false.contains("nested-arm-marker"),
        "active session preview leaked a sibling arm: {active_false}"
    );

    // Active session preview, nested outcome: pause a fresh session one
    // frame further (decide, decide-nested, pre-branch-sibling, then-leaf)
    // so the not-yet-accepted frame is the recursively nested `then` arm.
    let active_nested_session = home.join("branch-active-nested-cli.json");
    let seed_nested = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--no-drive",
            "--out",
            active_nested_session.to_str().unwrap(),
        ],
        &repo,
        &home,
    );
    assert_exit_code(&seed_nested, 0);
    let drive_nested = support::run_ctx_with_env(
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            generated.to_str().unwrap(),
            "--session",
            active_nested_session.to_str().unwrap(),
            "--max-frames",
            "4",
            "--no-worktree",
            "--no-wait",
            "--progress",
            "none",
        ],
        &repo,
        &home,
        &[
            ("CTX_TEST_CHOICE", "yes"),
            ("CTX_TEST_NESTED_CHOICE", "yes"),
        ],
    );
    assert_exit_code(&drive_nested, 0);
    let active_nested = session_preview(
        "true/true nested-then (active)",
        &active_nested_session,
        None,
    );
    assert_eq!(
        extract_intent(&active_nested),
        extract_intent(tt_nested_then),
        "active session preview intent differs from the recorded nested-then dispatch"
    );
    assert_eq!(
        extract_behavior(&active_nested),
        extract_behavior(tt_nested_then),
        "active session preview behavior differs from the recorded nested-then dispatch"
    );
    assert!(
        !active_nested.contains("then-arm-marker")
            && !active_nested.contains("otherwise-arm-marker"),
        "active session preview leaked a sibling arm: {active_nested}"
    );

    // A branch-path leaf's require/avoid conflict must still fail before the
    // harness ever runs, exactly as the top-level and named-leaf conflicts
    // above did — admission broadening must not move conflict detection.
    let branch_conflict_fixture = branch_fixture.replace(
        "intent = { require = [{ id = \"shared\", summary = \"Then leaf replacement text.\" }, { id = \"then-arm-marker\", summary = \"Then leaf marker.\" }] }",
        "intent = { avoid = [{ id = \"shared\", summary = \"Then leaf conflict.\" }] }",
    );
    assert_ne!(
        branch_conflict_fixture, branch_fixture,
        "then-leaf intent replacement point moved"
    );
    fs::write(&generated, &branch_conflict_fixture).unwrap();
    require_success(
        "approve branch conflict fixture",
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
    let _ = fs::remove_file(capture.with_extension("txt.calls"));
    let branch_conflict = support::run_ctx_with_env(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--out",
            home.join("branch-conflict.json").to_str().unwrap(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
        &[
            ("CTX_TEST_CHOICE", "yes"),
            ("CTX_TEST_NESTED_CHOICE", "yes"),
        ],
    );
    assert!(
        !branch_conflict.status.success(),
        "branch-path conflict unexpectedly dispatched"
    );
    let (_, stderr) = utf8(&branch_conflict);
    assert!(
        stderr.contains(
            "effective guidance id \"shared\" cannot appear in both require and avoid when ready-prompt intent participates"
        )
    );
    // Three prior steps (decide, decide-nested, pre-branch-sibling) dispatch
    // normally before the conflicting then-leaf is ever reached, so the
    // proof is that dispatch stops there — the harness must not have been
    // called a fourth time for the rejected leaf itself.
    let branch_conflict_calls =
        fs::read_to_string(capture.with_extension("txt.calls")).unwrap_or_default();
    assert_eq!(
        branch_conflict_calls.lines().count(),
        3,
        "harness ran for the conflicting then-leaf before the branch-path conflict was rejected: {branch_conflict_calls:?}"
    );

    // A branch leaf's own prompt-scoped guidance must be able to enable
    // categorized MCP participation entirely on its own, with no
    // assigned-agent-scoped guidance declared anywhere in the trait —
    // mirroring the equivalent named-leaf proof above, now on a branch arm.
    let branch_prompt_only_fixture = branch_fixture
        .replace(worker_intent, "")
        .replace(worker_behavior, "");
    assert_ne!(
        branch_prompt_only_fixture, branch_fixture,
        "agent-scoped guidance removal point moved"
    );
    fs::write(&generated, &branch_prompt_only_fixture).unwrap();
    require_success(
        "approve branch prompt-only fixture",
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
    let (_, _) = run_with_env(
        "mcp",
        "branch-prompt-only-mcp.json",
        &[
            ("CTX_TEST_CHOICE", "yes"),
            ("CTX_TEST_NESTED_CHOICE", "yes"),
        ],
    );
    let branch_prompt_only_then = fs::read_to_string(capture.with_extension("txt.3")).unwrap();
    let branch_prompt_only_intent = extract_intent(&branch_prompt_only_then);
    let branch_prompt_only_behavior = extract_behavior(&branch_prompt_only_then);
    assert!(
        branch_prompt_only_intent.contains("then-arm-marker"),
        "{branch_prompt_only_intent}"
    );
    assert!(
        !branch_prompt_only_intent.contains("agent-only"),
        "{branch_prompt_only_intent}"
    );
    assert!(
        branch_prompt_only_behavior.contains("then-arm-tone-marker"),
        "{branch_prompt_only_behavior}"
    );
    assert!(
        !branch_prompt_only_behavior.contains("agent-tone"),
        "{branch_prompt_only_behavior}"
    );

    // A loop body uses the same declaration lookup as named sequences, but a
    // live leaf has a `loop` owner segment and may activate more than once.
    // The nested branch makes the path mixed (`loop`, then `branch`).
    let loop_header = nested_header.replace(
        "[[slot]]\nid = \"answer\"\nschema = \"schema:text\"\ndescription = \"Answer.\"\n",
        "[[slot]]\nid = \"answer\"\nschema = \"schema:text\"\ndescription = \"Answer.\"\n\n[[slot]]\nid = \"loop-choice\"\nschema = \"schema:text\"\ndescription = \"Loop branch choice.\"\n\n[[slot]]\nid = \"loop-verdict\"\nschema = \"schema:text\"\ndescription = \"Loop verdict.\"\n",
    );
    assert_ne!(
        loop_header, nested_header,
        "loop slot insertion point moved"
    );
    let loop_fixture = format!(
        r#"{loop_header}[[sequence.loop-body.sequence]]
id = "loop-a"
title = "Loop A"
agent = "agent:worker"
prompt = "Produce loop-a answer."
intent = {{ require = [{{ id = "shared", summary = "Loop A replacement text." }}, {{ id = "loop-a-marker", summary = "Loop A marker." }}] }}
behavior = {{ tone = [{{ id = "shared-tone", summary = "Loop A tone." }}, {{ id = "loop-a-tone-marker", summary = "Loop A tone marker." }}], verbosity = {{ id = "loop-a-verbosity-marker", summary = "Loop A verbosity marker." }} }}
output = ["slot:answer"]

[[sequence.loop-body.sequence]]
id = "loop-branch"
title = "Loop branch"
kind = "branch"
sequence = "sequence:loop-branch-then"
when = {{ slot = "slot:loop-choice", equals = "ok" }}

[[sequence.loop-body.sequence]]
title = "Loop verdict"
agent = "agent:worker"
prompt = "Produce loop verdict."
output = ["slot:loop-verdict"]

[[sequence.loop-branch-then.sequence]]
id = "loop-nested-leaf"
title = "Loop nested leaf"
agent = "agent:worker"
prompt = "Produce loop-nested answer."
intent = {{ require = [{{ id = "shared", summary = "Loop nested replacement text." }}, {{ id = "loop-nested-marker", summary = "Loop nested marker." }}] }}
behavior = {{ tone = [{{ id = "shared-tone", summary = "Loop nested tone." }}, {{ id = "loop-nested-tone-marker", summary = "Loop nested tone marker." }}] }}
output = ["slot:answer"]

[procedure]
description = "Loop leaf guidance."

[[procedure.sequence]]
id = "decide"
title = "Decide"
agent = "agent:worker"
prompt = "Decide the loop branch."
intent = {{ require = [{{ id = "loop-pre-marker", summary = "Loop pre marker." }}] }}
output = ["slot:loop-choice"]

[[procedure.sequence]]
id = "the-loop"
title = "The loop"
kind = "loop"
sequence = "sequence:loop-body"
max-iterations = 3

[procedure.sequence.until]
slot = "slot:loop-verdict"
equals = "done"

[[procedure.sequence]]
id = "loop-later"
title = "Loop later"
agent = "agent:worker"
prompt = "Produce loop-later answer."
intent = {{ require = [{{ id = "loop-later-marker", summary = "Loop later marker." }}] }}
output = ["slot:answer"]
"#
    );
    fs::write(&generated, &loop_fixture).unwrap();
    require_success(
        "approve loop fixture",
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
    let loop_preview = preview_all("loop");
    assert_eq!(loop_preview.len(), 5, "loop preview: {loop_preview:?}");
    let loop_preview_decide = &loop_preview[0];
    let loop_preview_a = &loop_preview[1];
    let loop_preview_nested = &loop_preview[2];
    let loop_preview_verdict = &loop_preview[3];
    let loop_preview_later = &loop_preview[4];
    assert!(loop_preview_a.contains("loop-a-marker"), "{loop_preview_a}");
    assert!(
        loop_preview_nested.contains("loop-nested-marker"),
        "{loop_preview_nested}"
    );
    assert!(!loop_preview_verdict.contains("loop-a-marker"));

    let all_loop_markers = [
        "loop-pre-marker",
        "loop-a-marker",
        "loop-a-tone-marker",
        "loop-nested-marker",
        "loop-nested-tone-marker",
        "loop-later-marker",
    ];
    let all_loop_prompt_texts = ["Produce loop-a answer.", "Produce loop-nested answer."];

    let (_, loop_cli_args) =
        run_with_env("cli", "loop-cli.json", &[("CTX_TEST_LOOP_CHOICE", "ok")]);
    assert_system(&loop_cli_args, "--cli-system");
    let loop_cli_frames = read_frames(8);
    let loop_a0 = &loop_cli_frames[1];
    let loop_nested0 = &loop_cli_frames[2];
    let loop_a1 = &loop_cli_frames[4];
    let loop_nested1 = &loop_cli_frames[5];
    let loop_verdict0 = &loop_cli_frames[3];
    let loop_verdict1 = &loop_cli_frames[6];
    assert_eq!(
        fs::read_to_string(capture.with_extension("txt.calls"))
            .unwrap()
            .lines()
            .count(),
        8,
        "loop must run twice before its verdict exits"
    );
    for (index, prompt) in loop_cli_frames.iter().enumerate() {
        assert_system(
            &fs::read_to_string(capture.with_extension(format!("txt.args.{index}"))).unwrap(),
            "--cli-system",
        );
        assert!(
            !prompt.contains("the-loop")
                && !prompt.contains("loop-branch")
                && !prompt.contains("sequence:loop-body")
                && !prompt.contains("sequence:loop-branch-then"),
            "loop CLI frame {index}: {prompt}"
        );
        assert_unassigned_absent(prompt);
    }
    for (label, prompt, marker, tone_marker, text, tone, declared, verbosity) in [
        (
            "loop A0",
            loop_a0,
            "loop-a-marker",
            "loop-a-tone-marker",
            "Loop A replacement text.",
            "Loop A tone.",
            "Produce loop-a answer.",
            Some("loop-a-verbosity-marker"),
        ),
        (
            "loop nested0",
            loop_nested0,
            "loop-nested-marker",
            "loop-nested-tone-marker",
            "Loop nested replacement text.",
            "Loop nested tone.",
            "Produce loop-nested answer.",
            None,
        ),
        (
            "loop A1",
            loop_a1,
            "loop-a-marker",
            "loop-a-tone-marker",
            "Loop A replacement text.",
            "Loop A tone.",
            "Produce loop-a answer.",
            Some("loop-a-verbosity-marker"),
        ),
        (
            "loop nested1",
            loop_nested1,
            "loop-nested-marker",
            "loop-nested-tone-marker",
            "Loop nested replacement text.",
            "Loop nested tone.",
            "Produce loop-nested answer.",
            None,
        ),
    ] {
        assert_leaf(
            label,
            prompt,
            &all_loop_markers,
            &all_loop_prompt_texts,
            marker,
            tone_marker,
            text,
            tone,
            declared,
            verbosity,
            "loop-a-verbosity-marker",
        );
        assert_eq!(prompt.matches(marker).count(), 1, "{label}: {prompt}");
    }
    for (label, prompt, own_marker) in [
        ("decide", &loop_cli_frames[0], "loop-pre-marker"),
        ("verdict0", loop_verdict0, ""),
        ("verdict1", loop_verdict1, ""),
        ("later", &loop_cli_frames[7], "loop-later-marker"),
    ] {
        if own_marker.is_empty() {
            assert!(!prompt.contains("loop-a-marker"), "{label}: {prompt}");
        } else {
            assert_eq!(prompt.matches(own_marker).count(), 1, "{label}: {prompt}");
        }
        for marker in all_loop_markers {
            if marker != own_marker {
                assert!(
                    !prompt.contains(marker),
                    "{label} leaked {marker}: {prompt}"
                );
            }
        }
        assert!(!prompt.contains("source=\""), "{label}: {prompt}");
    }
    assert_eq!(extract_intent(loop_a0), extract_intent(loop_a1));
    assert_eq!(extract_behavior(loop_a0), extract_behavior(loop_a1));
    assert_eq!(extract_intent(loop_nested0), extract_intent(loop_nested1));
    assert_eq!(
        extract_behavior(loop_nested0),
        extract_behavior(loop_nested1)
    );
    assert_ne!(extract_intent(loop_a0), extract_intent(loop_nested0));
    assert_eq!(extract_intent(loop_verdict0), extract_intent(loop_verdict1));
    assert_eq!(
        extract_behavior(loop_verdict0),
        extract_behavior(loop_verdict1)
    );
    for distinct in [loop_a0, loop_nested0, loop_verdict0] {
        for other in [loop_a0, loop_nested0, loop_verdict0] {
            if std::ptr::eq(distinct, other) {
                continue;
            }
            assert_ne!(
                extract_intent(distinct),
                extract_intent(other),
                "loop declarations must remain distinct"
            );
        }
    }
    for (preview, cli, label) in [
        (loop_preview_decide, &loop_cli_frames[0], "decide"),
        (loop_preview_a, loop_a0, "loop A"),
        (loop_preview_nested, loop_nested0, "loop nested"),
        (loop_preview_verdict, loop_verdict0, "loop verdict"),
        (loop_preview_later, &loop_cli_frames[7], "loop later"),
    ] {
        assert_eq!(extract_intent(preview), extract_intent(cli), "{label}");
        assert_eq!(extract_behavior(preview), extract_behavior(cli), "{label}");
    }
    assert_eq!(extract_intent(loop_a0), extract_intent(loop_preview_a));
    assert_eq!(extract_behavior(loop_a0), extract_behavior(loop_preview_a));
    assert_eq!(
        extract_intent(loop_nested0),
        extract_intent(loop_preview_nested)
    );
    assert_eq!(
        extract_behavior(loop_nested0),
        extract_behavior(loop_preview_nested)
    );

    let (_, loop_mcp_args) =
        run_with_env("mcp", "loop-mcp.json", &[("CTX_TEST_LOOP_CHOICE", "ok")]);
    assert_system(&loop_mcp_args, "--mcp-system");
    let loop_mcp_frames = read_frames(8);
    for (index, mcp) in loop_mcp_frames.iter().enumerate() {
        assert_system(
            &fs::read_to_string(capture.with_extension(format!("txt.args.{index}"))).unwrap(),
            "--mcp-system",
        );
        assert_unassigned_absent(mcp);
    }
    for (label, prompt, marker, tone_marker, text, tone, declared, verbosity) in [
        (
            "loop A0 (MCP)",
            &loop_mcp_frames[1],
            "loop-a-marker",
            "loop-a-tone-marker",
            "Loop A replacement text.",
            "Loop A tone.",
            "Produce loop-a answer.",
            Some("loop-a-verbosity-marker"),
        ),
        (
            "loop nested0 (MCP)",
            &loop_mcp_frames[2],
            "loop-nested-marker",
            "loop-nested-tone-marker",
            "Loop nested replacement text.",
            "Loop nested tone.",
            "Produce loop-nested answer.",
            None,
        ),
        (
            "loop A1 (MCP)",
            &loop_mcp_frames[4],
            "loop-a-marker",
            "loop-a-tone-marker",
            "Loop A replacement text.",
            "Loop A tone.",
            "Produce loop-a answer.",
            Some("loop-a-verbosity-marker"),
        ),
        (
            "loop nested1 (MCP)",
            &loop_mcp_frames[5],
            "loop-nested-marker",
            "loop-nested-tone-marker",
            "Loop nested replacement text.",
            "Loop nested tone.",
            "Produce loop-nested answer.",
            None,
        ),
    ] {
        assert_leaf(
            label,
            prompt,
            &all_loop_markers,
            &all_loop_prompt_texts,
            marker,
            tone_marker,
            text,
            tone,
            declared,
            verbosity,
            "loop-a-verbosity-marker",
        );
    }
    for (label, prompt, own_markers) in [
        (
            "preview decide",
            loop_preview_decide.as_str(),
            &["loop-pre-marker"][..],
        ),
        (
            "preview A",
            loop_preview_a.as_str(),
            &["loop-a-marker", "loop-a-tone-marker"][..],
        ),
        (
            "preview nested",
            loop_preview_nested.as_str(),
            &["loop-nested-marker", "loop-nested-tone-marker"][..],
        ),
        ("preview verdict", loop_preview_verdict.as_str(), &[][..]),
        (
            "preview later",
            loop_preview_later.as_str(),
            &["loop-later-marker"][..],
        ),
    ] {
        for marker in all_loop_markers {
            if own_markers.contains(&marker) {
                assert_eq!(prompt.matches(marker).count(), 1, "{label}: {prompt}");
            } else {
                assert!(
                    !prompt.contains(marker),
                    "{label} leaked {marker}: {prompt}"
                );
            }
        }
        for container in [
            "the-loop",
            "loop-branch",
            "sequence:loop-body",
            "sequence:loop-branch-then",
        ] {
            assert!(!prompt.contains(container), "{label}: {prompt}");
        }
        assert!(!prompt.contains("source=\""), "{label}: {prompt}");
    }
    for (index, prompt) in loop_mcp_frames.iter().enumerate() {
        let guidance = format!("{}{}", extract_intent(prompt), extract_behavior(prompt));
        for container in [
            "the-loop",
            "loop-branch",
            "sequence:loop-body",
            "sequence:loop-branch-then",
        ] {
            assert!(
                !guidance.contains(container),
                "loop MCP frame {index}: {guidance}"
            );
        }
        assert!(
            !prompt.contains("source=\""),
            "loop MCP frame {index}: {prompt}"
        );
    }
    for (cli, mcp) in loop_cli_frames.iter().zip(&loop_mcp_frames) {
        assert_eq!(extract_intent(cli), extract_intent(mcp));
        assert_eq!(extract_behavior(cli), extract_behavior(mcp));
    }
    for frames in [&loop_cli_frames, &loop_mcp_frames] {
        for (index, own_markers) in [
            &["loop-pre-marker"][..],
            &["loop-a-marker", "loop-a-tone-marker"][..],
            &["loop-nested-marker", "loop-nested-tone-marker"][..],
            &[][..],
            &["loop-a-marker", "loop-a-tone-marker"][..],
            &["loop-nested-marker", "loop-nested-tone-marker"][..],
            &[][..],
            &["loop-later-marker"][..],
        ]
        .iter()
        .enumerate()
        {
            for marker in *own_markers {
                assert_eq!(
                    frames[index].matches(marker).count(),
                    1,
                    "loop frame {index}"
                );
            }
        }
    }

    let loop_session = home.join("loop-cli.json");
    for (label, prompt, step) in [
        ("loop A historical", loop_a1, "loop-a"),
        ("loop nested historical", loop_nested1, "loop-nested-leaf"),
    ] {
        let historical = session_preview(label, &loop_session, Some(step));
        assert_eq!(
            extract_intent(&historical),
            extract_intent(prompt),
            "{label}"
        );
        assert_eq!(
            extract_behavior(&historical),
            extract_behavior(prompt),
            "{label}"
        );
    }

    // After four accepted frames the next ready frame is the second loop-A
    // activation. Clearing the verdict ledger keeps this direct drive isolated
    // from the completed dispatches above.
    let loop_active_session = home.join("loop-active-cli.json");
    fs::write(repo.join(".ctx/traits/runtime.toml"), runtime("cli")).unwrap();
    let seed = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--no-drive",
            "--out",
            loop_active_session.to_str().unwrap(),
        ],
        &repo,
        &home,
    );
    assert_exit_code(&seed, 0);
    let _ = fs::remove_file(capture.with_extension("txt.verdict-calls"));
    let drive = support::run_ctx_with_env(
        &[
            "traits",
            "internal",
            "drive",
            "--file",
            generated.to_str().unwrap(),
            "--session",
            loop_active_session.to_str().unwrap(),
            "--max-frames",
            "4",
            "--no-worktree",
            "--no-wait",
            "--progress",
            "none",
        ],
        &repo,
        &home,
        &[("CTX_TEST_LOOP_CHOICE", "ok")],
    );
    assert_exit_code(&drive, 0);
    let loop_active = session_preview("loop A active", &loop_active_session, None);
    assert_eq!(extract_intent(&loop_active), extract_intent(loop_a1));
    assert_eq!(extract_behavior(&loop_active), extract_behavior(loop_a1));

    // Omitted and explicitly empty prompt guidance are participation-equivalent
    // to the id-less verdict declaration: only root and assigned guidance is
    // rendered for that leaf on preview and CLI dispatch.
    let verdict_anchor =
        "title = \"Loop verdict\"\nagent = \"agent:worker\"\nprompt = \"Produce loop verdict.\"\n";
    for (label, addition) in [
        ("absent", ""),
        ("empty-intent", "intent = {}\n"),
        ("empty-behavior", "behavior = {}\n"),
    ] {
        let fixture = loop_fixture.replace(verdict_anchor, &format!("{verdict_anchor}{addition}"));
        fs::write(&generated, &fixture).unwrap();
        let variant_preview = preview_all(label);
        let _ = run_with_env(
            "cli",
            &format!("loop-{label}.json"),
            &[("CTX_TEST_LOOP_CHOICE", "ok")],
        );
        let variant_frames = read_frames(8);
        assert_eq!(
            extract_intent(&variant_preview[3]),
            extract_intent(&variant_frames[3]),
            "{label}"
        );
        assert_eq!(
            extract_behavior(&variant_preview[3]),
            extract_behavior(&variant_frames[3]),
            "{label}"
        );
        assert_eq!(
            extract_intent(&variant_frames[3]),
            extract_intent(loop_verdict0),
            "{label}"
        );
        assert_eq!(
            extract_behavior(&variant_frames[3]),
            extract_behavior(loop_verdict0),
            "{label}"
        );
    }
    fs::write(&generated, &loop_fixture).unwrap();

    let loop_conflict_fixture = loop_fixture.replace(
        "intent = { require = [{ id = \"shared\", summary = \"Loop A replacement text.\" }, { id = \"loop-a-marker\", summary = \"Loop A marker.\" }] }",
        "intent = { avoid = [{ id = \"shared\", summary = \"Loop A conflict.\" }] }",
    );
    fs::write(&generated, &loop_conflict_fixture).unwrap();
    require_success(
        "approve loop conflict fixture",
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
    let _ = fs::remove_file(capture.with_extension("txt.calls"));
    let conflict = support::run_ctx_with_env(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--out",
            home.join("loop-conflict.json").to_str().unwrap(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
        &[("CTX_TEST_LOOP_CHOICE", "ok")],
    );
    assert!(!conflict.status.success());
    let (_, stderr) = utf8(&conflict);
    assert!(
        stderr.contains(
            "effective guidance id \"shared\" cannot appear in both require and avoid when ready-prompt intent participates"
        ),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(capture.with_extension("txt.calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );

    let loop_prompt_only_fixture = loop_fixture
        .replace(worker_intent, "")
        .replace(worker_behavior, "");
    fs::write(&generated, &loop_prompt_only_fixture).unwrap();
    let _ = run_with_env(
        "mcp",
        "loop-prompt-only-mcp.json",
        &[("CTX_TEST_LOOP_CHOICE", "ok")],
    );
    let loop_prompt_only_a = fs::read_to_string(capture.with_extension("txt.1")).unwrap();
    assert!(
        loop_prompt_only_a.contains("loop-a-marker")
            && loop_prompt_only_a.contains("loop-a-tone-marker")
    );
    assert!(
        !loop_prompt_only_a.contains("agent-only") && !loop_prompt_only_a.contains("agent-tone")
    );
    fs::write(&generated, &loop_fixture).unwrap();

    // The absent/empty runtime variants above intentionally emit no leaf
    // guidance. Add a static-only marker to make the id-less source fallback
    // observable without changing their participation contract.
    let loop_static_fixture = loop_fixture.replace(
        verdict_anchor,
        &format!(
            "{verdict_anchor}intent = {{ require = [{{ id = \"loop-verdict-marker\", summary = \"Loop verdict static marker.\" }}] }}\n"
        ),
    );
    fs::write(&generated, &loop_static_fixture).unwrap();
    require_success(
        "approve loop static fixture",
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
    let loop_export_dir = home.join("loop-static-export");
    let loop_export = run_ctx(
        &[
            "traits",
            "internal",
            "export",
            "--file",
            generated.to_str().unwrap(),
            "--profile",
            "agent-skills",
            "--format",
            "compat",
            "--out",
            loop_export_dir.to_str().unwrap(),
        ],
        &repo,
        &home,
    );
    assert_exit_code(&loop_export, 0);
    let loop_skill =
        fs::read_to_string(loop_export_dir.join("agent-intent").join("SKILL.md")).unwrap();
    for (source, text, tone) in [
        (
            "sequence:loop-body/loop-a",
            "Loop A replacement text.",
            "Loop A tone.",
        ),
        (
            "sequence:loop-branch-then/loop-nested-leaf",
            "Loop nested replacement text.",
            "Loop nested tone.",
        ),
    ] {
        for (id, own_text) in [("shared", text), ("shared-tone", tone)] {
            let own_tag = format!("id=\"{id}\" source=\"{source}\">");
            assert_eq!(loop_skill.matches(&own_tag).count(), 1, "{loop_skill}");
            let block_start = loop_skill
                .find(&own_tag)
                .unwrap_or_else(|| panic!("static export missing {own_tag}: {loop_skill}"));
            let block_end = loop_skill[block_start..]
                .find(if id == "shared" {
                    "</intent>"
                } else {
                    "</behavior>"
                })
                .map(|offset| block_start + offset)
                .unwrap_or(loop_skill.len());
            assert!(
                loop_skill[block_start..block_end].contains(own_text),
                "static export source block for {own_tag} leaked or omitted its own text: {loop_skill}"
            );
        }
    }
    let verdict_tag = "id=\"loop-verdict-marker\" source=\"sequence:loop-body/step-3\">";
    assert_eq!(loop_skill.matches(verdict_tag).count(), 1, "{loop_skill}");
    let verdict_start = loop_skill.find(verdict_tag).unwrap();
    let verdict_end = loop_skill[verdict_start..]
        .find("</intent>")
        .map(|offset| verdict_start + offset)
        .unwrap_or(loop_skill.len());
    assert!(
        loop_skill[verdict_start..verdict_end].contains("Loop verdict static marker."),
        "{loop_skill}"
    );
    assert!(
        loop_skill.contains("Static render advisory") && loop_skill.contains("Static host note"),
        "loop static export must remain explicitly labeled static: {loop_skill}"
    );
    for claim in ["active", "ready", "selected", "not-yet-accepted"] {
        assert!(
            !loop_skill.to_ascii_lowercase().contains(claim),
            "loop static export claimed {claim}: {loop_skill}"
        );
    }
    assert!(
        !loop_skill.contains("Loop 1/") && !loop_skill.contains("Loop 2/"),
        "loop static export synthesized runtime iteration metadata: {loop_skill}"
    );

    for frames in [
        &tt_frames,
        &tf_frames,
        &f_frames,
        &tt_mcp_frames,
        &f_mcp_frames,
    ] {
        for prompt in frames {
            assert_unassigned_absent(prompt);
            assert!(!prompt.contains("source=\""));
        }
    }
}
