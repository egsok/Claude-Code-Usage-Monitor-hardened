<!-- generated-by: gsd-doc-writer -->
# Development

## Local setup

Follow `docs/GETTING-STARTED.md` for the Windows/Rust prerequisites and first
build. There is no database, server, `.env` file, JavaScript toolchain, or code
generation step. Cargo resolves all project dependencies.

The installed application and a development build share the same global mutex,
settings, and usage cache. Before running a development binary, identify and
stop the existing monitor by its full executable path.

## Build and quality commands

| Command | Purpose |
|---|---|
| `cargo build` | Compile a debug executable. |
| `cargo run` | Build and launch the debug executable. |
| `cargo run -- --diagnose` | Launch with the temporary diagnostic log enabled. |
| `cargo build --release` | Build the optimized distributable EXE with embedded version metadata. |
| `cargo test --no-fail-fast` | Run all co-located Rust unit and regression tests. |
| `cargo fmt` | Format Rust source. |
| `cargo fmt -- --check` | Verify formatting without writing. |
| `git diff --check` | Detect whitespace errors before committing. |

The current strict Clippy baseline is:

```powershell
cargo clippy --all-targets -- -D warnings `
  -A clippy::single-match `
  -A clippy::manual-is-multiple-of `
  -A clippy::too-many-arguments
```

The three allowances cover existing style debt in polling and drawing code.
Do not use them to suppress a new warning category introduced by a change.

## Code organization and style

- Keep the application a native Rust/Win32 program unless a product decision
  explicitly changes that constraint.
- Use `rustfmt`; no separate style configuration is present.
- Preserve provider isolation: one provider's failure must not erase another
  provider's successful data.
- Treat authentication errors separately from transient network errors.
- UI mutation belongs on the Win32 thread. Background work communicates through
  shared state plus posted window messages.
- Changes to taskbar parenting must preserve layered rendering, floating mode,
  multi-monitor behavior, and Explorer-restart recovery.
- Never add a Claude Code, Codex, or other agent subprocess for login or token
  refresh. `wsl.exe` is allowed only for credential-file discovery and reading.
- Persist usage snapshots only after a successful provider response and never
  serialize credential-bearing structs.

Tests are co-located in each Rust module under `#[cfg(test)]`; match that pattern
instead of adding a parallel test framework.

## Change workflow

1. Create an `agent/<short-description>` branch for non-trivial work.
2. Write or update a regression test that captures the intended invariant.
3. Make the smallest source change that satisfies the test.
4. Run the automated checks in `docs/TESTING.md`.
5. Perform the relevant Windows manual checks.
6. Stage explicit paths. The repository contains old untracked `target-*`
   directories on the primary development machine; never stage them with a
   blanket add.
7. Use a terse commit subject describing the behavior change.

Pull requests are useful for CI, review history, or risky changes, but are not a
release requirement for a solo maintainer. A verified branch may be
fast-forwarded to `main` directly when that is the deliberate release choice.

## Release runbook

### 1. Prepare the version

Update:

- the package version in `Cargo.toml`;
- the root package version in `Cargo.lock`;
- the current release summary in `README.md`;
- a matching versioned file under `.github/release-notes/` in English and Russian.

`build.rs` reads `CARGO_PKG_VERSION` and embeds matching `FileVersion` and
`ProductVersion` values into the PE file.

### 2. Verify locally

Stop any process running from `target\release`, then run:

```powershell
cargo test --no-fail-fast
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings `
  -A clippy::single-match `
  -A clippy::manual-is-multiple-of `
  -A clippy::too-many-arguments
cargo build --release
git diff --check
```

Inspect the result:

```powershell
$exe = Get-Item target\release\claude-code-usage-monitor.exe
$exe.VersionInfo | Select-Object FileVersion, ProductVersion
Get-FileHash $exe.FullName -Algorithm SHA256
```

### 3. Test the installed copy

The stable per-user installation used during development is:

```text
%LOCALAPPDATA%\Programs\ClaudeCodeUsageMonitor\claude-code-usage-monitor.exe
```

Stop the exact running process, keep a versioned backup, copy the verified
release binary, and start it from the installed path. If **Start with Windows**
is enabled, verify that the registry value points to this stable path rather
than `target\release`.

### 4. Publish

Push the verified commit to `main`, create tag `vX.Y.Z`, and push the tag. The
tag-triggered workflow in `.github/workflows/release.yml`:

1. checks out the tagged commit on `windows-latest`;
2. installs the stable Rust toolchain;
3. builds with `cargo build --release`;
4. creates the GitHub Release with the matching versioned notes file;
5. optionally submits a dedicated WinGet package update when the repository
   secret is configured.

The workflow does not run the test suite. Local test completion is therefore a
required release gate until CI is expanded.

### 5. Verify the public artifact

Do not stop at a successful tag push. Confirm the workflow conclusion, release
body, asset presence, asset PE version, and SHA-256 after downloading the public
EXE. The release artifact is currently not Authenticode-signed; this is a known
residual distribution risk documented in `docs/SECURITY.md`.

## Rollback

For a local regression, stop the installed process, restore the versioned backup
EXE, and start it from the same installed path. Be aware that a newer usage-cache
schema may be unreadable by an older binary. Current schema v2 preserves v1 read
compatibility, but the reverse is not guaranteed.

For a bad GitHub release, do not move an existing tag. Fix forward with a new
patch version or explicitly remove the release and tag only after confirming the
exact targets and preserving the published artifact for diagnosis.
