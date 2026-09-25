//! Selected-monitor copies of one authored taskbar root. The controller never becomes a shell child.
use super::*;

#[derive(Clone)]
pub(super) struct ManagedWindow {
    pub hwnd: SendHwnd,
    pub surface_index: usize,
    pub monitor_id: Option<String>,
    pub temporary: bool,
    pub dpi: u32,
    pub display_index: usize,
    pub offset_dip: i32,
    parent: Option<SendHwnd>,
    auto_ejected: bool,
    drag: Option<Drag>,
}

#[derive(Clone)]
struct Drag {
    cursor: POINT,
    origin: POINT,
    offset: i32,
    moved: bool,
}

pub(super) fn primary_surface(theme: &ThemeDocument) -> Option<usize> {
    theme.surfaces.iter().position(|surface| {
        surface
            .placement
            .nest
            .resolve(surface.placement.reference.region)
            == SurfaceNest::Taskbar
    })
}

pub(super) fn primary_index(state: &AppState) -> Option<usize> {
    state.active_theme.as_ref().and_then(primary_surface)
}

pub(super) fn is_managed_index(index: usize) -> bool {
    lock_state().as_ref().and_then(primary_index) == Some(index)
}

pub(super) unsafe fn create_primary_surface() -> HWND {
    let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
    let class = windows::core::w!("CCUMHardenedUpstream2PrimarySurface");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(primary_proc),
        hInstance: HINSTANCE(instance.0),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        lpszClassName: class,
        ..Default::default()
    };
    RegisterClassExW(&wc);
    CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
        class,
        windows::core::w!("Hardened v2 experimental surface"),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        None,
        Some(HINSTANCE(instance.0)),
        None,
    )
    .unwrap_or_default()
}

unsafe extern "system" fn primary_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            crate::desktop_compositor::remove(hwnd);
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_COMMAND => {
            let owner = lock_state().as_ref().map(|s| s.hwnd.to_hwnd());
            if let Some(owner) = owner {
                let _ = PostMessageW(Some(owner), msg, w, l);
            }
            LRESULT(0)
        }
        WM_NCHITTEST | WM_PAINT | WM_ERASEBKGND | WM_SETCURSOR | WM_LBUTTONDOWN | WM_MOUSEMOVE
        | WM_MOUSELEAVE | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONUP | WM_CAPTURECHANGED => {
            wnd_proc(hwnd, msg, w, l)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED_MSG | WM_SETTINGCHANGE => {
            let owner = lock_state().as_ref().map(|s| s.hwnd.to_hwnd());
            if let Some(owner) = owner {
                let _ = PostMessageW(
                    Some(owner),
                    native_interop::WM_APP_TRAY_REPOSITION,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}

pub(super) fn refresh_topology() {
    let discovered = match monitors::enumerate() {
        Ok(monitors) => monitors,
        Err(error) => {
            diagnose::log(format!("keeping previous monitor topology: {error}"));
            return;
        }
    };
    let changed = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        s.monitors = discovered;
        let old = s.monitor_settings.clone();
        monitors::migrate_primary(&mut s.monitor_settings, &s.monitors, s.tray_offset);
        for monitor in &s.monitors {
            if let Some(saved) = s.monitor_settings.iter_mut().find(|m| m.id == monitor.id) {
                saved.name = monitor.name.clone();
            } else {
                s.monitor_settings.push(MonitorSetting {
                    id: monitor.id.clone(),
                    name: monitor.name.clone(),
                    enabled: false,
                    offset_dip: 0,
                });
            }
        }
        old != s.monitor_settings
    };
    if changed {
        save_state_settings();
    }
}

fn window_alive(hwnd: HWND, parent: Option<SendHwnd>) -> bool {
    unsafe {
        IsWindow(Some(hwnd)).as_bool()
            && parent.is_none_or(|parent| {
                IsWindow(Some(parent.to_hwnd())).as_bool()
                    && GetParent(hwnd).ok() == Some(parent.to_hwnd())
            })
    }
}

pub(super) fn recover_theme_windows() {
    let (primary, mirrors) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };
        (s.surface_hwnd, s.mirror_hwnds.clone())
    };
    if !window_alive(primary.to_hwnd(), None) {
        let replacement = unsafe { create_primary_surface() };
        if let Some(s) = lock_state().as_mut() {
            s.surface_hwnd = SendHwnd::from_hwnd(replacement);
        }
        diagnose::log(format!(
            "primary themed surface recovered hwnd={replacement:?}; controller retained"
        ));
    }
    for (index, window) in mirrors.iter().enumerate() {
        if !window_alive(window.to_hwnd(), None) {
            let replacement = unsafe { create_mirror_window() };
            if let Some(slot) = lock_state()
                .as_mut()
                .and_then(|s| s.mirror_hwnds.get_mut(index))
            {
                *slot = SendHwnd::from_hwnd(replacement);
            }
        }
    }
}

pub(super) fn reconcile() {
    let displays = native_interop::find_monitors();
    let scales = displays
        .iter()
        .copied()
        .map(monitor_scale)
        .collect::<Vec<_>>();
    let (desired, old) = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        let desired = if let Some(surface_index) = primary_index(s) {
            if s.managed_placement
                .as_ref()
                .is_some_and(|p| p.nest == "floating")
            {
                let display_index = s
                    .managed_placement
                    .as_ref()
                    .and_then(|p| {
                        displays.iter().position(|d| {
                            p.screen_x >= d.rect.left
                                && p.screen_x < d.rect.right
                                && p.screen_y >= d.rect.top
                                && p.screen_y < d.rect.bottom
                        })
                    })
                    .unwrap_or(0);
                let dpi = (scales.get(display_index).copied().unwrap_or(1.0) * 96.0).round() as u32;
                vec![ManagedWindow {
                    hwnd: SendHwnd(0),
                    surface_index,
                    monitor_id: None,
                    temporary: false,
                    dpi,
                    display_index,
                    offset_dip: 0,
                    parent: None,
                    auto_ejected: false,
                    drag: None,
                }]
            } else {
                monitors::selected_taskbars(&s.monitor_settings, &s.monitors)
                    .into_iter()
                    .filter_map(|(index, temporary)| {
                        let m = &s.monitors[index];
                        let offset = if temporary {
                            0
                        } else {
                            s.monitor_settings
                                .iter()
                                .find(|saved| saved.id == m.id)
                                .map_or(0, |saved| saved.offset_dip)
                        };
                        // Both discovery functions use primary, left, top ordering, but match geometry
                        // instead of assuming an enumeration index is stable between the two snapshots.
                        let display_index = displays.iter().position(|d| d.rect == m.rect)?;
                        Some(ManagedWindow {
                            hwnd: SendHwnd(0),
                            surface_index,
                            monitor_id: Some(m.id.clone()),
                            temporary,
                            dpi: m.dpi,
                            display_index,
                            offset_dip: offset,
                            parent: m.taskbar.map(|bar| SendHwnd::from_hwnd(bar.hwnd)),
                            auto_ejected: false,
                            drag: None,
                        })
                    })
                    .collect()
            }
        } else {
            Vec::new()
        };
        (desired, std::mem::take(&mut s.managed_windows))
    };
    let mut old = old;
    let mut result = Vec::new();
    for mut copy in desired {
        if copy
            .parent
            .is_some_and(|parent| unsafe { !IsWindow(Some(parent.to_hwnd())).as_bool() })
        {
            continue;
        }
        if let Some(index) = old.iter().position(|window| {
            window.monitor_id == copy.monitor_id
                && window.surface_index == copy.surface_index
                && window.temporary == copy.temporary
                && window.parent == copy.parent
                && window_alive(
                    window.hwnd.to_hwnd(),
                    if window.auto_ejected {
                        None
                    } else {
                        window.parent
                    },
                )
        }) {
            let previous = old.remove(index);
            copy.hwnd = previous.hwnd;
            copy.drag = previous.drag;
            copy.auto_ejected = previous.auto_ejected;
        } else {
            let window = unsafe { create_mirror_window() };
            if window.is_invalid() {
                continue;
            }
            copy.hwnd = SendHwnd::from_hwnd(window);
            diagnose::log(format!(
                "managed copy created hwnd={window:?} monitor={:?} dpi={} temporary={} surface={}",
                copy.monitor_id, copy.dpi, copy.temporary, copy.surface_index
            ));
        }
        result.push(copy);
    }
    if let Some(s) = lock_state().as_mut() {
        s.managed_windows = result;
    }
    for copy in old {
        diagnose::log(format!(
            "managed copy retired hwnd={:?} monitor={:?}",
            copy.hwnd.to_hwnd(),
            copy.monitor_id
        ));
        unsafe {
            let _ = DestroyWindow(copy.hwnd.to_hwnd());
        }
    }
}

pub(super) fn copy_theme(theme: &ThemeDocument, copy: &ManagedWindow) -> ThemeDocument {
    let mut view = theme.clone();
    let surface = &mut view.surfaces[copy.surface_index];
    if copy.monitor_id.is_some() {
        let horizontal = taskbar_is_horizontal(copy.display_index);
        surface.placement =
            positioning::dock_placement(copy.display_index, copy.offset_dip, 1.0, horizontal);
        surface.placement.clamp_taskbar_drag = true;
    }
    if copy.surface_index == 0 {
        view.placement = surface.placement.clone();
    }
    view
}

pub(super) fn render(theme: &ThemeDocument, data: Option<&AppUsageData>, runtime: ThemeRuntime) {
    let (copies, visible, auto_eject) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };
        (
            s.managed_windows.clone(),
            s.managed_visible,
            s.taskbar_auto_eject,
        )
    };
    for copy in copies {
        let hwnd = copy.hwnd.to_hwnd();
        if copy.drag.as_ref().is_some_and(|d| d.moved) {
            continue;
        }
        let view = copy_theme(theme, &copy);
        let index = copy.surface_index;
        let runtime = theme_runtime_for_surface(&view, index, runtime);
        let scale = if copy.monitor_id.is_some() {
            copy.dpi as f64 / 96.0
        } else {
            theme_surface_scale(&view, index)
        };
        if !visible || !theme_engine::surface_should_render(&view, index, data, runtime) {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            continue;
        }
        let rendered = theme_engine::render_theme_surface_with_runtime_at_scale(
            &view, index, data, runtime, scale,
        );
        let mut positioned = theme_for_surface(&view, index);
        let (width, height) = theme_engine::resolve_surface_size(&view, index, data, runtime);
        positioned.canvas.width = width;
        positioned.canvas.height = height;
        let offsets = theme_engine::resolve_surface_placement(&view, index, data, runtime);
        positioned.placement.offset_x = offsets.offset_x;
        positioned.placement.offset_y = offsets.offset_y;
        let ejected = position_around_collisions(&copy, &positioned, scale, auto_eject);
        if !ejected {
            position_custom_theme(hwnd, &positioned, scale);
        }
        render_custom_window(hwnd, &rendered, false);
        let show = (copy.monitor_id.is_some() && !ejected)
            || !foreground_is_fullscreen_on_display(positioned.placement.reference.display);
        unsafe {
            let _ = ShowWindow(hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }
}

/// Shell occupancy changes affect only runtime placement, never the saved DIP offset.
fn position_around_collisions(
    copy: &ManagedWindow,
    positioned: &ThemeDocument,
    scale: f64,
    auto_eject: bool,
) -> bool {
    let Some(parent) = copy.parent else {
        return false;
    };
    let Some(bar) = native_interop::get_taskbar_rect(parent.to_hwnd()) else {
        return false;
    };
    let displays = native_interop::find_monitors();
    let Some(display) = displays.get(copy.display_index) else {
        return false;
    };
    let tray = native_interop::find_child_window(parent.to_hwnd(), "TrayNotifyWnd")
        .and_then(native_interop::get_window_rect_safe);
    let width = scaled_theme_dimension(positioned.canvas.width, scale);
    let height = scaled_theme_dimension(positioned.canvas.height, scale);
    let dock = positioning::surface_screen_rect(
        &positioned.placement,
        width,
        height,
        scale,
        display.rect,
        Some(bar),
        tray,
    );
    let ejected = taskbar_collision::should_eject(
        auto_eject,
        copy.auto_ejected,
        taskbar_collision::cached(parent.to_hwnd(), bar).as_ref(),
        dock,
        monitors::scale(20, copy.dpi),
    );
    if ejected != copy.auto_ejected {
        if let Some(s) = lock_state().as_mut() {
            if let Some(current) = s
                .managed_windows
                .iter_mut()
                .find(|current| current.hwnd == copy.hwnd)
            {
                current.auto_ejected = ejected;
            }
        }
        diagnose::log(format!(
            "managed copy collision hwnd={:?} floating={ejected}",
            copy.hwnd.to_hwnd()
        ));
    }
    if !ejected {
        return false;
    }
    let mut point = positioning::auto_eject_origin(dock, bar, display.rect);
    point.x = point.x.clamp(
        display.rect.left,
        (display.rect.right - width).max(display.rect.left),
    );
    point.y = point.y.clamp(
        display.rect.top,
        (display.rect.bottom - height).max(display.rect.top),
    );
    native_interop::make_popup(copy.hwnd.to_hwnd(), true);
    unsafe {
        let _ = SetWindowPos(
            copy.hwnd.to_hwnd(),
            Some(HWND_TOPMOST),
            point.x,
            point.y,
            width,
            height,
            SWP_NOACTIVATE,
        );
    }
    true
}

pub(super) fn sync_visibility() {
    let snapshot = {
        let state = lock_state();
        state.as_ref().and_then(|s| {
            effective_theme_from_state(s).map(|theme| {
                (
                    theme,
                    s.data.clone(),
                    theme_runtime_from_state(s),
                    s.managed_windows.clone(),
                    s.managed_visible,
                )
            })
        })
    };
    let Some((theme, data, runtime, copies, visible)) = snapshot else {
        return;
    };
    for copy in copies {
        let view = copy_theme(&theme, &copy);
        let runtime = theme_runtime_for_surface(&view, copy.surface_index, runtime);
        let show = visible
            && theme_engine::surface_should_render(
                &view,
                copy.surface_index,
                data.as_ref(),
                runtime,
            )
            && ((copy.monitor_id.is_some() && !copy.auto_ejected)
                || !foreground_is_fullscreen_on_display(
                    view.surfaces[copy.surface_index]
                        .placement
                        .reference
                        .display,
                ));
        unsafe {
            let _ = ShowWindow(
                copy.hwnd.to_hwnd(),
                if show { SW_SHOWNOACTIVATE } else { SW_HIDE },
            );
        }
    }
}

pub(super) fn destroy_surfaces() {
    let windows = {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        let mut windows = std::mem::take(&mut s.managed_windows)
            .into_iter()
            .map(|copy| copy.hwnd)
            .collect::<Vec<_>>();
        windows.push(s.surface_hwnd);
        windows.extend(std::mem::take(&mut s.mirror_hwnds));
        windows
    };
    for window in windows {
        unsafe {
            let _ = DestroyWindow(window.to_hwnd());
        }
    }
}

/// Handle drag locally. No copy can overwrite another device's persisted position.
pub(super) unsafe fn window_message(
    hwnd: HWND,
    msg: u32,
    _w: WPARAM,
    l: LPARAM,
) -> Option<LRESULT> {
    let copy = lock_state()
        .as_ref()?
        .managed_windows
        .iter()
        .find(|copy| copy.hwnd.to_hwnd() == hwnd)
        .cloned()?;
    match msg {
        WM_LBUTTONDOWN => {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let rect = native_interop::get_window_rect_safe(hwnd).unwrap_or_default();
            if let Some(s) = lock_state().as_mut() {
                if let Some(copy) = s
                    .managed_windows
                    .iter_mut()
                    .find(|copy| copy.hwnd.to_hwnd() == hwnd)
                {
                    copy.drag = Some(Drag {
                        cursor,
                        origin: POINT {
                            x: rect.left,
                            y: rect.top,
                        },
                        offset: copy.offset_dip,
                        moved: false,
                    });
                }
            }
            SetCapture(hwnd);
        }
        WM_MOUSEMOVE if copy.drag.is_some() => {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let drag = copy.drag.as_ref().unwrap();
            let dx = cursor.x - drag.cursor.x;
            let dy = cursor.y - drag.cursor.y;
            if drag.moved
                || dx.abs() >= GetSystemMetrics(SM_CXDRAG)
                || dy.abs() >= GetSystemMetrics(SM_CYDRAG)
            {
                let mut point = [POINT {
                    x: drag.origin.x + dx,
                    y: drag.origin.y + dy,
                }];
                if let Some(parent) = copy.parent.filter(|_| !copy.auto_ejected) {
                    MapWindowPoints(None, Some(parent.to_hwnd()), &mut point);
                    if taskbar_is_horizontal(copy.display_index) {
                        point[0].y -= dy;
                    } else {
                        point[0].x -= dx;
                    }
                }
                if let Some(s) = lock_state().as_mut() {
                    if let Some(copy) = s
                        .managed_windows
                        .iter_mut()
                        .find(|copy| copy.hwnd.to_hwnd() == hwnd)
                    {
                        if let Some(drag) = copy.drag.as_mut() {
                            drag.moved = true;
                        }
                    }
                }
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    point[0].x,
                    point[0].y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        WM_LBUTTONUP if copy.drag.is_some() => {
            let drag = copy.drag.unwrap();
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let rect = native_interop::get_window_rect_safe(hwnd).unwrap_or_default();
            let horizontal = taskbar_is_horizontal(copy.display_index);
            {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    if let Some(current) = s
                        .managed_windows
                        .iter_mut()
                        .find(|copy| copy.hwnd.to_hwnd() == hwnd)
                    {
                        current.drag = None;
                    }
                    if drag.moved {
                        if let Some(id) = &copy.monitor_id {
                            if !copy.temporary {
                                let delta = if horizontal {
                                    cursor.x - drag.cursor.x
                                } else {
                                    cursor.y - drag.cursor.y
                                };
                                if let Some(saved) =
                                    s.monitor_settings.iter_mut().find(|saved| &saved.id == id)
                                {
                                    saved.offset_dip =
                                        (drag.offset - monitors::unscale(delta, copy.dpi)).max(0);
                                }
                            }
                        } else if let Some(placement) = s.managed_placement.as_mut() {
                            placement.screen_x = rect.left;
                            placement.screen_y = rect.top;
                        }
                    }
                }
            }
            let _ = ReleaseCapture();
            if drag.moved {
                save_state_settings();
                render_layered();
            } else if let Some((surface, object)) = mouse_target_at(hwnd, l) {
                schedule_or_dispatch_click(hwnd, surface, object);
            }
        }
        WM_CAPTURECHANGED => {
            if let Some(s) = lock_state().as_mut() {
                if let Some(copy) = s
                    .managed_windows
                    .iter_mut()
                    .find(|copy| copy.hwnd.to_hwnd() == hwnd)
                {
                    copy.drag = None;
                }
            }
        }
        WM_RBUTTONUP => {
            if let Some((surface, object)) = mouse_target_at(hwnd, l) {
                if dispatch_mouse_event_at(Some(hwnd), surface, &object, MouseEventKind::RightClick)
                {
                    return Some(LRESULT(0));
                }
            }
            let id = lock_state()
                .as_ref()
                .and_then(|s| s.active_theme.as_ref())
                .and_then(|t| t.surfaces.get(copy.surface_index))
                .map(|s| s.id.clone());
            if let Some(id) = id {
                show_context_menu_document(hwnd, None, Some((copy.surface_index, id)));
            }
        }
        WM_DPICHANGED_MSG => {
            let owner = lock_state().as_ref().map(|s| s.hwnd.to_hwnd());
            if let Some(owner) = owner {
                let _ = PostMessageW(
                    Some(owner),
                    native_interop::WM_APP_TRAY_REPOSITION,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        _ => return None,
    }
    Some(LRESULT(0))
}

const MENU_MONITOR_FIRST: usize = 40_000;
const MENU_RESET: usize = 39_990;
const MENU_FLOATING: usize = 39_991;
const MENU_VISIBLE: usize = 39_992;

pub(super) unsafe fn append_menu(menu: HMENU) -> Vec<String> {
    let (settings, enabled, floating, visible, language) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return Vec::new();
        };
        (
            s.monitor_settings.clone(),
            primary_index(s).is_some(),
            s.managed_placement
                .as_ref()
                .is_some_and(|p| p.nest == "floating"),
            s.managed_visible,
            s.language,
        )
    };
    let Ok(submenu) = CreatePopupMenu() else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for setting in settings {
        let label = native_interop::wide_str(&setting.name);
        let flags = if setting.enabled {
            MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(
            submenu,
            flags,
            MENU_MONITOR_FIRST + ids.len(),
            PCWSTR::from_raw(label.as_ptr()),
        );
        ids.push(setting.id);
    }
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let label = native_interop::wide_str(language.text(if enabled {
        "Monitors"
    } else {
        "Monitors: theme has no taskbar surface"
    }));
    let _ = AppendMenuW(
        menu,
        MF_POPUP | if enabled { MF_STRING } else { MF_GRAYED },
        submenu.0 as usize,
        PCWSTR(label.as_ptr()),
    );
    if enabled {
        let label = native_interop::wide_str(language.text("Floating"));
        let _ = AppendMenuW(
            menu,
            if floating { MF_CHECKED } else { MF_STRING },
            MENU_FLOATING,
            PCWSTR(label.as_ptr()),
        );
        let label = native_interop::wide_str(language.text("Show widget"));
        let _ = AppendMenuW(
            menu,
            if visible { MF_CHECKED } else { MF_STRING },
            MENU_VISIBLE,
            PCWSTR(label.as_ptr()),
        );
        let label = native_interop::wide_str(language.text("Reset position"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_RESET, PCWSTR(label.as_ptr()));
    }
    ids
}

pub(super) fn menu_command(hwnd: HWND, id: usize, ids: &[String]) -> bool {
    if !matches!(id, MENU_RESET | MENU_FLOATING | MENU_VISIBLE)
        && !(MENU_MONITOR_FIRST..MENU_MONITOR_FIRST + ids.len()).contains(&id)
    {
        return false;
    }
    let first_window = lock_state()
        .as_ref()
        .and_then(|s| s.managed_windows.first().map(|c| c.hwnd.to_hwnd()));
    let first_rect = first_window
        .and_then(native_interop::get_window_rect_safe)
        .unwrap_or_default();
    let reset_origin = native_interop::find_monitors()
        .first()
        .map(|display| POINT {
            x: display.rect.left + 64,
            y: display.rect.top + 64,
        })
        .unwrap_or_default();
    {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return true;
        };
        match id {
            MENU_RESET => {
                let copy_id = s
                    .managed_windows
                    .iter()
                    .find(|c| c.hwnd.to_hwnd() == hwnd)
                    .map(|c| (c.monitor_id.clone(), c.temporary));
                for saved in &mut s.monitor_settings {
                    if copy_id.is_none()
                        || copy_id.as_ref().is_some_and(|(id, temporary)| {
                            !temporary && id.as_ref() == Some(&saved.id)
                        })
                    {
                        saved.offset_dip = 0;
                    }
                }
                if copy_id.is_none() || copy_id.as_ref().is_some_and(|(id, _)| id.is_none()) {
                    if let Some(placement) = s.managed_placement.as_mut() {
                        placement.screen_x = reset_origin.x;
                        placement.screen_y = reset_origin.y;
                    }
                }
            }
            MENU_VISIBLE => s.managed_visible = !s.managed_visible,
            MENU_FLOATING => {
                if let Some(placement) = s.managed_placement.as_mut() {
                    placement.nest = if placement.nest == "floating" {
                        "taskbar"
                    } else {
                        "floating"
                    }
                    .into();
                } else {
                    let floating_host = primary_index(s)
                        .and_then(|index| s.active_theme.as_ref().map(|t| (index, t)))
                        .and_then(|(index, t)| {
                            let mut view = theme_for_surface(t, index);
                            view.surfaces = vec![t.surfaces[index].clone()];
                            floating_host_for_theme(&view, None)
                        });
                    s.managed_placement = Some(PlacementOverride {
                        nest: "floating".into(),
                        monitor_index: 0,
                        screen_x: first_rect.left,
                        screen_y: first_rect.top.saturating_sub(80),
                        tray_offset: 0,
                        floating_host,
                    });
                }
            }
            _ => {
                if let Some(id) = ids.get(id - MENU_MONITOR_FIRST) {
                    monitors::toggle(&mut s.monitor_settings, id);
                }
            }
        }
    }
    save_state_settings();
    render_layered();
    true
}

pub(super) fn toggle_visible() -> bool {
    if lock_state().as_ref().and_then(primary_index).is_none() {
        return false;
    }
    menu_command(HWND::default(), MENU_VISIBLE, &[])
}

pub(super) fn reset_position(hwnd: HWND) {
    menu_command(hwnd, MENU_RESET, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_selection_ignores_extra_floating_and_tray_roots() {
        let mut theme = ThemeDocument::starter();
        let taskbar = theme.surfaces[0].clone();
        theme.surfaces[0].placement.nest = SurfaceNest::Floating;
        theme.surfaces.insert(1, taskbar);
        assert_eq!(primary_surface(&theme), Some(1));
        theme.surfaces[1].placement.nest = SurfaceNest::TrayIcon;
        assert_eq!(primary_surface(&theme), None);
    }

    #[test]
    fn copies_keep_independent_dip_offsets_and_leave_extra_theme_roots_unchanged() {
        let mut theme = ThemeDocument::starter();
        let extra = theme.surfaces[0].clone();
        theme.surfaces.push(extra);
        theme.surfaces.last_mut().unwrap().placement.nest = SurfaceNest::Floating;
        let authored = serde_json::to_value(&theme).unwrap();
        for (display_index, dpi, offset) in [(0, 144, 37), (1, 96, 91)] {
            let copy = ManagedWindow {
                hwnd: SendHwnd(0),
                surface_index: 0,
                monitor_id: Some(format!("device-{display_index}")),
                temporary: false,
                dpi,
                display_index,
                offset_dip: offset,
                parent: None,
                auto_ejected: false,
                drag: None,
            };
            let runtime = copy_theme(&theme, &copy);
            assert_eq!(
                runtime.surfaces[0].placement.reference.display,
                display_index
            );
            assert_eq!(runtime.surfaces[0].placement.offset_x, -offset);
            assert_eq!(
                copy.offset_dip, offset,
                "runtime position never overwrites persisted DIP units"
            );
            assert_eq!(
                serde_json::to_value(runtime.surfaces.last().unwrap()).unwrap(),
                serde_json::to_value(theme.surfaces.last().unwrap()).unwrap()
            );
        }
        assert_eq!(serde_json::to_value(&theme).unwrap(), authored);
    }

    #[test]
    fn huge_saved_position_is_only_clamped_at_runtime_for_current_dpi() {
        let mut placement = positioning::dock_placement(0, 9000, 1.0, true);
        placement.clamp_taskbar_drag = true;
        let bar = RECT {
            left: -1920,
            top: 1040,
            right: 0,
            bottom: 1080,
        };
        for scale in [1.0, 1.5] {
            let rectangle =
                positioning::surface_screen_rect(&placement, 300, 40, scale, bar, Some(bar), None);
            assert_eq!(rectangle.left, -1920);
            assert_eq!(
                placement.offset_x, -9000,
                "reconnecting a larger monitor must recover its original offset"
            );
        }
    }

    #[test]
    fn a_position_save_preserves_settings_changed_by_studio_before_reload() {
        let observed = SettingsFile::default();
        let mut on_disk = observed.clone();
        on_disk.poll_interval_ms = POLL_1_MIN;
        on_disk.language = Some("ru".into());
        on_disk.taskbar_auto_eject = false;
        on_disk.active_theme_path = Some("new-theme.json".into());
        let mut desired = observed.clone();
        desired.monitors = vec![MonitorSetting {
            id: "device-A".into(),
            name: "A".into(),
            enabled: true,
            offset_dip: 42,
        }];
        merge_settings_edits(&mut on_disk, &observed, &desired);
        assert_eq!(on_disk.poll_interval_ms, POLL_1_MIN);
        assert_eq!(on_disk.language.as_deref(), Some("ru"));
        assert!(!on_disk.taskbar_auto_eject);
        assert_eq!(on_disk.active_theme_path.as_deref(), Some("new-theme.json"));
        assert_eq!(on_disk.monitors, desired.monitors);
    }

    #[test]
    fn shell_child_loss_does_not_destroy_controller() {
        unsafe {
            let controller = CreateWindowExW(
                Default::default(),
                windows::core::w!("STATIC"),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let parent = CreateWindowExW(
                Default::default(),
                windows::core::w!("STATIC"),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            // Exercise the production procedure: its destruction must not quit
            // the controller's message loop when Explorer destroys the host.
            let child = create_primary_surface();
            native_interop::embed_as_child(child, parent);
            let mirror = create_mirror_window();
            native_interop::embed_as_child(mirror, parent);
            DestroyWindow(parent).unwrap();
            assert!(!window_alive(child, Some(SendHwnd::from_hwnd(parent))));
            assert!(!window_alive(mirror, Some(SendHwnd::from_hwnd(parent))));
            assert!(window_alive(controller, None));
            let mut message = MSG::default();
            assert!(!PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool());
            let replacement = create_primary_surface();
            assert!(window_alive(replacement, None));
            DestroyWindow(replacement).unwrap();
            DestroyWindow(controller).unwrap();
        }
    }
}
