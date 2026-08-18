<!-- generated-by: gsd-doc-writer -->
# Developer Getting Started

## Prerequisites

Development is Windows-only because the crate directly links Win32 APIs and
uses `#![windows_subsystem = "windows"]`.

Install:

- Windows 10 or 11;
- Git;
- the stable Rust toolchain with the MSVC target;
- Microsoft C++ build tools and a Windows SDK, as required by the Rust MSVC
  toolchain and the `windows`/`winres` crates.

No minimum supported Rust version is pinned in the repository. GitHub Releases
build with the current stable toolchain declared by the release workflow.

## Clone and build

```powershell
git clone https://github.com/egsok/Claude-Code-Usage-Monitor-hardened.git
Set-Location Claude-Code-Usage-Monitor-hardened
cargo build
```

The debug binary is written to:

```text
target\debug\claude-code-usage-monitor.exe
```

Build the distributable binary with:

```powershell
cargo build --release
```

The release profile optimizes for size, enables LTO, strips symbols, uses one
codegen unit, and aborts on panic.

## First run

For a normal development launch:

```powershell
cargo run
```

For an opt-in diagnostic log:

```powershell
cargo run -- --diagnose
```

The application does not present a console. Use the taskbar widget and tray
icons to confirm it started. Diagnostic output is written to
`%TEMP%\claude-code-usage-monitor.log`.

Provider data appears only when the corresponding provider is enabled and its
own application has already created local credentials. The monitor never opens
a login flow or launches an agent to create credentials.

## Required first checks

Run these before modifying code:

```powershell
cargo test --no-fail-fast
cargo fmt -- --check
```

For the current strict Clippy baseline, see `docs/DEVELOPMENT.md`. GUI and
taskbar behavior also require manual verification; the relevant matrix is in
`docs/TESTING.md`.

## Common setup issues

### A second process exits immediately

The application uses the global mutex `Global\ClaudeCodeUsageMonitor`. Stop the
installed copy before testing another binary. Otherwise the new process exits
silently by design.

```powershell
Get-Process claude-code-usage-monitor -ErrorAction SilentlyContinue |
  Select-Object Id, Path
```

### A release rebuild cannot replace the EXE

Windows locks the running executable. Stop the process that points to
`target/release/claude-code-usage-monitor.exe` before rebuilding that target.
Do not stop an unrelated process by name without checking its `Path` first.

### The widget shows `...`, `!`, or `~`

- `...` means no snapshot exists yet and a request is pending or failed.
- `!` or the authentication UI means the provider needs credentials or login.
- `~` means the displayed value is the last known snapshot during a transient
  failure or less authoritative fallback response.

Run the provider's own CLI or desktop login flow when credentials need refresh,
then refresh the monitor. Do not add credential-refresh subprocesses to the
monitor.

### The widget overlaps centered Windows 11 icons

The widget is reparented into the taskbar but does not reserve AppBar space.
Move it to the left side or use floating placement. Automatic centered-icon
collision avoidance is tracked in `BACKLOG.md`.

## Next steps

- `docs/ARCHITECTURE.md` — component and runtime data flow.
- `docs/CONFIGURATION.md` — settings, cache, credentials, and startup.
- `docs/DEVELOPMENT.md` — code workflow and release runbook.
- `docs/TESTING.md` — automated and manual verification.
- `docs/SECURITY.md` — trust boundaries and residual risks.
