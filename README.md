# Claude Code Usage Monitor — Hardened v2 Experiment

This branch integrates upstream v2.23.6 with the hardened fork's passive
polling, atomic settings and independent monitor copies. Prerelease version:
`2.23.6-hardened.1`, installed locally for daily evaluation.
Full taskbar/multi-monitor/soak acceptance remains pending; see the release notes.

**Experimental release:** [2.23.6-hardened.1](https://github.com/egsok/Claude-Code-Usage-Monitor-hardened/releases/tag/v2.23.6-hardened.1).
**Release notes and known limitations:** [details](.github/release-notes/v2.23.6-hardened.1.md).
**Integration:** [decisions and validation](docs/UPSTREAM-V2.23-INTEGRATION.md).
This candidate keeps the separate v2 profile/startup identity. It does not launch
provider agents, refresh credentials, or download/replace its executable.

Claude statistics use only `GET /api/oauth/usage`: no Messages API probes,
including when reset fields are absent. Automatic cycles wait at least the selected
interval after the previous cycle completes; countdown rendering stays local.
Manual refresh shares the in-flight guard and respects server `Retry-After`.

![Windows](https://img.shields.io/badge/platform-Windows-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A lightweight, open-source Windows taskbar widget for monitoring Claude Code usage limits and reset times. It can also display usage for Codex, Google Antigravity, OpenCode Go, Cursor, Grok Build, and GitHub Copilot.

See the [user guide](USER_GUIDE.md) for theme customisation and everyday settings,
or the [changelog](CHANGELOG.md) for version history and notable changes.

![Claude Code Usage Monitor running in the Windows taskbar](.github/animation.gif)

## Features

- Displays current usage and time remaining until each limit resets
- Counts usage up from zero or down from the full allowance, whichever you prefer
- Supports Claude Code, Codex, Google Antigravity, OpenCode Go, Cursor, Grok Build, and GitHub Copilot
- Supports multiple accounts for Claude Code and Codex
- Lives in the Windows taskbar with quick controls in the system tray
- Supports multiple monitors and Windows startup
- Includes configurable refresh intervals, providers, languages, and updates
- Provides built-in themes and a visual Theme Studio for custom layouts
- Collects no analytics or telemetry

## Requirements

- Windows 10 or Windows 11
- At least one supported provider installed and signed in

Claude Code credentials can be detected from the CLI, Claude desktop app, or WSL. Other providers are optional and can be enabled independently from the dashboard.

## Installation

Download the EXE and checksum from the prerelease linked above.
Compare `Get-FileHash .\claude-code-usage-monitor.exe -Algorithm SHA256` with the checksum file.

For a new v2 installation, use a stable location such as:

```text
%LOCALAPPDATA%\Programs\ClaudeCodeUsageMonitorHardenedUpstream2\claude-code-usage-monitor.exe
```

For an existing v2 installation, close the monitor, keep a copy of the previous EXE,
and replace it **at the same path and filename**. This preserves path-based proxy/TUN
rules and the existing startup entry. Start the new EXE normally. Do not delete the
profile or provider credentials. The application does not change network routing.

V2 uses `%APPDATA%\ClaudeCodeUsageMonitorHardenedUpstream2`, separate from v1.
It does not automatically import a v1 profile. If running v1, explicitly choose
which version should start with Windows; do not enable both accidentally.
The compact three-row theme is available at
[src/themes/hardened-claude-three-rows.json](src/themes/hardened-claude-three-rows.json)
for manual import into Studio; a fresh profile does not include the author's settings.

There is no WinGet release for v2. See [update policy](docs/updater.md).

## Usage

Start the monitor:

```powershell
claude-code-usage-monitor
```

Open the settings dashboard directly:

```powershell
claude-code-usage-monitor --studio
```

Use the dashboard to select providers, change the refresh interval, choose a display, enable startup, or customize the widget. **Settings > Display > Usage direction** switches the default theme and other themes that support this setting between showing what has been used and what is left, with Used as the default. Selecting Remaining makes a fresh limit read 100% and drain as you work.

Theme authors can opt in with `.display` bindings, including `{claude.session.display:usage_line}` and `{claude.session.display:usage_badge}`. Existing `.percentage`, `.remaining`, and unsuffixed usage summaries keep their meaning; warning thresholds should continue to use `.percentage`.

In the default theme, left-click a provider tray icon to show or hide the widget and right-click it to open the menu.

## Provider setup

| Provider | Setup |
| --- | --- |
| Claude Code | Sign in with the Claude Code CLI or desktop app. Windows and WSL credentials are detected automatically. |
| Codex | Install and sign in to the Codex CLI, then enable Codex in **Providers**. |
| Google Antigravity | Sign in to Antigravity, then enable it in **Providers**. |
| OpenCode Go | Connect an OpenCode Go account, configure the credentials described below, then enable OpenCode in **Providers**. |
| Cursor | Sign in to Cursor, then enable it in **Providers**. The local session is detected automatically. |
| Grok Build | Run `grok login` in the Grok Build CLI, then enable Grok in **Providers**. The signed-in session is detected automatically. |
| GitHub Copilot | Sign in with the Copilot CLI (`copilot`, then `/login`) or the GitHub CLI (`gh auth login`), then enable Copilot in **Providers**. The stored login is detected automatically. |

For OpenCode Go, set `OPENCODE_GO_WORKSPACE_ID` and `OPENCODE_GO_AUTH_COOKIE`, or create `%APPDATA%\opencode-go\config.json`:

```json
{
  "workspaceId": "wrk_01...",
  "authCookie": "__Host-console_session=your-session-cookie-value"
}
```

The workspace ID is part of the OpenCode Go console URL: `https://opencode.ai/console/<workspaceId>/go`. Copy the `__Host-console_session` cookie from an authenticated `opencode.ai` browser session, including its name as shown above. A full Cookie header containing `__Host-console_session` or the legacy `auth` cookie is also accepted unchanged; no empty `auth=;` prefix is needed. Bare legacy `auth` cookie values remain supported. These formats work for both `authCookie` and `OPENCODE_GO_AUTH_COOKIE`. Set `OPENCODE_GO_CONFIG_FILE` to use a different config path. The monitor reads usage from the console JSON API using this workspace ID and cookie.

For Cursor, `CURSOR_SESSION_TOKEN` can override the automatically detected local session.

Grok Build usage comes from the session the CLI stores in `%USERPROFILE%\.grok\auth.json`, read through the same billing endpoint as the CLI's own `/usage` panel. Set `GROK_HOME` if the CLI keeps its home directory elsewhere. Only xAI sign-in entries are used; corporate identity-provider tokens and stored API keys are excluded. A bare `XAI_API_KEY` is not enough: the shared weekly allowance is only readable with a signed-in session. Grok reports one pool per billing period rather than a five-hour window, so the monitor shows it on the long-window row alongside the other providers, leaving the short-window row empty. On-demand spending replaces the pool on that row once any is used, as it already does for Claude Code and Codex.

GitHub Copilot usage comes from `api.github.com/copilot_internal/user`, the endpoint Copilot's own editors and CLI read. The monitor uses the first GitHub login that endpoint accepts, in this order: the `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, or `GITHUB_TOKEN` environment variable, the Copilot CLI login, then the GitHub CLI login, both read from Windows Credential Manager. Copilot spends one premium-request pool per calendar month, so the monitor shows it on the long-window row and leaves the short-window row empty. Metered chat and completion allowances, as on Copilot Free, are available to themes through `copilot.limits.chat` and `copilot.limits.completions`.

## Data and privacy

The monitor reads local sign-in credentials for enabled providers and sends usage requests directly to their official services. It has no backend service, collects no telemetry, and does not upload credentials or project files.

Credentials are read without modifying the provider files that contain them. Rejected credentials must be renewed in the provider application by the user; the monitor never launches an agent CLI or refreshes OAuth tokens itself. OpenCode Go credentials saved in a JSON configuration file are plain text and should be protected like a browser session cookie.

## Troubleshooting

Hover over the tray icon for the latest failure reason, or check the account
status under **Settings > Providers > Accounts**. Missing credentials, expired
or rejected logins, network failures, HTTP errors, and unexpected usage responses
have distinct messages. Sign in again using the Claude desktop app or the CLI
that owns the affected account, then refresh the monitor.

Open **Diagnostics** in the dashboard and enable recording to inspect or copy
polling errors. Logging is optional; enable it before reproducing the problem.

Run diagnostics with:

```powershell
claude-code-usage-monitor --diagnose
```

The diagnostic log is written to `%TEMP%\claude-code-usage-monitor-hardened-upstream2.log`. Application settings are stored in `%APPDATA%\ClaudeCodeUsageMonitorHardenedUpstream2\settings.json`.

If the app unexpectedly closes because of a Rust panic, it automatically appends
the panic message, source location, and thread details to the same log, even when
diagnostic recording is off. Copy the log before starting a new `--diagnose`
session, which clears it. Review and redact credentials, account identifiers,
and personal information before sharing relevant excerpts in a crash report.
Report suspected vulnerabilities privately using [SECURITY.md](SECURITY.md).

## Build from source

Install [Rust via rustup](https://www.rust-lang.org/tools/install), then run:

```powershell
cargo +1.95.0 build --release --locked
```

The repository pins Rust 1.95.0 in `rust-toolchain.toml`; rustup automatically
selects this version. CI uses the same version. When upgrading Rust, update
`rust-toolchain.toml`, `.github/workflows/release.yml`, and the `rust-version`
in `Cargo.toml` together.

The executable will be created at `target\release\claude-code-usage-monitor.exe`.

Windows MSVC builds statically link the C runtime through `.cargo/config.toml`,
including release builds in CI. The executable does not require a separate
Microsoft Visual C++ Redistributable installation; it still uses built-in Windows
system libraries.

See [dependency security](docs/dependency-security.md) for automated dependency
updates, CI security checks, and the commands to run those checks locally.

## Contributing and community

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, reporting bugs,
and submitting pull requests, and follow our [Code of Conduct](CODE_OF_CONDUCT.md).
For suspected vulnerabilities or credential exposure, use the private reporting
instructions in our [security policy](SECURITY.md).

## License

Licensed under the [MIT License](LICENSE).
