# 2.23.6-hardened.2 — validation record

Scope: ship the existing Hardened Compact three-row theme inside the EXE and make
it selectable without JSON import. Keep the current selection and imported edits.
Baseline/rollback source: 2ca17226f5a1260273aa1fbb895d2ed6a67200da (.1 tag).
Working branch: agent/upstream-v2.23-hardened-integration.

## Implementation

The build minifies and embeds hardened-claude-three-rows.json. The existing editable
theme installer now uses a separate marker for each bundled theme, retaining the
original Minecraft marker. Existing files are not overwritten. Once installed,
an explicitly deleted bundled theme stays deleted on subsequent starts. A Compact
theme deleted before .2's first run may be added again: old versions did not record
that deletion. The active theme/default remains unchanged.

No provider, credentials, polling, startup, routing or updater code changed.
Dependency versions/checksums are unchanged; only the root package version changed
in Cargo.lock. The user-facing instructions are in USER_GUIDE.md and release notes.

## Local checks

Rust 1.95.0; offline and locked. 587 tests passed, 0 failed, 4 live tests ignored.
Formatting and strict all-target Clippy passed. Debug build passed.
Test settings use thread-local temporary profiles; new installer tests also use
tempfile directories. No provider credentials or live provider requests are used.
The validation wrapper checks OBS before each stage and every five seconds.

| Expected | Method | Observed | Evidence |
| --- | --- | --- | --- |
| Compact selectable without import | Fresh-profile Studio unit test and real debug Studio | Present in dropdown; selecting changes preview and persisted active path | studio_app::tests::bundled_compact_is_selectable_on_fresh_profile_and_preserves_existing_choice; local studio-compact-selected.png |
| Existing theme/settings stay selected | Studio restart regression test | Existing Classic selection and settings bytes unchanged | Same test |
| .1 profile receives the new theme | Existing Minecraft marker fixture | Compact installed independently | bundled_hardened_compact_upgrades_old_library_without_overwriting_or_resurrecting |
| Imported edits and later deletion survive | Installer fixture | Existing bytes unchanged; deletion after installation stays deleted | Same test |
| Three Claude rows fit the taskbar height | Synthetic Fable geometry test | Rows do not overlap and fit 46px | bundled_hardened_compact_keeps_three_claude_rows_in_taskbar_height |
| Previous behavior remains valid | Full offline suite | 587 pass; Minecraft lifecycle tests still pass | test.out.log, clippy.err.log, fmt-check.err.log |

UI verification used a separate empty APPDATA under
target/release-hardened2-20261112/ui-profile and --studio --owner 0 --theme-studio.
No monitor poll controller was started. Theme appeared without importing a file,
was selected in the real dropdown and persisted. No Fable quota was supplied to
that empty-profile preview, so its screenshot shows two rows; the three-row
geometry was separately checked with synthetic data. Test Studio closed normally.
The working installed monitor/profile were not replaced by this check.

Local logs/screenshots reside under target/release-hardened2-20261112 (not committed).
The first wrapper attempt used Windows PowerShell 5 and returned an empty ExitCode
after formatting; rerunning under PowerShell 7 completed all checks successfully.

## Independent review

Read-only reviewer /root/review_compact_release checked embedding, individual
installation markers, existing imports, current-theme loading, editing/deletion,
test isolation and synchronized versions. No confirmed defects found. The reviewer
explicitly noted the pre-.2 deletion limitation described above. No tests or UI
actions were performed by the reviewer; this is source review, not visual acceptance.

## Release gates and remaining checks

Tag CI must pass audit, deny, tests, Clippy and release build before publication.
Assets/checksum and the final release status are checked separately at publication.
Full multi-monitor/DPI/hotplug/Explorer-restart/soak acceptance remains incomplete.
Side-monitor top clipping during Computer Use remains unexplained and was not fixed
by this change. The owner still decides whether the layout suits everyday use.
