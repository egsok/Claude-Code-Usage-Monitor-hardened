# Hardened upstream v2 experiment

## Identity and scope

- Upstream tag: v2.15.14, commit `814ff7339f70ac8234f4a8177dd2b061dc2235fa`.
- Hardened behavior reference: v1.7.0, commit `bea4ab9`.
- Branch: `agent/upstream-v2-hardened-experiment`.
- Application version: `2.15.14-hardened.1`.
- Keep the upstream native renderer, Studio, themes, account profiles and providers.
- Apply selected-monitor copies to the first authored taskbar root. Additional
  roots retain their authored behavior. With no taskbar root, copies are unavailable
  and saved monitor selection/positions remain intact.

The installed v1.7 executable, process, settings and startup registration are not
replaced. The experiment uses its own application directory, named mutexes, window
classes, Studio title/event, and diagnostic log.

## Build and launch

Use the pinned Rust 1.95.0 toolchain; do not change the global rustup default.
From the experimental worktree:

```powershell
cargo test --locked --offline --no-fail-fast --target-dir ../upstream-v2-build
cargo build --locked --offline --release --target-dir ../upstream-v2-build
../upstream-v2-build/release/claude-code-usage-monitor.exe --diagnose --dashboard
```

First dependency download requires `cargo fetch --locked`. Where Cargo does not
inherit the Windows proxy, set CARGO_HTTP_PROXY for that command from the existing
system proxy; do not change system settings.

Application data: `%APPDATA%\ClaudeCodeUsageMonitorHardenedUpstream2`.
Diagnostic log: `%TEMP%\claude-code-usage-monitor-hardened-upstream2.log`.
First launch starts the managed widget in Floating. The monitor menu switches to
taskbar copies and chooses screens. Studio opens from the tray or `--dashboard`.

For a one-time v1 import, copy only settings.json, usage-cache.json and, if present,
settings.pre-v2.json to a snapshot directory, then pass `--import-v1 <directory>`
on first launch. Import rejects the live v1 directory and an already populated
experimental profile. It keeps byte-exact backups inside the experiment and never
copies provider credentials. A completed import is not repeated.

Close only the experiment to return to v1.7. Do not register the experiment for
startup or copy it over the installed executable. No public release is implied.

## Deliberate differences from upstream

- Claude, Codex and Grok CLIs are never launched, including version probes.
  Expired authorization is renewed by the user in the provider's application.
- Antigravity does not exchange refresh tokens. Cursor reads its SQLite store
  directly without copying a credential-bearing database to a temporary file.
- WSL subprocesses are restricted to bounded credential discovery/read/stat;
  non-login shells avoid loading login profiles.
- Releases are informational, scoped to the hardened repository. Executable
  download/replacement, updater helper and WinGet execution are removed.
- Startup controls are disabled in this experimental build.
- Codex credits stores a domain-separated SHA-256 account key rather than the
  provider account ID. The original ID exists only in memory for request headers.
- Claude/Codex successes retain separate timestamps and stale readings on failure.
  Local account profiles stay distinct. The v1 cache adapter binds old readings
  only to default accounts and imports Fable explicitly.
- Classic shows Fable. Incomplete fallback retains it with a stale marker;
  authoritative absence removes it. Reset timers from 24 hours show decimal days,
  rounded down.
- Selected taskbars share polling but retain device-path identity, independent
  DIP offsets and DPI. Temporary fallback/clamping must not replace saved choices.
  The hidden controller survives destruction of shell-owned windows.
- Monitor and Studio settings changes use cross-process transactions and update
  only edited fields, preventing one process from overwriting another's settings.

Device identity is not guaranteed across port or driver changes. Floating means
one managed primary widget; additional authored theme windows are unaffected.

## Validation record

Before porting, the v1.7 baseline passed 43 tests, with none skipped.
The unmodified upstream baseline compiled with Rust 1.95.0: 447 passed, one failed,
and three were ignored. The failure subtracted a duration from Windows Instant
past its representable origin; the experimental test advances a simulated clock.
The three ignored upstream tests require live Claude Desktop credentials/requests
or the desktop taskbar accessibility tree.

The final integrated suite passed 435 regular tests. All three environment-dependent
tests were then run explicitly and passed: 438 tested, zero failures, none left
unexecuted. The suite is different from upstream: tests for removed CLI/updater
paths were removed, and hardened behavior gained regression coverage.
`cargo fmt -- --check`, strict Clippy with the project's three existing allowances,
and `git diff --check` passed. The final review also fixed Studio hiding an imported
stale snapshot until a successful request; a regression checks both visible numbers
and the stale marker.

Desktop validation used the isolated release and its profile, while v1.7 remained
running. The first launch imported a snapshot once, created one Floating surface,
and received real Claude and Codex readings. The diagnostic log recorded one
initial poll in the controller; Studio consumed the shared cache.

Controlled restarts with only experimental settings changed confirmed:

- Two selected device paths create two copies at 144 and 96 DPI.
- Selecting an unavailable test device creates one temporary primary copy and
  preserves the unavailable selection and its offset. This simulates selection
  absence; it does not simulate Windows physically removing a screen.
- Restoring Floating creates one copy and retains all saved monitor settings.
- Taskbar collision handling independently ejects the affected primary copy.
- A custom theme without a taskbar root creates no managed copies and retains
  monitor selections and positions.

The final release is 7,396,864 bytes (7.40 MB), versus 923,648 bytes (0.92 MB) for
the installed v1.7.0. Its final launch again received both providers (including
Fable in the cache), created one Floating surface and recorded one initial poll.
This size comparison is not a CPU or memory benchmark.

After validation, hashes of the installed executable and v1 settings were
unchanged; the startup registration and original v1 process were unchanged too.
All three imported backup files matched the snapshots byte for byte. The new
Codex credits file contained a 64-character SHA-256 key and no raw account ID.
The initial validation left the experiment running with Studio and Floating.
Implementation commit: `5383c32`. Nothing was pushed, tagged or installed.

These are process/log/configuration checks on the real desktop, not a complete
visual acceptance test. Screenshot validation was curtailed because another
private application overlapped Studio. Dragging, menu interaction and visual
layout still need a manual pass. Physical monitor disconnection, a real Explorer
restart and Windows reboot also remain untested; they require a separately agreed
desktop test. Unit coverage of lifecycle and positioning is not a replacement.

Local evidence is under `target/upstream-v2-validation` in the original workspace;
it is excluded from Git and includes no copied provider credentials.

## Compact theme follow-up

The selected editable theme is [Hardened Compact · 3 rows](../src/themes/hardened-claude-three-rows.json).
It is a standalone theme file: import it through Theme Studio on a fresh profile.
The local experimental profile already uses it. This change needs no executable
replacement and does not modify the built-in Classic theme or its user copies.

With Claude, Codex and Fable present, the taskbar surface is 311 × 46 logical pixels,
versus 439 × 46 in the original experiment and 325 × 46 in the first compact preview.
Claude has 5h, 7d and F rows; Codex occupies the first two. The Codex text field is
68 pixels in the three-row layout, retaining room for 100%, reset time and `~`.
Without the third row it uses 74 pixels; the original credits field keeps 82.

Native renderer checks covered 100%, 125%, 150%, 175% and 200% scaling, English and
Russian reset suffixes, long percentages/stale markers, row overlap and the case
where Claude is disabled. A one-off geometry check was run with the normal suite:
436 passed; the three live environment tests also passed separately. Formatting,
strict Clippy and diff checks passed. The check and PNGs remain in local evidence
under `theme-variants`, rather than adding a permanent test for a theme-only edit.

Only the experiment was restarted to apply the theme and reopen Theme Studio.
Both selected monitor copies were created; monitor settings and positions matched
the pre-change snapshot, and the v1 settings hash remained unchanged.
