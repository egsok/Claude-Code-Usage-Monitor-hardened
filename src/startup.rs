use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};
use windows::Win32::System::Registry::*;

use crate::native_interop::wide_str;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ClaudeCodeUsageMonitorHardenedUpstream2";

pub(crate) fn is_enabled() -> bool {
    std::env::current_exe()
        .ok()
        .is_some_and(|exe| is_enabled_at(RUN_KEY, &quoted_command(&exe)))
}

pub(crate) fn set_enabled(enable: bool) -> Result<(), String> {
    let command = if enable {
        quoted_command(
            &std::env::current_exe()
                .map_err(|error| format!("Cannot locate the executable for startup: {error}"))?,
        )
    } else {
        Vec::new()
    };
    set_enabled_at(RUN_KEY, enable, &command)
}

fn quoted_command(exe: &Path) -> Vec<u16> {
    std::iter::once(b'"' as u16)
        .chain(exe.as_os_str().encode_wide())
        .chain([b'"' as u16, 0])
        .collect()
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn registry_result(result: WIN32_ERROR, operation: &str) -> Result<(), String> {
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!(
            "Cannot {operation} startup registry value (Windows error {})",
            result.0
        ))
    }
}

fn open_key(path: &str, access: REG_SAM_FLAGS, create: bool) -> Result<Option<Key>, String> {
    let path = wide_str(path);
    let mut key = HKEY::default();
    let result = unsafe {
        if create {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                access,
                None,
                &mut key,
                None,
            )
        } else {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                access,
                &mut key,
            )
        }
    };
    if !create && result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    registry_result(result, "open")?;
    Ok(Some(Key(key)))
}

fn write_string(key: &Key, name: &str, value: &[u16]) -> Result<(), String> {
    let name = wide_str(name);
    let bytes: Vec<u8> = value.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    registry_result(
        unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes)) },
        "write",
    )
}

fn read_string(key: &Key, name: &str) -> Result<Option<Vec<u16>>, String> {
    let name = wide_str(name);
    let mut kind = REG_VALUE_TYPE::default();
    let mut size = 0;
    let result = unsafe {
        RegQueryValueExW(
            key.0,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut size),
        )
    };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    registry_result(result, "read")?;
    if kind != REG_SZ || size < 2 || size % 2 != 0 || size > 65_536 {
        return Ok(None);
    }
    let mut bytes = vec![0u8; size as usize];
    registry_result(
        unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&mut kind),
                Some(bytes.as_mut_ptr()),
                Some(&mut size),
            )
        },
        "read",
    )?;
    if kind != REG_SZ || size < 2 || size % 2 != 0 {
        return Ok(None);
    }
    bytes.truncate(size as usize);
    let value: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        .collect();
    if value.last() != Some(&0) || value[..value.len() - 1].contains(&0) {
        return Ok(None);
    }
    Ok(Some(value))
}

fn is_enabled_at(path: &str, command: &[u16]) -> bool {
    let Ok(Some(key)) = open_key(path, KEY_QUERY_VALUE, false) else {
        return false;
    };
    let Ok(Some(value)) = read_string(&key, VALUE_NAME) else {
        return false;
    };
    unsafe { CompareStringOrdinal(&value, command, true) == CSTR_EQUAL }
}

fn set_enabled_at(path: &str, enable: bool, command: &[u16]) -> Result<(), String> {
    let Some(key) = open_key(path, KEY_SET_VALUE, enable)? else {
        return Ok(());
    };
    if enable {
        write_string(&key, VALUE_NAME, command)
    } else {
        let name = wide_str(VALUE_NAME);
        let result = unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) };
        if result == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            registry_result(result, "delete")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestKey(String);

    impl TestKey {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(format!(
                r"Software\CCUMStartupTests.{}.{}.{}",
                std::process::id(),
                nonce,
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            let path = wide_str(&self.0);
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()));
            }
        }
    }

    #[test]
    fn startup_roundtrip_requires_the_quoted_current_path() {
        let test = TestKey::new();
        let command = quoted_command(Path::new(r"C:\Программы\Usage Monitor\widget.exe"));
        assert_eq!(
            String::from_utf16(&command[..command.len() - 1]).unwrap(),
            "\"C:\\Программы\\Usage Monitor\\widget.exe\""
        );
        assert!(!is_enabled_at(&test.0, &command));
        set_enabled_at(&test.0, true, &command).unwrap();
        assert!(is_enabled_at(&test.0, &command));
        assert!(is_enabled_at(
            &test.0,
            &quoted_command(Path::new(r"c:\ПРОГРАММЫ\USAGE MONITOR\WIDGET.EXE"))
        ));
        assert!(!is_enabled_at(
            &test.0,
            &quoted_command(Path::new(r"C:\old\widget.exe"))
        ));
        let key = open_key(&test.0, KEY_QUERY_VALUE | KEY_SET_VALUE, false)
            .unwrap()
            .unwrap();
        assert_eq!(
            read_string(&key, VALUE_NAME).unwrap(),
            Some(command.clone())
        );
        write_string(
            &key,
            VALUE_NAME,
            &wide_str(r"C:\Программы\Usage Monitor\widget.exe"),
        )
        .unwrap();
        assert!(!is_enabled_at(&test.0, &command));
        let name = wide_str(VALUE_NAME);
        let result = unsafe {
            RegSetValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                REG_DWORD,
                Some(&1u32.to_le_bytes()),
            )
        };
        assert_eq!(result, ERROR_SUCCESS);
        assert!(!is_enabled_at(&test.0, &command));
    }

    #[test]
    fn disabling_is_idempotent_and_preserves_other_startup_values() {
        let test = TestKey::new();
        set_enabled_at(&test.0, false, &[]).unwrap();
        assert!(open_key(&test.0, KEY_QUERY_VALUE, false).unwrap().is_none());
        let command = quoted_command(Path::new(r"C:\app\widget.exe"));
        set_enabled_at(&test.0, true, &command).unwrap();
        let key = open_key(&test.0, KEY_QUERY_VALUE | KEY_SET_VALUE, false)
            .unwrap()
            .unwrap();
        for name in ["ClaudeCodeUsageMonitor", "OtherApplication"] {
            write_string(&key, name, &wide_str("keep this value")).unwrap();
        }
        set_enabled_at(&test.0, false, &[]).unwrap();
        set_enabled_at(&test.0, false, &[]).unwrap();
        assert!(!is_enabled_at(&test.0, &command));
        assert!(read_string(&key, VALUE_NAME).unwrap().is_none());
        for name in ["ClaudeCodeUsageMonitor", "OtherApplication"] {
            assert_eq!(
                read_string(&key, name).unwrap(),
                Some(wide_str("keep this value"))
            );
        }
    }
}
