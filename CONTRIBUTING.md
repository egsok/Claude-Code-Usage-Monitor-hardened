<!-- generated-by: gsd-doc-writer -->
# Contributing

## Development setup

Read `docs/GETTING-STARTED.md` for prerequisites and a first build, then
`docs/DEVELOPMENT.md` for the change and release workflow. This project is a
Windows-native Rust application; taskbar behavior must be tested on Windows.

## Coding standards

- Format Rust changes with `cargo fmt`.
- Run the full test and scoped strict-Clippy commands documented in
  `docs/TESTING.md`.
- Keep changes surgical and preserve the existing Win32/event-driven design.
- Add a regression test for changed polling, cache, settings, placement, or
  security behavior.
- Never add background provider-agent launches or serialize credentials into
  the monitor's files.

## Pull requests

- Use an `agent/<short-description>` branch for substantial changes.
- Keep commits focused and use concise behavior-oriented subjects.
- Explain the user-visible problem, root cause, chosen fix, and verification.
- Include screenshots for layout changes and exact manual scenarios for Win32
  lifecycle changes.
- Do not stage build artifacts, local diagnostic logs, credentials, settings,
  usage caches, or the maintainer's untracked `target-*` directories.
- A PR should be ready only after automated checks and relevant manual checks
  pass. Draft PRs are appropriate when Windows validation is still pending.

## Security-sensitive changes

Read `docs/SECURITY.md` before modifying credential discovery, subprocesses,
network destinations, cache contents, update behavior, or the release workflow.
Changes that broaden any trust boundary must state the new capability and why it
is required.

Do not include real tokens, cookies, account IDs, credential-file contents, or
diagnostic logs containing local paths in an issue or pull request.

## Reporting issues

Open an issue in the hardened fork and include:

- application version and executable path;
- Windows version and taskbar/monitor arrangement;
- enabled providers and placement mode;
- steps to reproduce, expected behavior, and actual behavior;
- whether a VPN, proxy, or restrictive firewall was active;
- a redacted diagnostic log only when it is needed.

For security-sensitive reports, avoid publishing secrets or exploit details in
a public issue. Contact the repository owner through a private channel first.
