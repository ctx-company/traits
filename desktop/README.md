# ctx-traits-desktop

The standalone gpui desktop shell (0256.1). This project lives outside the
root Cargo workspace on purpose: `gpui` resolves ~700 crates, and none of
that cost may reach `just test`, `just test-full`, or a CI lane that
otherwise builds and gates the workspace. `desktop/` has its own
`[workspace]` table, its own committed `Cargo.lock`, and is listed in the
root `Cargo.toml`'s `exclude`, so no cargo command run from the repo root
ever resolves or builds it.

## Commands

Run these from the repo root via `just`, which sets `CARGO_TARGET_DIR`
explicitly so a desktop build never lands in the workspace gates' shared
cache slot:

```
just desktop-build   # cargo build
just desktop-test    # cargo test
just desktop-lint    # cargo fmt --check && cargo clippy -D warnings
just desktop-run     # cargo run — opens the window
```

Equivalent plain-cargo commands work from inside `desktop/` as long as
`CARGO_TARGET_DIR` is not inherited from a workspace gate run; prefer the
`just` recipes to avoid cache cross-contamination.

## Prerequisites (macOS)

`gpui`'s build script needs full Xcode, not just the Xcode Command Line
Tools: it runs `bindgen` (needs libclang), `cbindgen`, and compiles Metal
shaders via `xcrun metal`. Verify with:

```
xcode-select -p        # -> /Applications/Xcode.app/Contents/Developer
xcrun --find metal      # must resolve
```

## Prerequisites (Linux, unvalidated by this project)

`gpui` on Linux needs X11/Wayland/xkbcommon development packages
(`libxkbcommon-dev`, `libx11-dev`, `libxcb-*-dev`, `libwayland-dev`, or your
distribution's equivalents). This has not been built or run on Linux as
part of 0256.1 — treat it as unverified until a later task validates it.

## Toolchain

No second `rust-toolchain.toml`: rustup walks up from `desktop/` and finds
the root pin (1.97.1), so the desktop lane always builds with the same
compiler as the rest of the repo.

## Center link (0256.2)

On startup the shell connects to the machine-local run center through
`ctx_traits_io::center::subscribe_existing` and receives one coherent
`SnapshotStart → SnapshotRow* → SnapshotEnd` snapshot on a background
thread, forwarded to the gpui UI thread over an `async-channel`. It speaks
the same version-scoped socket as the CLI (`/tmp/ctx-{uid}-{version}.sock`,
scoped by `ctx-traits-io`'s own `CARGO_PKG_VERSION`), so a center started
from a different installed `ctx` version is a different socket and the
desktop reports it unavailable rather than connecting to it.

The desktop **never launches a center** — `subscribe_existing` only connects
to one that is already serving, fails fast otherwise, and never retries.
Absent-center startup posture and reconnection are 0256.5's; applying later
deltas after the initial snapshot is 0256.4's.

For `just desktop-run` to show anything, start a matching-version center
first, e.g. from the repo root:

```
cargo run --bin ctx -- traits internal stats --json
```

then run `just desktop-run` within that center's idle window (300s by
default).
