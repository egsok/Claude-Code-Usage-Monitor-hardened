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

const CACHE_VERSION: u32 = 1;

pub(crate) struct CachedClaudeUsage {
    pub(crate) usage: UsageData,
    pub(crate) updated_at_unix: u64,
}

#[derive(Deserialize, Serialize)]
struct CacheFile {
    version: u32,
    updated_at_unix: u64,
    claude: UsageData,
}

pub(crate) fn path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata)
        .join("ClaudeCodeUsageMonitor")
        .join("usage-cache.json")
}

pub(crate) fn load() -> Result<Option<CachedClaudeUsage>, String> {
    load_from(&path())
}

pub(crate) fn save(usage: &UsageData, updated_at_unix: u64) -> Result<(), String> {
    save_to(&path(), usage, updated_at_unix)
}

fn load_from(path: &Path) -> Result<Option<CachedClaudeUsage>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("unable to read {}: {error}", path.display())),
    };
    let cache: CacheFile = serde_json::from_str(&content)
        .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))?;

    if cache.version != CACHE_VERSION {
        return Err(format!(
            "unsupported cache version {} in {}",
            cache.version,
            path.display()
        ));
    }
    if cache.updated_at_unix == 0 || !usage_is_valid(&cache.claude) {
        return Err(format!("invalid usage values in {}", path.display()));
    }

    Ok(Some(CachedClaudeUsage {
        usage: cache.claude,
        updated_at_unix: cache.updated_at_unix,
    }))
}

fn save_to(path: &Path, usage: &UsageData, updated_at_unix: u64) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("cache path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("unable to create {}: {error}", parent.display()))?;

    let json = serde_json::to_vec_pretty(&CacheFile {
        version: CACHE_VERSION,
        updated_at_unix,
        claude: usage.clone(),
    })
    .map_err(|error| format!("unable to serialize Claude usage cache: {error}"))?;
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
        }
    }

    #[test]
    fn successful_claude_snapshot_survives_file_round_trip() {
        let path = temporary_cache_path("usage-cache.json");
        save_to(&path, &sample_usage(), 1_800_000_001).expect("cache should be written atomically");

        let restored = load_from(&path)
            .expect("valid cache should load")
            .expect("cache should exist");
        assert_eq!(restored.updated_at_unix, 1_800_000_001);
        assert_eq!(restored.usage.session.percentage, 13.0);
        assert_eq!(restored.usage.weekly.percentage, 6.0);
        assert_eq!(
            restored
                .usage
                .scoped_weekly_for("Fable")
                .expect("Fable should survive restart")
                .percentage,
            12.0
        );

        let json = std::fs::read_to_string(&path).expect("cache should remain readable");
        assert!(json.contains("\"claude\""));
        assert!(!json.contains("token"));
        assert!(!json.contains("credential"));

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
