<!-- generated-by: gsd-doc-writer -->
# Architecture

## System overview

Claude Code Usage Monitor Hardened is a native, single-process Windows desktop
application. It reads usage credentials already created by supported provider
tools, polls provider-owned usage endpoints, and renders compact usage meters in
the Windows taskbar, a floating window, and the notification area. The program
uses an event-driven Win32 UI thread plus short-lived background threads for
network polling, update checks, and taskbar recovery.

## Component diagram

```text
main.rs
  |
  v
window.rs <------> native_interop.rs <------> Windows taskbars / Explorer
  |   |                     |
  |   +------> tray_icon.rs +------> Win32 windowing and registry APIs
  |   +------> theme.rs
  |   +------> localization/
  |
  +----------> poller.rs ----------> provider credential stores
  |                 |
  |                 +-------------> provider HTTPS endpoints
  |
  +----------> usage_cache.rs -----> %APPDATA% usage snapshot
  +----------> updater.rs ---------> GitHub Releases / optional WinGet
  +----------> diagnose.rs --------> opt-in temporary log
```

## Runtime data flow

1. `src/main.rs` detects the optional `--diagnose` flag and enters `window::run()`.
2. `src/window.rs` establishes the global single-instance mutex, loads the
   runtime settings and provider usage cache, creates the layered Win32
   window, tray icons, timers, and taskbar watchdog.
3. Timer events start `do_poll()` on a background thread. `src/poller.rs` reads only
   the enabled providers' existing credentials and makes synchronous HTTPS
   requests with a 30-second request timeout.
4. `PollOutcome` carries data and errors independently for Claude, Codex, and
   Antigravity. One successful provider therefore keeps a combined poll useful
   when another provider fails.
5. The polling thread merges successful responses into `AppState`, preserves
   prior values for transient failures, writes successful Claude/Codex snapshots
   through `src/usage_cache.rs`, and posts `WM_APP_USAGE_UPDATED` to the UI thread.
6. The UI thread formats countdowns and stale markers, redraws the layered
   window, updates tray icons, and schedules either the normal interval or
   exponential retries beginning at 30 seconds.

## Provider polling

`src/poller.rs` treats providers as independent inputs to `AppUsageData`:

- Claude: reads Windows Claude credentials first, then configured WSL
  distributions. It prefers the OAuth usage endpoint and uses a minimal Messages
  request as a fallback for general 5-hour and 7-day rate-limit data.
- Codex: reads the provider auth file from `CODEX_HOME` or the default `.codex` directory and
  requests the ChatGPT usage endpoint.
- Antigravity: reads the `gemini:antigravity` Windows generic credential and
  probes the supported Cloud Code endpoints until one returns quota data.

Credential errors pause the affected authentication flow and watch for a local
credential change. Network failures retain known values and use retry backoff.
The monitor does not refresh credentials itself and does not launch provider
agents.

## Usage cache and stale-state contract

`src/usage_cache.rs` owns schema version 2 of the runtime usage cache:

- Claude and Codex snapshots have separate update timestamps.
- Fable is stored as a model-scoped Claude weekly limit.
- The file contains usage percentages and reset times, not tokens or account
  credentials.
- Writes use a temporary file, `sync_all`, and `MoveFileExW` with replace and
  write-through flags.
- Schema v1 Claude-only cache files remain readable and are migrated on the next
  successful save.
- Corrupt, empty, out-of-range, or unsupported cache data is rejected.

Cached or temporarily preserved data is rendered with `~`. `...` means no
snapshot exists for that provider. A manual refresh must not erase a known
snapshot while its replacement request is pending.

## Window and taskbar lifecycle

`src/window.rs` owns application state and the Win32 message procedure. The widget
starts as a layered popup and can be:

- reparented into a primary or secondary taskbar;
- detached into a top-level floating window;
- hidden while tray icons remain active.

`src/native_interop.rs` discovers `Shell_TrayWnd` and
`Shell_SecondaryTrayWnd`, adjusts popup/child styles around `SetParent`, and
restores layered rendering after Explorer or placement changes. A watchdog
checks taskbar availability every two seconds. Position, monitor selection, and
placement are persisted in the runtime settings file.

The widget does not register reserved taskbar space. Windows 11 centered icons
can therefore overlap a taskbar-positioned widget; collision avoidance remains
an explicit backlog item.

## Key abstractions

| Abstraction | Location | Responsibility |
|---|---|---|
| `AppState` | `src/window.rs` | Live UI, polling, placement, retry, and provider state. |
| `WidgetPlacement` | `src/window.rs` | Taskbar or floating window mode; tray-only is represented by widget visibility. |
| `PollOutcome` | `src/poller.rs` | Provider-isolated data and error result. |
| `PollError` | `src/poller.rs` | Authentication, credential, expiry, and transient request failures. |
| `AppUsageData` | `src/models.rs` | Optional usage snapshot for each provider. |
| `UsageData` | `src/models.rs` | Session, weekly, and model-scoped weekly limits. |
| `CachedAppUsage` | `src/usage_cache.rs` | Persisted Claude and Codex snapshots. |
| `TaskbarWindow` | `src/native_interop.rs` | A discovered taskbar handle and screen rectangle. |
| `InstallChannel` | `src/updater.rs` | Portable versus WinGet update behavior. |

## Repository structure

```text
src/
  main.rs              process entry point
  window.rs            Win32 lifecycle, state, layout, menus, timers
  poller.rs            credentials, provider requests, response parsing
  usage_cache.rs       validated, atomic usage snapshot persistence
  models.rs            provider-neutral usage data types
  native_interop.rs    taskbar and window-style operations
  tray_icon.rs         dynamic notification-area icons
  updater.rs           release checks and WinGet-only update path
  diagnose.rs          opt-in diagnostic logging
  theme.rs             Windows light/dark theme lookup
  localization/        localized UI strings
.github/
  workflows/release.yml  tag-triggered release build
  release-notes/         versioned bilingual release bodies
docs/                    developer documentation
```

The project deliberately keeps these modules in one crate. Its state and APIs
are Windows-specific and tightly coupled to a single native UI process, so a
multi-crate split would add indirection without creating a useful isolation
boundary.
