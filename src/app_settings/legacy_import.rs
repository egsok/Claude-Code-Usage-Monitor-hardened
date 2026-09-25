//! Explicit, one-time import from a read-only snapshot of the hardened v1 profile.
use super::*;
use crate::models::{AccountUsage, UsageData, UsageLimit};

/// Returns false when this experimental profile has already imported a snapshot.
/// Existing experimental settings are never replaced by a later import request.
pub fn import_legacy_profile(source: &Path) -> Result<bool, String> {
    import_into(source, &app_data_directory())
}

fn import_into(source: &Path, destination: &Path) -> Result<bool, String> {
    let _transaction = SettingsTransaction::acquire(&destination.join("settings.json"))?;
    let marker = destination.join("import-v1.7.complete.json");
    if marker.exists() {
        return Ok(false);
    }
    if destination.join("settings.json").exists() || destination.join("usage-cache.json").exists() {
        return Err(
            "The experimental profile already contains settings or usage; import refused".into(),
        );
    }
    let source = std::fs::canonicalize(source)
        .map_err(|error| format!("Unable to read snapshot directory: {error}"))?;
    if let Some(root) = std::env::var_os("APPDATA") {
        let live = PathBuf::from(root).join("ClaudeCodeUsageMonitor");
        if std::fs::canonicalize(live).ok().as_ref() == Some(&source) {
            return Err("Import requires a snapshot directory, not the live v1 profile".into());
        }
    }
    let settings_bytes = std::fs::read(source.join("settings.json"))
        .map_err(|error| format!("Unable to read snapshot settings: {error}"))?;
    let settings_text = std::str::from_utf8(&settings_bytes)
        .map_err(|error| format!("Snapshot settings are not UTF-8: {error}"))?;
    let mut settings = decode_settings(settings_text).ok_or("Invalid v1 settings snapshot")?;
    // v1 has no named accounts. Never attach a legacy snapshot to a new profile.
    settings.accounts = Default::default();
    settings.monitor_widget_visible = settings.widget_visible;
    settings.legacy_visibility_pending = false;
    settings.widget_visible = true;
    settings.settings_schema_version = settings_schema_version();
    settings.monitor_placement = Some(initial_floating_placement());
    settings.placement_override = None;
    settings.legacy_placement_pending = false;
    settings.tray_offset = 0;
    settings.taskbar_index = 0;
    settings.active_theme_path = None;
    settings.normalize();

    let cache_bytes = optional_bytes(&source.join("usage-cache.json"))?;
    let cache = cache_bytes.as_deref().map(convert_cache).transpose()?;
    let pre_v2 = optional_bytes(&source.join("settings.pre-v2.json"))?;
    let backup = destination.join("import-v1.7");
    std::fs::create_dir_all(&backup).map_err(|error| error.to_string())?;
    preserve_bytes(&backup.join("settings.json"), &settings_bytes)?;
    for (name, bytes) in [
        ("usage-cache.json", cache_bytes),
        ("settings.pre-v2.json", pre_v2),
    ] {
        if let Some(bytes) = bytes {
            preserve_bytes(&backup.join(name), &bytes)?;
        }
    }
    // Backups precede all writes. A partial import is reported, never retried by
    // silently replacing an already-created experimental profile.
    if let Some(cache) = cache {
        write_json_atomic(&destination.join("usage-cache.json"), &cache)?;
    }
    save_settings_to(&destination.join("settings.json"), &settings)?;
    write_json_atomic(
        &marker,
        &serde_json::json!({"version": 1, "imported_at_unix": now_unix()}),
    )?;
    Ok(true)
}

fn optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Unable to read {}: {error}", path.display())),
    }
}

fn preserve_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(bytes).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::read(path).map_err(|error| error.to_string())? == bytes {
                Ok(())
            } else {
                Err(format!(
                    "Original migration backup already exists: {}",
                    path.display()
                ))
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

fn convert_cache(bytes: &[u8]) -> Result<UsageCache, String> {
    convert_cache_with(bytes, crate::poller::default_account_source)
}

fn convert_cache_with(
    bytes: &[u8],
    account_source: impl Fn(ProviderId) -> (Option<PathBuf>, String),
) -> Result<UsageCache, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("Invalid legacy usage cache: {error}"))?;
    let version = value["version"]
        .as_u64()
        .ok_or("Legacy cache version missing")?;
    if !matches!(version, 1 | 2) {
        return Err(format!("Unsupported legacy cache version {version}"));
    }
    let mut cache = UsageCache::default();
    for (key, provider) in [("claude", ProviderId::Claude), ("codex", ProviderId::Codex)] {
        let bucket = &value[key];
        if bucket.is_null() || (version == 1 && provider != ProviderId::Claude) {
            continue;
        }
        let (reading, timestamp) = if version == 1 {
            (bucket, value["updated_at_unix"].as_u64())
        } else {
            (&bucket["usage"], bucket["updated_at_unix"].as_u64())
        };
        let timestamp = timestamp
            .filter(|stamp| *stamp > 0)
            .ok_or("Invalid legacy timestamp")?;
        let mut usage: UsageData = serde_json::from_value(reading.clone())
            .map_err(|error| format!("Invalid legacy usage: {error}"))?;
        // v1 stored both windows even when idle and without a reset timestamp.
        usage.session.available = true;
        usage.weekly.available = true;
        if let Some(limits) = reading["scoped_weekly"].as_array() {
            for limit in limits {
                let model = limit["model_name"]
                    .as_str()
                    .ok_or("Invalid scoped model name")?;
                let mut section: crate::models::UsageSection =
                    serde_json::from_value(limit["usage"].clone())
                        .map_err(|error| format!("Invalid scoped usage: {error}"))?;
                section.available = true;
                usage.limits.push(UsageLimit {
                    key: format!("weekly_scoped_{}", crate::models::limit_slug(model)),
                    kind: "weekly_scoped".into(),
                    label: model.into(),
                    model: Some(model.into()),
                    usage: section,
                    stale: true,
                    ..Default::default()
                });
            }
        }
        if usage.sections().any(|section| {
            !section.percentage.is_finite() || !(0.0..=100.0).contains(&section.percentage)
        }) {
            return Err("Legacy usage percentage is outside 0..100".into());
        }
        usage.stale = true;
        usage.updated_at_unix = Some(timestamp);
        let (source_path, source_signature) = account_source(provider);
        cache.data.insert(provider, usage.clone());
        cache.data.accounts.push(AccountUsage {
            provider,
            profile: Default::default(),
            source_path,
            source_signature,
            usage: Some(usage),
            error: None,
            selected: true,
        });
        cache.updated_unix = cache.updated_unix.max(timestamp);
    }
    if cache.data.is_empty() {
        return Err("Legacy usage cache contains no provider snapshots".into());
    }
    Ok(cache)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Migration fixtures must never discover or read the user's credentials.
    fn convert_cache(bytes: &[u8]) -> Result<UsageCache, String> {
        super::convert_cache_with(bytes, |_| (None, "fixture-default-source".into()))
    }

    fn legacy_usage() -> serde_json::Value {
        serde_json::json!({"session":{"percentage":25,"resets_at":null},"weekly":{"percentage":35,"resets_at":null},"scoped_weekly":[{"model_name":"Fable","usage":{"percentage":55,"resets_at":null}}]})
    }

    #[test]
    fn legacy_cache_versions_keep_fable_timestamps_and_bind_only_default_accounts() {
        for value in [
            serde_json::json!({"version":1,"updated_at_unix":101,"claude":legacy_usage()}),
            serde_json::json!({"version":2,"claude":{"updated_at_unix":101,"usage":legacy_usage()},"codex":{"updated_at_unix":202,"usage":legacy_usage()}}),
        ] {
            let cache = convert_cache(&serde_json::to_vec(&value).unwrap()).unwrap();
            let claude = cache.data.get(ProviderId::Claude).unwrap();
            assert_eq!(claude.updated_at_unix, Some(101));
            assert!(claude.stale);
            assert_eq!(claude.limits[0].model.as_deref(), Some("Fable"));
            assert!(cache
                .data
                .accounts
                .iter()
                .all(|account| account.profile.id == "default"));
            assert!(cache
                .data
                .accounts
                .iter()
                .all(|account| account.source_path.is_none()
                    && account.source_signature == "fixture-default-source"));
            if let Some(codex) = cache.data.get(ProviderId::Codex) {
                assert_eq!(codex.updated_at_unix, Some(202));
            }
            let mut settings = crate::accounts::AccountSettings::default();
            settings.claude.add();
            settings.claude.profiles[1].enabled = true;
            settings.claude.profiles[1].config_dir = "C:\\different-account".into();
            settings.claude.selected = settings.claude.profiles[1].id.clone();
            let mut data = cache.data;
            data.select_accounts(&settings);
            assert!(
                data.get(ProviderId::Claude).is_none(),
                "legacy usage must never leak to a named account"
            );
        }
    }

    #[test]
    fn import_preserves_exact_backup_and_never_overwrites_experimental_changes() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let bytes = b"{\r\n  \"show_codex\": true, \"poll_interval_ms\": 300000, \"monitors\": [{\"id\":\"device-a\",\"name\":\"A\",\"enabled\":true,\"offset_dip\":42}]\r\n}\r\n";
        std::fs::write(source.path().join("settings.json"), bytes).unwrap();
        std::fs::write(source.path().join("settings.pre-v2.json"), b"original").unwrap();
        assert!(import_into(source.path(), destination.path()).unwrap());
        let path = destination.path().join("settings.json");
        let mut settings = load_settings_from(&path);
        assert_eq!(settings.monitors[0].offset_dip, 42);
        assert_eq!(
            settings.monitor_placement.as_ref().unwrap().nest,
            "floating"
        );
        settings.poll_interval_ms = POLL_1_MIN;
        save_settings_to(&path, &settings).unwrap();
        assert!(!import_into(source.path(), destination.path()).unwrap());
        assert_eq!(load_settings_from(&path).poll_interval_ms, POLL_1_MIN);
        assert_eq!(
            std::fs::read(destination.path().join("import-v1.7/settings.json")).unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read(source.path().join("settings.json")).unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read(source.path().join("settings.pre-v2.json")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn existing_profile_and_invalid_cache_are_not_silently_replaced() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("settings.json"), b"{}").unwrap();
        std::fs::write(destination.path().join("settings.json"), b"original").unwrap();
        assert!(import_into(source.path(), destination.path()).is_err());
        assert_eq!(
            std::fs::read(destination.path().join("settings.json")).unwrap(),
            b"original"
        );
        assert!(convert_cache(b"{\"version\":99}").is_err());
        let invalid = serde_json::json!({"version":1,"updated_at_unix":1,"claude":{"session":{"percentage":101,"resets_at":null},"weekly":{"percentage":0,"resets_at":null}}});
        assert!(convert_cache(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }

    #[test]
    fn idle_legacy_windows_remain_available_without_reset_timestamps() {
        let value = serde_json::json!({"version":1,"updated_at_unix":101,"claude":{
            "session":{"percentage":0,"resets_at":null},
            "weekly":{"percentage":0,"resets_at":null}
        }});
        let cache = convert_cache(&serde_json::to_vec(&value).unwrap()).unwrap();
        let usage = cache.data.get(ProviderId::Claude).unwrap();
        assert!(usage.session.available);
        assert!(usage.weekly.available);
        assert_eq!(usage.session.percentage, 0.0);
    }
}
