<!-- generated-by: gsd-doc-writer -->
# Configuration

## Configuration locations

The application has no `.env` file and does not require a manually provisioned
API key. Runtime configuration and cached usage live under the current Windows
user profile.

| Data | Location | Contains secrets |
|---|---|---|
| UI settings | `%APPDATA%\ClaudeCodeUsageMonitor\settings.json` | No |
| Usage cache | `%APPDATA%\ClaudeCodeUsageMonitor\usage-cache.json` | No |
| Diagnostic log | `%TEMP%\claude-code-usage-monitor.log` | No tokens by design; may contain local paths and provider error context |
| Autostart | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `ClaudeCodeUsageMonitor` | No |

The settings writer creates the application directory when required. Invalid
or unreadable settings fall back to defaults. Invalid cache data is ignored
until a successful provider response repairs it.

## Settings file format

The settings file is a user-scoped JSON object. Missing fields use the defaults
below, which keeps files written by older versions compatible.

```json
{
  "tray_offset": 0,
  "taskbar_index": 0,
  "widget_placement": "taskbar",
  "floating_x": null,
  "floating_y": null,
  "poll_interval_ms": 900000,
  "language": null,
  "last_update_check_unix": null,
  "widget_visible": true,
  "show_claude_code": true,
  "show_codex": false,
  "show_antigravity": false
}
```

`floating_x`, `floating_y`, `language`, and `last_update_check_unix` are omitted
when unset. If all providers are disabled in the file, Claude is enabled during
load so the application never starts with an empty provider set.

| Setting | Default | Meaning |
|---|---:|---|
| `tray_offset` | `0` | Horizontal taskbar placement offset. |
| `taskbar_index` | `0` | Selected taskbar in the sorted taskbar list. |
| `widget_placement` | `taskbar` | `taskbar` or `floating`; tray-only uses `widget_visible: false`. |
| `floating_x`, `floating_y` | unset | Saved top-level window coordinates. |
| `poll_interval_ms` | `900000` | Normal polling interval; UI choices are 1, 5, 15, or 60 minutes. |
| `language` | unset | Explicit language code; unset follows the Windows UI language. |
| `last_update_check_unix` | unset | Last successful release-check timestamp. |
| `widget_visible` | `true` | Whether the taskbar/floating widget is visible. |
| `show_claude_code` | `true` | Enable Claude polling and display. |
| `show_codex` | `false` | Enable Codex polling and display. |
| `show_antigravity` | `false` | Enable Antigravity polling and display. |

These fields are implementation-owned. Edit them only while the monitor is not
running, because a later menu or placement change can overwrite the file.

## Usage cache format

The usage cache currently uses schema version 2. It stores separate optional
`claude` and `codex` objects, each with `updated_at_unix` and a provider-neutral
usage payload. Claude's payload may contain model-scoped weekly limits such as
Fable.

The cache intentionally excludes access tokens, refresh tokens, account IDs,
session cookies, and credential source paths. Percentages must be finite and in
the inclusive range 0 through 100. The loader also accepts schema version 1,
which contains one Claude snapshot.

## Credential discovery

Credentials remain owned by the provider applications. The monitor reads them
at poll time and does not copy them into its own settings or cache.

| Provider | Discovery order |
|---|---|
| Claude | The Windows Claude credential file under `%USERPROFILE%\.claude`, then the equivalent file in each WSL home returned by `wsl.exe -l -q`. |
| Codex | The Codex auth file under `%CODEX_HOME%` when set; otherwise under `%USERPROFILE%\.codex`. |
| Antigravity | Windows Credential Manager generic credential `gemini:antigravity`. |

When a Claude credential has an expired `expiresAt`, the monitor tries the next
configured credential source and otherwise waits for the user to refresh or log
in through Claude Code. Codex and Antigravity follow the same manual-login
principle: this application does not invoke their login flows.

## Environment variables

| Variable | Required | Default | Use |
|---|---|---|---|
| `APPDATA` | No | Current directory fallback | Settings and usage-cache root. |
| `CODEX_HOME` | No | `%USERPROFILE%\.codex` | Overrides the Codex authentication directory. |
| `LOCALAPPDATA` | No | none | Detects whether the EXE is under a user WinGet package root. |
| `ProgramFiles` | No | `C:\Program Files` | Detects a machine WinGet package root. |
| `ProgramFiles(x86)` | No | `C:\Program Files (x86)` | Detects a 32-bit WinGet package root. |
| `CCUM_RELAUNCH` | Internal | unset | Marks a controlled relaunch after taskbar recovery. |
| `CCUM_LAST_RELAUNCH_UNIX` | Internal | unset | Throttles repeated relaunch attempts. |

`ureq` is compiled with proxy-from-environment support, so standard process
proxy settings may affect HTTPS routing. The application itself does not expose
a proxy setting.

## Polling and retries

The normal interval is selected in the tray menu. Transient provider failures
use exponential retry delays beginning at 30 seconds and capped by the selected
normal interval. A detailed Claude retry is also scheduled when only the less
detailed Messages fallback is available.

Authentication failures are distinct from network failures. They pause the
affected flow, show an authentication state, and watch the local credential
source every two seconds. Known network-failure values remain visible with `~`.

## Startup configuration

The **Start with Windows** menu item writes a quoted absolute path for the
currently running EXE to:

```text
HKCU\Software\Microsoft\Windows\CurrentVersion\Run
  ClaudeCodeUsageMonitor = "C:\path\to\claude-code-usage-monitor.exe"
```

Startup is considered enabled only when that registry value matches the current
EXE path. Therefore, testing a build from `target\release` can make the menu look
disabled even when another installed copy was previously registered. For a
stable setup, register and run the copy under the chosen installation directory.

## Diagnostics

Launch the binary with `--diagnose` to truncate and enable the temporary log:

```powershell
.\claude-code-usage-monitor.exe --diagnose
```

Without that flag, diagnostic calls are no-ops. The log is intended for
short-lived troubleshooting and is not rotated.
