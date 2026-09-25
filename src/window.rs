use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app_settings::{
    self, load_settings, LegacyPlacement, PlacementOverride, SettingsFile, POLL_15_MIN,
    POLL_15_MIN_SECONDS, POLL_1_HOUR, POLL_1_HOUR_SECONDS, POLL_1_MIN, POLL_1_MIN_SECONDS,
    POLL_5_MIN, POLL_5_MIN_SECONDS,
};
use crate::context_menu::{self, ContextMenuAction, ContextMenuItem, ContextMenuItemKind};
use crate::diagnose;
use crate::monitors::{self, Monitor, MonitorSetting};
mod managed;
use crate::localization::{self, LanguageId, Strings};
use crate::models::AppUsageData;
use crate::native_interop::{
    self, TIMER_CLOCK, TIMER_COUNTDOWN, TIMER_MOUSE_CLICK, TIMER_POLL, TIMER_RESET_POLL,
    TIMER_TRAY_HOVER, TIMER_TRAY_REPOSITION, TIMER_UPDATE_CHECK, TIMER_WINDOW_STATE,
    WM_APP_DISABLE_DIAGNOSTICS, WM_APP_ENABLE_DIAGNOSTICS, WM_APP_OPEN_DASHBOARD, WM_APP_QUIT,
    WM_APP_REFRESH_NOW, WM_APP_SETTINGS_UPDATED, WM_APP_TASKBAR_COLLISION, WM_APP_TRAY,
    WM_APP_USAGE_UPDATED,
};
use crate::poller;
use crate::providers::{ProviderId, ProviderSet};
use crate::theme;
use crate::theme_engine::{
    self, Canvas, DataContext, HorizontalAnchor, MouseActionEffect, MouseActionOverrideKey,
    MouseEventKind, ReferenceRegion, SurfaceNest, ThemeDocument, ThemeRuntime, VerticalAnchor,
};
use crate::tray_icon;
use crate::updater::{self, ReleaseDescriptor, UpdateCheckResult};

/// Copyable HWND value used by the watchdog after the UI thread publishes it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SendHwnd(isize);

// SAFETY: this wrapper never transfers ownership of a window. Cross-thread users
// only pass the value back to Win32 APIs that explicitly accept handles created
// by another thread (for example IsWindow and PostMessageW).
unsafe impl Send for SendHwnd {}

impl SendHwnd {
    fn from_hwnd(hwnd: HWND) -> Self {
        Self(hwnd.0 as isize)
    }
    fn to_hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

/// Copyable event-hook value whose lifetime remains owned by the UI controller.
#[derive(Clone, Copy)]
struct SendWinEventHook(isize);

// SAFETY: the hook is only stored or passed to UnhookWinEvent. Callback work is
// marshalled through Win32; Rust data is never dereferenced through this value.
unsafe impl Send for SendWinEventHook {}

impl SendWinEventHook {
    fn from_hook(hook: HWINEVENTHOOK) -> Self {
        Self(hook.0 as isize)
    }

    fn to_hook(self) -> HWINEVENTHOOK {
        HWINEVENTHOOK(self.0 as *mut _)
    }
}

/// Shared application state
struct AppState {
    hwnd: SendHwnd,
    surface_hwnd: SendHwnd,
    monitors: Vec<Monitor>,
    monitor_settings: Vec<MonitorSetting>,
    managed_windows: Vec<managed::ManagedWindow>,
    managed_visible: bool,
    managed_placement: Option<PlacementOverride>,
    observed_settings: SettingsFile,
    taskbar_hwnd: Option<SendHwnd>,
    tray_notify_hwnd: Option<SendHwnd>,
    win_event_hook: Option<SendWinEventHook>,
    is_dark: bool,
    embedded: bool,
    language_override: Option<LanguageId>,
    language: LanguageId,

    providers: ProviderSet,
    accounts: crate::accounts::AccountSettings,

    data: Option<AppUsageData>,

    poll_interval_ms: u32,
    polling_enabled: bool,
    retry_count: u32,
    force_notify_auth_error: bool,
    auth_error_paused_polling: bool,
    auth_watch_mode: poller::CredentialWatchMode,
    auth_watch_snapshot: poller::CredentialWatchSnapshot,
    last_poll_ok: bool,
    last_poll_failure: Option<poller::PollFailure>,
    update_status: UpdateStatus,
    last_update_check_unix: Option<u64>,

    taskbar_index: usize,
    tray_offset: i32,
    dragging: bool,
    pending_drag: bool,
    drag_start_cursor: POINT,
    drag_start_origin: POINT,
    drag_start_client_x: i32,
    auto_ejected: bool,
    taskbar_auto_eject: bool,
    auto_ejected_origin: Option<POINT>,
    auto_ejected_host: Option<app_settings::FloatingHost>,
    is_switching_window_style: bool,
    is_snapped: bool,
    placement_override: Option<PlacementOverride>,
    floating_card_opacity: Option<u8>,
    window_state_timer_active: bool,

    custom_theme_enabled: bool,
    usage_countdown: bool,
    active_theme_path: Option<PathBuf>,
    active_theme: Option<ThemeDocument>,
    theme_clock_interval: Option<Duration>,
    tray_theme_uses_current_time: bool,
    mirror_hwnds: Vec<SendHwnd>,
    desktop_hwnds: Vec<Option<SendHwnd>>,
    mouse_action_overrides: HashMap<MouseActionOverrideKey, theme_engine::Expression>,
    hovered_mouse_layer: Option<(usize, String)>,
    pending_mouse_click: Option<PendingMouseClick>,
    suppress_next_left_up: bool,
}

#[derive(Clone, Debug)]
struct PendingMouseClick {
    surface_index: usize,
    object_id: String,
}

#[derive(Clone, Debug)]
enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available(ReleaseDescriptor),
}

fn publish_update_status(state: &AppState) {
    use crate::dashboard::UpdateStatus as DashboardStatus;
    let status = match &state.update_status {
        UpdateStatus::Idle | UpdateStatus::UpToDate => DashboardStatus::Idle,
        UpdateStatus::Checking => DashboardStatus::Checking,
        UpdateStatus::Available(release) => {
            DashboardStatus::Available(release.latest_version.clone())
        }
    };
    crate::dashboard::publish_update_status(state.hwnd.to_hwnd(), status);
}

fn perform_update_action(hwnd: HWND) {
    let release = lock_state()
        .as_ref()
        .and_then(|state| match &state.update_status {
            UpdateStatus::Available(release) => Some(release.clone()),
            _ => None,
        });
    if let Some(release) = release {
        open_web_url(
            hwnd,
            &release.release_url,
            "release page could not be opened",
        );
    } else {
        begin_update_check(hwnd, true);
    }
}

const RETRY_BASE_MS: u32 = 30_000; // 30 seconds

// Menu item IDs for update frequency
const IDM_FREQ_1MIN: u16 = 10;
const IDM_FREQ_5MIN: u16 = 11;
const IDM_FREQ_15MIN: u16 = 12;
const IDM_FREQ_1HOUR: u16 = 13;
const IDM_START_WITH_WINDOWS: u16 = 20;
const IDM_VERSION_ACTION: u16 = 31;
const IDM_LANG_SYSTEM: u16 = 100;
const IDM_LANG_FIRST: u16 = 101;
const IDM_DASHBOARD: u16 = 71;

const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_APP_UPDATE_CHECK_COMPLETE: u32 = WM_APP + 2;
const TRAY_ICON_UPDATE_REPOSITION_SUPPRESS_MS: u64 = 750;
const WINDOW_STATE_INTERVAL_MS: u32 = 250;

fn language_menu_command_id(language: LanguageId) -> u16 {
    IDM_LANG_FIRST
        .checked_add(u16::try_from(language.index()).expect("language index exceeds u16"))
        .expect("language menu command id exceeds u16")
}

fn language_from_menu_command_id(command: u16) -> Option<LanguageId> {
    command
        .checked_sub(IDM_LANG_FIRST)
        .and_then(|index| LanguageId::from_index(index.into()))
}

fn open_web_url(hwnd: HWND, url: &str, failure_message: &'static str) {
    if !native_interop::open_web_url(Some(hwnd), url) {
        diagnose::log(failure_message);
    }
}

static SUPPRESS_TRAY_REPOSITION_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// Current system DPI (96 = 100% scaling, 144 = 150%, 192 = 200%, etc.)
static CURRENT_DPI: AtomicU32 = AtomicU32::new(96);
static POLL_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static POLL_PENDING: AtomicBool = AtomicBool::new(false);

/// Re-query the monitor DPI for our window and update the cached value.
/// Uses GetDpiForWindow which returns the live DPI (unlike GetDpiForSystem
/// which is cached at process startup and never changes).
fn refresh_dpi() {
    let hwnd = {
        let state = lock_state();
        state.as_ref().map(|s| s.surface_hwnd.to_hwnd())
    };
    if let Some(hwnd) = hwnd {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi > 0 {
            CURRENT_DPI.store(dpi, Ordering::Relaxed);
        }
    }
}

fn display_scale(display_index: usize) -> f64 {
    let displays = native_interop::find_monitors();
    let Some(display) = displays
        .get(display_index)
        .copied()
        .or_else(|| displays.first().copied())
    else {
        return 1.0;
    };
    monitor_scale(display)
}

fn monitor_scale(display: native_interop::DisplayMonitor) -> f64 {
    let mut dpi_x = 96;
    let mut dpi_y = 96;
    if unsafe { GetDpiForMonitor(display.handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
        .is_ok()
        && dpi_x > 0
    {
        (dpi_x as f64 / 96.0).clamp(0.25, 8.0)
    } else {
        let system_dpi = unsafe { GetDpiForSystem() };
        if system_dpi > 0 {
            (system_dpi as f64 / 96.0).clamp(0.25, 8.0)
        } else {
            1.0
        }
    }
}

fn migrated_theme_placement(legacy: LegacyPlacement) -> (usize, i32) {
    let displays = native_interop::find_monitors();
    let taskbars = native_interop::find_taskbars();
    let display_index = taskbars
        .get(legacy.taskbar_index)
        .or_else(|| taskbars.first())
        .map(|taskbar| unsafe { MonitorFromWindow(taskbar.hwnd, MONITOR_DEFAULTTOPRIMARY) })
        .and_then(|monitor| {
            displays
                .iter()
                .position(|display| display.handle == monitor)
        })
        .unwrap_or_else(|| legacy.taskbar_index.min(displays.len().saturating_sub(1)));
    let offset_x = legacy_offset_to_theme_offset(legacy.tray_offset, display_scale(display_index));
    (display_index, offset_x)
}

fn legacy_offset_to_theme_offset(tray_offset: i32, scale: f64) -> i32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    -((tray_offset.max(0) as f64 / scale).round() as i32)
}

fn theme_surface_scale(theme: &ThemeDocument, surface_index: usize) -> f64 {
    let display_index = theme
        .surfaces
        .get(surface_index)
        .map(|surface| surface.placement.reference.display)
        .unwrap_or(theme.placement.reference.display);
    display_scale(display_index)
}

fn logical_host_dimension(physical: i32, scale: f64) -> u32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (physical.max(1) as f64 / scale)
        .round()
        .clamp(1.0, u32::MAX as f64) as u32
}

// The studio runs in a separate process without the monitor's layout cache.
pub(crate) fn query_theme_runtime_for_surface(
    theme: &ThemeDocument,
    surface_index: usize,
    runtime: ThemeRuntime,
) -> ThemeRuntime {
    let Some(surface) = theme.surfaces.get(surface_index) else {
        return runtime;
    };
    let displays = native_interop::find_monitors();
    let Some(display) = displays
        .get(surface.placement.reference.display)
        .copied()
        .or_else(|| displays.first().copied())
    else {
        return runtime;
    };
    let nest = surface
        .placement
        .nest
        .resolve(surface.placement.reference.region);
    let host_rect = if matches!(nest, SurfaceNest::Taskbar | SurfaceNest::TrayIcon) {
        native_interop::find_taskbars()
            .into_iter()
            .find(|taskbar| unsafe {
                MonitorFromWindow(taskbar.hwnd, MONITOR_DEFAULTTOPRIMARY) == display.handle
            })
            .map(|taskbar| taskbar.rect)
            .unwrap_or(display.rect)
    } else {
        display.rect
    };
    let scale = monitor_scale(display);
    runtime.with_host_dimensions(
        logical_host_dimension(host_rect.right - host_rect.left, scale),
        logical_host_dimension(host_rect.bottom - host_rect.top, scale),
    )
}

fn scaled_theme_dimension(logical: u32, scale: f64) -> i32 {
    (logical as f64 * scale).round().clamp(1.0, 8192.0) as i32
}

/// The unparented controller owns this timer, so shell child destruction cannot stop recovery.
const TIMER_TOPOLOGY: usize = 91;
fn spawn_taskbar_watchdog() {
    let owner = lock_state().as_ref().map(|s| s.hwnd.to_hwnd());
    if let Some(owner) = owner {
        unsafe {
            SetTimer(Some(owner), TIMER_TOPOLOGY, 2_000, None);
        }
    }
}

fn taskbar_collision_action(state: &AppState) -> Option<usize> {
    if managed::primary_index(state).is_some()
        || state.dragging
        || state.pending_drag
        || state.is_switching_window_style
    {
        return None;
    }
    let auto_eject = state.taskbar_auto_eject;
    if !auto_eject {
        return state.auto_ejected.then_some(0);
    }
    let taskbar = state.taskbar_hwnd?.to_hwnd();
    let bounds = native_interop::get_taskbar_rect(taskbar)?;
    let occupancy = taskbar_collision::cached(taskbar, bounds);
    if state.auto_ejected {
        let target = restored_dock_rect(state, taskbar, bounds)?;
        let margin = (20.0 * CURRENT_DPI.load(Ordering::Relaxed) as f64 / 96.0).round() as i32;
        (!taskbar_collision::should_eject(auto_eject, true, occupancy.as_ref(), target, margin))
            .then_some(0)
    } else if state.embedded
        && state
            .placement_override
            .as_ref()
            .is_none_or(|p| p.nest != "floating")
    {
        let widget = native_interop::get_window_rect_safe(state.surface_hwnd.to_hwnd())?;
        taskbar_collision::should_eject(auto_eject, false, occupancy.as_ref(), widget, 0)
            .then_some(1)
    } else {
        None
    }
}

static STATE: Mutex<Option<AppState>> = Mutex::new(None);

/// Lock STATE safely, recovering from poisoned mutex
fn lock_state() -> MutexGuard<'static, Option<AppState>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn theme_runtime_from_state(state: &AppState) -> ThemeRuntime {
    let (poll_ok, has_error) = poll_display_state(
        state.last_poll_ok,
        state.retry_count,
        state.auth_error_paused_polling,
        state.data.as_ref(),
    );
    let nest = if state.auto_ejected {
        SurfaceNest::Floating
    } else if let Some(ref ov) = state.placement_override {
        if ov.nest == "floating" {
            SurfaceNest::Floating
        } else {
            SurfaceNest::Taskbar
        }
    } else {
        SurfaceNest::Taskbar
    };
    let opacity = state.floating_card_opacity.unwrap_or(85);
    ThemeRuntime::from_providers(state.providers)
        .with_poll_state(poll_ok, has_error)
        .with_language(state.language)
        .with_countdown(state.usage_countdown)
        .with_nest(nest)
        .with_floating_card_opacity(opacity)
}

/// A transient outage can keep presenting the last real reading while its
/// retry runs. Authentication failures and failures without cached data still
/// need the explicit error state.
fn poll_display_state(
    last_poll_ok: bool,
    retry_count: u32,
    auth_error_paused_polling: bool,
    data: Option<&AppUsageData>,
) -> (bool, bool) {
    if let Some(data) = data.filter(|data| !data.accounts.is_empty()) {
        let has_error = data.accounts.iter().any(|account| account.error.is_some());
        return (
            !data.is_empty(),
            data.is_empty() && (has_error || retry_count > 0),
        );
    }
    let has_usable_stale_data = !auth_error_paused_polling
        && data.is_some_and(|data| data.iter().any(|(_, usage)| usage.stale));
    (
        last_poll_ok || has_usable_stale_data,
        retry_count > 0 && !has_usable_stale_data,
    )
}

fn effective_theme_from_state(state: &AppState) -> Option<ThemeDocument> {
    theme_with_placement(state, state.auto_ejected)
}

fn theme_with_placement(state: &AppState, auto_ejected: bool) -> Option<ThemeDocument> {
    let mut theme = state.active_theme.as_ref().map(|theme| {
        theme_engine::apply_mouse_action_overrides(theme, &state.mouse_action_overrides)
    })?;
    if let Some(index) = managed::primary_index(state) {
        if let Some(p) = state
            .managed_placement
            .as_ref()
            .filter(|p| p.nest == "floating")
        {
            let displays = native_interop::find_monitors();
            let point = POINT {
                x: p.screen_x,
                y: p.screen_y,
            };
            let (display_index, display) = positioning::monitor_for_point(&displays, point);
            let mut view = theme_for_surface(&theme, index);
            view.surfaces = vec![theme.surfaces[index].clone()];
            let mut placement = positioning::floating_placement(display_index);
            let host = floating_host_for_theme(&view, p.floating_host.as_ref());
            placement.host_dimensions = host.map(|host| (host.width, host.height));
            positioning::override_primary_placement(&mut view, placement.clone());
            let scale = monitor_scale(display);
            let runtime = theme_runtime_for_surface(&view, 0, theme_runtime_from_state(state));
            let frame = positioning::widget_frame(&view, state.data.as_ref(), runtime, scale);
            let offset = positioning::clamped_floating_offset(point, display.rect, &frame, scale);
            placement.offset_x = offset.x;
            placement.offset_y = offset.y;
            theme.surfaces[index].placement = placement.clone();
            if index == 0 {
                theme.placement = placement;
            }
        }
    }
    let floating = if auto_ejected {
        state.auto_ejected_origin.map(|point| (None, point))
    } else {
        state
            .placement_override
            .as_ref()
            .filter(|p| p.nest == "floating")
            .map(|p| {
                (
                    Some(p.monitor_index),
                    POINT {
                        x: p.screen_x,
                        y: p.screen_y,
                    },
                )
            })
    };
    if let Some((saved_monitor, point)) = floating {
        let displays = native_interop::find_monitors();
        let selected = saved_monitor
            .and_then(|index| displays.get(index).copied().map(|d| (index, d)))
            .unwrap_or_else(|| positioning::monitor_for_point(&displays, point));
        apply_floating_position(&mut theme, state, selected.0, selected.1, point);
    } else if let Some(p) = state
        .placement_override
        .as_ref()
        .filter(|p| p.nest == "taskbar")
    {
        let displays = native_interop::find_monitors();
        let index = if p.monitor_index < displays.len() {
            p.monitor_index
        } else {
            0
        };
        let horizontal = taskbar_is_horizontal(index);
        let placement = if p.screen_x > 0 || p.screen_y > 0 {
            let offset = if horizontal { p.screen_x } else { p.screen_y };
            positioning::taskbar_dock_placement(index, offset, display_scale(index), horizontal)
        } else {
            positioning::dock_placement(index, p.tray_offset, display_scale(index), horizontal)
        };
        positioning::override_primary_placement(&mut theme, placement);
    }
    Some(theme)
}

fn apply_floating_position(
    theme: &mut ThemeDocument,
    state: &AppState,
    index: usize,
    display: native_interop::DisplayMonitor,
    point: POINT,
) {
    let mut placement = positioning::floating_placement(index);
    let saved_host = if state.auto_ejected {
        state.auto_ejected_host.as_ref()
    } else {
        state
            .placement_override
            .as_ref()
            .and_then(|p| p.floating_host.as_ref())
    };
    placement.host_dimensions =
        floating_host_for_theme(theme, saved_host).map(|host| (host.width, host.height));
    positioning::override_primary_placement(theme, placement.clone());
    let scale = monitor_scale(display);
    let runtime = theme_runtime_for_surface(theme, 0, theme_runtime_from_state(state));
    let frame = positioning::widget_frame(theme, state.data.as_ref(), runtime, scale);
    let offset = positioning::clamped_floating_offset(point, display.rect, &frame, scale);
    placement.offset_x = offset.x;
    placement.offset_y = offset.y;
    positioning::override_primary_placement(theme, placement);
}

/// Resolve the actual rectangle that will be restored, including authored
/// placement or a saved drag offset. The watchdog must test this same target.
fn restored_dock_rect(state: &AppState, taskbar: HWND, taskbar_rect: RECT) -> Option<RECT> {
    let theme = theme_with_placement(state, false)?;
    let surface = theme.surfaces.first()?;
    if surface
        .placement
        .nest
        .resolve(surface.placement.reference.region)
        != SurfaceNest::Taskbar
    {
        return None;
    }
    let displays = native_interop::find_monitors();
    let display = displays
        .get(surface.placement.reference.display)
        .or_else(|| displays.first())?;
    if unsafe { MonitorFromWindow(taskbar, MONITOR_DEFAULTTOPRIMARY) } != display.handle {
        return None;
    }
    let runtime = theme_runtime_for_surface(&theme, 0, theme_runtime_from_state(state));
    let scale = monitor_scale(*display);
    let frame = positioning::widget_frame(&theme, state.data.as_ref(), runtime, scale);
    let offsets = theme_engine::resolve_surface_placement(&theme, 0, state.data.as_ref(), runtime);
    let mut placement = surface.placement.clone();
    placement.offset_x = offsets.offset_x;
    placement.offset_y = offsets.offset_y;
    let tray = native_interop::find_child_window(taskbar, "TrayNotifyWnd")
        .and_then(native_interop::get_window_rect_safe);
    Some(positioning::surface_screen_rect(
        &placement,
        frame.width,
        frame.height,
        scale,
        display.rect,
        Some(taskbar_rect),
        tray,
    ))
}

fn theme_has_floating_surface(theme: &ThemeDocument) -> bool {
    theme.surfaces.iter().any(|surface| {
        surface
            .placement
            .nest
            .resolve(surface.placement.reference.region)
            == SurfaceNest::Floating
    })
}

fn window_state_timer_required(state: &AppState) -> bool {
    state.custom_theme_enabled
        && effective_theme_from_state(state)
            .as_ref()
            .is_some_and(theme_has_floating_surface)
}

fn sync_window_state_timer(hwnd: HWND) {
    let required = lock_state()
        .as_ref()
        .is_some_and(window_state_timer_required);
    set_window_state_timer(hwnd, required);
}

fn set_window_state_timer(hwnd: HWND, required: bool) {
    {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        if required == state.window_state_timer_active {
            return;
        }
        state.window_state_timer_active = required;
    }
    unsafe {
        if required {
            SetTimer(
                Some(hwnd),
                TIMER_WINDOW_STATE,
                WINDOW_STATE_INTERVAL_MS,
                None,
            );
        } else {
            let _ = KillTimer(Some(hwnd), TIMER_WINDOW_STATE);
        }
    }
}

/// Merge only this process's edits, retaining unrelated changes made by Studio.
fn merge_settings_edits(
    current: &mut SettingsFile,
    observed: &SettingsFile,
    desired: &SettingsFile,
) {
    macro_rules! edited { ($($field:ident),* $(,)?) => { $(
        if observed.$field != desired.$field { current.$field = desired.$field.clone(); }
    )* }; }
    macro_rules! copied { ($($field:ident),* $(,)?) => { $(
        if observed.$field != desired.$field { current.$field = desired.$field; }
    )* }; }
    copied!(
        tray_offset,
        taskbar_index,
        legacy_placement_pending,
        widget_visible,
        legacy_visibility_pending,
        poll_interval_ms,
        last_update_check_unix,
        custom_theme_enabled,
        floating_card_opacity,
        monitor_widget_visible
    );
    edited!(
        language,
        active_theme_path,
        placement_override,
        monitors,
        monitor_placement
    );
    if observed.enabled_providers() != desired.enabled_providers() {
        current.set_enabled_providers(desired.enabled_providers());
    }
}

fn save_state_settings() {
    static SAVE: Mutex<()> = Mutex::new(());
    let _save = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    let mut snapshot = None;
    let result = app_settings::update_settings(|persisted| {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };
        let mut desired = s.observed_settings.clone();
        desired.tray_offset = s.tray_offset;
        desired.taskbar_index = s.taskbar_index;
        desired.legacy_placement_pending = false;
        desired.widget_visible = true;
        desired.legacy_visibility_pending = false;
        desired.poll_interval_ms = s.poll_interval_ms;
        desired.language = s
            .language_override
            .map(|language| language.code().to_string());
        desired.last_update_check_unix = s.last_update_check_unix;
        desired.set_enabled_providers(s.providers);
        desired.custom_theme_enabled = s.custom_theme_enabled;
        desired.active_theme_path = s
            .active_theme_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
        desired.placement_override = s.placement_override.clone();
        desired.floating_card_opacity = s.floating_card_opacity;
        desired.monitors = s.monitor_settings.clone();
        desired.monitor_widget_visible = s.managed_visible;
        desired.monitor_placement = s.managed_placement.clone();
        merge_settings_edits(persisted, &s.observed_settings, &desired);
        snapshot = Some(desired);
    });
    match result {
        Ok(_) => {
            if let (Some(state), Some(snapshot)) = (lock_state().as_mut(), snapshot) {
                state.observed_settings = snapshot;
            }
        }
        Err(error) => diagnose::log(format!("unable to save settings: {error}")),
    }
}

fn save_settings_or_log(settings: &SettingsFile, observed: &mut SettingsFile, context: &str) {
    match app_settings::update_settings(|current| merge_settings_edits(current, observed, settings))
    {
        Ok(_) => *observed = settings.clone(),
        Err(error) => diagnose::log(format!("{context}: {error}")),
    }
}

fn tray_usage_summary_lines(
    data: &AppUsageData,
    providers: ProviderSet,
    language: LanguageId,
    countdown: bool,
) -> Vec<String> {
    let strings = language.strings();
    let shown = |percentage: f64| {
        if countdown {
            100.0 - percentage
        } else {
            percentage
        }
    };
    providers
        .iter()
        .filter_map(|provider| {
            let usage = data.get(provider)?;
            let descriptor = provider.descriptor();
            let weekly_label = usage
                .weekly_label
                .as_deref()
                .unwrap_or(strings.weekly_window);
            Some(format!(
                "{} {}: {:.0}% | {}: {:.0}%",
                match data.selected_account_name(provider) {
                    Some(name) => format!("{} ({name})", language.text(descriptor.display_name)),
                    None => language.text(descriptor.display_name).to_string(),
                },
                strings.session_window,
                shown(usage.session.percentage),
                weekly_label,
                shown(usage.weekly.percentage),
            ))
        })
        .collect()
}

fn tray_usage_summary_from_state() -> Option<String> {
    let state = lock_state();
    let state = state.as_ref()?;
    let errors = tray_error_lines(
        state.data.as_ref(),
        state.last_poll_failure,
        state.providers,
        state.language,
    );
    if !errors.is_empty() {
        return Some(errors.join("\n"));
    }
    if !state.last_poll_ok {
        return None;
    }
    let lines = tray_usage_summary_lines(
        state.data.as_ref()?,
        state.providers,
        state.language,
        state.usage_countdown,
    );
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn tray_error_lines(
    data: Option<&AppUsageData>,
    failure: Option<poller::PollFailure>,
    providers: ProviderSet,
    language: LanguageId,
) -> Vec<String> {
    let mut accounts: Vec<_> = data
        .into_iter()
        .flat_map(|data| &data.accounts)
        .filter(|account| {
            providers.contains(account.provider)
                && account.profile.enabled
                && account.error.is_some()
        })
        .collect();
    // Windows truncates tray tooltips: put the selected account's error first.
    accounts.sort_by_key(|account| !account.selected);
    let mut lines: Vec<_> = accounts
        .into_iter()
        .map(|account| {
            format!(
                "{} ({}): {}",
                language.text(account.provider.descriptor().display_name),
                account.profile.name,
                account.error.unwrap().message(language),
            )
        })
        .collect();
    if let Some(failure) = failure.filter(|failure| providers.contains(failure.provider)) {
        if !data.is_some_and(|data| {
            data.accounts
                .iter()
                .any(|account| account.provider == failure.provider && account.error.is_some())
        }) {
            lines.push(format!(
                "{}: {}",
                language.text(failure.provider.descriptor().display_name),
                failure.error.message(language)
            ));
        }
    }
    lines
}

fn tray_icon_tooltip_from_state() -> String {
    tray_usage_summary_from_state().unwrap_or_else(|| {
        lock_state()
            .as_ref()
            .map(|state| state.language.strings().window_title.to_string())
            .unwrap_or_else(|| "Claude Code Usage Monitor".to_string())
    })
}

fn sync_tray_icon(hwnd: HWND) {
    let usage_tooltip = tray_usage_summary_from_state();
    let themed = {
        let state = lock_state();
        state.as_ref().and_then(|state| {
            effective_theme_from_state(state)
                .map(|theme| (theme, state.data.clone(), theme_runtime_from_state(state)))
        })
    };
    if let Some((theme, data, runtime)) = themed {
        let has_tray_surfaces = theme.surfaces.iter().any(|surface| {
            surface
                .placement
                .nest
                .resolve(surface.placement.reference.region)
                == SurfaceNest::TrayIcon
        });
        if has_tray_surfaces {
            let icons = theme
                .surfaces
                .iter()
                .enumerate()
                .filter(|(surface_index, surface)| {
                    let surface_runtime =
                        theme_runtime_for_surface(&theme, *surface_index, runtime);
                    surface
                        .placement
                        .nest
                        .resolve(surface.placement.reference.region)
                        == SurfaceNest::TrayIcon
                        && theme_engine::surface_should_render(
                            &theme,
                            *surface_index,
                            data.as_ref(),
                            surface_runtime,
                        )
                })
                .filter_map(|(surface_index, surface)| {
                    let surface_runtime = theme_runtime_for_surface(&theme, surface_index, runtime);
                    let (logical_width, logical_height) = theme_engine::resolve_surface_size(
                        &theme,
                        surface_index,
                        data.as_ref(),
                        surface_runtime,
                    );
                    let max_dimension = logical_width.max(logical_height) as f64;
                    let scale =
                        theme_surface_scale(&theme, surface_index).min(if max_dimension > 0.0 {
                            512.0 / max_dimension
                        } else {
                            1.0
                        });
                    if scale < 0.25 {
                        diagnose::log(format!(
                            "tray-icon theme surface '{}' exceeds the 512px source limit",
                            surface.name
                        ));
                        return None;
                    }
                    let rendered = theme_engine::render_theme_surface_with_runtime_at_scale(
                        &theme,
                        surface_index,
                        data.as_ref(),
                        surface_runtime,
                        scale,
                    );
                    Some(tray_icon::ThemedTrayIcon {
                        surface_index,
                        tooltip: usage_tooltip
                            .clone()
                            .unwrap_or_else(|| surface.name.clone()),
                        width: rendered.width,
                        height: rendered.height,
                        pixels: rendered.pixels,
                    })
                })
                .collect::<Vec<_>>();
            tray_icon::sync_themed(hwnd, &icons);
            return;
        }
    }
    tray_icon::sync(hwnd, &tray_icon_tooltip_from_state());
}

fn theme_tray_uses_current_time(theme: &ThemeDocument) -> bool {
    theme
        .surfaces
        .iter()
        .enumerate()
        .filter(|(_, surface)| {
            surface
                .placement
                .nest
                .resolve(surface.placement.reference.region)
                == SurfaceNest::TrayIcon
        })
        .any(|(surface_index, _)| {
            theme
                .surface_current_time_refresh_interval(surface_index)
                .is_some()
        })
}

fn taskbar_created_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    *MESSAGE.get_or_init(|| unsafe {
        let name = native_interop::wide_str("TaskbarCreated");
        RegisterWindowMessageW(PCWSTR::from_raw(name.as_ptr()))
    })
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn update_check_interval() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

fn auto_update_check_due(last_update_check_unix: Option<u64>) -> bool {
    let Some(last_update_check_unix) = last_update_check_unix else {
        return true;
    };

    now_unix_secs().saturating_sub(last_update_check_unix) >= update_check_interval().as_secs()
}

fn schedule_auto_update_check(hwnd: HWND) {
    let delay_ms = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };

        if auto_update_check_due(s.last_update_check_unix) {
            None
        } else {
            let elapsed = now_unix_secs().saturating_sub(s.last_update_check_unix.unwrap_or(0));
            let remaining_secs = update_check_interval().as_secs().saturating_sub(elapsed);
            Some((remaining_secs.saturating_mul(1000)).min(u32::MAX as u64) as u32)
        }
    };

    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_UPDATE_CHECK);
        if let Some(delay_ms) = delay_ms {
            SetTimer(Some(hwnd), TIMER_UPDATE_CHECK, delay_ms.max(1), None);
        }
    }
}

fn set_window_title(hwnd: HWND, strings: Strings) {
    unsafe {
        let title = native_interop::wide_str(strings.window_title);
        let _ = SetWindowTextW(hwnd, PCWSTR::from_raw(title.as_ptr()));
    }
}

fn show_info_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            Some(hwnd),
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn show_error_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            Some(hwnd),
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn apply_language_to_state(state: &mut AppState, language_override: Option<LanguageId>) {
    state.language_override = language_override;
    state.language = localization::resolve_language(language_override);
    set_window_title(state.hwnd.to_hwnd(), state.language.strings());
}

fn update_language_change() -> bool {
    let mut state = lock_state();
    let Some(app_state) = state.as_mut() else {
        return false;
    };

    if app_state.language_override.is_some() {
        return false;
    }

    let new_language = localization::detect_system_language();
    if new_language == app_state.language {
        return false;
    }

    apply_language_to_state(app_state, None);
    true
}

fn begin_update_check(hwnd: HWND, interactive: bool) {
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    let strings = {
        let mut state = lock_state();
        let Some(app_state) = state.as_mut() else {
            return;
        };

        if matches!(app_state.update_status, UpdateStatus::Checking) {
            if interactive {
                show_info_message(
                    hwnd,
                    app_state.language.strings().updates,
                    app_state.language.strings().update_in_progress,
                );
            }
            return;
        }

        app_state.update_status = UpdateStatus::Checking;
        publish_update_status(app_state);
        app_state.language.strings()
    };

    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        let checked_at = now_unix_secs();
        match updater::check_for_updates() {
            Ok(UpdateCheckResult::UpToDate) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::UpToDate;
                        s.last_update_check_unix = Some(checked_at);
                        publish_update_status(s);
                    }
                }
                save_state_settings();
                if interactive {
                    show_info_message(hwnd, strings.updates, strings.up_to_date);
                }
                unsafe {
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_APP_UPDATE_CHECK_COMPLETE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
            Ok(UpdateCheckResult::Available(release)) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Available(release.clone());
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive {
                    show_info_message(
                        hwnd,
                        strings.update_available,
                        &format!("{}\n\n{}", release.latest_version, release.release_url),
                    );
                }
                if let Some(state) = lock_state().as_ref() {
                    publish_update_status(state);
                }
                unsafe {
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_APP_UPDATE_CHECK_COMPLETE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
            Err(error) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Idle;
                        s.last_update_check_unix = Some(checked_at);
                        publish_update_status(s);
                    }
                }
                save_state_settings();
                if interactive {
                    let message = format!("{}.\n\n{}", strings.update_failed, error);
                    show_error_message(hwnd, strings.updates, &message);
                }
                unsafe {
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_APP_UPDATE_CHECK_COMPLETE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }
    });
}

// Experiments never inspect or modify the installed application's startup registration.
pub(crate) fn is_startup_enabled() -> bool {
    false
}
pub(crate) fn set_startup_enabled(_enable: bool) {
    diagnose::log("startup changes are disabled for the isolated experiment");
}

fn total_widget_width_for_state(state: &AppState) -> i32 {
    widget_frame_for_state(state, None).width
}

fn widget_frame_for_state(state: &AppState, nest: Option<SurfaceNest>) -> positioning::WidgetFrame {
    effective_theme_from_state(state).as_ref().map_or(
        positioning::WidgetFrame {
            width: 1,
            height: 1,
            content_width: 1,
            inset: 0,
        },
        |theme| {
            let runtime = theme_runtime_for_surface(theme, 0, theme_runtime_from_state(state));
            let mut runtime = nest.map_or(runtime, |nest| runtime.with_nest(nest));
            if nest == Some(SurfaceNest::Floating) {
                if let Some(host) = floating_host_for_theme(theme, None) {
                    runtime = runtime.with_host_dimensions(host.width, host.height);
                }
            }
            let scale = theme_surface_scale(theme, 0);
            positioning::widget_frame(theme, state.data.as_ref(), runtime, scale)
        },
    )
}

fn floating_frame_for_state(
    state: &AppState,
    display_index: usize,
    scale: f64,
) -> Option<positioning::WidgetFrame> {
    let mut theme = effective_theme_from_state(state)?;
    let mut placement = positioning::floating_placement(display_index);
    placement.host_dimensions =
        floating_host_for_theme(&theme, None).map(|host| (host.width, host.height));
    positioning::override_primary_placement(&mut theme, placement);
    let runtime = theme_runtime_for_surface(&theme, 0, theme_runtime_from_state(state));
    Some(positioning::widget_frame(
        &theme,
        state.data.as_ref(),
        runtime,
        scale,
    ))
}

fn apply_custom_theme(
    hwnd: HWND,
    _enabled: bool,
    path: Option<PathBuf>,
    document: Option<ThemeDocument>,
) -> Result<(), String> {
    let loaded = match (document, path.as_deref()) {
        (Some(document), _) => Some(document),
        (None, Some(path)) => Some(theme_engine::load_theme(path)?),
        (None, None) => lock_state()
            .as_ref()
            .and_then(|state| state.active_theme.clone()),
    };
    let loaded = loaded.unwrap_or_else(ThemeDocument::starter);
    let theme_clock_interval = loaded.current_time_refresh_interval();
    let tray_theme_uses_current_time = theme_tray_uses_current_time(&loaded);
    let old_hook = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return Err("Application is not ready".into());
        };
        state.custom_theme_enabled = true;
        state.active_theme = Some(loaded);
        state.theme_clock_interval = theme_clock_interval;
        state.tray_theme_uses_current_time = tray_theme_uses_current_time;
        state.mouse_action_overrides.clear();
        state.hovered_mouse_layer = None;
        state.pending_mouse_click = None;
        state.suppress_next_left_up = false;
        if path.is_some() {
            state.active_theme_path = path;
        }
        state.embedded = false;
        state.win_event_hook.take()
    };
    if let Some(hook) = old_hook {
        native_interop::unhook_win_event(hook.to_hook());
    }
    let surface = lock_state()
        .as_ref()
        .map(|s| s.surface_hwnd.to_hwnd())
        .unwrap_or_default();
    unsafe {
        native_interop::make_popup(surface, false);
        ensure_layered_window(surface);
        let _ = SetWindowPos(
            surface,
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    sync_custom_mirrors();
    sync_window_state_timer(hwnd);
    schedule_countdown_timer();
    schedule_clock_timer();
    Ok(())
}

fn sync_custom_mirrors() {
    managed::recover_theme_windows();
    let (desired_total, desktop_surfaces) = {
        let state = lock_state();
        state
            .as_ref()
            .map(|state| {
                let surfaces = state
                    .active_theme
                    .as_ref()
                    .map(|theme| {
                        theme
                            .surfaces
                            .iter()
                            .map(|surface| {
                                surface
                                    .placement
                                    .nest
                                    .resolve(surface.placement.reference.region)
                                    == SurfaceNest::Desktop
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                (surfaces.len().max(1), surfaces)
            })
            .unwrap_or_else(|| (1, Vec::new()))
    };
    let desired_mirrors = desired_total.saturating_sub(1);
    loop {
        let remove = {
            let mut state = lock_state();
            state.as_mut().and_then(|state| {
                if state.mirror_hwnds.len() > desired_mirrors {
                    state.mirror_hwnds.pop()
                } else {
                    None
                }
            })
        };
        match remove {
            Some(hwnd) => unsafe {
                let _ = DestroyWindow(hwnd.to_hwnd());
            },
            None => break,
        }
    }
    while lock_state()
        .as_ref()
        .map(|state| state.mirror_hwnds.len())
        .unwrap_or(0)
        < desired_mirrors
    {
        let mirror = unsafe { create_mirror_window() };
        if mirror.is_invalid() {
            break;
        }
        if let Some(state) = lock_state().as_mut() {
            state.mirror_hwnds.push(SendHwnd::from_hwnd(mirror));
        }
    }

    let stale_desktop_windows = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        state.desktop_hwnds.resize_with(desired_total, || None);
        let removed = state.desktop_hwnds.split_off(desired_total);
        let mut stale = removed.into_iter().flatten().collect::<Vec<_>>();
        for (surface_index, window) in state.desktop_hwnds.iter_mut().enumerate() {
            let wanted = desktop_surfaces.get(surface_index) == Some(&true);
            let valid =
                window.is_some_and(|window| unsafe { IsWindow(Some(window.to_hwnd())).as_bool() });
            if !wanted || !valid {
                if let Some(window) = window.take() {
                    stale.push(window);
                }
            }
        }
        stale
    };
    for window in stale_desktop_windows {
        unsafe {
            let _ = DestroyWindow(window.to_hwnd());
        }
    }
    for (surface_index, wanted) in desktop_surfaces.into_iter().enumerate() {
        if !wanted {
            continue;
        }
        let missing = lock_state()
            .as_ref()
            .and_then(|state| state.desktop_hwnds.get(surface_index))
            .is_none_or(Option::is_none);
        if !missing {
            continue;
        }
        let window = unsafe { create_desktop_surface_window() };
        if window.is_invalid() {
            continue;
        }
        unsafe {
            let _ = ShowWindow(window, SW_HIDE);
        }
        if let Some(slot) = lock_state()
            .as_mut()
            .and_then(|state| state.desktop_hwnds.get_mut(surface_index))
        {
            *slot = Some(SendHwnd::from_hwnd(window));
        } else {
            unsafe {
                let _ = DestroyWindow(window);
            }
        }
    }
}

unsafe fn create_desktop_surface_window() -> HWND {
    let Some(desktop) = native_interop::find_desktop_host() else {
        return HWND::default();
    };
    let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
    let class = native_interop::wide_str("CCUMHardenedUpstream2DesktopSurface");
    let title = native_interop::wide_str("");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(mirror_wnd_proc),
        hInstance: HINSTANCE(instance.0),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszClassName: PCWSTR::from_raw(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let previous_hosting = SetThreadDpiHostingBehavior(DPI_HOSTING_BEHAVIOR_MIXED);
    let previous_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
    let window = CreateWindowExW(
        WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE,
        PCWSTR::from_raw(class.as_ptr()),
        PCWSTR::from_raw(title.as_ptr()),
        WINDOW_STYLE(
            native_interop::WS_CHILD_STYLE | native_interop::WS_CLIPSIBLINGS_STYLE | WS_VISIBLE.0,
        ),
        0,
        0,
        198,
        144,
        Some(desktop.parent),
        None,
        Some(HINSTANCE(instance.0)),
        None,
    )
    .unwrap_or_default();
    let _ = SetThreadDpiAwarenessContext(previous_dpi);
    let _ = SetThreadDpiHostingBehavior(previous_hosting);
    if window.is_invalid() {
        diagnose::log("unable to create raised-desktop surface window");
    }
    window
}

unsafe fn create_mirror_window() -> HWND {
    let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
    let class = native_interop::wide_str("CCUMHardenedUpstream2ThemeMirror");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(mirror_wnd_proc),
        hInstance: HINSTANCE(instance.0),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszClassName: PCWSTR::from_raw(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let title = native_interop::wide_str("Usage theme mirror");
    CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
        PCWSTR::from_raw(class.as_ptr()),
        PCWSTR::from_raw(title.as_ptr()),
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

unsafe extern "system" fn mirror_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(result) = managed::window_message(hwnd, msg, wparam, lparam) {
        return result;
    }
    match msg {
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_SETCURSOR if set_surface_cursor(hwnd) => LRESULT(1),
        WM_MOUSEMOVE => {
            update_mouse_hover(hwnd, lparam);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            clear_mouse_hover(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let suppressed = {
                let mut state = lock_state();
                state.as_mut().is_some_and(|state| {
                    let suppressed = state.suppress_next_left_up;
                    state.suppress_next_left_up = false;
                    suppressed
                })
            };
            if !suppressed {
                if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                    schedule_or_dispatch_click(hwnd, surface, object);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                dispatch_double_click(hwnd, surface, object);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                let _ = dispatch_mouse_event(surface, &object, MouseEventKind::RightClick);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut paint);
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DESTROY => {
            crate::desktop_compositor::remove(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn total_widget_height_for_state(state: &AppState) -> i32 {
    widget_frame_for_state(state, None).height
}

fn total_widget_height() -> i32 {
    lock_state()
        .as_ref()
        .map(total_widget_height_for_state)
        .unwrap_or(1)
}

fn total_widget_width() -> i32 {
    lock_state()
        .as_ref()
        .map(total_widget_width_for_state)
        .unwrap_or(1)
}

pub fn run() {
    let run_args: Vec<String> = std::env::args().collect();
    let open_dashboard_on_start = run_args.iter().any(|argument| argument == "--dashboard");
    let allow_multiple = run_args
        .iter()
        .any(|argument| argument == "--allow-multiple");
    let no_poll = run_args.iter().any(|argument| argument == "--no-poll");
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CURRENT_DPI.store(GetDpiForSystem(), Ordering::Relaxed);
    }
    diagnose::log("window::run started");

    // Single-instance guard: silently exit if another instance is running in this session.
    // Use the local namespace so other users' desktop/RDP sessions remain independent.
    // Exception: when relaunched after an explorer restart (ENV_RELAUNCH set),
    // wait for the previous instance to release the mutex, then take over.
    let is_relaunch = false;
    let mutex_name = native_interop::wide_str(&if allow_multiple {
        format!(
            "Local\\ClaudeCodeUsageMonitorHardenedUpstream2-{}",
            std::process::id()
        )
    } else {
        "Local\\ClaudeCodeUsageMonitorHardenedUpstream2".to_string()
    });
    let _mutex = unsafe {
        let handle = CreateMutexW(None, true, PCWSTR::from_raw(mutex_name.as_ptr()));
        match handle {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    if is_relaunch {
                        diagnose::log("relaunch: waiting for previous instance to exit");
                        let wait_result = WaitForSingleObject(h, 10_000);
                        if wait_result != WAIT_OBJECT_0 && wait_result != WAIT_ABANDONED {
                            diagnose::log(format!(
                                "startup aborted: previous instance did not exit cleanly ({wait_result:?})"
                            ));
                            return;
                        }
                    } else {
                        if open_dashboard_on_start {
                            if let Err(error) = crate::dashboard::request_from_existing_monitor() {
                                crate::dashboard::report_launch_failure(HWND::default(), &error);
                            }
                        }
                        diagnose::log("startup aborted: another instance is already running");
                        return;
                    }
                }
                h
            }
            Err(error) => {
                diagnose::log_error(
                    "startup aborted: unable to create single-instance mutex",
                    error,
                );
                return;
            }
        }
    };

    let class_name = native_interop::wide_str("ClaudeCodeUsageMonitorHardenedUpstream2");

    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap();
        let (large_icon, small_icon) = tray_icon::load_app_icons();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(hinstance.0),
            hIcon: large_icon,
            hIconSm: small_icon,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };

        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            diagnose::log("RegisterClassExW returned 0");
        }

        let mut settings = load_settings();
        let mut observed_settings = settings.clone();
        let classic_theme_path = theme_engine::ensure_starter_theme().ok();
        let mut configured_theme_path = settings.active_theme_path.as_deref().map(PathBuf::from);
        let mut configured_theme = configured_theme_path
            .as_deref()
            .and_then(|path| theme_engine::load_theme(path).ok())
            .filter(|theme| !theme.is_obsolete_studio_starter());
        let legacy_placement = settings.legacy_placement();
        let legacy_visibility = settings.legacy_widget_visibility();
        if legacy_placement.is_some() || legacy_visibility.is_some() {
            if configured_theme
                .as_ref()
                .is_some_and(|theme| !theme.is_builtin_classic())
            {
                // A user-selected writable theme already owns its presentation.
                // Consume the obsolete settings without replacing that theme.
                settings.consume_legacy_placement();
                settings.consume_legacy_widget_visibility();
                save_settings_or_log(
                    &settings,
                    &mut observed_settings,
                    "unable to consume legacy settings",
                );
            } else if legacy_placement.is_some() || legacy_visibility == Some(false) {
                let placement = legacy_placement.map(migrated_theme_placement);
                let migrated = ThemeDocument::migrated_from_legacy(
                    placement,
                    legacy_visibility.unwrap_or(true),
                );
                match theme_engine::save_theme(&migrated) {
                    Ok(path) => {
                        configured_theme_path = Some(path.clone());
                        configured_theme = Some(migrated);
                        settings.active_theme_path = Some(path.to_string_lossy().into_owned());
                        settings.custom_theme_enabled = true;
                        settings.consume_legacy_placement();
                        settings.consume_legacy_widget_visibility();
                        if let Err(error) = app_settings::update_settings(|current| merge_settings_edits(current, &observed_settings, &settings)) {
                            diagnose::log(format!(
                                "migrated theme created but settings cleanup failed: {error}"
                            ));
                        } else {
                            observed_settings = settings.clone();
                            diagnose::log(
                                "legacy placement and visibility migrated to Migrated Theme",
                            );
                        }
                    }
                    Err(error) => diagnose::log(format!(
                        "legacy theme migration deferred because the copied theme could not be saved: {error}"
                    )),
                }
            } else {
                // An explicitly visible v1.4.9 widget already matches the
                // built-in theme's Render value, so no copy is necessary.
                settings.consume_legacy_widget_visibility();
                save_settings_or_log(
                    &settings,
                    &mut observed_settings,
                    "unable to consume legacy visibility",
                );
            }
        }
        let (active_theme_path, active_theme) = configured_theme
            .map(|theme| (configured_theme_path, Some(theme)))
            .unwrap_or_else(|| {
                let path = classic_theme_path;
                let theme = path
                    .as_deref()
                    .and_then(|path| theme_engine::load_theme(path).ok())
                    .or_else(|| Some(ThemeDocument::starter()));
                (path, theme)
            });
        let custom_theme_enabled = true;
        let theme_clock_interval = active_theme
            .as_ref()
            .and_then(ThemeDocument::current_time_refresh_interval);
        let tray_theme_uses_current_time = active_theme
            .as_ref()
            .is_some_and(theme_tray_uses_current_time);
        if let Some(path) = &active_theme_path {
            let path = path.to_string_lossy().into_owned();
            if settings.active_theme_path.as_deref() != Some(path.as_str())
                || !settings.custom_theme_enabled
            {
                settings.active_theme_path = Some(path);
                settings.custom_theme_enabled = true;
                save_settings_or_log(
                    &settings,
                    &mut observed_settings,
                    "unable to persist active theme",
                );
            }
        }
        let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
        let language = localization::resolve_language(language_override);

        refresh_theme_host_geometry();

        // Create as layered popup (will be reparented into taskbar)
        let title = native_interop::wide_str(language.strings().window_title);
        let initial_runtime = ThemeRuntime::from_providers(settings.enabled_providers())
            .with_poll_state(false, false)
            .with_language(language)
            .with_countdown(settings.usage_countdown);
        let (initial_width, initial_height) = active_theme
            .as_ref()
            .map(|theme| {
                let initial_runtime = theme_runtime_for_surface(theme, 0, initial_runtime);
                let (width, height) =
                    theme_engine::resolve_surface_size(theme, 0, None, initial_runtime);
                let scale = theme_surface_scale(theme, 0);
                (
                    scaled_theme_dimension(width, scale),
                    scaled_theme_dimension(height, scale),
                )
            })
            .unwrap_or((1, 1));
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            initial_width,
            initial_height,
            None,
            None,
            Some(HINSTANCE(hinstance.0)),
            None,
        )
        .unwrap();

        if !large_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(large_icon.0 as isize)),
            );
        }
        if !small_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(small_icon.0 as isize)),
            );
        }

        diagnose::log(format!("main window created hwnd={:?}", hwnd));

        // Display the last same-source reading before network access. A failed
        // first poll must not replace imported or persisted data with nothing.
        let cached_data = app_settings::load_usage_cache().map(|cache| {
            let mut data = cache.data;
            data.invalidate_fallback_credentials();
            data.select_accounts(&settings.accounts);
            for provider in ProviderId::ALL {
                if let Some(mut usage) = data.get(provider).cloned() {
                    usage.stale = true;
                    data.insert(provider, usage);
                }
            }
            for account in &mut data.accounts {
                if let Some(usage) = account.usage.as_mut() {
                    usage.stale = true;
                }
            }
            data
        });
        let surface_hwnd = managed::create_primary_surface();
        let is_dark = theme::is_dark_mode();
        {
            let mut state = lock_state();
            *state = Some(AppState {
                hwnd: SendHwnd::from_hwnd(hwnd),
                surface_hwnd: SendHwnd::from_hwnd(surface_hwnd),
                monitors: Vec::new(),
                monitor_settings: settings.monitors.clone(),
                managed_windows: Vec::new(),
                managed_visible: settings.monitor_widget_visible,
                managed_placement: settings.monitor_placement.clone(),
                observed_settings: settings.clone(),
                taskbar_hwnd: None,
                tray_notify_hwnd: None,
                win_event_hook: None,
                is_dark,
                embedded: false,
                language_override,
                language,
                providers: settings.enabled_providers(),
                accounts: settings.accounts.clone(),
                data: cached_data,
                poll_interval_ms: settings.poll_interval_ms,
                polling_enabled: !no_poll,
                retry_count: 0,
                force_notify_auth_error: false,
                auth_error_paused_polling: false,
                auth_watch_mode: poller::CredentialWatchMode::ActiveSource(
                    settings.enabled_providers().first().unwrap_or_default(),
                ),
                auth_watch_snapshot: Vec::new(),
                last_poll_ok: false,
                last_poll_failure: None,
                update_status: UpdateStatus::Idle,
                last_update_check_unix: settings.last_update_check_unix,
                taskbar_index: settings.taskbar_index,
                tray_offset: settings.tray_offset,
                dragging: false,
                pending_drag: false,
                drag_start_cursor: POINT::default(),
                drag_start_origin: POINT::default(),
                drag_start_client_x: 0,
                auto_ejected: false,
                taskbar_auto_eject: settings.taskbar_auto_eject,
                auto_ejected_origin: None,
                auto_ejected_host: None,
                is_switching_window_style: false,
                is_snapped: false,
                placement_override: settings.placement_override.clone(),
                floating_card_opacity: settings.floating_card_opacity,
                window_state_timer_active: false,
                custom_theme_enabled,
                usage_countdown: settings.usage_countdown,
                active_theme_path,
                active_theme,
                theme_clock_interval,
                tray_theme_uses_current_time,
                mirror_hwnds: Vec::new(),
                desktop_hwnds: Vec::new(),
                mouse_action_overrides: HashMap::new(),
                hovered_mouse_layer: None,
                pending_mouse_click: None,
                suppress_next_left_up: false,
            });
        }

        if let Some(state) = lock_state().as_ref() {
            publish_update_status(state);
        }
        if let Err(error) = crate::dashboard::start_request_listener(hwnd) {
            diagnose::log_error("dashboard request listener failed", error);
        }

        managed::refresh_topology();
        sync_custom_mirrors();

        // Register the persistent application tray icon.
        if !no_poll {
            sync_tray_icon(hwnd);
        }

        // Theme surfaces decide whether their windows render.
        position_at_taskbar();
        diagnose::log("window shown");

        // Initial render using the presenter selected by the surface nest.
        render_layered();
        schedule_countdown_timer();
        schedule_clock_timer();

        if open_dashboard_on_start {
            crate::dashboard::show(hwnd);
        }

        // Poll timer: 15 minutes
        let initial_poll_ms = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| s.poll_interval_ms)
                .unwrap_or(POLL_15_MIN)
        };
        SetTimer(Some(hwnd), TIMER_POLL, initial_poll_ms, None);
        sync_window_state_timer(hwnd);

        // Watch for explorer.exe restarts so we can re-embed and re-add the tray
        // icon (the shell discards tray registrations when it restarts). This
        // runs on a dedicated thread, NOT a window timer: once explorer destroys
        // the taskbar, our embedded child window stops receiving all messages
        // (WM_TIMER included), so a timer would never fire again.
        taskbar_collision::spawn_reader();
        spawn_taskbar_watchdog();

        // Initial poll
        if !no_poll {
            diagnose::log("initial poll requested");
            request_poll(hwnd);
        }

        if !no_poll {
            schedule_auto_update_check(hwnd);
        }
        let should_check_updates = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| auto_update_check_due(s.last_update_check_unix))
                .unwrap_or(false)
        };
        if should_check_updates && !no_poll {
            begin_update_check(hwnd, false);
        }

        // Initial theme check
        check_theme_change();

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Render every theme surface, then dispatch it to the presenter selected by
/// its nest: DirectComposition for desktop and layered windows elsewhere.
fn render_layered() {
    refresh_dpi();
    managed::reconcile();
    sync_custom_mirrors();
    let (hwnd_val, active_theme, usage_data, runtime, mirror_hwnds, desktop_hwnds) = {
        let state = lock_state();
        let Some(state) = state.as_ref() else {
            return;
        };
        (
            state.surface_hwnd,
            effective_theme_from_state(state),
            state.data.clone(),
            theme_runtime_from_state(state),
            state.mirror_hwnds.clone(),
            state.desktop_hwnds.clone(),
        )
    };

    // Theme rendering is the widget renderer. Startup and theme changes always
    // install Classic in memory when a selected theme cannot be loaded.
    let theme = active_theme.unwrap_or_else(ThemeDocument::starter);
    let hwnd = hwnd_val.to_hwnd();
    let controller = lock_state()
        .as_ref()
        .map(|s| s.hwnd.to_hwnd())
        .unwrap_or_default();
    let has_managed_copies = lock_state()
        .as_ref()
        .is_some_and(|s| !s.managed_windows.is_empty());
    set_window_state_timer(
        controller,
        theme_has_floating_surface(&theme) || has_managed_copies,
    );
    let target_count = theme.surfaces.len();
    for surface_index in 0..target_count {
        let regular_hwnd = if surface_index == 0 {
            hwnd
        } else if let Some(mirror) = mirror_hwnds.get(surface_index - 1) {
            mirror.to_hwnd()
        } else {
            continue;
        };
        if managed::is_managed_index(surface_index) {
            unsafe {
                let _ = ShowWindow(regular_hwnd, SW_HIDE);
            }
            continue;
        }
        let surface = &theme.surfaces[surface_index];
        let surface_runtime = theme_runtime_for_surface(&theme, surface_index, runtime);
        let nest = surface
            .placement
            .nest
            .resolve(surface.placement.reference.region);
        let desktop_nested = nest == SurfaceNest::Desktop;
        let target_hwnd = if desktop_nested {
            unsafe {
                let _ = ShowWindow(regular_hwnd, SW_HIDE);
            }
            desktop_hwnds
                .get(surface_index)
                .and_then(|window| *window)
                .map(SendHwnd::to_hwnd)
                .unwrap_or(regular_hwnd)
        } else {
            regular_hwnd
        };
        if nest == SurfaceNest::TrayIcon {
            unsafe {
                let _ = ShowWindow(target_hwnd, SW_HIDE);
            }
            continue;
        }
        if !theme_engine::surface_should_render(
            &theme,
            surface_index,
            usage_data.as_ref(),
            surface_runtime,
        ) {
            unsafe {
                let _ = ShowWindow(target_hwnd, SW_HIDE);
            }
            continue;
        }

        let scale = theme_surface_scale(&theme, surface_index);
        let rendered = theme_engine::render_theme_surface_with_runtime_at_scale(
            &theme,
            surface_index,
            usage_data.as_ref(),
            surface_runtime,
            scale,
        );
        let mut positioned = theme_for_surface(&theme, surface_index);
        let (logical_width, logical_height) = theme_engine::resolve_surface_size(
            &theme,
            surface_index,
            usage_data.as_ref(),
            surface_runtime,
        );
        positioned.canvas.width = logical_width;
        positioned.canvas.height = logical_height;
        let placement = theme_engine::resolve_surface_placement(
            &theme,
            surface_index,
            usage_data.as_ref(),
            surface_runtime,
        );
        positioned.placement.offset_x = placement.offset_x;
        positioned.placement.offset_y = placement.offset_y;
        position_custom_theme(target_hwnd, &positioned, scale);
        if desktop_nested {
            unsafe {
                let _ = ShowWindow(target_hwnd, SW_SHOWNOACTIVATE);
            }
        }
        render_custom_window(target_hwnd, &rendered, desktop_nested);
        unsafe {
            let show = nest != SurfaceNest::Floating
                || !foreground_is_fullscreen_on_display(positioned.placement.reference.display);
            let _ = ShowWindow(target_hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    managed::render(&theme, usage_data.as_ref(), runtime);

    for target in std::iter::once(hwnd)
        .chain(mirror_hwnds.iter().map(|mirror| mirror.to_hwnd()))
        .skip(target_count)
    {
        unsafe {
            let _ = ShowWindow(target, SW_HIDE);
        }
    }
}
fn theme_for_surface(theme: &ThemeDocument, surface_index: usize) -> ThemeDocument {
    let mut result = theme.clone();
    if let Some(surface) = theme.surfaces.get(surface_index) {
        result.canvas.width_expression = Some(surface.width.clone());
        result.canvas.height_expression = Some(surface.height.clone());
        result.canvas.background = match &surface.background {
            crate::theme_engine::LayerBackground::Colour { colour } => colour.clone(),
            crate::theme_engine::LayerBackground::None
            | crate::theme_engine::LayerBackground::Gradient { .. }
            | crate::theme_engine::LayerBackground::Image { .. } => Default::default(),
        };
        result.placement = surface.placement.clone();
        result.children = surface.children.clone();
    }
    result
}

fn request_poll(hwnd: HWND) {
    request_poll_inner(hwnd, true);
}

/// Request a timer-driven poll without extending an already-running poll cycle.
fn request_scheduled_poll(hwnd: HWND) {
    request_poll_inner(hwnd, false);
}

fn request_poll_inner(hwnd: HWND, queue_if_busy: bool) {
    if lock_state()
        .as_ref()
        .is_some_and(|state| !state.polling_enabled)
    {
        return;
    }
    if POLL_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        if queue_if_busy {
            POLL_PENDING.store(true, Ordering::Release);
            diagnose::log("poll already running; manual refresh queued");
        }
        return;
    }
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    std::thread::spawn(move || poll_worker(send_hwnd, !queue_if_busy));
}

/// Run credential watching under the same in-flight guard as usage polling.
/// Timer ticks cannot pile up workers, and manual refreshes still queue behind
/// a slow credential scan. Credential changes do not synthesize manual actions.
fn poll_worker(send_hwnd: SendHwnd, scheduled: bool) {
    run_poll_worker(&POLL_IN_FLIGHT, &POLL_PENDING, scheduled, |scheduled| {
        if !scheduled || scheduled_poll_needed() {
            do_poll_once(send_hwnd.to_hwnd());
        }
    });
}

fn run_poll_worker(
    in_flight: &AtomicBool,
    pending: &AtomicBool,
    mut scheduled: bool,
    mut poll: impl FnMut(bool),
) {
    loop {
        poll(scheduled);
        // Any queued request is an explicit refresh, not another timer tick.
        scheduled = false;
        if pending.swap(false, Ordering::AcqRel) {
            continue;
        }

        in_flight.store(false, Ordering::Release);
        if !pending.swap(false, Ordering::AcqRel) {
            break;
        }

        // A request can arrive between the pending check and releasing the
        // in-flight flag. Reacquire ownership unless that request already
        // started a replacement worker.
        if in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            break;
        }
    }
}

/// A passive two-second watch shares the polling guard, so slow WSL reads do
/// not pile up and manual refresh still runs immediately after a pending watch.
fn request_credential_watch(hwnd: HWND) {
    let paused = lock_state().as_ref().is_some_and(|state| {
        state.auth_error_paused_polling
            || state.data.as_ref().is_some_and(poller::has_paused_accounts)
    });
    if !paused
        || POLL_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }
    let hwnd = SendHwnd::from_hwnd(hwnd);
    std::thread::spawn(move || {
        run_poll_worker(&POLL_IN_FLIGHT, &POLL_PENDING, true, |watch_only| {
            if !watch_only || recovery_watch_changed() {
                do_poll_once(hwnd.to_hwnd());
            }
        });
    });
}

fn recovery_watch_changed() -> bool {
    let (data, globally_paused, providers, accounts) = {
        let state = lock_state();
        let Some(state) = state.as_ref() else {
            return false;
        };
        (
            state.data.clone(),
            state.auth_error_paused_polling,
            state.providers,
            state.accounts.clone(),
        )
    };
    let changed = data
        .as_ref()
        .is_some_and(poller::paused_account_credentials_changed)
        || (globally_paused && scheduled_poll_needed());
    changed
        && lock_state()
            .as_ref()
            .is_some_and(|state| state.providers == providers && state.accounts == accounts)
}

fn scheduled_poll_needed() -> bool {
    let watch = {
        let state = lock_state();
        let Some(state) = state.as_ref() else {
            return false;
        };
        if !state.auth_error_paused_polling {
            return true;
        }
        (
            state.auth_watch_mode,
            state.auth_watch_snapshot.clone(),
            state.providers,
            state.accounts.clone(),
        )
    };
    // No STATE lock is held while reading files, credentials, or WSL.
    let current = poller::credential_watch_snapshot(watch.0);
    current != watch.1
        && lock_state().as_ref().is_some_and(|state| {
            state.auth_error_paused_polling
                && state.auth_watch_mode == watch.0
                && state.auth_watch_snapshot == watch.1
                && state.providers == watch.2
                && state.accounts == watch.3
        })
}

fn do_poll_once(hwnd: HWND) {
    let poll_started = Instant::now();
    let (enabled_providers, accounts, previous, force) = {
        let mut state = lock_state();
        state
            .as_mut()
            .map(|state| {
                (
                    state.providers,
                    state.accounts.clone(),
                    state.data.clone(),
                    std::mem::take(&mut state.force_notify_auth_error),
                )
            })
            .unwrap_or_default()
    };

    diagnose::log_lazy(|| format!("poll started providers={enabled_providers:?} force={force}"));
    let result = poller::poll(
        enabled_providers,
        &accounts,
        previous.as_ref(),
        force,
        |update| {
            let cache_data = {
                let mut state = lock_state();
                let Some(state) = state.as_mut() else {
                    return;
                };
                // A result from an old provider/account selection must not be shown.
                if state.providers != enabled_providers || state.accounts != accounts {
                    return;
                }
                let data = poller::merge_poll_progress(
                    update,
                    &state.data.clone().unwrap_or_default(),
                    &accounts,
                );
                state.data = Some(data.clone());
                state.last_poll_ok = true;
                state.last_poll_failure = None;
                data
            };
            // The dashboard runs separately and follows the same cache as the
            // widget. Publish before waiting for slower providers to finish.
            if let Err(error) = app_settings::save_usage_cache(&cache_data, true) {
                diagnose::log_error("unable to save partial usage cache", error);
            }
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        },
    );
    match result {
        Ok(data) => {
            let mut state = lock_state();
            if state
                .as_ref()
                .is_some_and(|s| s.providers != enabled_providers || s.accounts != accounts)
            {
                return;
            }
            let mut data = match state.as_ref().and_then(|s| s.data.as_ref()) {
                Some(previous) => poller::carry_forward_failures(data, previous, enabled_providers),
                None => data,
            };
            data.select_accounts(&accounts);
            let notifications: Vec<_> = data
                .new_auth_failures(previous.as_ref(), force)
                .into_iter()
                .map(|account| (account.provider, account.profile.name.clone()))
                .collect();
            let language = state
                .as_ref()
                .map(|state| state.language)
                .unwrap_or(LanguageId::English);
            let cache_data = data.clone();
            let mut next_poll_ms = None;
            if let Some(s) = state.as_mut() {
                // Stop fast-poll if reset data is now fresh
                if !poller::app_is_past_reset(&data) {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_RESET_POLL);
                    }
                }

                let retry = poller::account_retry_delay_ms(
                    &data,
                    s.retry_count.saturating_add(1),
                    s.poll_interval_ms,
                );
                s.data = Some(data);
                s.last_poll_ok = true;
                s.last_poll_failure = None;

                if let Some(retry) = retry {
                    s.retry_count = s.retry_count.saturating_add(1);
                    next_poll_ms = Some(poller::retry_delay_ms(retry, poll_started));
                } else if s.retry_count > 0 {
                    s.retry_count = 0;
                    next_poll_ms = Some(s.poll_interval_ms);
                }
                s.auth_error_paused_polling = false;
                s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource(
                    s.providers.first().unwrap_or_default(),
                );
                s.auth_watch_snapshot.clear();
            }
            drop(state);
            if let Some(interval) = next_poll_ms {
                unsafe {
                    SetTimer(Some(hwnd), TIMER_POLL, interval, None);
                }
            }
            match app_settings::save_usage_cache(&cache_data, true) {
                Ok(()) => diagnose::log_lazy(|| {
                    format!(
                        "usage cache saved: accounts={} elapsed_ms={}",
                        cache_data.accounts.len(),
                        poll_started.elapsed().as_millis()
                    )
                }),
                Err(error) => diagnose::log_error("unable to save usage cache", error),
            }
            if !notifications.is_empty() {
                let body = notifications
                    .iter()
                    .map(|(provider, name)| {
                        format!(
                            "{} ({name}): {}",
                            language.text(provider.descriptor().display_name),
                            language.provider_auth_error(*provider).1
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                tray_icon::notify_balloon(hwnd, language.text("Sign in again"), &body);
            }

            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
        Err(failure) => {
            diagnose::log_lazy(|| {
                format!(
                    "poll failed: {failure:?} elapsed_ms={}",
                    poll_started.elapsed().as_millis()
                )
            });
            if lock_state()
                .as_ref()
                .is_some_and(|s| s.providers != enabled_providers || s.accounts != accounts)
            {
                return;
            }
            let auth_watch = match failure.error {
                poller::PollError::AuthRequired
                | poller::PollError::TokenExpired
                | poller::PollError::HttpStatus(401 | 403) => {
                    let mode = poller::CredentialWatchMode::ActiveSource(failure.provider);
                    Some((mode, poller::credential_watch_snapshot(mode)))
                }
                poller::PollError::NoCredentials => {
                    let mode = poller::CredentialWatchMode::AllSources(failure.provider);
                    Some((mode, poller::credential_watch_snapshot(mode)))
                }
                poller::PollError::RequestFailed
                | poller::PollError::NetworkError
                | poller::PollError::UnexpectedResponse
                | poller::PollError::HttpStatus(_) => None,
            };
            // Distinguish auth-required errors from transient errors.
            let (notify_auth_error, cache_data, cache_poll_ok) = {
                let mut state = lock_state();
                if state
                    .as_ref()
                    .is_some_and(|s| s.providers != enabled_providers || s.accounts != accounts)
                {
                    return;
                }
                let mut should_notify = false;
                if let Some(s) = state.as_mut() {
                    if failure.error.is_transient() {
                        if let Some(previous) = s.data.as_ref() {
                            let carried = poller::carry_forward_failures(
                                AppUsageData::default(),
                                previous,
                                enabled_providers,
                            );
                            s.data = Some(carried);
                        }
                    }
                    s.last_poll_ok = false;
                    s.last_poll_failure = Some(failure);
                    match auth_watch {
                        Some((watch_mode, watch_snapshot)) => {
                            // Only show the balloon on the first failure so it doesn't spam.
                            if s.retry_count == 0 || force {
                                should_notify = true;
                            }
                            s.auth_error_paused_polling = true;
                            s.auth_watch_mode = watch_mode;
                            s.auth_watch_snapshot = watch_snapshot;
                            s.retry_count = s.retry_count.saturating_add(1);
                            unsafe {
                                let _ = KillTimer(Some(hwnd), TIMER_POLL);
                                let _ = KillTimer(Some(hwnd), TIMER_RESET_POLL);
                                let _ = KillTimer(Some(hwnd), TIMER_COUNTDOWN);
                                SetTimer(Some(hwnd), TIMER_POLL, s.poll_interval_ms, None);
                            }
                        }
                        _ => {
                            // Transient errors: exponential backoff, respecting server cooldowns.
                            s.auth_error_paused_polling = false;
                            s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource(
                                s.providers.first().unwrap_or_default(),
                            );
                            s.auth_watch_snapshot.clear();
                            s.retry_count = s.retry_count.saturating_add(1);
                            let backoff = RETRY_BASE_MS.saturating_mul(
                                1u32.checked_shl(s.retry_count - 1).unwrap_or(u32::MAX),
                            );
                            let retry_ms = poller::retry_delay_ms(
                                backoff.min(s.poll_interval_ms),
                                poll_started,
                            );
                            unsafe {
                                let _ = KillTimer(Some(hwnd), TIMER_RESET_POLL);
                                SetTimer(Some(hwnd), TIMER_POLL, retry_ms, None);
                            }
                        }
                    }
                }
                let cache_data = state
                    .as_ref()
                    .and_then(|state| state.data.clone())
                    .unwrap_or_default();
                let cache_poll_ok = state.as_ref().is_some_and(|state| {
                    poll_display_state(
                        state.last_poll_ok,
                        state.retry_count,
                        state.auth_error_paused_polling,
                        state.data.as_ref(),
                    )
                    .0
                });
                (should_notify, cache_data, cache_poll_ok)
            };
            // Theme Studio is a separate process and follows this cache. A
            // transient failure with usable stale data remains displayable;
            // hard failures and failures without a reading stay errors.
            let _ = app_settings::save_usage_cache(&cache_data, cache_poll_ok);

            if notify_auth_error {
                let balloon = {
                    let state = lock_state();
                    state
                        .as_ref()
                        .map(|state| state.language.provider_auth_error(failure.provider))
                };
                if let Some((title, body)) = balloon {
                    tray_icon::notify_balloon(hwnd, title, body);
                }
            }

            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
    }
}

fn schedule_countdown_timer() {
    let state = lock_state();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return,
    };

    let hwnd = s.hwnd.to_hwnd();
    if !s.last_poll_ok {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_COUNTDOWN);
            let _ = KillTimer(Some(hwnd), TIMER_RESET_POLL);
        }
        return;
    }

    // If a reset time has passed, poll every 5s to pick up fresh data
    if s.data.as_ref().is_some_and(poller::app_is_past_reset) {
        unsafe {
            SetTimer(Some(hwnd), TIMER_RESET_POLL, 5_000, None);
        }
    }

    let min_delay = s.data.as_ref().and_then(|data| {
        data.all_usage()
            .flat_map(|usage| usage.sections())
            .filter_map(|section| poller::time_until_display_change(section.resets_at))
            .min()
    });

    let ms = min_delay
        .unwrap_or(Duration::from_secs(60))
        .as_millis()
        .max(1000) as u32;

    unsafe {
        SetTimer(Some(hwnd), TIMER_COUNTDOWN, ms, None);
    }
}

fn schedule_clock_timer() {
    let state = lock_state();
    let Some(s) = state.as_ref() else {
        return;
    };
    let hwnd = s.hwnd.to_hwnd();
    let Some(interval) = s.theme_clock_interval else {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_CLOCK);
        }
        return;
    };
    let ms = time_until_next_clock_refresh(interval).as_millis().max(1) as u32;
    unsafe {
        SetTimer(Some(hwnd), TIMER_CLOCK, ms, None);
    }
}

fn time_until_next_clock_refresh(interval: Duration) -> Duration {
    let interval_ms = interval.as_millis().max(1);
    let elapsed_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let remaining_ms = interval_ms - elapsed_ms % interval_ms;
    Duration::from_millis(remaining_ms as u64)
}

fn check_theme_change() {
    let new_dark = theme::is_dark_mode();
    let changed = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            if s.is_dark != new_dark {
                s.is_dark = new_dark;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if changed {
        render_layered();
    }
}

fn check_language_change() {
    if update_language_change() {
        render_layered();
    }
}

fn reload_external_settings(hwnd: HWND) {
    let settings = load_settings();
    let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
    let theme_path = settings.active_theme_path.as_ref().map(PathBuf::from);
    let providers_changed;
    {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        providers_changed =
            state.providers != settings.enabled_providers() || state.accounts != settings.accounts;
        state.accounts = settings.accounts.clone();
        state.monitor_settings = settings.monitors.clone();
        state.managed_visible = settings.monitor_widget_visible;
        state.managed_placement = settings.monitor_placement.clone();
        state.observed_settings = settings.clone();
        if let Some(data) = state.data.as_mut() {
            data.select_accounts(&settings.accounts);
        }
        state.poll_interval_ms = settings.poll_interval_ms;
        state.providers = settings.enabled_providers();
        state.usage_countdown = settings.usage_countdown;
        state.taskbar_auto_eject = settings.taskbar_auto_eject;
        state.taskbar_index = settings.taskbar_index;
        state.tray_offset = settings.tray_offset;
        state.placement_override = settings.placement_override;
        state.floating_card_opacity = settings.floating_card_opacity;
        apply_language_to_state(state, language_override);
    }
    unsafe {
        SetTimer(Some(hwnd), TIMER_POLL, settings.poll_interval_ms, None);
    }
    let _ = apply_custom_theme(hwnd, settings.custom_theme_enabled, theme_path, None);
    if providers_changed {
        request_poll(hwnd);
    }
    sync_tray_icon(hwnd);
    position_at_taskbar();
    render_layered();
}

fn suppress_tray_reposition_for(duration: Duration) {
    let mut until = SUPPRESS_TRAY_REPOSITION_UNTIL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *until = Some(Instant::now() + duration);
}

fn tray_reposition_is_suppressed() -> bool {
    let now = Instant::now();
    let mut until = SUPPRESS_TRAY_REPOSITION_UNTIL
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    match *until {
        Some(deadline) if now < deadline => true,
        Some(_) => {
            *until = None;
            false
        }
        None => false,
    }
}

mod host_geometry;
mod message_loop;
use host_geometry::*;
use message_loop::wnd_proc;
mod positioning;
mod taskbar_collision;
use positioning::*;
mod mouse;
use mouse::*;
mod window_context_menu;
use window_context_menu::*;

#[cfg(test)]
mod placement_tests;

#[cfg(test)]
mod layered_window_tests;

#[cfg(test)]
mod language_menu_tests {
    use super::*;

    #[test]
    fn generated_language_menu_commands_round_trip() {
        assert_eq!(language_from_menu_command_id(IDM_LANG_SYSTEM), None);
        for language in LanguageId::ALL {
            assert_eq!(
                language_from_menu_command_id(language_menu_command_id(language)),
                Some(language)
            );
        }
    }
}

#[cfg(test)]
mod tray_usage_summary_tests {
    use super::*;
    use crate::models::{UsageData, UsageSection};

    fn usage(session: f64, weekly: f64, weekly_label: Option<&str>) -> UsageData {
        UsageData {
            session: UsageSection {
                available: true,
                percentage: session,
                resets_at: None,
            },
            weekly: UsageSection {
                available: true,
                percentage: weekly,
                resets_at: None,
            },
            weekly_label: weekly_label.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn tray_errors_distinguish_causes_and_disappear_after_recovery() {
        use crate::poller::{PollError, PollFailure};
        let providers = ProviderSet::from_enabled([ProviderId::Claude]);
        for (error, expected) in [
            (PollError::TokenExpired, "Login expired"),
            (PollError::NoCredentials, "No usable login"),
            (PollError::AuthRequired, "Login rejected"),
            (PollError::RequestFailed, "Usage request failed"),
            (PollError::NetworkError, "Service unreachable"),
            (PollError::UnexpectedResponse, "Unexpected usage response"),
            (PollError::HttpStatus(429), "HTTP 429"),
        ] {
            let failure = PollFailure {
                provider: ProviderId::Claude,
                error,
            };
            let lines = tray_error_lines(None, Some(failure), providers, LanguageId::English);
            assert_eq!(lines.len(), 1);
            assert!(lines[0].contains(expected), "{:?}", lines);
            assert!(tray_error_lines(
                None,
                Some(failure),
                ProviderSet::from_enabled([ProviderId::Codex]),
                LanguageId::English
            )
            .is_empty());
        }
        assert!(tray_error_lines(None, None, providers, LanguageId::English).is_empty());
        let mut data = AppUsageData::default();
        for (name, selected) in [("Work", false), ("Personal", true)] {
            data.accounts.push(crate::models::AccountUsage {
                provider: ProviderId::Claude,
                profile: crate::accounts::AccountProfile {
                    name: name.into(),
                    enabled: true,
                    ..Default::default()
                },
                source_signature: String::new(),
                source_path: None,
                usage: None,
                error: Some(PollError::TokenExpired),
                selected,
            });
        }
        let lines = tray_error_lines(Some(&data), None, providers, LanguageId::English);
        assert!(lines[0].starts_with("Claude Code (Personal): Login expired"));
        assert!(lines[1].contains("Work"));
        for account in &mut data.accounts {
            account.error = None;
        }
        assert!(tray_error_lines(Some(&data), None, providers, LanguageId::English).is_empty());
        let instruction = LanguageId::English
            .provider_auth_error(ProviderId::Claude)
            .1;
        assert!(instruction.contains("desktop app") && instruction.contains("/login"));
    }

    #[test]
    fn tray_summary_formats_enabled_provider_usage() {
        let data = [(ProviderId::Claude, usage(4.6, 42.4, None))]
            .into_iter()
            .collect();

        assert_eq!(
            tray_usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::Claude]),
                LanguageId::English,
                false,
            ),
            ["Claude Code 5h: 5% | 7d: 42%"]
        );
    }

    #[test]
    fn tray_summary_counts_down_when_the_widget_shows_what_is_left() {
        let data = [(ProviderId::Claude, usage(4.6, 42.4, None))]
            .into_iter()
            .collect();

        assert_eq!(
            tray_usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::Claude]),
                LanguageId::English,
                true,
            ),
            ["Claude Code 5h: 95% | 7d: 58%"]
        );
    }

    #[test]
    fn tray_summary_uses_provider_window_labels_and_selection() {
        let data = [
            (ProviderId::Claude, usage(10.0, 20.0, None)),
            (ProviderId::OpenCode, usage(30.0, 40.0, Some("30d"))),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            tray_usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::OpenCode]),
                LanguageId::English,
                false,
            ),
            ["OpenCode 5h: 30% | 30d: 40%"]
        );
    }
}

#[cfg(test)]
mod poll_display_state_tests {
    use super::*;
    use crate::models::UsageData;

    fn cached_usage(stale: bool) -> AppUsageData {
        let usage = UsageData {
            stale,
            ..Default::default()
        };
        [(ProviderId::Claude, usage)].into_iter().collect()
    }

    #[test]
    fn transient_failure_keeps_a_stale_reading_displayable() {
        let data = cached_usage(true);
        assert_eq!(
            poll_display_state(false, 1, false, Some(&data)),
            (true, false)
        );
    }

    #[test]
    fn failures_without_stale_data_remain_errors() {
        let fresh = cached_usage(false);
        let stale = cached_usage(true);

        assert_eq!(poll_display_state(false, 1, false, None), (false, true));
        assert_eq!(
            poll_display_state(false, 1, false, Some(&fresh)),
            (false, true)
        );
        assert_eq!(
            poll_display_state(false, 1, true, Some(&stale)),
            (false, true)
        );
    }
}

#[cfg(test)]
mod placement_regression_tests;

#[cfg(test)]
mod credential_watch_worker_tests {
    use super::*;

    #[test]
    fn a_manual_refresh_queued_during_a_slow_watch_is_not_lost() {
        let in_flight = AtomicBool::new(true);
        let pending = AtomicBool::new(false);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let in_flight = &in_flight;
            let pending = &pending;
            let worker = scope.spawn(move || {
                let mut calls = Vec::new();
                run_poll_worker(in_flight, pending, true, |scheduled| {
                    calls.push(scheduled);
                    if scheduled {
                        started_tx.send(()).unwrap();
                        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                });
                calls
            });
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            // A second timer tick cannot acquire the worker while discovery is
            // blocked, but an explicit refresh can queue for that worker.
            assert!(in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err());
            pending.store(true, Ordering::Release);
            resume_tx.send(()).unwrap();
            assert_eq!(worker.join().unwrap(), [true, false]);
        });
        assert!(!in_flight.load(Ordering::Acquire));
        assert!(!pending.load(Ordering::Acquire));
    }
}
