use semver::Version;
use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct ReleaseDescriptor {
    pub latest_version: String,
    pub release_url: String,
}

#[derive(Deserialize)]
pub(super) struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

pub(super) fn release_descriptor(
    release: GitHubRelease,
    current: &str,
) -> Result<Option<ReleaseDescriptor>, String> {
    let latest = parse_version(&release.tag_name)?;
    let current = parse_version(current)?;
    if release.draft
        || release.prerelease
        || !latest.pre.is_empty()
        || !latest.cmp_precedence(&current).is_gt()
    {
        return Ok(None);
    }
    let (owner, repo) = super::github_repo()?;
    Ok(Some(ReleaseDescriptor {
        latest_version: latest.to_string(),
        release_url: format!(
            "https://github.com/{owner}/{repo}/releases/tag/{}",
            release.tag_name
        ),
    }))
}

fn parse_version(version: &str) -> Result<Version, String> {
    Version::parse(version.strip_prefix('v').unwrap_or(version))
        .map_err(|error| format!("Invalid release version {version:?}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.into(),
            draft: false,
            prerelease: false,
        }
    }

    #[test]
    fn informational_checks_need_no_executable_asset_or_digest() {
        let release = release_descriptor(release("v3.0.0"), "2.15.14-hardened.1")
            .unwrap()
            .unwrap();
        assert_eq!(release.latest_version, "3.0.0");
        assert_eq!(
            release.release_url,
            "https://github.com/egsok/Claude-Code-Usage-Monitor-hardened/releases/tag/v3.0.0"
        );
    }

    #[test]
    fn only_newer_stable_releases_are_reported() {
        for tag in ["v1.7.0", "v2.15.14-alpha.1"] {
            assert!(release_descriptor(release(tag), "2.15.14-hardened.1")
                .unwrap()
                .is_none());
        }
        let mut draft = release("v3.0.0");
        draft.draft = true;
        assert!(release_descriptor(draft, "2.15.14-hardened.1")
            .unwrap()
            .is_none());
        assert!(
            release_descriptor(release("v2.15.14"), "2.15.14-hardened.1")
                .unwrap()
                .is_some()
        );
        assert!(
            release_descriptor(release("v2.15.14+build.2"), "2.15.14+build.1")
                .unwrap()
                .is_none()
        );
        assert!(release_descriptor(release("not-a-version"), "2.15.14").is_err());
    }
}
