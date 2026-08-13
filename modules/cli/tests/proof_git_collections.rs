//! 0191 behavioral proofs: git-first trait collections. A skills.sh-shaped
//! spec (`owner/repo/trait[@ref]`, or a bare collection with `--trait`/
//! `--all`) resolves refs over smart-HTTP, downloads a codeload tarball by
//! RESOLVED sha, vendors exactly the selected trait subtree, and locks the
//! commit — all served here by a local fixture remote (the public hosts are
//! off-limits for tests), reached through the `CTX_TRAITS_GIT_REMOTE_BASE`
//! and `CTX_TRAITS_GIT_CODELOAD_BASE` seams. Fixture tarballs carry a
//! `pax_global_header` entry because real `git archive` output does, and the
//! extraction path's skip for it must be exercised, not assumed.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

use support::{ScratchRoot, git_init, run_ctx_with_env, utf8};

// ---------------------------------------------------------------------------
// Fixture git remote: /owner/repo/info/refs + /owner/repo/tar.gz/<sha>
// ---------------------------------------------------------------------------

struct FixtureRoute {
    path: String,
    body: Vec<u8>,
}

struct FixtureRemote {
    base_url: String,
}

impl FixtureRemote {
    fn start(routes: Vec<FixtureRoute>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture remote");
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                serve_one(stream, &routes);
            }
        });
        FixtureRemote { base_url }
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
        Some(route) => http_response(200, "OK", &route.body),
        None => http_response(404, "Not Found", b"not found"),
    };
    let _ = stream.write_all(&response);
}

fn http_response(status: u16, reason: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn pkt_line(payload: &str) -> String {
    format!("{:04x}{}", payload.len() + 4, payload)
}

/// A `git-upload-pack` smart-HTTP advertisement: HEAD at `head_sha` with
/// `refs/heads/main`, plus a `v1` tag peeled to `tag_sha`.
fn ref_advertisement(head_sha: &str, tag_sha: &str) -> Vec<u8> {
    let mut body = String::new();
    body.push_str(&pkt_line("# service=git-upload-pack\n"));
    body.push_str("0000");
    body.push_str(&pkt_line(&format!(
        "{head_sha} HEAD\0multi_ack thin-pack\n"
    )));
    body.push_str(&pkt_line(&format!("{head_sha} refs/heads/main\n")));
    body.push_str(&pkt_line(&format!("{tag_sha} refs/tags/v1\n")));
    body.push_str(&pkt_line(&format!("{tag_sha} refs/tags/v1^{{}}\n")));
    body.push_str("0000");
    body.into_bytes()
}

/// A codeload-shaped tarball: top prefix `{repo}-{sha}/`, a leading
/// `pax_global_header` global-extended-header entry exactly like real
/// `git archive` output, then the given files.
fn codeload_tarball(repo: &str, sha: &str, entries: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);

    let pax_body = format!("52 comment={sha}\n");
    let mut pax_header = tar::Header::new_gnu();
    pax_header.set_entry_type(tar::EntryType::XGlobalHeader);
    pax_header.set_size(pax_body.len() as u64);
    pax_header.set_mode(0o644);
    pax_header.set_cksum();
    builder
        .append_data(&mut pax_header, "pax_global_header", pax_body.as_bytes())
        .expect("append pax_global_header");

    for (path, contents) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        let full = format!("{repo}-{sha}/{path}");
        builder
            .append_data(&mut header, full.as_str(), *contents)
            .unwrap_or_else(|error| panic!("cannot append {path} to fixture tarball: {error}"));
    }
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip")
}

/// One minimal command-only trait package under
/// `.ctx/traits/authored/<id>/` in the collection.
fn trait_package_entries(id: &str) -> Vec<(String, Vec<u8>)> {
    let canonical = format!(
        r#"id = "{id}"
schema-version = "0.4"
version = "0.1.0"
name = "{id}"
description = "Collection fixture trait {id}."

[procedure]
description = "One deterministic command."

[[slot]]
id = "out"
schema = "schema:text"

[[procedure.sequence]]
id = "speak"
title = "Speak"
kind = "command"
cmd = "printf {id}"
output = ["slot:out"]
"#
    );
    let manifest = format!(
        "[package]\nid = \"{id}\"\nversion = \"0.1.0\"\nname = \"{id}\"\nstatus = \"ready\"\n"
    );
    vec![
        (
            format!(".ctx/traits/authored/{id}/trait.toml"),
            manifest.into_bytes(),
        ),
        (
            format!(".ctx/traits/authored/{id}/generated/index.toml"),
            canonical.into_bytes(),
        ),
    ]
}

/// The full fixture collection: traits `one` and `two`, plus a JS library
/// (package.json, NO trait.toml) that classification must skip.
fn collection_entries() -> Vec<(String, Vec<u8>)> {
    let mut entries = trait_package_entries("one");
    entries.extend(trait_package_entries("two"));
    entries.push((
        "packages/js-lib/package.json".to_string(),
        b"{ \"name\": \"js-lib\", \"version\": \"1.0.0\" }".to_vec(),
    ));
    entries
}

struct GitFixture {
    remote: FixtureRemote,
    scratch: ScratchRoot,
    repo: std::path::PathBuf,
    cache_root: std::path::PathBuf,
}

const OWNER: &str = "octo";
const REPO: &str = "collection";

impl GitFixture {
    fn start(scratch_name: &str, head_sha: &str, tag_sha: &str) -> Self {
        let entries = collection_entries();
        let borrowed: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        let mut routes = vec![
            FixtureRoute {
                path: format!("/{OWNER}/{REPO}/info/refs?service=git-upload-pack"),
                body: ref_advertisement(head_sha, tag_sha),
            },
            FixtureRoute {
                path: format!("/{OWNER}/{REPO}/tar.gz/{head_sha}"),
                body: codeload_tarball(REPO, head_sha, &borrowed),
            },
        ];
        if tag_sha != head_sha {
            routes.push(FixtureRoute {
                path: format!("/{OWNER}/{REPO}/tar.gz/{tag_sha}"),
                body: codeload_tarball(REPO, tag_sha, &borrowed),
            });
        }
        let remote = FixtureRemote::start(routes);
        let scratch = ScratchRoot::new(scratch_name);
        let repo = scratch.home().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_init(&repo);
        let cache_root = scratch.home().join("git-cache");
        GitFixture {
            remote,
            scratch,
            repo,
            cache_root,
        }
    }

    fn run(&self, args: &[&str]) -> (String, String, bool) {
        let base = self.remote.base_url.clone();
        let cache = self.cache_root.to_string_lossy().to_string();
        let output = run_ctx_with_env(
            args,
            &self.repo,
            &self.scratch.home(),
            &[
                ("CTX_TRAITS_GIT_REMOTE_BASE", base.as_str()),
                ("CTX_TRAITS_GIT_CODELOAD_BASE", base.as_str()),
                ("CTX_TRAITS_GIT_CACHE_ROOT", cache.as_str()),
            ],
        );
        let (stdout, stderr) = utf8(&output);
        (stdout, stderr, output.status.success())
    }
}

fn head_sha() -> String {
    "a".repeat(40)
}

fn tag_sha() -> String {
    "b".repeat(40)
}

#[test]
fn shorthand_add_vendors_exactly_the_selected_trait_and_locks_the_sha() {
    let fixture = GitFixture::start("git-collection-shorthand", &head_sha(), &tag_sha());
    let (stdout, stderr, success) = fixture.run(&[
        "traits",
        "dependency",
        "add",
        &format!("{OWNER}/{REPO}/one"),
    ]);
    assert!(
        success,
        "shorthand add must vendor trait one\nstdout: {stdout}\nstderr: {stderr}"
    );
    let vendored = fixture.repo.join(".ctx/traits/vendored/one");
    assert!(
        vendored.join("trait.toml").is_file() && vendored.join("generated/index.toml").is_file(),
        "the vendored tree must carry exactly the selected trait package"
    );
    assert!(
        !fixture.repo.join(".ctx/traits/vendored/two").exists(),
        "the sibling trait must NOT be vendored by a single-trait add"
    );
    let lock = fs::read_to_string(fixture.repo.join(".ctx/traits/config.lock")).unwrap();
    assert!(
        lock.contains(&head_sha()),
        "the lock must pin the RESOLVED commit sha: {lock}"
    );

    // Locked reproduction: a plain install replays the locked snapshot.
    let (stdout, stderr, success) = fixture.run(&["traits", "dependency", "install"]);
    assert!(
        success,
        "install after add must reproduce the locked snapshot\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        vendored.join("generated/index.toml").is_file(),
        "reproduction must leave the vendored canonical in place"
    );
}

#[test]
fn bare_collection_spec_lists_traits_and_skips_the_js_package() {
    let fixture = GitFixture::start("git-collection-listing", &head_sha(), &tag_sha());
    let (stdout, stderr, success) =
        fixture.run(&["traits", "dependency", "add", &format!("{OWNER}/{REPO}")]);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        success,
        "a bare collection spec is a listing, not an error: {combined}"
    );
    assert!(
        combined.contains("one") && combined.contains("two"),
        "the listing must name both contained traits: {combined}"
    );
    assert!(
        !combined.contains("js-lib"),
        "a package.json-only entry is not a trait and must not be listed: {combined}"
    );
    assert!(
        !fixture.repo.join(".ctx/traits/vendored/one").exists(),
        "listing must not vendor anything"
    );
}

#[test]
fn wrong_trait_name_lists_what_exists_instead_of_bare_not_found() {
    let fixture = GitFixture::start("git-collection-wrong-name", &head_sha(), &tag_sha());
    let (stdout, stderr, success) = fixture.run(&[
        "traits",
        "dependency",
        "add",
        &format!("{OWNER}/{REPO}/three"),
    ]);
    let combined = format!("{stdout}\n{stderr}");
    assert!(!success, "an unknown trait name must refuse: {combined}");
    assert!(
        combined.contains("one") && combined.contains("two"),
        "the refusal must list the traits that DO exist: {combined}"
    );
}

#[test]
fn flag_form_and_tag_pin_resolve_like_the_shorthand() {
    let fixture = GitFixture::start("git-collection-flag-form", &head_sha(), &tag_sha());
    let (stdout, stderr, success) = fixture.run(&[
        "traits",
        "dependency",
        "add",
        &format!("https://github.com/{OWNER}/{REPO}"),
        "--trait",
        "one",
    ]);
    assert!(
        success,
        "the --trait flag form must behave like the shorthand\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        fixture
            .repo
            .join(".ctx/traits/vendored/one/trait.toml")
            .is_file(),
        "flag form must vendor the selected trait"
    );

    let (stdout, stderr, success) = fixture.run(&[
        "traits",
        "dependency",
        "add",
        &format!("{OWNER}/{REPO}/two@v1"),
    ]);
    assert!(
        success,
        "an @tag pin must resolve through the advertised tag\nstdout: {stdout}\nstderr: {stderr}"
    );
    let lock = fs::read_to_string(fixture.repo.join(".ctx/traits/config.lock")).unwrap();
    assert!(
        lock.contains(&tag_sha()),
        "the tag pin must lock the PEELED tag sha: {lock}"
    );
}

#[test]
fn usage_conflicts_refuse_before_any_network_call() {
    let fixture = GitFixture::start("git-collection-usage", &head_sha(), &tag_sha());
    let cases: &[&[&str]] = &[
        &[
            "traits",
            "dependency",
            "add",
            "octo/collection/one",
            "--trait",
            "two",
        ],
        &[
            "traits",
            "dependency",
            "add",
            "octo/collection",
            "--all",
            "--trait",
            "one",
        ],
        &[
            "traits",
            "dependency",
            "add",
            "octo/collection",
            "--trait",
            "one",
            "--trait",
            "two",
            "--alias",
            "x",
        ],
    ];
    for args in cases {
        let (stdout, stderr, success) = fixture.run(args);
        assert!(
            !success,
            "usage conflict must refuse: {args:?}\nstdout: {stdout}\nstderr: {stderr}"
        );
    }
}

#[test]
fn all_flag_vendors_every_listed_trait() {
    let fixture = GitFixture::start("git-collection-all", &head_sha(), &tag_sha());
    let (stdout, stderr, success) = fixture.run(&[
        "traits",
        "dependency",
        "add",
        &format!("{OWNER}/{REPO}"),
        "--all",
    ]);
    assert!(
        success,
        "--all must vendor every trait in the collection\nstdout: {stdout}\nstderr: {stderr}"
    );
    for id in ["one", "two"] {
        assert!(
            fixture
                .repo
                .join(format!(".ctx/traits/vendored/{id}/trait.toml"))
                .is_file(),
            "--all must vendor trait {id}"
        );
    }
    assert!(
        !fixture.repo.join(".ctx/traits/vendored/js-lib").exists(),
        "--all must never vendor a non-trait package"
    );
}

#[test]
fn github_shaped_specs_never_invoke_git_or_node() {
    // The ctx binary runs by absolute path; PATH holds one empty directory,
    // so any child-process reach for `git`/`node`/`npm` fails loudly.
    let fixture = GitFixture::start("git-collection-no-toolchain", &head_sha(), &tag_sha());
    let empty_bin = fixture.scratch.home().join("empty-bin");
    fs::create_dir_all(&empty_bin).unwrap();
    let base = fixture.remote.base_url.clone();
    let cache = fixture.cache_root.to_string_lossy().to_string();
    let empty = empty_bin.to_string_lossy().to_string();
    let output = run_ctx_with_env(
        &[
            "traits",
            "dependency",
            "add",
            &format!("{OWNER}/{REPO}/one"),
        ],
        &fixture.repo,
        &fixture.scratch.home(),
        &[
            ("CTX_TRAITS_GIT_REMOTE_BASE", base.as_str()),
            ("CTX_TRAITS_GIT_CODELOAD_BASE", base.as_str()),
            ("CTX_TRAITS_GIT_CACHE_ROOT", cache.as_str()),
            ("PATH", empty.as_str()),
        ],
    );
    let (stdout, stderr) = utf8(&output);
    assert!(
        output.status.success(),
        "the pure-Rust git transport must work with no git/node on PATH\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        fixture
            .repo
            .join(".ctx/traits/vendored/one/trait.toml")
            .is_file(),
        "the vendored tree must land without any toolchain"
    );
}
