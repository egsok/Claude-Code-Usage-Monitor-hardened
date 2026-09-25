//! Informational release checks. This experiment never installs updates.

use std::time::Duration;

mod release;
pub use release::ReleaseDescriptor;
use release::{release_descriptor, GitHubRelease};

#[derive(Debug)]
pub enum UpdateCheckResult {
    UpToDate,
    Available(ReleaseDescriptor),
}

pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
    let (owner, repo) = github_repo()?;
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(Duration::from_secs(30)))
        .tls_config(tls)
        .build()
        .into();
    let mut response = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
        )
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|error| format!("Unable to check GitHub releases: {error}"))?;
    let release: GitHubRelease = response
        .body_mut()
        .read_json()
        .map_err(|error| format!("Unable to parse GitHub release data: {error}"))?;
    Ok(
        match release_descriptor(release, env!("CARGO_PKG_VERSION"))? {
            Some(release) => UpdateCheckResult::Available(release),
            None => UpdateCheckResult::UpToDate,
        },
    )
}

fn github_repo() -> Result<(&'static str, &'static str), String> {
    let repository = env!("CARGO_PKG_REPOSITORY")
        .strip_prefix("https://github.com/")
        .ok_or("Package repository must be a GitHub HTTPS URL.")?;
    let (owner, repo) = repository
        .trim_end_matches('/')
        .split_once('/')
        .ok_or("Package repository is missing its owner or name.")?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err("Invalid package repository URL.".into());
    }
    Ok((owner, repo))
}

#[cfg(test)]
mod tests {
    #[test]
    fn releases_belong_to_the_hardened_fork() {
        assert_eq!(
            super::github_repo().unwrap(),
            ("egsok", "Claude-Code-Usage-Monitor-hardened")
        );
    }

    #[test]
    fn release_checks_cannot_launch_or_write_executables() {
        let production = include_str!("updater.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in [
            "Command::",
            "std::fs::",
            "--apply-update",
            "download_release_asset",
        ] {
            assert!(
                !production.contains(forbidden),
                "release checker contains {forbidden}"
            );
        }
    }
}
