<!-- generated-by: gsd-doc-writer -->
# Security Model

## Scope

This document describes the security boundaries of the hardened fork. The
monitor is intentionally a passive local utility: it may read existing provider
credentials and send them to the matching provider endpoint, but it must not
authenticate, refresh credentials through an agent, or silently replace its own
portable executable.

The design reduces authority; it does not make a credential-reading desktop
application risk-free.

## Protected assets

- Claude, Codex, and Antigravity access credentials.
- Provider account identity and usage/quota data.
- Integrity of the executable installed on the workstation.
- Integrity of the Windows autostart entry and saved widget configuration.
- Availability of provider CLIs, which must remain under explicit user control.

## Trust boundaries

```text
provider credential stores
          |
          | read at poll time
          v
     poller.rs --------------------> official provider HTTPS endpoints
          |
          | usage data only
          v
 usage_cache.rs ----> %APPDATA%\ClaudeCodeUsageMonitor\usage-cache.json

updater.rs ---------> public GitHub release metadata
          \
           `--------> WinGet command only for a detected WinGet install
```

The Windows user account, provider-owned credential files, Windows Credential
Manager, operating-system TLS validation, GitHub release infrastructure, and an
eventual WinGet package are trusted dependencies.

## Credential sources and destinations

| Provider | Credential source | Network destination |
|---|---|---|
| Claude | The Claude credential file under `%USERPROFILE%\.claude`, then the same relative file in discovered WSL distributions | `https://api.anthropic.com/api/oauth/usage`; fallback `https://api.anthropic.com/v1/messages` |
| Codex | The Codex auth file under `%CODEX_HOME%` or `%USERPROFILE%\.codex` | `https://chatgpt.com/backend-api/wham/usage` |
| Antigravity | Windows generic credential `gemini:antigravity` | supported `daily-cloudcode-pa` and `cloudcode-pa` Google endpoints |

Bearer credentials are placed only in the request to the corresponding
provider. The Claude Messages fallback sends a minimal request with
`max_tokens: 1` to obtain rate-limit headers when the dedicated usage response
is unavailable or incomplete.

The monitor does not copy access tokens into its settings file, usage cache,
release checks, or tray text.

## Subprocess policy

The polling module may start only `wsl.exe`, for two bounded purposes:

- enumerate WSL distributions;
- read or stat the Claude credential file in a selected distribution.

These calls have a five-second timeout, hidden windows, captured output, and no
provider prompt. A regression test scans `src/poller.rs` to prevent reintroducing
Claude Code or Codex agent command construction.

The separate updater module may start PowerShell only for a WinGet-managed
installation. Portable builds receive an informational update message and do
not download or replace themselves.

## Local persistence

The runtime settings file stores UI state only. The usage cache stores validated usage
percentages, reset times, model-scoped quota names, and update timestamps. It is
written atomically through a temporary file and write-through replacement.

The usage cache has no credential fields and rejects invalid percentages,
empty provider sets, malformed JSON, and unsupported schema versions. It does
not currently enforce a maximum snapshot age. Stale values can remain visible
with `~` until a fresh response arrives; they must not be interpreted as a
security or billing guarantee.

The application does not apply a custom ACL to settings or cache files. They
inherit the current user's profile-directory permissions. Another process
running as the same Windows user can read or alter them, although those files do
not contain provider credentials.

## Network and TLS behavior

Provider and GitHub calls use `ureq` with `native-tls`, a 30-second timeout, and
normal certificate/hostname validation. The hardened fork does not disable TLS
verification or certificate revocation checks in source code. Proxy-from-
environment support is compiled in, so workstation proxy or VPN routing can
affect availability and trust.

Provider isolation is a reliability and security property: a failed provider
does not cause successful data from another provider to be discarded. Network
errors preserve known data with a visible stale marker and exponential retry;
authentication errors require explicit provider-side login.

## Update and release boundary

The application checks the hardened fork's public GitHub Releases API. It sends
only normal HTTP metadata such as IP address, TLS connection data, and the
application User-Agent; provider tokens are not added to GitHub requests.

Portable self-update is disabled. WinGet update execution is enabled only when
the current executable path is detected under a WinGet package root, and it
targets the fork-specific package ID `egsok.ClaudeCodeUsageMonitorHardened`.

GitHub Releases are built on `windows-latest` from a pushed `v*` tag. The
workflow uses version-tagged third-party actions rather than commit-digest pins,
and the resulting EXE is not Authenticode-signed. There is currently no checksum
file, provenance attestation, or in-application digest verification. These are
the largest remaining distribution risks.

Before trusting a release, a maintainer should verify the tag commit, workflow
conclusion, PE version metadata, and SHA-256 of the downloaded asset. Windows may
show an unknown-publisher warning until code signing is introduced.

## Diagnostic data

Diagnostics are disabled unless the process is started with `--diagnose`. The
temporary log is truncated at startup and may contain timestamps, local
credential paths, WSL distribution names, provider names, error categories,
retry timing, and window/taskbar state. Code does not intentionally log token
contents.

Treat a diagnostic log as private workstation data and redact local paths before
sharing it publicly.

## Hardened invariants

Changes must preserve these invariants:

1. No provider agent or login process is launched in the background.
2. No credential or session token is persisted by the monitor.
3. Each credential is sent only to its corresponding provider endpoint.
4. Portable builds do not download and execute a replacement binary.
5. Authentication failures remain explicit user actions.
6. Transient network failures do not destroy the last known snapshot.
7. Provider-specific failure does not block successful providers.
8. Release identity and future package identity remain separate from upstream.

## Residual risks and recommended next work

| Risk | Current status | Recommended mitigation |
|---|---|---|
| Unsigned executable | Open | Authenticode-sign release artifacts with a protected signing identity. |
| Mutable GitHub Action tags | Open | Pin actions to reviewed commit SHAs and use dependency update automation. |
| No release checksum or attestation | Open | Publish SHA-256 and provenance for each asset. |
| Release workflow skips tests | Open | Add a pull-request/push CI workflow and make release depend on it. |
| Same-user processes can read provider credential files | Inherent | Keep the utility small, auditable, and least-privileged; avoid administrator execution. |
| Usage cache has no age limit | Accepted UX tradeoff | Continue showing `~`; consider an age tooltip or expiry policy without hiding provenance. |
| VPN/proxy route selection can break one provider | External | Diagnose per-provider reachability; do not weaken the application or workstation kill switch. |

## Security review checklist

For any change touching `src/poller.rs`, `src/usage_cache.rs`, `src/updater.rs`,
`.github/workflows/release.yml`, or startup behavior:

- enumerate new files, registry keys, subprocesses, hosts, and request headers;
- confirm whether any credential crosses a new boundary;
- add a regression test for the invariant being changed;
- verify diagnostic output cannot expose a token;
- run the full automated suite and relevant manual network/authentication cases;
- document any residual risk that remains accepted.
