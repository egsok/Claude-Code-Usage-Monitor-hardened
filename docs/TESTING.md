<!-- generated-by: gsd-doc-writer -->
# Testing

## Test framework and layout

The project uses Rust's built-in test harness. Tests are co-located in source
modules under `#[cfg(test)] mod tests`; there is no separate `tests/` directory,
mocking framework, coverage tool, or GUI automation suite.

The current tests exercise pure parsing and state-transition helpers rather than
calling provider APIs. Filesystem cache tests use unique directories beneath the
system temporary directory and clean up their own fixtures.

## Running tests

Run the full suite and continue after individual failures:

```powershell
cargo test --no-fail-fast
```

Run one module:

```powershell
cargo test usage_cache::tests
cargo test poller::tests
cargo test window::tests
```

Run one regression by name:

```powershell
cargo test simultaneous_transient_failures_preserve_both_provider_snapshots
```

Build release code after tests because Windows resources and release-only
settings are not fully exercised by the debug test binary:

```powershell
cargo build --release
```

## What the automated suite protects

### Security invariants

- Polling may start `wsl.exe` for credential-file access but must not start
  Claude Code, Codex, or another agent CLI.
- Usage-cache JSON must not contain token or credential fields.
- Corrupt cache data must be rejected.

### Provider behavior

- A Claude failure does not block a successful Codex response and vice versa.
- An Antigravity failure does not block another enabled provider.
- Partial transient failures request a faster retry.
- Authentication failures do not enter network retry backoff.
- The Claude Messages fallback requests a later detailed usage refresh.
- Dynamic model-scoped limits, including Fable, are parsed from the usage
  response rather than hard-coded as a separate endpoint.

### Cache and stale rendering

- Claude/Fable and Codex snapshots round-trip independently.
- Updating one provider preserves the other provider's cached snapshot.
- A schema v1 Claude-only cache migrates without losing Fable.
- Cached values receive a compact `~` marker.
- Simultaneous transient failures preserve both provider snapshots.
- An authoritative response can remove a no-longer-returned Fable quota.

### Windows state and placement

- Taskbar child styles retain layered rendering flags.
- Floating coordinates round-trip and clamp into a work area.
- Taskbar placement remains the default for older settings.
- Startup registry commands quote executable paths and recognize the legacy
  unquoted form.
- Invalid temporary tray geometry does not collapse the widget position.
- Layout retains room for the model-scoped Fable text as provider count changes.

## Writing new tests

Put a test beside the code that owns the invariant. Prefer a named behavioral
contract over a low-level implementation assertion. For example, a retry test
should prove that one provider's temporary failure preserves other provider
data, not merely assert an internal counter value.

Avoid live network, real credentials, global registry mutation, or interacting
with Explorer in unit tests. Extract deterministic helpers for response parsing,
state merging, layout calculations, path matching, and serialization.

When fixing a bug:

1. write a test that fails for the observed reason;
2. make the minimal source change;
3. prove the new test and the full suite pass;
4. perform a manual check for any Win32 behavior that the unit test cannot
   represent.

## Formatting and static checks

```powershell
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings `
  -A clippy::single-match `
  -A clippy::manual-is-multiple-of `
  -A clippy::too-many-arguments
git diff --check
```

No coverage threshold is configured.

## Manual verification matrix

Automated tests cannot prove taskbar integration. Select checks proportional to
the change, and run the full matrix before a release affecting `src/window.rs`,
`src/native_interop.rs`, authentication, cache, or update behavior.

| Area | Checks |
|---|---|
| Startup | Start with no cache, with schema v1 cache, with schema v2 cache, and with a corrupt cache. |
| Providers | Claude only; Claude + Codex; all enabled providers; one provider unavailable while another succeeds. |
| Network | Deny network at startup; restore it; interrupt during refresh; verify known values remain with `~`. |
| Authentication | Expire/remove each enabled provider's credentials; refresh them through the provider tool; verify automatic recovery without agent launch. |
| Fable | Detailed response includes Fable; Messages fallback omits it; later authoritative response removes it. |
| Refresh | Manual refresh with known values; provider toggle off/on; normal interval; exponential retry. |
| Placement | Primary and secondary taskbars; drag; floating; tray-only; placement reset; centered Windows 11 icons. |
| Explorer | Restart Explorer and verify the widget reattaches or relaunches without becoming invisible. |
| Windows restart | Verify installed-path autostart, saved position, visibility, provider selection, and cached bars. |
| Theme/DPI | Light and dark taskbars, mixed-DPI monitors, and taskbar scale changes. |
| Release | Installed EXE and downloaded GitHub asset report the intended PE version. |

## CI integration

`.github/workflows/release.yml` runs only for `v*` tag pushes. It builds a
release binary and publishes it, but currently does not run `cargo test`,
`cargo fmt -- --check`, or Clippy. Until a separate CI workflow is added, those
checks depend on the developer's local release discipline.
