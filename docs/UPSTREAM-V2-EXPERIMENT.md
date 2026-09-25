# Hardened upstream v2 experiment

## Current status — 2026-09-25

The session is complete. `2.15.14-hardened.3` is installed for daily testing, with
its own Windows startup enabled and the old v1 startup disabled. The v1 executable
and profile are preserved. The active compact theme is 296 × 46 logical pixels;
Claude uses three rows, Codex uses two, and automatic taskbar ejection is disabled.
All three selected taskbar copies retain their independent positions.

The latest implementation is commit `be3f706`. The experimental branch is pushed
to the hardened repository. Stable `main` and `agent/multi-monitor-widgets` remain
at `bea4ab9` (v1.7.0); the experiment has not been merged into `main`. No tag or
GitHub Release was published.

Latest validation passed **441 regular tests plus all three explicitly run live
tests (444 total)**, formatting, strict Clippy, diff checks and the release build.
The installed executable matches the build. Actual startup after signing in to
Windows and the remaining manual checks below are still pending. The sections
below retain the validation history for each earlier stage.

## Identity and scope

- Upstream tag: v2.15.14, commit `814ff7339f70ac8234f4a8177dd2b061dc2235fa`.
- Hardened behavior reference: v1.7.0, commit `bea4ab9`.
- Branch: `agent/upstream-v2-hardened-experiment`.
- Application version: `2.15.14-hardened.3`.
- Keep the upstream native renderer, Studio, themes, account profiles and providers.
- Apply selected-monitor copies to the first authored taskbar root. Additional
  roots retain their authored behavior. With no taskbar root, copies are unavailable
  and saved monitor selection/positions remain intact.

The v1.7 executable and settings are retained. The experiment uses its own
application directory, startup value, named mutexes, window
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

Daily use can install this build into
`%LOCALAPPDATA%\Programs\ClaudeCodeUsageMonitorHardenedUpstream2` and enable its own
startup entry. Keep the v1 executable separate; avoid enabling both versions at
login. See the rollback steps below before switching back. No public release is
implied.

## Deliberate differences from upstream

- Claude, Codex and Grok CLIs are never launched, including version probes.
  Expired authorization is renewed by the user in the provider's application.
- Antigravity does not exchange refresh tokens. Cursor reads its SQLite store
  directly without copying a credential-bearing database to a temporary file.
- WSL subprocesses are restricted to bounded credential discovery/read/stat;
  non-login shells avoid loading login profiles.
- Releases are informational, scoped to the hardened repository. Executable
  download/replacement, updater helper and WinGet execution are removed.
- Startup is opt-in and uses a separate v2 value. The app never disables v1's
  registration automatically; switching versions is an explicit installation step.
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

## Initial port validation — hardened.1

Before porting, the v1.7 baseline passed 43 tests, with none skipped.
The unmodified upstream baseline compiled with Rust 1.95.0: 447 passed, one failed,
and three were ignored. The failure subtracted a duration from Windows Instant
past its representable origin; the experimental test advances a simulated clock.
The three ignored upstream tests require live Claude Desktop credentials/requests
or the desktop taskbar accessibility tree.

The initial port's integrated suite passed 435 regular tests. All three environment-dependent
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

The initial hardened.1 release is 7,396,864 bytes (7.40 MB), versus 923,648 bytes (0.92 MB) for
the installed v1.7.0. Its final launch again received both providers (including
Fable in the cache), created one Floating surface and recorded one initial poll.
This size comparison is not a CPU or memory benchmark.

After validation, hashes of the installed executable and v1 settings were
unchanged; the startup registration and original v1 process were unchanged too.
All three imported backup files matched the snapshots byte for byte. The new
Codex credits file contained a 64-character SHA-256 key and no raw account ID.
The initial validation left the experiment running with Studio and Floating.
Implementation commit: `5383c32`. At that stage, nothing had been pushed, tagged or
installed; later publication and daily installation are recorded above and below.

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

With Claude, Codex and Fable present, the taskbar surface is 296 × 46 logical pixels,
versus 439 × 46 in the original experiment and 325 × 46 in the first compact preview.
Claude has 5h, 7d and F rows; Codex occupies the first two. Both text fields use
68 pixels in the three-row layout, retaining room for 100%, reset time and `~`.
Without the third row they use 74 pixels; credit balances keep the original 82.
The final spacing pass reduced Claude's unused text reserve and the provider gap
from 3 to 2 pixels, saving another 15 pixels from the previously applied 311-wide
theme without changing the font, bars or row height.

Native renderer checks covered 100%, 125%, 150%, 175% and 200% scaling, English and
Russian reset suffixes, long percentages/stale markers, row overlap and the case
where Claude is disabled. A one-off geometry check was run with the normal suite:
436 passed; the three live environment tests also passed separately. Formatting,
strict Clippy and diff checks passed. The check and PNGs remain in local evidence
under `theme-variants`, rather than adding a permanent test for a theme-only edit.

Only the experiment was restarted to apply the theme and reopen Theme Studio.
Both selected monitor copies were created; monitor settings and positions matched
the pre-change snapshot, and the v1 settings hash remained unchanged.

The final spacing pass passed two focused native renderer/geometry checks,
including text fitting at 100–200% scaling. The theme change alone retained the
collision policy: this upstream version had no setting to disable automatic
taskbar ejection. A narrower theme alone does not guarantee that a saved position
is free of taskbar buttons.

## Optional automatic taskbar movement

Version `2.15.14-hardened.2` adds **Settings → Display → Auto-move above taskbar**
(**Настройки → Отображение → Автоперенос над панелью**). It defaults to enabled
for existing and new profiles; the user's experimental profile has it disabled.
The persisted field is `taskbar_auto_eject`.

Disabling it keeps all selected copies inside their taskbars even when buttons
overlap, and immediately re-docks an already ejected copy at its saved position.
Buttons may be covered when space is tight. Manual Floating remains independent;
monitor selection, DIP offsets and temporary fallback rules are unchanged.
Studio saves only the edited policy field. Runtime state is separate from the
persistence baseline so a concurrent background save cannot restore an old value.

Validation passed 439 regular tests and all three separately executed environment
tests, plus formatting, strict Clippy and diff checks. New regressions cover
default/round-trip behavior, crowded and unavailable occupancy samples, restoration,
and stale Studio/monitor saves preserving the policy and monitor positions.

A temporary isolated profile placed the primary copy over occupied taskbar space,
which produced a real ejected window. Changing the setting while the process ran
returned that same HWND to `Shell_TrayWnd`; all three copies were taskbar children,
within the respective panel rectangles, with their saved positions unchanged.
The real profile was then started with the setting disabled, and all three copies
were again verified inside their taskbars. Its theme, monitor positions, and the
stable v1 executable/settings hashes were preserved. This checks native parents
and geometry, without capturing other applications on the desktop.

The updated release is 7,398,912 bytes. Local evidence and the previous executable
backup are under `target/upstream-v2-validation/auto-eject-runtime`; normal and live
test logs are `auto-eject-tests.log` and `auto-eject-live-tests.log`. This step did
not publish a release, restart Explorer or change Windows settings.

## Daily installation and login startup

Version `2.15.14-hardened.3` enables **Settings → General → Start with Windows**
and the corresponding context-menu action. It registers only
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ClaudeCodeUsageMonitorHardenedUpstream2`,
with the current executable's full path in quotes. Disabling it removes only that
value. Registry failures are displayed; the switch reads back the actual state.
Enabling startup is always an explicit user action.

The user requested replacing v1 startup with v2. The build is installed at
`%LOCALAPPDATA%\Programs\ClaudeCodeUsageMonitorHardenedUpstream2\claude-code-usage-monitor.exe`.
The original `ClaudeCodeUsageMonitor` Run value was backed up and removed, while
the v1 executable and settings remain intact. The existing v2 AppData profile,
compact theme, monitor positions and disabled auto-eject policy are reused.
The registration starts the monitor alone; Studio opens when requested.

`ureq` now enables `win-system-proxy`. When proxy environment variables are absent,
it reads the user's existing Windows proxy configuration; the app does not change
that configuration. This removes the previous launcher's `HTTPS_PROXY` requirement
for the tested single-server Windows proxy. Other environment overrides retain
ureq's normal precedence. This does not add PAC or per-protocol-list support beyond
what the library provides. Restart the app after changing proxy settings, since
the provider agent is cached.

Validation: 441 regular tests and all three live environment tests passed, including
real usage retrieval without proxy environment overrides. Formatting and strict
Clippy passed. Registry tests use disposable isolated keys and verify quoted Unicode
paths, value types, idempotent removal and preservation of unrelated startup values.
The installed executable's actual startup handler was toggled on/off/on; the final
Run value matches its quoted installed path. Both providers returned fresh data,
all three native windows remained inside their taskbars, and positions were unchanged.
Windows sign-out/reboot was not performed. Local evidence, previous executable and
startup/settings snapshots are in `target/upstream-v2-validation/daily-install`.

## Remaining acceptance checks and rollback

Daily use is the next step; there is no unfinished implementation or active blocker
from this session. These checks are not authorization to interrupt the desktop:

- Confirm that a normal Windows sign-in starts only the installed v2 monitor,
  with the saved theme, positions and startup setting intact.
- Complete a manual Studio, menu, dragging, Floating/Taskbar and hide/show pass.
- Separately agree and test physical monitor disconnection/reconnection, display
  layout/DPI changes and recovery after a real Explorer restart. The existing
  synthetic and native-window checks do not replace those hardware/shell tests.

If a rollback is requested, disable v2's **Start with Windows**, exit its monitor
and Studio, then launch the preserved
`%LOCALAPPDATA%\Programs\ClaudeCodeUsageMonitor\claude-code-usage-monitor.exe`
and enable startup in v1. Keep the profiles separate; do not copy v2 settings into
v1 or repeat the completed v1 import. No rollback was performed in this session.

Promoting v2 to `main` is a later user decision after daily testing. The branches
have diverged, so adoption needs integration and verification before merging;
force-replacing `main` is not the planned path. Release packaging, migration and
any tag/GitHub Release also require a separate decision.
