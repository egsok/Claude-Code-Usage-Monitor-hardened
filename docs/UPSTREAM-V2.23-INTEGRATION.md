# Upstream v2.23.6 integration

Authorized on 2026-11-12 after publishing and installing hardened.5.
Base/rollback: f1077b2, local tag before-upstream-v2.23.6-20261112.
Target: a2b2b31 (upstream v2.23.6). Candidate: 2.23.6-hardened.1.
Separate worktree/branch; installed .5 is not replaced by this integration.

## Merge policy

- Claude keeps GET usage only; Messages probes, CLI auth refresh and credential
  writes are excluded. Compatible desktop discovery and quota fixes are retained.
- Full Retry-After and completion-based automatic interval are retained; no reset
  fast polling, short partial-provider retries or manual cooldown bypass.
- The hidden controller and managed per-monitor windows keep their ownership and
  device-path identity. Upstream DISPLAYn positions do not replace saved IDs.
- taskbar_auto_eject=false remains distinct from the new lock_taskbar setting.
- Upstream GUI 0.37, WARP fallback, optional Copilot and compatible fixes are merged.
- Settings are saved as atomic field transactions, including new UI options.
- Informational-only updates remain; upstream download/apply/WinGet code is excluded.
- Existing decimal-day countdown and compact Fable layout remain; Copilot UI is added.
  Generic scoped quotas remain available to custom themes without guessing a model.
- Hardened application/profile/startup names and Windows system-proxy support remain.

## Validation

On 2026-11-12, Rust 1.95.0 offline/locked checks passed: 584 tests, Clippy,
formatting and a debug executable build. Four tests requiring real Desktop
credentials, public HTTPS or the real taskbar accessibility tree were ignored.
Studio opened the Hardened Compact 3 rows theme from a separate synthetic profile
and closed normally. This proves basic GUI startup/rendering, not tray, DPI,
multiple-monitor, WARP fallback or long-running acceptance.

No provider credentials were copied or changed. No Explorer restart, monitor
disconnect, network reconfiguration or real-provider stress tests were performed.
Release build is pending the dependency security gate: local cargo-audit and
cargo-deny are unavailable; CI must check this integration's lockfile before a
release build. The successful .5 security run does not validate this new graph.

Independent static review found and verified fixes for update-check completion
targeting a transient surface instead of the controller, and managed copies
ignoring lock_taskbar during automatic ejection. A local injected-result native
test now checks update completion without contacting GitHub. Review did not
establish hardware behavior; nonmanaged taskbar collision recovery remains
unverified on a real desktop.

## Conflict decisions

| Files | Resolution and reason |
| --- | --- |
| Cargo.toml, Cargo.lock | Upstream GUI/build dependencies; hardened identity, Windows system proxy, zip 9.5.1 and rustix 1.1.6 retained. |
| src/poller/claude.rs, src/poller/accounts.rs | Passive usage endpoint and source-pinned accounts retained; compatible Desktop discovery and absent-quota handling integrated. |
| src/poller/retry_after.rs | Hardened full server delay retained, including manual refresh. |
| src/poller/antigravity.rs, cursor.rs, grok.rs, opencode.rs | Upstream provider changes combined with stale-data/error handling; no provider recovery agent execution. |
| src/models.rs | Upstream quota/pool fields combined with recursive stale marking and scoped quota metadata. |
| src/window.rs, message_loop.rs, window_context_menu.rs, placement_regression_tests.rs | Upstream UI/lifecycle changes adapted to the hardened hidden controller and per-monitor copies; automatic polling gate retained. |
| src/app_settings.rs | New settings added without replacing atomic transactions, hardened profile identity or saved monitor positions. |
| src/studio_app.rs, studio_core.rs, studio_diagnostics.rs, tests.rs | New GUI API and options combined with field-level settings transactions and hardened controller discovery. |
| src/dashboard.rs | Upstream renderer fallback retained with hardened application/window identity. |
| src/localization/locales/*.toml | New upstream font options and existing hardened strings both retained. |
| src/theme_engine/tests.rs, theme_expression.rs, theme_limits.rs, src/themes/classic-usage-widget.json | Preserve readable countdowns and compact Fable layout; integrate absent-session visibility, Copilot and hover summaries. |
| src/theme_package.rs | Upstream name validation combined with zip 9.5.1 compatibility and hardened compressed-package tests. |
| src/updater.rs, src/updater/download.rs, src/updater/download/tests.rs, src/updater/tests.rs, docs/updater.md | Informational-only updater retained; upstream downloader/install tests and implementation excluded. |
| CHANGELOG.md | Preserve fork history and add upstream history with an explicitly unreleased integration candidate. |

The initial test run exposed ten merge-related test/theme mismatches; all were
resolved. Fable width assertions now distinguish Classic (+114 pixels) from the
upstream Compact Fluent Quad (+96), while other surfaces and headlines retain
their exact invariance checks. Unknown reset remains explicit as `--`.
