//! Task 0168: `#trait/*` source-root imports resolve against the package
//! manifest's `imports` mapping, produce a canonical output byte-identical
//! to the equivalent `../` import, and a missing alias target fails the
//! build with the resolved path.

use std::fs;
use std::path::Path;

use support::{ScratchRoot, git_init, repo_root, run_ctx, utf8};

fn scaffold_repo(scratch: &ScratchRoot, label: &str) -> std::path::PathBuf {
    let proj = scratch.home().join(label);
    fs::create_dir_all(&proj).unwrap();
    git_init(&proj);
    #[cfg(unix)]
    std::os::unix::fs::symlink(repo_root().join("node_modules"), proj.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(repo_root().join("node_modules"), proj.join("node_modules"))
        .unwrap_or_else(|error| panic!("cannot symlink node_modules: {error}"));
    proj
}

const PACKAGE_JSON_WITH_ALIAS: &str =
    "{\n  \"imports\": {\n    \"#trait/*\": \"./source/*\"\n  }\n}\n";

fn fixture_source(data_specifier: &str) -> String {
    format!(
        "import {{ agent, input, port, procedure, sequence, slot, trait }} from \"@ctx-traits/cdk\";\n\
import {{ summaryText }} from \"{data_specifier}\";\n\
\n\
const summary = slot.text(\"summary\");\n\
const output = port.output.text({{ id: \"summary\", value: summary }});\n\
const worker = agent(\"worker\", {{ description: \"Completes the starter task.\" }});\n\
\n\
export const draft = trait({{\n\
  id: \"alias-fixture\",\n\
  name: \"alias-fixture\",\n\
  description: summaryText,\n\
  port: output,\n\
  procedure: procedure({{\n\
    description: summaryText,\n\
    sequence: sequence.prompt({{\n\
      id: \"run\",\n\
      agent: worker,\n\
      prompt: input.prompt`Describe the task for this trait.`,\n\
      output: summary,\n\
    }}),\n\
  }}),\n\
}});\n"
    )
}

const DATA_TS: &str =
    "export const summaryText = \"Describe what this trait should accomplish.\";\n";

fn init_and_build(
    proj: &Path,
    home: &Path,
    trait_id: &str,
    package_json: Option<&str>,
    data_specifier: &str,
) {
    let init = run_ctx(&["traits", "init", trait_id], proj, home);
    assert!(init.status.success(), "{}", utf8(&init).1);
    if let Some(package_json) = package_json {
        fs::write(
            proj.join(format!(".ctx/traits/packages/{trait_id}/package.json")),
            package_json,
        )
        .unwrap();
    }
    fs::write(
        proj.join(format!(".ctx/traits/packages/{trait_id}/source/index.ts")),
        fixture_source(data_specifier),
    )
    .unwrap();
    fs::create_dir_all(proj.join(format!(".ctx/traits/packages/{trait_id}/source/shared")))
        .unwrap();
    fs::write(
        proj.join(format!(
            ".ctx/traits/packages/{trait_id}/source/shared/data.ts"
        )),
        DATA_TS,
    )
    .unwrap();
    let build = run_ctx(
        &[
            "traits",
            "build",
            &format!(".ctx/traits/packages/{trait_id}/source/index.ts"),
        ],
        proj,
        home,
    );
    let (stdout, stderr) = utf8(&build);
    assert!(
        build.status.success(),
        "build failed\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// A `#trait/shared/data.ts` import resolves via the package.json `imports`
/// mapping and builds identically to the equivalent `../`-free relative
/// import — same canonical bytes, proving the alias is a pure spelling
/// change, not a different resolution.
#[test]
fn alias_import_builds_byte_identical_to_relative_import() {
    let scratch = ScratchRoot::new("p0168-alias-byte-identical");
    let home = scratch.home();
    let trait_id = "alias-fixture";

    let relative_proj = scaffold_repo(&scratch, "relative-repo");
    init_and_build(&relative_proj, &home, trait_id, None, "./shared/data.ts");

    let alias_proj = scaffold_repo(&scratch, "alias-repo");
    init_and_build(
        &alias_proj,
        &home,
        trait_id,
        Some(PACKAGE_JSON_WITH_ALIAS),
        "#trait/shared/data.ts",
    );

    let relative_canonical = fs::read(relative_proj.join(format!(
        ".ctx/traits/packages/{trait_id}/generated/index.toml"
    )))
    .unwrap();
    let alias_canonical = fs::read(alias_proj.join(format!(
        ".ctx/traits/packages/{trait_id}/generated/index.toml"
    )))
    .unwrap();
    assert_eq!(
        relative_canonical, alias_canonical,
        "alias-spelled and relative-spelled imports must produce byte-identical canonical output"
    );
}

/// `#trait/missing.ts` naming a file that does not exist fails the build,
/// and the error names the resolved path under the package's `source/`
/// root — not the raw alias specifier.
#[test]
fn alias_import_to_missing_file_fails_build_with_resolved_path() {
    let scratch = ScratchRoot::new("p0168-alias-missing-file");
    let home = scratch.home();
    let proj = scaffold_repo(&scratch, "repo");
    let trait_id = "alias-fixture";

    let init = run_ctx(&["traits", "init", trait_id], &proj, &home);
    assert!(init.status.success(), "{}", utf8(&init).1);
    fs::write(
        proj.join(format!(".ctx/traits/packages/{trait_id}/package.json")),
        PACKAGE_JSON_WITH_ALIAS,
    )
    .unwrap();
    fs::write(
        proj.join(format!(".ctx/traits/packages/{trait_id}/source/index.ts")),
        fixture_source("#trait/missing.ts"),
    )
    .unwrap();

    let build = run_ctx(
        &[
            "traits",
            "build",
            &format!(".ctx/traits/packages/{trait_id}/source/index.ts"),
        ],
        &proj,
        &home,
    );
    let (stdout, stderr) = utf8(&build);
    assert!(
        !build.status.success(),
        "build over a missing alias target must fail\nstdout: {stdout}\nstderr: {stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains(&format!("packages/{trait_id}/source/missing.ts")),
        "expected the resolved missing-file path in the error: {combined}"
    );
}
