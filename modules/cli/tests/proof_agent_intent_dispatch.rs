use std::fs;
use std::os::unix::fs::PermissionsExt;

use support::{ScratchRoot, assert_exit_code, git_init, require_success, run_ctx, utf8};

#[test]
fn assigned_agent_intent_dispatch_is_shared_and_legacy_compatible() {
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
require = [{ id = "shared", summary = "Root replacement text." }, { id = "root-only", summary = "Root remains first." }]
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

    let preview = run_ctx(
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
    assert_exit_code(&preview, 0);
    let (preview_stdout, preview_stderr) = utf8(&preview);
    let preview_json: serde_json::Value =
        serde_json::from_str(&preview_stdout).unwrap_or_else(|error| {
            panic!(
                "preview was not JSON: {error}\nstdout={preview_stdout}\nstderr={preview_stderr}"
            )
        });
    let preview_prompt = preview_json["frames"][0]["prompt"]
        .as_str()
        .unwrap_or_else(|| panic!("preview had no prompt: {preview_json}"));
    let extract_intent = |text: &str| {
        let start = text.find("<intent>\n").expect("intent opening");
        let end = text[start..].find("</intent>\n").expect("intent closing")
            + start
            + "</intent>\n".len();
        text[start..end].to_string()
    };
    let preview_intent = extract_intent(preview_prompt);
    assert!(preview_intent.find("root-only").unwrap() < preview_intent.find("shared").unwrap());
    assert_eq!(preview_intent.matches("id=\"shared\"").count(), 1);
    assert!(preview_intent.contains("Assigned replacement text."));
    assert!(!preview_intent.contains("unassigned-only"));
    assert!(!preview_intent.contains("ASSIGNED SYSTEM"));

    let ledger = home.join("cli.json");
    let cli = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--out",
            ledger.to_str().unwrap(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&cli, 0);
    let cli_prompt = fs::read_to_string(&capture).expect("CLI prompt capture");
    let cli_args =
        fs::read_to_string(capture.with_extension("txt.args")).expect("CLI args capture");
    assert_eq!(extract_intent(&cli_prompt), preview_intent);
    assert!(cli_args.contains("--cli-system\nASSIGNED SYSTEM"));

    fs::remove_file(&capture).unwrap();
    fs::remove_file(capture.with_extension("txt.args")).unwrap();
    fs::write(repo.join(".ctx/traits/runtime.toml"), runtime("mcp")).unwrap();
    let mcp = run_ctx(
        &[
            "traits",
            "run",
            "--file",
            generated.to_str().unwrap(),
            "--out",
            home.join("mcp.json").to_str().unwrap(),
            "--json",
            "--progress",
            "none",
        ],
        &repo,
        &home,
    );
    assert_exit_code(&mcp, 0);
    let mcp_prompt = fs::read_to_string(&capture).expect("MCP prompt capture");
    let mcp_args =
        fs::read_to_string(capture.with_extension("txt.args")).expect("MCP args capture");
    assert_eq!(extract_intent(&mcp_prompt), preview_intent);
    assert!(mcp_args.contains("--mcp-system\nASSIGNED SYSTEM"));

    // The legacy path is structurally gated: no assigned intent leaves MCP
    // onboarding without the categorized envelope, including default-empty intent.
    for intent in ["", "[agent.intent]\n"] {
        let legacy = canonical.replace(
            "[agent.intent]\nrequire = [{ id = \"shared\", summary = \"Assigned replacement text.\" }, { id = \"agent-only\", summary = \"Assigned guidance.\" }]",
            intent,
        );
        fs::write(&generated, legacy).unwrap();
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
    }
}
