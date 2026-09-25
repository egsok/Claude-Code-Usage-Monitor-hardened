//! Display identity and pure selection rules. Never persist an enumeration index.
use serde::{Deserialize, Serialize};
use windows::core::BOOL;
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use crate::native_interop::{self, TaskbarWindow};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorSetting {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub offset_dip: i32,
}

#[derive(Clone, Debug)]
pub struct Monitor {
    pub id: String,
    pub name: String,
    pub rect: RECT,
    pub primary: bool,
    pub dpi: u32,
    pub taskbar: Option<TaskbarWindow>,
}

// HWNDs here are immutable topology identifiers. Only the UI thread creates,
// positions, or destroys windows; other readers may inspect copied geometry.
unsafe impl Send for Monitor {}

pub fn scale(px: i32, dpi: u32) -> i32 {
    (px as f64 * dpi as f64 / 96.0).round() as i32
}

pub fn unscale(px: i32, dpi: u32) -> i32 {
    (px as f64 * 96.0 / dpi.max(1) as f64).round() as i32
}

pub fn migrate_primary(
    settings: &mut Vec<MonitorSetting>,
    monitors: &[Monitor],
    offset: i32,
) -> bool {
    if !settings.is_empty() {
        return false;
    }
    let Some(monitor) = monitors.iter().find(|m| m.primary) else {
        return false;
    };
    settings.push(MonitorSetting {
        id: monitor.id.clone(),
        name: monitor.name.clone(),
        enabled: true,
        offset_dip: unscale(offset.max(0), monitor.dpi),
    });
    true
}

/// A fallback is runtime-only; it must never replace the user's selection.
pub fn selected_taskbars(settings: &[MonitorSetting], monitors: &[Monitor]) -> Vec<(usize, bool)> {
    let selected: Vec<_> = monitors
        .iter()
        .enumerate()
        .filter(|(_, m)| m.taskbar.is_some() && settings.iter().any(|s| s.enabled && s.id == m.id))
        .map(|(i, _)| (i, false))
        .collect();
    if !selected.is_empty() {
        return selected;
    }
    monitors
        .iter()
        .position(|m| m.primary && m.taskbar.is_some())
        .map(|i| vec![(i, true)])
        .unwrap_or_default()
}

pub fn toggle(settings: &mut [MonitorSetting], id: &str) -> bool {
    let enabled = settings.iter().filter(|s| s.enabled).count();
    let Some(setting) = settings.iter_mut().find(|s| s.id == id) else {
        return false;
    };
    if setting.enabled && enabled <= 1 {
        return false;
    }
    setting.enabled = !setting.enabled;
    true
}

fn text(wide: &[u16]) -> String {
    String::from_utf16_lossy(&wide[..wide.iter().position(|c| *c == 0).unwrap_or(wide.len())])
}

/// Read CCD device paths and join them to GDI monitors through the current source name.
/// A topology race is retried a bounded number of times; callers keep their last good snapshot.
pub fn enumerate() -> Result<Vec<Monitor>, String> {
    unsafe {
        let flags = QDC_ONLY_ACTIVE_PATHS | QDC_VIRTUAL_MODE_AWARE;
        let mut paths = Vec::new();
        let mut complete = false;
        for _ in 0..3 {
            let (mut path_count, mut mode_count) = (0, 0);
            let result = GetDisplayConfigBufferSizes(flags, &mut path_count, &mut mode_count);
            if result != ERROR_SUCCESS {
                return Err(format!("GetDisplayConfigBufferSizes: {result:?}"));
            }
            paths.resize(path_count as usize, DISPLAYCONFIG_PATH_INFO::default());
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
            let result = QueryDisplayConfig(
                flags,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            );
            if result == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            if result != ERROR_SUCCESS {
                return Err(format!("QueryDisplayConfig: {result:?}"));
            }
            paths.truncate(path_count as usize);
            complete = true;
            break;
        }
        if !complete {
            return Err("Display topology kept changing".into());
        }

        let mut names = Vec::new();
        for path in paths {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of_val(&source) as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            };
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: std::mem::size_of_val(&target) as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            };
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0
                || DisplayConfigGetDeviceInfo(&mut target.header) != 0
            {
                return Err("Unable to read display device identity".into());
            }
            let id = text(&target.monitorDevicePath).to_lowercase();
            if id.is_empty() {
                return Err("Display has no device path".into());
            }
            names.push((
                text(&source.viewGdiDeviceName),
                id,
                text(&target.monitorFriendlyDeviceName),
            ));
        }
        // Clone-mode targets share one logical desktop and must get only one window.
        names.sort();
        names.dedup_by(|a, b| a.0 == b.0);
        let mut gdi: Vec<(HMONITOR, MONITORINFOEXW)> = Vec::new();
        unsafe extern "system" fn callback(
            monitor: HMONITOR,
            _: HDC,
            _: *mut RECT,
            data: LPARAM,
        ) -> BOOL {
            let list = &mut *(data.0 as *mut Vec<(HMONITOR, MONITORINFOEXW)>);
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = std::mem::size_of_val(&info) as u32;
            if GetMonitorInfoW(monitor, &mut info.monitorInfo).as_bool() {
                list.push((monitor, info));
            }
            BOOL(1)
        }
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM(&mut gdi as *mut _ as isize),
        );
        let taskbars = native_interop::find_taskbars();
        let mut result = Vec::new();
        for (handle, info) in gdi {
            let source = text(&info.szDevice);
            let Some((_, id, name)) = names.iter().find(|n| n.0 == source) else {
                return Err(format!("No CCD identity for {source}"));
            };
            let (mut x, mut y) = (96, 96);
            let _ = GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut x, &mut y);
            result.push(Monitor {
                id: id.clone(),
                name: if name.is_empty() {
                    source.clone()
                } else {
                    format!("{} ({})", name, source.trim_start_matches(r"\\.\"))
                },
                rect: info.monitorInfo.rcMonitor,
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                dpi: x.max(96),
                taskbar: taskbars
                    .iter()
                    .find(|bar| MonitorFromWindow(bar.hwnd, MONITOR_DEFAULTTONULL) == handle)
                    .copied(),
            });
        }
        result.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn monitor(id: &str, primary: bool) -> Monitor {
        Monitor {
            id: id.into(),
            name: id.into(),
            primary,
            dpi: 144,
            rect: RECT::default(),
            taskbar: Some(TaskbarWindow {
                hwnd: HWND::default(),
                rect: RECT::default(),
            }),
        }
    }
    fn chosen(id: &str, offset: i32) -> MonitorSetting {
        MonitorSetting {
            id: id.into(),
            name: id.into(),
            enabled: true,
            offset_dip: offset,
        }
    }
    #[test]
    fn reordering_and_primary_changes_do_not_move_selected_widget() {
        let settings = vec![chosen("right", 55)];
        let a = vec![monitor("left", true), monitor("right", false)];
        let b = vec![monitor("right", true), monitor("left", false)];
        assert_eq!(a[selected_taskbars(&settings, &a)[0].0].id, "right");
        assert_eq!(b[selected_taskbars(&settings, &b)[0].0].id, "right");
    }
    #[test]
    fn unplug_fallback_and_reconnect_preserve_independent_positions() {
        let settings = vec![chosen("left", 10), chosen("right", 80)];
        let saved = settings.clone();
        assert_eq!(
            selected_taskbars(&settings, &[monitor("main", true)]),
            vec![(0, true)]
        );
        assert_eq!(
            selected_taskbars(&settings, &[monitor("main", true), monitor("right", false)]),
            vec![(1, false)]
        );
        assert_eq!(
            selected_taskbars(&settings, &[monitor("right", false), monitor("left", true)]),
            vec![(0, false), (1, false)]
        );
        assert_eq!(settings, saved);
    }
    #[test]
    fn migration_chooses_primary_not_old_coordinate_order_and_scales_offset() {
        let mut settings = Vec::new();
        assert!(migrate_primary(
            &mut settings,
            &[monitor("left", false), monitor("main", true)],
            60
        ));
        assert_eq!(settings, vec![chosen("main", 40)]);
        assert!(!migrate_primary(
            &mut settings,
            &[monitor("left", true)],
            10
        ));
    }
    #[test]
    fn last_selection_cannot_be_removed_and_disabling_retains_position() {
        let mut settings = vec![chosen("a", 12), chosen("b", 45)];
        assert!(toggle(&mut settings, "a"));
        assert!(!toggle(&mut settings, "b"));
        assert!(toggle(&mut settings, "a"));
        assert_eq!(settings[0].offset_dip, 12);
    }
    #[test]
    fn a_missing_taskbar_is_not_a_place_to_create_a_widget() {
        let mut m = monitor("main", true);
        m.taskbar = None;
        assert!(selected_taskbars(&[chosen("main", 0)], &[m]).is_empty());
    }
}
