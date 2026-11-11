# Update policy for hardened v2

The portable application never downloads, replaces or executes an update EXE.
Its release check is informational and reads this fork's latest stable GitHub
release. Prereleases are not offered by that check: download v2 explicitly from
[GitHub Releases](https://github.com/egsok/Claude-Code-Usage-Monitor-hardened/releases).

## Manual update

1. Download the EXE and SHA256SUMS.txt from the same release.
2. Compare the EXE SHA-256 using PowerShell `Get-FileHash`.
3. Close the monitor and retain the old EXE for rollback.
4. Replace the EXE at its existing path and start it. Keep the profile and
   provider credentials; path-based routing and startup continue using that path.

For rollback, close the monitor and put the saved EXE back at the same path.
V1 and v2 use separate profiles; replacing one with the other is not a profile migration.

## Release integrity and limits

The release workflow builds the tagged commit using the lockfile and Rust 1.95.0,
checks the version, writes SHA256SUMS.txt, and uploads an EXE plus checksum as a draft.
Prereleases are not marked Latest or submitted to WinGet. The draft is published
after its assets and metadata are verified.

A checksum detects corruption relative to the published file; it is not an
Authenticode signature or protection against a compromised publishing account.
CI and local binaries built from the same source may differ; only matching hashes
prove identical files. Check the checksum shipped with the downloaded release.

`cargo +1.95.0 test --locked updater` covers informational-release safeguards.
There is no portable download/install helper in this fork.
