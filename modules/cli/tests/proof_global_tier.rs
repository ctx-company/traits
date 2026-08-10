//! P439: global-tier trait distribution + run-anywhere behavioral proof
//! (task 0121). Drives the real `ctx` binary against a minimal in-process
//! npm-registry stub (a raw `TcpListener` HTTP server; the public registry
//! is off-limits for tests and no HTTP-mocking crate is a workspace
//! dependency) — never the real npm registry, never `node`/`npm`/`pnpm`.
//!
//! Covers the four Done-when clauses of `archived/board/execution-plan.md`
//! Group 108 that had zero prior test coverage: `-g` install lands under the
//! isolated config home and appends to the audit journal; the same trait
//! installed at both tiers resolves project-first with the shadow surfaced;
//! an ad-hoc (non-repository) invocation resolves the global tier and gets
//! an `adhoc-<slug>` checkout key; a `worktreeRequired` trait refuses
//! outside a Git repository; `trust approve <alias>` admits every current
//! variant digest of a family package in one review.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;

use base64::Engine;
use sha2::{Digest as _, Sha512};
use support::{ScratchRoot, git_init, require_success, require_success_with_env, run_ctx, utf8};

// ---------------------------------------------------------------------------
// Minimal in-process npm-registry fixture
// ---------------------------------------------------------------------------

/// One route this fixture registry serves verbatim on an exact path match.
struct FixtureRoute {
    path: String,
    content_type: &'static str,
    body: Vec<u8>,
}

/// A minimal single-package npm registry, bound to an ephemeral localhost
/// port and served on a background thread for the lifetime of the process
/// (test binaries exit without joining background threads, same as every
/// other fixture in this suite). `Connection: close` on every response
/// means each request opens a fresh connection, so the accept loop never has
/// to implement HTTP keep-alive.
struct FixtureRegistry {
    base_url: String,
}

impl FixtureRegistry {
    /// Bind an ephemeral localhost port first, then hand its base URL to
    /// `build_routes` — so a route body that must embed its own server's
    /// base URL (the tarball URL inside the metadata JSON) never has to
    /// guess it or bind a throwaway listener first.
    fn start(build_routes: impl FnOnce(&str) -> Vec<FixtureRoute>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture registry");
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let routes = build_routes(&base_url);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                serve_one(stream, &routes);
            }
        });
        FixtureRegistry { base_url }
    }
}

fn serve_one(mut stream: TcpStream, routes: &[FixtureRoute]) {
    let peer = stream.try_clone().expect("clone fixture connection");
    let mut reader = BufReader::new(peer);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();
    loop {
        let mut header_line = String::new();
        match reader.read_line(&mut header_line) {
            Ok(0) => break,
            Ok(_) if header_line == "\r\n" || header_line == "\n" => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    let response = match routes.iter().find(|route| route.path == path) {
        Some(route) => http_response(200, "OK", route.content_type, &route.body),
        None => http_response(404, "Not Found", "text/plain", b"not found"),
    };
    let _ = stream.write_all(&response);
}

fn http_response(status: u16, reason: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// Build a gzip-tar npm tarball from `(relative_path, contents)` entries —
/// the same `package.toml` + `generated/index.toml` shape the `path:`
/// transport proofs (`proof_path_distribution.rs`) use, just packed as a
/// tarball instead of copied as a directory tree.
fn build_tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (path, contents) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, *contents)
            .unwrap_or_else(|error| panic!("cannot append {path} to fixture tarball: {error}"));
    }
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip")
}

fn sha512_integrity(bytes: &[u8]) -> String {
    let mut hasher = Sha512::new();
    hasher.update(bytes);
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
    )
}

fn trait_doc(id: &str, summary: &str, worktree_required: bool) -> String {
    let worktree_line = if worktree_required {
        "worktree-required = true\n"
    } else {
        ""
    };
    format!(
        "id = \"{id}\"\n\
         schema-version = \"0.2\"\n\
         version = \"0.1.0\"\n\
         name = \"Demo\"\n\
         summary = \"{summary}\"\n\
         \n\
         [procedure]\n\
         description = \"Run command\"\n\
         {worktree_line}\
         \n\
         [[slot]]\n\
         id = \"notified\"\n\
         schema = \"schema:text\"\n\
         \n\
         [[procedure.sequence]]\n\
         id = \"command\"\n\
         title = \"Run command\"\n\
         kind = \"command\"\n\
         cmd = \"true\"\n\
         output = [\"slot:notified\"]\n"
    )
}

fn family_variant_trait_doc(id: &str, variant: &str, summary: &str) -> String {
    format!(
        "id = \"{id}\"\n\
         schema-version = \"0.3\"\n\
         version = \"0.1.0\"\n\
         name = \"Demo\"\n\
         summary = \"{summary}\"\n\
         variant = \"{variant}\"\n"
    )
}

/// A single-trait npm package fixture, published under `package` at version
/// `version`, and its resolved SRI integrity — everything
/// `dependency add -g` needs to install it from [`FixtureRegistry`].
struct SingleTraitPackage {
    version: String,
    registry: FixtureRegistry,
}

/// Publishes a single tarball as an npm package: builds the npm registry
/// metadata JSON, computes SRI integrity, and starts a [`FixtureRegistry`]
/// serving both the metadata and tarball routes.
fn npm_registry_serving(package: &str, version: &str, tarball: Vec<u8>) -> FixtureRegistry {
    let integrity = sha512_integrity(&tarball);
    let tarball_route = format!("/{package}/-/{package}-{version}.tgz");
    FixtureRegistry::start(|base_url| {
        let metadata = format!(
            "{{\"name\":\"{package}\",\"dist-tags\":{{\"latest\":\"{version}\"}},\"versions\":{{\"{version}\":{{\"dist\":{{\"tarball\":\"{base_url}{tarball_route}\",\"integrity\":\"{integrity}\"}}}}}}}}",
        );
        vec![
            FixtureRoute {
                path: format!("/{package}"),
                content_type: "application/json",
                body: metadata.into_bytes(),
            },
            FixtureRoute {
                path: tarball_route.clone(),
                content_type: "application/octet-stream",
                body: tarball.clone(),
            },
        ]
    })
}

fn single_trait_npm_package(
    package: &str,
    trait_id: &str,
    worktree_required: bool,
) -> SingleTraitPackage {
    let version = "0.1.0";
    let manifest = format!(
        "[package]\nid = \"{trait_id}\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"ready\"\n"
    );
    let doc = trait_doc(trait_id, "Demo", worktree_required);
    let tarball = build_tarball(&[
        ("package.toml", manifest.as_bytes()),
        ("generated/index.toml", doc.as_bytes()),
    ]);
    let registry = npm_registry_serving(package, version, tarball);
    SingleTraitPackage {
        version: version.to_string(),
        registry,
    }
}

/// A two-variant native family npm package fixture (`default` + `quick`,
/// matching `proof_path_distribution.rs`'s `path_dependency_installs_every_family_variant`
/// producer shape), for the package-granular trust proof.
struct FamilyPackage {
    registry: FixtureRegistry,
}

fn family_npm_package(package: &str, trait_id: &str) -> FamilyPackage {
    let version = "0.1.0";
    let manifest = format!(
        "[package]\n\
         id = \"{trait_id}\"\n\
         version = \"0.1.0\"\n\
         name = \"Family Demo\"\n\
         status = \"ready\"\n\
         \n\
         [family]\n\
         default = \"default\"\n\
         \n\
         [family.variant.default]\n\
         path = \"generated/default/index.toml\"\n\
         \n\
         [family.variant.quick]\n\
         path = \"generated/quick/index.toml\"\n"
    );
    let default_doc = family_variant_trait_doc(trait_id, "default", "default variant");
    let quick_doc = family_variant_trait_doc(trait_id, "quick", "quick variant");
    let tarball = build_tarball(&[
        ("package.toml", manifest.as_bytes()),
        ("generated/default/index.toml", default_doc.as_bytes()),
        ("generated/quick/index.toml", quick_doc.as_bytes()),
    ]);
    let registry = npm_registry_serving(package, version, tarball);
    FamilyPackage { registry }
}

fn write_fixture_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

// ---------------------------------------------------------------------------
// 1. `-g` install lands manifest/lock/vendor under the isolated config home
//    and appends to the audit journal.
// ---------------------------------------------------------------------------

#[test]
fn global_install_records_manifest_lock_vendor_and_audit_journal() {
    let scratch = ScratchRoot::new("global-tier-install");
    let home = scratch.home();
    let fixture = single_trait_npm_package("global-demo", "global-demo", false);
    let cwd = home.join("nonrepo");
    fs::create_dir_all(&cwd).unwrap();

    let add_stdout = require_success_with_env(
        "`ctx traits dependency add -g global-demo --json`",
        &["traits", "dependency", "add", "-g", "global-demo", "--json"],
        &cwd,
        &home,
        &[("CTX_TRAITS_REGISTRY_BASE", &fixture.registry.base_url)],
    );
    assert!(
        add_stdout.contains("\"transport\": \"npm\""),
        "global install report did not report npm transport: {add_stdout}"
    );
    assert!(
        add_stdout.contains(&format!("\"resolved-version\": \"{}\"", fixture.version)),
        "global install report did not resolve the fixture version: {add_stdout}"
    );

    let manifest_text = fs::read_to_string(home.join("ctx/traits.toml")).unwrap();
    assert!(
        manifest_text.contains("global-demo"),
        "global manifest at ~/.config/ctx/traits.toml missing the installed package: {manifest_text}"
    );
    let lock_text = fs::read_to_string(home.join("ctx/traits.lock")).unwrap();
    assert!(
        lock_text.contains("package = \"global-demo\""),
        "global lock at ~/.config/ctx/traits.lock missing the installed package: {lock_text}"
    );
    assert!(
        home.join("ctx/traits/global-demo/generated/index.toml")
            .is_file(),
        "global vendor tree was not written under ~/.config/ctx/traits/global-demo/"
    );

    let audit_dir = home.join("ctx/audit");
    let journals: Vec<_> = fs::read_dir(&audit_dir)
        .unwrap_or_else(|error| panic!("no audit journal directory at {audit_dir:?}: {error}"))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    assert_eq!(
        journals.len(),
        1,
        "expected exactly one month's audit journal file, found {}",
        journals.len()
    );
    let journal_text = fs::read_to_string(journals[0].path()).unwrap();
    assert!(
        journal_text.contains("\"package\":\"global-demo\"")
            || journal_text.contains("\"package\": \"global-demo\""),
        "audit journal did not record the global install: {journal_text}"
    );
    assert!(
        journal_text.contains("\"scope\":\"global\"")
            || journal_text.contains("\"scope\": \"global\""),
        "audit journal did not record a global-scope mutation: {journal_text}"
    );
}

// ---------------------------------------------------------------------------
// 2. Same trait id installed at both tiers: project wins, shadow named in
//    `list`/`doctor`.
// ---------------------------------------------------------------------------

#[test]
fn project_tier_shadows_global_tier() {
    let scratch = ScratchRoot::new("global-tier-shadow");
    let home = scratch.home();
    let fixture = single_trait_npm_package("shadow-demo", "shadow-demo", false);

    // Global install.
    require_success_with_env(
        "`ctx traits dependency add -g shadow-demo`",
        &["traits", "dependency", "add", "-g", "shadow-demo"],
        &home,
        &home,
        &[("CTX_TRAITS_REGISTRY_BASE", &fixture.registry.base_url)],
    );

    // Project install of the *same trait id*, via `path:` (P535), in a repo.
    let producer = home.join("producer/shadow-demo");
    write_fixture_file(
        &producer.join("package.toml"),
        "[package]\nid = \"shadow-demo\"\nversion = \"0.1.0\"\nname = \"Demo\"\nstatus = \"ready\"\n",
    );
    write_fixture_file(
        &producer.join("generated/index.toml"),
        &trait_doc("shadow-demo", "project copy", false),
    );
    let consumer = home.join("consumer");
    fs::create_dir_all(&consumer).unwrap();
    git_init(&consumer);
    require_success(
        "`ctx traits dependency add path:../producer/shadow-demo`",
        &[
            "traits",
            "dependency",
            "add",
            "path:../producer/shadow-demo",
        ],
        &consumer,
        &home,
    );

    let list_stdout = require_success(
        "`ctx traits list --json`",
        &["traits", "list", "--json"],
        &consumer,
        &home,
    );
    assert!(
        list_stdout.contains("\"origin\": \"path:../producer/shadow-demo\""),
        "project-tier winner must report its path origin: {list_stdout}"
    );
    assert!(
        list_stdout.contains(&format!(
            "\"shadow\": \"npm (global):shadow-demo@{}\"",
            fixture.version
        )),
        "list must surface the shadowed global-tier candidate: {list_stdout}"
    );
}

// ---------------------------------------------------------------------------
// 3. Ad-hoc (non-repository) run resolves the global tier, records an
//    `adhoc-<slug>` checkout key, and a `worktreeRequired` global trait
//    refuses outside a Git repository.
// ---------------------------------------------------------------------------

#[test]
fn adhoc_run_resolves_global_tier_and_worktree_required_trait_refuses() {
    let scratch = ScratchRoot::new("global-tier-adhoc-run");
    let home = scratch.home();
    let plain = single_trait_npm_package("adhoc-plain", "adhoc-plain", false);
    let worktree = single_trait_npm_package("adhoc-worktree", "adhoc-worktree", true);

    require_success_with_env(
        "`ctx traits dependency add -g adhoc-plain`",
        &["traits", "dependency", "add", "-g", "adhoc-plain"],
        &home,
        &home,
        &[("CTX_TRAITS_REGISTRY_BASE", &plain.registry.base_url)],
    );
    require_success_with_env(
        "`ctx traits dependency add -g adhoc-worktree`",
        &["traits", "dependency", "add", "-g", "adhoc-worktree"],
        &home,
        &home,
        &[("CTX_TRAITS_REGISTRY_BASE", &worktree.registry.base_url)],
    );

    let cwd = home.join("nonrepo");
    fs::create_dir_all(&cwd).unwrap();
    require_success(
        "`ctx traits trust approve adhoc-plain`",
        &["traits", "trust", "approve", "adhoc-plain"],
        &cwd,
        &home,
    );
    require_success(
        "`ctx traits trust approve adhoc-worktree`",
        &["traits", "trust", "approve", "adhoc-worktree"],
        &cwd,
        &home,
    );

    let run_output = run_ctx(&["traits", "run", "adhoc-plain", "--verbose"], &cwd, &home);
    let (run_stdout, run_stderr) = utf8(&run_output);
    assert!(
        run_output.status.success(),
        "ad-hoc run of the global-tier trait must complete\nstdout: {run_stdout}\nstderr: {run_stderr}"
    );
    assert!(
        run_stdout.contains("origin: trait-id") || run_stdout.contains("origin:"),
        "run --verbose must print the resolved origin line: {run_stdout}"
    );

    let checkouts_text = fs::read_to_string(home.join("ctx/checkouts.toml"))
        .unwrap_or_else(|error| panic!("no checkouts.toml recorded for the ad-hoc run: {error}"));
    assert!(
        checkouts_text.contains("adhoc-"),
        "ad-hoc run must record an `adhoc-<slug>` checkout key: {checkouts_text}"
    );

    let worktree_output = run_ctx(&["traits", "run", "adhoc-worktree"], &cwd, &home);
    assert!(
        !worktree_output.status.success(),
        "a worktreeRequired trait must refuse outside a Git repository"
    );
    let (_, worktree_stderr) = utf8(&worktree_output);
    assert!(
        worktree_stderr.contains("requires a prepared worktree, which requires a Git repository")
            && worktree_stderr.contains("run inside a Git checkout"),
        "worktreeRequired refusal must name the exact repository-requirement message: {worktree_stderr}"
    );
}

// ---------------------------------------------------------------------------
// 4. `trust approve <alias>` admits every current variant digest of an
//    installed family package in one review (the P534 lineage-regression
//    class: package-granular approval must never revoke a sibling variant).
// ---------------------------------------------------------------------------

#[test]
fn package_granular_trust_approves_every_family_variant_in_one_review() {
    let scratch = ScratchRoot::new("global-tier-family-trust");
    let home = scratch.home();
    let family = family_npm_package("family-pkg", "family-demo");

    require_success_with_env(
        "`ctx traits dependency add -g family-pkg --alias family-install --json`",
        &[
            "traits",
            "dependency",
            "add",
            "-g",
            "family-pkg",
            "--alias",
            "family-install",
            "--json",
        ],
        &home,
        &home,
        &[("CTX_TRAITS_REGISTRY_BASE", &family.registry.base_url)],
    );

    let cwd = home.join("nonrepo");
    fs::create_dir_all(&cwd).unwrap();

    // The manifest alias ("family-install") does not resolve as a trait id
    // (that's "family-demo"), so `trust approve` falls through to
    // package-granular resolution and approves every current variant digest
    // atomically in this one call (`ctx_traits_io::distribution::approve_package`).
    let approve_stdout = require_success(
        "`ctx traits trust approve family-install --json`",
        &["traits", "trust", "approve", "family-install", "--json"],
        &cwd,
        &home,
    );
    let digest_occurrences = approve_stdout.matches("sha256:").count();
    assert!(
        digest_occurrences >= 2,
        "package-granular approve of a 2-variant family must record at least 2 digests in one report: {approve_stdout}"
    );

    for reference in ["family-demo", "family-demo:quick"] {
        let check = run_ctx(&["traits", "check", reference, "--json"], &cwd, &home);
        let (check_stdout, check_stderr) = utf8(&check);
        assert!(
            check.status.success(),
            "{reference} must check clean after one package-granular approve\nstdout: {check_stdout}\nstderr: {check_stderr}"
        );
    }
}
