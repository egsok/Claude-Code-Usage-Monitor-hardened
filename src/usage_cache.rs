use std::fs::File;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::models::UsageData;

const CACHE_VERSION: u32 = 2;

#[derive(Clone)]
pub(crate) struct CachedProviderUsage {
    pub(crate) usage: UsageData,
    pub(crate) updated_at_unix: u64,
}

#[derive(Clone, Default)]
pub(crate) struct CachedAppUsage {
    pub(crate) claude: Option<CachedProviderUsage>,
    pub(crate) codex: Option<CachedProviderUsage>,
}

#[derive(Deserialize, Serialize)]
struct CacheFileV2 {
    version: u32,
    claude: Option<ProviderCacheFile>,
    codex: Option<ProviderCacheFile>,
}

#[derive(Deserialize, Serialize)]
struct ProviderCacheFile {
    updated_at_unix: u64,
    usage: UsageData,
}

#[derive(Deserialize)]
struct CacheFileV1 {
    version: u32,
    updated_at_unix: u64,
    claude: UsageData,
}

#[derive(Deserialize)]
struct CacheVersion {
    version: u32,
}

pub(crate) fn path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata)
        .join("ClaudeCodeUsageMonitor")
        .join("usage-cache.json")
}

pub(crate) fn load() -> Result<Option<CachedAppUsage>, String> {
    load_from(&path())
}

pub(crate) fn save_updates(
    claude: Option<CachedProviderUsage>,
    codex: Option<CachedProviderUsage>,
) -> Result<(), String> {
    save_updates_to(&path(), claude, codex)
}

fn load_from(path: &Path) -> Result<Option<CachedAppUsage>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("unable to read {}: {error}", path.display())),
    };
    let version: CacheVersion = serde_json::from_str(&content)
        .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))?;

    let cache = match version.version {
        1 => {
            let legacy: CacheFileV1 = serde_json::from_str(&content)
                .map_err(|error| format!("invalid v1 cache in {}: {error}", path.display()))?;
            debug_assert_eq!(legacy.version, 1);
            CachedAppUsage {
                claude: Some(CachedProviderUsage {
                    usage: legacy.claude,
                    updated_at_unix: legacy.updated_at_unix,
                }),
                codex: None,
            }
        }
        CACHE_VERSION => {
            let current: CacheFileV2 = serde_json::from_str(&content)
                .map_err(|error| format!("invalid v2 cache in {}: {error}", path.display()))?;
            CachedAppUsage {
                claude: current.claude.map(CachedProviderUsage::from),
                codex: current.codex.map(CachedProviderUsage::from),
            }
        }
        unsupported => {
            return Err(format!(
                "unsupported cache version {unsupported} in {}",
                path.display()
            ));
        }
    };

    validate_cache(&cache, path)?;
    Ok(Some(cache))
}

fn save_to(path: &Path, cache: &CachedAppUsage) -> Result<(), String> {
    validate_cache(cache, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("cache path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("unable to create {}: {error}", parent.display()))?;

    let json = serde_json::to_vec_pretty(&CacheFileV2 {
        version: CACHE_VERSION,
        claude: cache.claude.clone().map(ProviderCacheFile::from),
        codex: cache.codex.clone().map(ProviderCacheFile::from),
    })
    .map_err(|error| format!("unable to serialize usage cache: {error}"))?;
    let temporary_path = path.with_extension("json.tmp");

    let write_result = (|| {
        let mut file = File::create(&temporary_path)
            .map_err(|error| format!("unable to create {}: {error}", temporary_path.display()))?;
        file.write_all(&json)
            .map_err(|error| format!("unable to write {}: {error}", temporary_path.display()))?;
        file.sync_all()
            .map_err(|error| format!("unable to flush {}: {error}", temporary_path.display()))?;
        replace_file(&temporary_path, path)
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    write_result
}

fn save_updates_to(
    path: &Path,
    claude: Option<CachedProviderUsage>,
    codex: Option<CachedProviderUsage>,
) -> Result<(), String> {
    if claude.is_none() && codex.is_none() {
        return Ok(());
    }

    // A fresh successful response is more trustworthy than an unreadable old cache.
    // Starting over also preserves the previous behavior where a successful poll repaired it.
    let mut cache = load_from(path).ok().flatten().unwrap_or_default();
    if let Some(claude) = claude {
        cache.claude = Some(claude);
    }
    if let Some(codex) = codex {
        cache.codex = Some(codex);
    }
    save_to(path, &cache)
}

impl From<ProviderCacheFile> for CachedProviderUsage {
    fn from(value: ProviderCacheFile) -> Self {
        Self {
            usage: value.usage,
            updated_at_unix: value.updated_at_unix,
        }
    }
}

impl From<CachedProviderUsage> for ProviderCacheFile {
    fn from(value: CachedProviderUsage) -> Self {
        Self {
            updated_at_unix: value.updated_at_unix,
            usage: value.usage,
        }
    }
}

fn validate_cache(cache: &CachedAppUsage, path: &Path) -> Result<(), String> {
    if cache.claude.is_none() && cache.codex.is_none() {
        return Err(format!("usage cache is empty in {}", path.display()));
    }
    if cache
        .claude
        .iter()
        .chain(cache.codex.iter())
        .any(|provider| provider.updated_at_unix == 0 || !usage_is_valid(&provider.usage))
    {
        return Err(format!("invalid usage values in {}", path.display()));
    }
    Ok(())
}

fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    let source_wide = wide_path(source);
    let destination_wide = wide_path(destination);
    unsafe {
        MoveFileExW(
            PCWSTR::from_raw(source_wide.as_ptr()),
            PCWSTR::from_raw(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|error| {
            format!(
                "unable to replace {} with {}: {error}",
                destination.display(),
                source.display()
            )
        })
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn usage_is_valid(usage: &UsageData) -> bool {
    [&usage.session, &usage.weekly]
        .into_iter()
        .chain(usage.scoped_weekly.iter().map(|limit| &limit.usage))
        .all(|section| {
            section.percentage.is_finite() && (0.0..=100.0).contains(&section.percentage)
        })
        && usage
            .scoped_weekly
            .iter()
            .all(|limit| !limit.model_name.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ModelUsageLimit, UsageSection};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn temporary_cache_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        std::env::temp_dir()
            .join(format!(
                "claude-usage-cache-{}-{unique}",
                std::process::id()
            ))
            .join(name)
    }

    fn sample_usage() -> UsageData {
        UsageData {
            session: UsageSection {
                percentage: 13.0,
                resets_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            },
            weekly: UsageSection {
                percentage: 6.0,
                resets_at: None,
            },
            scoped_weekly: vec![ModelUsageLimit {
                model_name: "Fable".to_string(),
                usage: UsageSection {
                    percentage: 12.0,
                    resets_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_100_000)),
                },
            }],
            scoped_weekly_authoritative: false,
        }
    }

    #[test]
    fn provider_snapshots_survive_file_round_trip_without_credentials() {
        let path = temporary_cache_path("usage-cache.json");
        let cache = CachedAppUsage {
            claude: Some(CachedProviderUsage {
                usage: sample_usage(),
                updated_at_unix: 1_800_000_001,
            }),
            codex: Some(CachedProviderUsage {
                usage: UsageData {
                    session: UsageSection {
                        percentage: 42.0,
                        resets_at: None,
                    },
                    weekly: UsageSection {
                        percentage: 17.0,
                        resets_at: None,
                    },
                    ..Default::default()
                },
                updated_at_unix: 1_800_000_002,
            }),
        };
        save_to(&path, &cache).expect("cache should be written atomically");

        let restored = load_from(&path)
            .expect("valid cache should load")
            .expect("cache should exist");
        let claude = restored.claude.expect("Claude snapshot should survive");
        let codex = restored.codex.expect("Codex snapshot should survive");
        assert_eq!(claude.updated_at_unix, 1_800_000_001);
        assert_eq!(claude.usage.session.percentage, 13.0);
        assert_eq!(claude.usage.weekly.percentage, 6.0);
        assert!(!claude.usage.scoped_weekly_authoritative);
        assert_eq!(
            claude
                .usage
                .scoped_weekly_for("Fable")
                .expect("Fable should survive restart")
                .percentage,
            12.0
        );
        assert_eq!(codex.updated_at_unix, 1_800_000_002);
        assert_eq!(codex.usage.session.percentage, 42.0);
        assert_eq!(codex.usage.weekly.percentage, 17.0);

        let json = std::fs::read_to_string(&path).expect("cache should remain readable");
        assert!(json.contains("\"version\": 2"));
        assert!(json.contains("\"claude\""));
        assert!(json.contains("\"codex\""));
        assert!(!json.contains("token"));
        assert!(!json.contains("credential"));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(path.parent().expect("cache should have a parent"));
    }

    #[test]
    fn legacy_claude_only_cache_migrates_without_losing_fable() {
        let path = temporary_cache_path("usage-cache.json");
        std::fs::create_dir_all(path.parent().expect("cache should have a parent"))
            .expect("temporary cache directory should be created");
        let legacy = serde_json::json!({
            "version": 1,
            "updated_at_unix": 1_800_000_001_u64,
            "claude": sample_usage(),
        });
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&legacy).expect("legacy fixture should serialize"),
        )
        .expect("legacy fixture should be written");

        let restored = load_from(&path)
            .expect("v1 cache should remain readable")
            .expect("cache should exist");
        let claude = restored
            .claude
            .expect("legacy Claude snapshot should migrate");
        assert!(restored.codex.is_none());
        assert_eq!(claude.updated_at_unix, 1_800_000_001);
        assert_eq!(
            claude
                .usage
                .scoped_weekly_for("Fable")
                .expect("legacy Fable value should survive")
                .percentage,
            12.0
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(path.parent().expect("cache should have a parent"));
    }

    #[test]
    fn updating_one_provider_preserves_the_other_snapshot() {
        let path = temporary_cache_path("usage-cache.json");
        let original = CachedAppUsage {
            claude: Some(CachedProviderUsage {
                usage: sample_usage(),
                updated_at_unix: 1_800_000_001,
            }),
            codex: Some(CachedProviderUsage {
                usage: UsageData {
                    session: UsageSection {
                        percentage: 42.0,
                        resets_at: None,
                    },
                    ..Default::default()
                },
                updated_at_unix: 1_800_000_002,
            }),
        };
        save_to(&path, &original).expect("initial cache should be written");

        save_updates_to(
            &path,
            Some(CachedProviderUsage {
                usage: UsageData {
                    session: UsageSection {
                        percentage: 19.0,
                        resets_at: None,
                    },
                    ..Default::default()
                },
                updated_at_unix: 1_800_000_003,
            }),
            None,
        )
        .expect("partial update should preserve the other provider");

        let restored = load_from(&path)
            .expect("updated cache should load")
            .expect("cache should exist");
        assert_eq!(
            restored
                .claude
                .expect("Claude should be updated")
                .usage
                .session
                .percentage,
            19.0
        );
        assert_eq!(
            restored
                .codex
                .expect("Codex should be preserved")
                .usage
                .session
                .percentage,
            42.0
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(path.parent().expect("cache should have a parent"));
    }

    #[test]
    fn corrupted_cache_is_rejected_instead_of_becoming_usage_data() {
        let path = temporary_cache_path("usage-cache.json");
        std::fs::create_dir_all(path.parent().expect("cache should have a parent"))
            .expect("temporary cache directory should be created");
        std::fs::write(&path, b"not-json").expect("corrupted fixture should be written");

        assert!(load_from(&path).is_err());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(path.parent().expect("cache should have a parent"));
    }
}
