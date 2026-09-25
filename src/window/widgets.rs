//! UI-thread-owned windows. Polling, tray icons and application timers belong to the hidden owner.
use super::*;

#[derive(Clone)]
pub(super) struct WidgetState {
    pub hwnd: SendHwnd,
    pub monitor_id: Option<String>,
    pub temporary: bool,
    pub taskbar: Option<native_interop::TaskbarWindow>,
    pub tray: Option<HWND>,
    pub dpi: u32,
    pub offset_dip: i32,
    drag: Option<Drag>,
}

#[derive(Clone, Copy)]
struct Drag {
    cursor: POINT,
    origin: POINT,
    offset_px: i32,
}

pub(super) fn widget_snapshots() -> Vec<WidgetState> {
    lock_state()
        .as_ref()
        .map(|s| s.widgets.clone())
        .unwrap_or_default()
}

pub(super) unsafe fn register_widget_class(instance: windows::Win32::Foundation::HMODULE) {
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(widget_proc),
        hInstance: HINSTANCE(instance.0),
        hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
        lpszClassName: windows::core::w!("CCUMWidget"),
        ..Default::default()
    };
    RegisterClassExW(&class);
}

fn create_widget(
    monitor: Option<&Monitor>,
    temporary: bool,
    offset_dip: i32,
) -> Option<WidgetState> {
    let taskbar = monitor.and_then(|m| m.taskbar);
    let instance = unsafe { GetModuleHandleW(PCWSTR::null()).ok()? };
    let dpi = monitor.map_or(96, |m| m.dpi);
    let rect = monitor.map_or(RECT::default(), |m| m.rect);
    let hwnd = match unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            windows::core::w!("CCUMWidget"),
            windows::core::w!("Claude Code Usage Monitor"),
            WS_POPUP,
            rect.left,
            rect.top,
            total_widget_width(dpi),
            sc(dpi, WIDGET_HEIGHT),
            HWND::default(),
            HMENU::default(),
            instance,
            None,
        )
    } {
        Ok(hwnd) => hwnd,
        Err(error) => {
            diagnose::log_error("unable to create widget", error);
            return None;
        }
    };
    if let Some(bar) = taskbar {
        if let Err(error) = native_interop::embed_in_taskbar(hwnd, bar.hwnd) {
            diagnose::log_error("unable to attach widget", error);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return None;
        }
    } else {
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    diagnose::log(format!(
        "widget created hwnd={hwnd:?} monitor={:?} temporary={temporary} dpi={dpi} parent={:?}",
        monitor.map(|m| &m.id),
        unsafe { GetParent(hwnd).ok() }
    ));
    Some(WidgetState {
        hwnd: SendHwnd::from_hwnd(hwnd),
        monitor_id: monitor.map(|m| m.id.clone()),
        temporary,
        taskbar,
        tray: taskbar.and_then(|b| native_interop::find_child_window(b.hwnd, "TrayNotifyWnd")),
        dpi,
        offset_dip,
        drag: None,
    })
}

pub(super) fn reconcile_widgets() {
    let discovery = monitors::enumerate();
    let (available, settings, placement, visible, migrated) = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        match discovery {
            Ok(monitors) => s.monitors = monitors,
            Err(error) => diagnose::log_error("keeping last display topology", error),
        }
        let migrated =
            monitors::migrate_primary(&mut s.monitor_settings, &s.monitors, s.legacy_offset);
        if migrated {
            s.settings_version = 2;
        }
        for m in &s.monitors {
            if let Some(saved) = s.monitor_settings.iter_mut().find(|saved| saved.id == m.id) {
                saved.name = m.name.clone();
            } else if s.settings_version == 2 {
                s.monitor_settings.push(MonitorSetting {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    enabled: false,
                    offset_dip: 0,
                });
            }
        }
        (
            s.monitors.clone(),
            s.monitor_settings.clone(),
            s.widget_placement,
            s.widget_visible,
            migrated,
        )
    };
    if migrated {
        save_state_settings();
    }
    let desired: Vec<(Option<Monitor>, bool, i32)> = if placement == WidgetPlacement::Floating {
        vec![(None, false, 0)]
    } else {
        monitors::selected_taskbars(&settings, &available)
            .into_iter()
            .map(|(index, temporary)| {
                let m = &available[index];
                let offset = if temporary {
                    0
                } else {
                    settings
                        .iter()
                        .find(|s| s.id == m.id)
                        .map_or(0, |s| s.offset_dip)
                };
                (Some(m.clone()), temporary, offset)
            })
            .collect()
    };
    let existing = widget_snapshots();
    for widget in existing {
        let keep = unsafe { IsWindow(widget.hwnd.to_hwnd()).as_bool() }
            && desired.iter().any(|(m, temp, _)| {
                widget.monitor_id.as_deref() == m.as_ref().map(|m| m.id.as_str())
                    && widget.temporary == *temp
                    && widget.taskbar.map(|b| b.hwnd)
                        == m.as_ref().and_then(|m| m.taskbar.map(|b| b.hwnd))
            });
        if !keep {
            if let Some(s) = lock_state().as_mut() {
                s.widgets.retain(|w| w.hwnd.0 != widget.hwnd.0);
            }
            unsafe {
                if IsWindow(widget.hwnd.to_hwnd()).as_bool() {
                    let _ = DestroyWindow(widget.hwnd.to_hwnd());
                }
            }
        }
    }
    for (monitor, temporary, offset) in desired {
        let id = monitor.as_ref().map(|m| m.id.as_str());
        let existing = widget_snapshots()
            .into_iter()
            .find(|w| w.monitor_id.as_deref() == id && w.temporary == temporary);
        let (widget, created) = if let Some(mut widget) = existing {
            if let Some(m) = &monitor {
                widget.dpi = m.dpi;
                widget.taskbar = m.taskbar;
                widget.tray = m
                    .taskbar
                    .and_then(|b| native_interop::find_child_window(b.hwnd, "TrayNotifyWnd"));
                if !temporary && widget.drag.is_none() {
                    widget.offset_dip = offset;
                }
                if let Some(s) = lock_state().as_mut() {
                    if let Some(w) = s.widgets.iter_mut().find(|w| w.hwnd.0 == widget.hwnd.0) {
                        *w = widget.clone();
                    }
                }
            }
            (widget, false)
        } else {
            let Some(widget) = create_widget(monitor.as_ref(), temporary, offset) else {
                continue;
            };
            if let Some(s) = lock_state().as_mut() {
                s.widgets.push(widget.clone());
            }
            (widget, true)
        };
        if created {
            position_window(&widget);
        } else {
            queue_position(widget.hwnd.to_hwnd());
        }
        unsafe {
            let _ = ShowWindow(
                widget.hwnd.to_hwnd(),
                if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
            );
        }
        if created {
            render_widget(&widget);
        }
    }
}

pub(super) fn destroy_widgets() {
    let widgets = lock_state()
        .as_mut()
        .map(|s| std::mem::take(&mut s.widgets))
        .unwrap_or_default();
    for widget in widgets {
        unsafe {
            let _ = DestroyWindow(widget.hwnd.to_hwnd());
        }
    }
}

fn queue_position(hwnd: HWND) {
    unsafe {
        SetTimer(hwnd, TIMER_TRAY_REPOSITION, 80, None);
    }
}

/// Read/position only after the trailing timer, including watchdog observations.
pub(super) unsafe extern "system" fn on_tray_location_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    for widget in widget_snapshots() {
        if widget.tray == Some(hwnd) || widget.taskbar.is_some_and(|bar| bar.hwnd == hwnd) {
            queue_position(widget.hwnd.to_hwnd());
        }
    }
}

fn taskbar_position(widget: &WidgetState, width: i32, height: i32) -> Option<(i32, i32)> {
    let bar = widget.taskbar?;
    let rect = native_interop::get_taskbar_rect(bar.hwnd)?;
    let tray_left = tray_left_for_taskbar(bar.hwnd, rect, width);
    let offset = clamped_offset(widget.offset_dip, widget.dpi, tray_left - rect.left - width);
    Some((
        tray_left - rect.left - width - offset,
        compute_anchor_y(0, rect.bottom - rect.top, height),
    ))
}

fn clamped_offset(saved_dip: i32, dpi: u32, available: i32) -> i32 {
    sc(dpi, saved_dip).clamp(0, available.max(0))
}

fn position_window(widget: &WidgetState) {
    if widget.drag.is_some() {
        return;
    }
    let hwnd = widget.hwnd.to_hwnd();
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    if let Some(s) = lock_state().as_mut() {
        if let Some(w) = s.widgets.iter_mut().find(|w| w.hwnd.0 == widget.hwnd.0) {
            w.dpi = dpi;
        }
    }
    let width = total_widget_width(dpi);
    let height = sc(dpi, WIDGET_HEIGHT);
    if widget.taskbar.is_some() {
        let mut current = widget.clone();
        current.dpi = dpi;
        if let Some((x, y)) = taskbar_position(&current, width, height) {
            let actual = native_interop::get_window_rect_safe(hwnd);
            if actual.is_none_or(|rect| {
                widget.taskbar.is_some_and(|bar| {
                    rect.left != bar.rect.left + x
                        || rect.top != bar.rect.top + y
                        || rect.right - rect.left != width
                        || rect.bottom - rect.top != height
                })
            }) {
                diagnose::log(format!("widget position hwnd={hwnd:?} x={x} y={y} width={width} height={height} dpi={dpi}"));
            }
            native_interop::move_window(hwnd, x, y, width, height);
        }
    } else {
        let (x, y) = lock_state()
            .as_ref()
            .map(|s| (s.floating_x, s.floating_y))
            .unwrap_or_default();
        let monitor = unsafe {
            MonitorFromPoint(
                POINT {
                    x: x.unwrap_or(0),
                    y: y.unwrap_or(0),
                },
                MONITOR_DEFAULTTOPRIMARY,
            )
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info).as_bool() } {
            let (x, y) = resolve_floating_position(info.rcWork, width, height, x, y, sc(dpi, 16));
            native_interop::move_window(hwnd, x, y, width, height);
            if let Some(s) = lock_state().as_mut() {
                s.floating_x = Some(x);
                s.floating_y = Some(y);
            }
        }
    }
}

fn finish_drag(hwnd: HWND) {
    let changed = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        let Some(widget) = s.widgets.iter_mut().find(|w| w.hwnd.to_hwnd() == hwnd) else {
            return;
        };
        if widget.drag.take().is_none() {
            return;
        }
        if !widget.temporary {
            if let Some(saved) = s
                .monitor_settings
                .iter_mut()
                .find(|m| Some(&m.id) == widget.monitor_id.as_ref())
            {
                saved.offset_dip = widget.offset_dip;
            }
        }
        !widget.temporary
    };
    if changed {
        save_state_settings();
    }
    queue_position(hwnd);
}

unsafe extern "system" fn widget_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let floating = widget_snapshots()
                .iter()
                .any(|w| w.hwnd.to_hwnd() == hwnd && w.taskbar.is_none());
            if floating {
                paint(dc, hwnd);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER if wparam.0 == TIMER_TRAY_REPOSITION => {
            let _ = KillTimer(hwnd, TIMER_TRAY_REPOSITION);
            if let Some(widget) = widget_snapshots()
                .into_iter()
                .find(|w| w.hwnd.to_hwnd() == hwnd)
            {
                position_window(&widget);
                if let Some(current) = widget_snapshots()
                    .into_iter()
                    .find(|w| w.hwnd.to_hwnd() == hwnd)
                {
                    render_widget(&current);
                }
            }
            LRESULT(0)
        }
        WM_DPICHANGED_MSG => {
            if let Some(s) = lock_state().as_mut() {
                if let Some(w) = s.widgets.iter_mut().find(|w| w.hwnd.to_hwnd() == hwnd) {
                    w.dpi = (wparam.0 as u32 & 0xffff).max(96);
                }
            }
            queue_position(hwnd);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let _ = ScreenToClient(hwnd, &mut point);
            let dpi = GetDpiForWindow(hwnd).max(96);
            if is_drag_handle_point(dpi, point.x, point.y) {
                let _ =
                    SetCursor(LoadCursorW(HINSTANCE::default(), IDC_SIZEALL).unwrap_or_default());
                LRESULT(1)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_LBUTTONDOWN => {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let mut client = point;
            let _ = ScreenToClient(hwnd, &mut client);
            let dpi = GetDpiForWindow(hwnd).max(96);
            if is_drag_handle_point(dpi, client.x, client.y) {
                if let Some(rect) = native_interop::get_window_rect_safe(hwnd) {
                    let width = total_widget_width(dpi);
                    let mut state = lock_state();
                    if let Some(w) = state
                        .as_mut()
                        .and_then(|s| s.widgets.iter_mut().find(|w| w.hwnd.to_hwnd() == hwnd))
                    {
                        let offset = w
                            .taskbar
                            .map(|bar| {
                                let bounds =
                                    native_interop::get_taskbar_rect(bar.hwnd).unwrap_or(bar.rect);
                                tray_left_for_taskbar(bar.hwnd, bounds, width) - rect.right
                            })
                            .unwrap_or(0);
                        w.drag = Some(Drag {
                            cursor: point,
                            origin: POINT {
                                x: rect.left,
                                y: rect.top,
                            },
                            offset_px: offset,
                        });
                    }
                    drop(state);
                    SetCapture(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let current = widget_snapshots()
                .into_iter()
                .find(|w| w.hwnd.to_hwnd() == hwnd);
            if let Some(mut widget) = current {
                if let Some(drag) = widget.drag {
                    let mut point = POINT::default();
                    let _ = GetCursorPos(&mut point);
                    let dpi = GetDpiForWindow(hwnd).max(96);
                    widget.dpi = dpi;
                    let width = total_widget_width(dpi);
                    let height = sc(dpi, WIDGET_HEIGHT);
                    if let Some(bar) = widget.taskbar {
                        if let Some(bounds) = native_interop::get_taskbar_rect(bar.hwnd) {
                            let end = tray_left_for_taskbar(bar.hwnd, bounds, width);
                            let offset = (drag.offset_px + drag.cursor.x - point.x)
                                .clamp(0, (end - bounds.left - width).max(0));
                            widget.offset_dip = monitors::unscale(offset, dpi);
                            if let Some(s) = lock_state().as_mut() {
                                if let Some(w) =
                                    s.widgets.iter_mut().find(|w| w.hwnd.to_hwnd() == hwnd)
                                {
                                    w.offset_dip = widget.offset_dip;
                                }
                            }
                            native_interop::move_window(
                                hwnd,
                                end - bounds.left - width - offset,
                                compute_anchor_y(0, bounds.bottom - bounds.top, height),
                                width,
                                height,
                            );
                        }
                    } else {
                        let x = drag.origin.x + point.x - drag.cursor.x;
                        let y = drag.origin.y + point.y - drag.cursor.y;
                        if let Some(s) = lock_state().as_mut() {
                            s.floating_x = Some(x);
                            s.floating_y = Some(y);
                        }
                        native_interop::move_window(hwnd, x, y, width, height);
                    }
                    render_widget(&widget);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            finish_drag(hwnd);
            let _ = ReleaseCapture();
            LRESULT(0)
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            finish_drag(hwnd);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            show_context_menu(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            // Explorer may destroy this child; the hidden owner and polling survive.
            let owner = lock_state()
                .as_ref()
                .filter(|s| s.widgets.iter().any(|w| w.hwnd.to_hwnd() == hwnd))
                .map(|s| s.hwnd);
            if let Some(owner) = owner {
                let _ = PostMessageW(owner.to_hwnd(), WM_APP_RECONCILE, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

pub(super) unsafe fn append_monitor_menu(parent: HMENU, language: LanguageId) {
    let strings = language.monitor_strings();
    let menu = match CreatePopupMenu() {
        Ok(menu) => menu,
        Err(_) => return,
    };
    let entries = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        let mut entries = s.monitor_settings.clone();
        entries.sort_by_key(|entry| {
            s.monitors
                .iter()
                .position(|m| m.id == entry.id)
                .unwrap_or(usize::MAX)
        });
        s.menu_monitors = entries.iter().map(|entry| entry.id.clone()).collect();
        let count = entries.iter().filter(|e| e.enabled).count();
        entries
            .into_iter()
            .map(|entry| {
                let monitor = s.monitors.iter().find(|m| m.id == entry.id);
                let label = match monitor {
                    Some(m) => format!(
                        "{} · {}×{}{}{}",
                        m.name,
                        m.rect.right - m.rect.left,
                        m.rect.bottom - m.rect.top,
                        if m.primary {
                            format!(" · {}", strings.primary)
                        } else {
                            String::new()
                        },
                        if m.taskbar.is_none() {
                            format!(" · {}", strings.no_taskbar)
                        } else {
                            String::new()
                        }
                    ),
                    None => format!("{} · {}", entry.name, strings.disconnected),
                };
                let disabled = (entry.enabled && count <= 1)
                    || (!entry.enabled && monitor.is_none_or(|m| m.taskbar.is_none()));
                (label, entry.enabled, disabled)
            })
            .collect::<Vec<_>>()
    };
    for (i, (label, checked, disabled)) in entries.into_iter().enumerate() {
        let text = native_interop::wide_str(&label);
        let mut flags = MENU_ITEM_FLAGS(0);
        if checked {
            flags |= MF_CHECKED;
        }
        if disabled {
            flags |= MF_GRAYED;
        }
        let _ = AppendMenuW(
            menu,
            flags,
            IDM_MONITOR_BASE as usize + i,
            PCWSTR(text.as_ptr()),
        );
    }
    let text = native_interop::wide_str(strings.monitors);
    let _ = AppendMenuW(parent, MF_POPUP, menu.0 as usize, PCWSTR(text.as_ptr()));
}

pub(super) fn toggle_monitor(command: u16) {
    let changed = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        let Some(id) = s
            .menu_monitors
            .get((command - IDM_MONITOR_BASE) as usize)
            .cloned()
        else {
            return;
        };
        monitors::toggle(&mut s.monitor_settings, &id)
    };
    if changed {
        save_state_settings();
        reconcile_widgets();
    }
}

pub(super) fn reset_positions() {
    {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        let target = s
            .menu_target
            .and_then(|target| s.widgets.iter().find(|w| w.hwnd.0 == target.0))
            .cloned();
        if let Some(target) = target {
            if let Some(w) = s.widgets.iter_mut().find(|w| w.hwnd.0 == target.hwnd.0) {
                w.offset_dip = 0;
            }
            if let Some(id) = target.monitor_id {
                if !target.temporary {
                    if let Some(m) = s.monitor_settings.iter_mut().find(|m| m.id == id) {
                        m.offset_dip = 0;
                    }
                }
            } else {
                s.floating_x = None;
                s.floating_y = None;
            }
        } else if s.menu_target.is_none() {
            for m in &mut s.monitor_settings {
                m.offset_dip = 0;
            }
            for w in &mut s.widgets {
                w.offset_dip = 0;
            }
            s.floating_x = None;
            s.floating_y = None;
        }
    }
    save_state_settings();
    reconcile_widgets();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_owned_child_can_be_recreated_without_destroying_controller() {
        // Exercise real Win32 parenting without touching Explorer, user settings or provider APIs.
        unsafe {
            let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
            register_widget_class(instance);
            let make_host = || {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    windows::core::w!("STATIC"),
                    PCWSTR::null(),
                    WS_POPUP,
                    0,
                    0,
                    1920,
                    48,
                    HWND::default(),
                    HMENU::default(),
                    instance,
                    None,
                )
                .unwrap()
            };
            let controller = make_host();
            let first_host = make_host();
            let mut monitor = Monitor {
                id: "test-device".into(),
                name: "Test".into(),
                primary: true,
                dpi: 96,
                rect: RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                taskbar: Some(native_interop::TaskbarWindow {
                    hwnd: first_host,
                    rect: RECT::default(),
                }),
            };
            let first = create_widget(Some(&monitor), false, 40).unwrap();
            assert_eq!(GetParent(first.hwnd.to_hwnd()).unwrap(), first_host);
            assert_eq!(SetTimer(controller, TIMER_POLL, 60_000, None), TIMER_POLL);
            DestroyWindow(first_host).unwrap();
            assert!(!IsWindow(first.hwnd.to_hwnd()).as_bool());
            assert!(
                IsWindow(controller).as_bool(),
                "losing Explorer's child must not lose the polling owner"
            );
            let replacement_host = make_host();
            monitor.taskbar.as_mut().unwrap().hwnd = replacement_host;
            let replacement = create_widget(Some(&monitor), false, 40).unwrap();
            assert_eq!(
                GetParent(replacement.hwnd.to_hwnd()).unwrap(),
                replacement_host
            );
            assert_eq!(replacement.offset_dip, 40);
            DestroyWindow(replacement_host).unwrap();
            let _ = KillTimer(controller, TIMER_POLL);
            DestroyWindow(controller).unwrap();
        }
    }
    #[test]
    fn temporary_tray_expansion_does_not_destroy_saved_offset() {
        let saved = 80;
        assert_eq!(clamped_offset(saved, 144, 30), 30);
        assert_eq!(clamped_offset(saved, 144, 300), 120);
        assert_eq!(clamped_offset(saved, 96, -20), 0);
    }
    #[test]
    fn layouts_use_their_own_dpi_without_cross_window_state() {
        let normal = total_widget_width_for(96, 2);
        let scaled = total_widget_width_for(144, 2);
        assert!(scaled > normal);
        assert_eq!(total_widget_width_for(96, 2), normal);
        assert_eq!(sc(192, WIDGET_HEIGHT), sc(96, WIDGET_HEIGHT) * 2);
    }
}
