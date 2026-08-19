mod app_error;
mod commands;
mod contracts;
mod domain;
mod events;
mod logging;
mod message_catalog;
mod repositories;
mod services;
mod storage;
mod window;

use crate::services::AppServices;
use crate::window::{
    animation_frame_times_for, animation_generation_is_current, clamp_scale,
    clamp_width_to_work_area, eased_window_frame_with_spec, handle_application_lifecycle_event,
    logical_size_for_state, native_window_material_for_glass_transparency, physical_corner_radius,
    resized_x_for_fixed_edge, safe_restore_y_physical, should_tuck_physical, top_center_physical,
    tucked_y_physical, window_animation_spec, ApplicationLifecycleActions,
    ApplicationLifecycleEvent, FixedHorizontalEdge, IslandMode, IslandWindowState,
    NativeWindowMaterial, PhysicalPoint, PhysicalWindowFrame, SavedPlacement, WindowAnimationSpec,
    DEFAULT_EXPANDED_HEIGHT, MAX_EXPANDED_HEIGHT,
};
use serde::Serialize;
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewWindow, WindowEvent};

const WINDOW_LABEL: &str = "main";
const AISLAND_GITHUB_URL: &str = "https://github.com/ErdonChen/AIsland";

static WINDOW_STATE: OnceLock<Mutex<IslandWindowState>> = OnceLock::new();
static STATE_TRANSITION: OnceLock<Mutex<()>> = OnceLock::new();
static LAST_RASTERIZATION_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static NATIVE_UI_LANGUAGE: OnceLock<Mutex<UiLanguage>> = OnceLock::new();
static TRAY_NAVIGATION_STATE: TrayNavigationState = TrayNavigationState::new();
static PROGRAMMATIC_MOVE_DEPTH: AtomicUsize = AtomicUsize::new(0);
static DPI_RETRY_STATE: OnceLock<Mutex<DpiRetryState>> = OnceLock::new();
static MODE_ANIMATION_GENERATION: AtomicU64 = AtomicU64::new(0);
static CURRENT_CORNER_RADIUS: AtomicI32 = AtomicI32::new(0);

const MAX_DPI_RETRY_ATTEMPTS: u8 = 6;
const UI_LOCALE_SETTING_KEY: &str = "ui.locale";
const DEFAULT_GLASS_TRANSPARENCY: i32 = 58;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiLanguage {
    ZhCn,
    EnUs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrayLabels {
    show_hide: &'static str,
    settings: &'static str,
    quit: &'static str,
}

fn parse_ui_language(value: &str) -> Result<UiLanguage, String> {
    match value {
        "zh-CN" => Ok(UiLanguage::ZhCn),
        "en-US" => Ok(UiLanguage::EnUs),
        _ => Err(format!("native_language stage=parse unsupported={value}")),
    }
}

fn tray_labels(language: UiLanguage) -> TrayLabels {
    match language {
        UiLanguage::ZhCn => TrayLabels {
            show_hide: "显示/隐藏",
            settings: "设置",
            quit: "退出",
        },
        UiLanguage::EnUs => TrayLabels {
            show_hide: "Show/Hide",
            settings: "Settings",
            quit: "Quit",
        },
    }
}

fn native_ui_language_state() -> &'static Mutex<UiLanguage> {
    NATIVE_UI_LANGUAGE.get_or_init(|| Mutex::new(UiLanguage::ZhCn))
}

fn current_native_ui_language() -> Result<UiLanguage, String> {
    native_ui_language_state()
        .lock()
        .map(|language| *language)
        .map_err(|_| "native_language stage=commit state lock poisoned".to_string())
}

fn persisted_native_ui_language(
    settings: &crate::repositories::app_settings::AppSettingsRepository,
) -> Result<UiLanguage, crate::contracts::CommandError> {
    let Some((stored, _revision)) = settings.get::<String>(UI_LOCALE_SETTING_KEY)? else {
        return Ok(UiLanguage::ZhCn);
    };
    parse_ui_language(&stored).map_err(|reason| {
        crate::contracts::CommandError::with_detail(
            crate::contracts::AppErrorCode::InvalidInput,
            "errors.invalidInput",
            "reasonCode",
            crate::contracts::SafeParameterValue::String(reason),
            false,
        )
    })
}

pub(crate) fn restore_native_ui_language(
    settings: &crate::repositories::app_settings::AppSettingsRepository,
) -> Result<UiLanguage, crate::contracts::CommandError> {
    let restored = persisted_native_ui_language(settings)?;
    *native_ui_language_state().lock().map_err(|_| {
        crate::contracts::CommandError::with_detail(
            crate::contracts::AppErrorCode::IoFailure,
            "errors.ioFailure",
            "reasonCode",
            crate::contracts::SafeParameterValue::String("nativeLanguageUnavailable".into()),
            false,
        )
    })? = restored;
    Ok(restored)
}

fn ui_language_value(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::ZhCn => "zh-CN",
        UiLanguage::EnUs => "en-US",
    }
}

fn native_now_millis() -> Result<i64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("native_language stage=clock error={error}"))
        .and_then(|duration| {
            i64::try_from(duration.as_millis())
                .map_err(|_| "native_language stage=clock overflow".to_string())
        })
}

pub(crate) fn native_locale() -> Result<crate::contracts::Locale, crate::contracts::CommandError> {
    match current_native_ui_language() {
        Ok(UiLanguage::ZhCn) => Ok(crate::contracts::Locale::ZhCn),
        Ok(UiLanguage::EnUs) => Ok(crate::contracts::Locale::EnUs),
        Err(_) => Err(crate::contracts::CommandError::with_detail(
            crate::contracts::AppErrorCode::IoFailure,
            "errors.ioFailure",
            "reasonCode",
            crate::contracts::SafeParameterValue::String("nativeLanguageUnavailable".into()),
            false,
        )),
    }
}

#[cfg(test)]
fn apply_native_language_change(
    state: &Mutex<UiLanguage>,
    candidate: UiLanguage,
    apply_menu: impl FnOnce(TrayLabels) -> Result<(), String>,
) -> Result<(), String> {
    let mut current = state
        .lock()
        .map_err(|_| "native_language stage=commit state lock poisoned".to_string())?;
    if *current == candidate {
        return Ok(());
    }

    apply_menu(tray_labels(candidate))
        .map_err(|error| format!("native_language stage=menu error={error}"))?;
    *current = candidate;
    Ok(())
}

fn commit_native_language_change(
    state: &Mutex<UiLanguage>,
    settings: &crate::repositories::app_settings::AppSettingsRepository,
    candidate: UiLanguage,
    now: i64,
    apply_menu: impl Fn(TrayLabels) -> Result<(), String>,
) -> Result<(), String> {
    let mut current = state
        .lock()
        .map_err(|_| "native_language stage=commit state lock poisoned".to_string())?;
    let persisted = settings
        .get::<String>(UI_LOCALE_SETTING_KEY)
        .map_err(|error| format!("native_language stage=settings read={}", error.message_key))?;
    let is_persisted = persisted
        .as_ref()
        .is_some_and(|(value, _)| value == ui_language_value(candidate));
    let persist = || {
        if is_persisted {
            return Ok(());
        }
        settings
            .put(
                UI_LOCALE_SETTING_KEY,
                &ui_language_value(candidate),
                persisted.as_ref().map(|(_, revision)| *revision),
                now,
            )
            .map(|_| ())
            .map_err(|error| format!("native_language stage=settings write={}", error.message_key))
    };

    if *current == candidate {
        return persist();
    }
    let prior = *current;
    apply_menu(tray_labels(candidate))
        .map_err(|error| format!("native_language stage=menu error={error}"))?;
    if let Err(error) = persist() {
        let rollback = apply_menu(tray_labels(prior));
        return Err(match rollback {
            Ok(()) => error,
            Err(rollback_error) => {
                format!("{error}; native_language stage=menu rollback={rollback_error}")
            }
        });
    }
    *current = candidate;
    Ok(())
}

#[derive(Default)]
struct TrayNavigationState {
    latest: AtomicU64,
    acknowledged: AtomicU64,
}

impl TrayNavigationState {
    const fn new() -> Self {
        Self {
            latest: AtomicU64::new(0),
            acknowledged: AtomicU64::new(0),
        }
    }

    fn request(&self) -> u64 {
        self.latest.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn pending(&self) -> Option<PendingTrayNavigation> {
        let latest = self.latest.load(Ordering::SeqCst);
        let acknowledged = self.acknowledged.load(Ordering::SeqCst);
        (latest > acknowledged).then_some(PendingTrayNavigation {
            page: "settings",
            sequence: latest,
        })
    }

    fn acknowledge(&self, sequence: u64) {
        let latest = self.latest.load(Ordering::SeqCst);
        self.acknowledged
            .fetch_max(sequence.min(latest), Ordering::SeqCst);
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingTrayNavigation {
    page: &'static str,
    sequence: u64,
}

#[derive(Clone, Copy)]
struct PhysicalGeometrySnapshot {
    position: PhysicalPoint,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
struct PhysicalWorkArea {
    x: i32,
    width: u32,
}

#[derive(Clone, Copy)]
enum GeometryIntent {
    PreserveAnchor,
    Tuck,
    RestoreVisible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MonitorPreference {
    CurrentFirst,
    SavedFirst,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MonitorCandidate {
    Current,
    Saved,
    Primary,
}

#[derive(Default)]
struct DpiRetryState {
    latest_generation: u64,
    applied_generation: u64,
    worker_running: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DpiWorkerDecision {
    Retry(u8),
    Exit,
}

impl DpiRetryState {
    fn note_dpi_event(&mut self) {
        self.latest_generation = self.latest_generation.saturating_add(1);
    }

    fn mark_applied(&mut self) {
        self.applied_generation = self.latest_generation;
    }

    fn has_pending_geometry(&self) -> bool {
        self.latest_generation > self.applied_generation
    }

    fn start_worker_if_idle(&mut self) -> bool {
        if self.worker_running {
            false
        } else {
            self.worker_running = true;
            true
        }
    }

    fn next_retry_attempt(&self, observed_generation: &mut u64, attempts: &mut u8) -> Option<u8> {
        if *observed_generation != self.latest_generation {
            *observed_generation = self.latest_generation;
            *attempts = 0;
        }
        if *attempts >= MAX_DPI_RETRY_ATTEMPTS {
            return None;
        }
        *attempts += 1;
        Some(*attempts)
    }

    fn worker_attempt_or_exit(
        &mut self,
        observed_generation: &mut u64,
        attempts: &mut u8,
    ) -> DpiWorkerDecision {
        match self.next_retry_attempt(observed_generation, attempts) {
            Some(attempt) => DpiWorkerDecision::Retry(attempt),
            None => {
                self.worker_running = false;
                DpiWorkerDecision::Exit
            }
        }
    }
}

struct ProgrammaticMoveGuard;

impl ProgrammaticMoveGuard {
    fn enter() -> Self {
        PROGRAMMATIC_MOVE_DEPTH.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for ProgrammaticMoveGuard {
    fn drop(&mut self) {
        PROGRAMMATIC_MOVE_DEPTH.fetch_sub(1, Ordering::SeqCst);
    }
}

fn is_programmatic_move() -> bool {
    PROGRAMMATIC_MOVE_DEPTH.load(Ordering::SeqCst) > 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowAnimationOutcome {
    Applied,
    Superseded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CommittedModeGeometry {
    frame: PhysicalWindowFrame,
    remember_visible_position: bool,
}

const SUPERSEDED_MODE_ERROR: &str =
    "state_transition field=mode stage=superseded error=newer_request";

fn require_current_mode_request(is_current: bool) -> Result<(), String> {
    if is_current {
        Ok(())
    } else {
        Err(SUPERSEDED_MODE_ERROR.to_string())
    }
}

fn reject_superseded_mode_request_with_rollback<Rollback>(rollback: Rollback) -> Result<(), String>
where
    Rollback: FnOnce() -> Result<(), String>,
{
    match rollback() {
        Ok(()) => Err(SUPERSEDED_MODE_ERROR.to_string()),
        Err(rollback_error) => Err(format!(
            "{SUPERSEDED_MODE_ERROR} rollback_error={rollback_error}"
        )),
    }
}

fn mode_geometry_repair_obligation() -> &'static Mutex<Option<CommittedModeGeometry>> {
    static MODE_GEOMETRY_REPAIR: OnceLock<Mutex<Option<CommittedModeGeometry>>> = OnceLock::new();
    MODE_GEOMETRY_REPAIR.get_or_init(|| Mutex::new(None))
}

fn rollback_committed_mode_geometry_or_record<Rollback>(
    pending: &Mutex<Option<CommittedModeGeometry>>,
    committed: CommittedModeGeometry,
    rollback: Rollback,
) -> Result<(), String>
where
    Rollback: FnOnce() -> Result<(), String>,
{
    let rollback_error = match rollback() {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let mut pending = pending
        .lock()
        .map_err(|_| format!("{rollback_error}; repair_record_error=lock_poisoned"))?;
    match *pending {
        None => *pending = Some(committed),
        Some(existing) if existing == committed => {}
        Some(_) => {
            return Err(format!(
                "{rollback_error}; repair_record_error=conflicting_obligation"
            ));
        }
    }
    Err(rollback_error)
}

fn repair_pending_mode_geometry_before_snapshot<Repair, Snapshot, T>(
    pending: &Mutex<Option<CommittedModeGeometry>>,
    repair: Repair,
    snapshot: Snapshot,
) -> Result<T, String>
where
    Repair: FnOnce(CommittedModeGeometry) -> Result<(), String>,
    Snapshot: FnOnce() -> Result<T, String>,
{
    let mut pending = pending.lock().map_err(|_| {
        "state_transition field=mode stage=repair_pending error=lock_poisoned".to_string()
    })?;
    if let Some(committed) = *pending {
        repair(committed).map_err(|error| {
            format!("state_transition field=mode stage=repair_pending error={error}")
        })?;
        *pending = None;
    }
    snapshot()
}

fn mode_commit_gate() -> &'static Mutex<()> {
    static MODE_COMMIT_GATE: OnceLock<Mutex<()>> = OnceLock::new();
    MODE_COMMIT_GATE.get_or_init(|| Mutex::new(()))
}

fn issue_mode_request_generation(gate: &Mutex<()>, generations: &AtomicU64) -> Result<u64, String> {
    let _guard = gate.lock().map_err(|_| {
        "state_transition field=mode stage=serialize error=lock_poisoned".to_string()
    })?;
    Ok(generations.fetch_add(1, Ordering::SeqCst).wrapping_add(1))
}

fn complete_current_mode_request<T, Commit>(
    gate: &Mutex<()>,
    generations: &AtomicU64,
    generation: u64,
    commit: Commit,
) -> Result<T, String>
where
    Commit: FnOnce() -> Result<T, String>,
{
    let _guard = gate.lock().map_err(|_| {
        "state_transition field=mode stage=serialize error=lock_poisoned".to_string()
    })?;
    require_current_mode_request(animation_generation_is_current(
        generation,
        generations.load(Ordering::SeqCst),
    ))?;
    commit()
}

fn drive_window_animation<IsCurrent, Apply, Sleep>(
    start: PhysicalWindowFrame,
    end: PhysicalWindowFrame,
    animated: bool,
    spec: WindowAnimationSpec,
    mut is_current: IsCurrent,
    mut apply: Apply,
    mut sleep: Sleep,
) -> Result<WindowAnimationOutcome, String>
where
    IsCurrent: FnMut() -> bool,
    Apply: FnMut(PhysicalWindowFrame) -> Result<(), String>,
    Sleep: FnMut(std::time::Duration),
{
    if !animated {
        if !is_current() {
            return Ok(WindowAnimationOutcome::Superseded);
        }
        return Ok(WindowAnimationOutcome::Applied);
    }

    let mut previous_elapsed = 0;
    for elapsed in animation_frame_times_for(true, spec.duration_ms) {
        if !is_current() {
            return Ok(WindowAnimationOutcome::Superseded);
        }
        sleep(std::time::Duration::from_millis(
            elapsed.saturating_sub(previous_elapsed),
        ));
        if !is_current() {
            return Ok(WindowAnimationOutcome::Superseded);
        }
        if elapsed >= spec.duration_ms {
            break;
        }
        let frame = eased_window_frame_with_spec(start, end, elapsed, spec);
        if frame != end {
            apply(frame)?;
        }
        previous_elapsed = elapsed;
    }

    Ok(if is_current() {
        WindowAnimationOutcome::Applied
    } else {
        WindowAnimationOutcome::Superseded
    })
}

fn should_ignore_moved_event(state: &IslandWindowState) -> bool {
    is_programmatic_move() || state.is_tucked
}

fn select_monitor_candidate(
    has_current: bool,
    has_saved: bool,
    has_primary: bool,
    preference: MonitorPreference,
) -> Option<MonitorCandidate> {
    let candidates = match preference {
        MonitorPreference::CurrentFirst => [
            (has_current, MonitorCandidate::Current),
            (has_saved, MonitorCandidate::Saved),
            (has_primary, MonitorCandidate::Primary),
        ],
        MonitorPreference::SavedFirst => [
            (has_saved, MonitorCandidate::Saved),
            (has_current, MonitorCandidate::Current),
            (has_primary, MonitorCandidate::Primary),
        ],
    };
    candidates
        .into_iter()
        .find_map(|(available, candidate)| available.then_some(candidate))
}

fn window_state() -> &'static Mutex<IslandWindowState> {
    WINDOW_STATE.get_or_init(|| Mutex::new(IslandWindowState::default()))
}

fn state_transition() -> &'static Mutex<()> {
    STATE_TRANSITION.get_or_init(|| Mutex::new(()))
}

fn last_rasterization_error() -> &'static Mutex<Option<String>> {
    LAST_RASTERIZATION_ERROR.get_or_init(|| Mutex::new(None))
}

fn dpi_retry_state() -> &'static Mutex<DpiRetryState> {
    DPI_RETRY_STATE.get_or_init(|| Mutex::new(DpiRetryState::default()))
}

fn set_last_rasterization_error(error: Option<String>) {
    if let Ok(mut last_error) = last_rasterization_error().lock() {
        *last_error = error;
    }
}

fn latest_rasterization_error() -> Option<String> {
    last_rasterization_error()
        .lock()
        .ok()
        .and_then(|error| error.clone())
}

fn state_snapshot() -> Result<IslandWindowState, String> {
    window_state()
        .lock()
        .map(|state| state.clone())
        .map_err(|_| "window state poisoned".to_string())
}

fn main_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window(WINDOW_LABEL)
        .ok_or_else(|| "main window not found".to_string())
}

trait WindowDecorationPort {
    fn set_borderless(&self) -> Result<(), String>;
}

impl WindowDecorationPort for WebviewWindow {
    fn set_borderless(&self) -> Result<(), String> {
        self.set_decorations(false)
            .map_err(|error| error.to_string())
    }
}

fn enforce_borderless_window(window: &impl WindowDecorationPort) -> Result<(), String> {
    window.set_borderless()
}

/// Windows 无边框窗口的底层保险：subclass 窗口过程，拦截系统非客户区消息，
/// 阻止 DWM 在激活/失焦/尺寸变化时重新绘制原生标题栏与边框。
/// `window_vibrancy` 的 DwmExtendFrameIntoClientArea 会让 DWM 认为窗口仍有
/// 可绘制的 frame，仅靠 set_decorations(false) 清样式压不住运行时的 chrome 复活。
#[cfg(target_os = "windows")]
mod borderless_chrome {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{WM_NCACTIVATE, WM_NCCALCSIZE, WM_NCPAINT};

    const BORDERLESS_SUBCLASS_ID: usize = 0x4153_4C44; // "ASLD"

    unsafe extern "system" fn borderless_subclass_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _subclass_id: usize,
        _reference_data: usize,
    ) -> LRESULT {
        match message {
            // 客户区占满整个窗口区域，不给系统留非客户区
            WM_NCCALCSIZE if wparam.0 != 0 => LRESULT(0),
            // 激活/失焦时不再重绘标准标题栏
            WM_NCACTIVATE => LRESULT(1),
            // 吞掉非客户区绘制请求
            WM_NCPAINT => LRESULT(0),
            _ => unsafe { DefSubclassProc(window, message, wparam, lparam) },
        }
    }

    pub fn install(window: &tauri::WebviewWindow) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;
        let installed = unsafe {
            SetWindowSubclass(
                hwnd,
                Some(borderless_subclass_proc),
                BORDERLESS_SUBCLASS_ID,
                0,
            )
        };
        if installed.as_bool() {
            Ok(())
        } else {
            Err("SetWindowSubclass failed for main window".to_string())
        }
    }
}

fn show_borderless_window(window: &WebviewWindow) -> Result<(), String> {
    enforce_borderless_window(window)?;
    #[cfg(target_os = "windows")]
    borderless_chrome::install(window)?;
    window.show().map_err(|error| error.to_string())
}

fn remember_visible_placement(
    window: &WebviewWindow,
    position: PhysicalPoint,
) -> Result<(), String> {
    let monitor_name = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .and_then(|monitor| monitor.name().cloned());
    let dpi = window.scale_factor().map_err(|error| error.to_string())?;
    let mut state = window_state()
        .lock()
        .map_err(|_| "window state poisoned".to_string())?;
    if !state.is_tucked {
        state.saved_visible_placement = Some(SavedPlacement {
            position,
            monitor_name,
            dpi,
        });
    }
    Ok(())
}

fn snapshot_physical_geometry(window: &WebviewWindow) -> Result<PhysicalGeometrySnapshot, String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let size = window.outer_size().map_err(|error| error.to_string())?;
    Ok(PhysicalGeometrySnapshot {
        position: PhysicalPoint {
            x: position.x,
            y: position.y,
        },
        width: size.width,
        height: size.height,
    })
}

#[cfg(target_os = "windows")]
fn work_area_for_window(window: &WebviewWindow) -> Result<PhysicalWorkArea, String> {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };

    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(format!(
            "monitor_work_area stage=query error={}",
            windows_core::Error::from_win32()
        ));
    }
    Ok(PhysicalWorkArea {
        x: info.rcWork.left,
        width: info.rcWork.right.saturating_sub(info.rcWork.left) as u32,
    })
}

#[cfg(not(target_os = "windows"))]
fn work_area_for_window(window: &WebviewWindow) -> Result<PhysicalWorkArea, String> {
    let monitor = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "monitor not found".to_string())?;
    Ok(PhysicalWorkArea {
        x: monitor.position().x,
        width: monitor.size().width,
    })
}

fn monitor_for_placement(
    window: &WebviewWindow,
    placement: Option<&SavedPlacement>,
    preference: MonitorPreference,
) -> Result<tauri::window::Monitor, String> {
    let current = window
        .current_monitor()
        .map_err(|error| error.to_string())?;
    let saved = if let Some(name) = placement.and_then(|saved| saved.monitor_name.as_deref()) {
        window
            .available_monitors()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|monitor| monitor.name().is_some_and(|candidate| candidate == name))
    } else {
        None
    };
    let primary = window
        .primary_monitor()
        .map_err(|error| error.to_string())?;

    match select_monitor_candidate(
        current.is_some(),
        saved.is_some(),
        primary.is_some(),
        preference,
    ) {
        Some(MonitorCandidate::Current) => Ok(current.expect("candidate exists")),
        Some(MonitorCandidate::Saved) => Ok(saved.expect("candidate exists")),
        Some(MonitorCandidate::Primary) => Ok(primary.expect("candidate exists")),
        None => Err("monitor not found".to_string()),
    }
}

fn clamp_x_physical(
    requested_x: i32,
    window_width: u32,
    monitor_x: i32,
    monitor_width: u32,
) -> i32 {
    let minimum = monitor_x as i64;
    let maximum = minimum
        .saturating_add(monitor_width as i64)
        .saturating_sub(window_width as i64)
        .max(minimum);
    (requested_x as i64).clamp(minimum, maximum) as i32
}

fn clamp_x_to_monitor(
    requested_x: i32,
    window_width: u32,
    monitor: &tauri::window::Monitor,
) -> i32 {
    clamp_x_physical(
        requested_x,
        window_width,
        monitor.position().x,
        monitor.size().width,
    )
}

fn restore_target_physical(
    requested_x: i32,
    window_width: u32,
    monitor_x: i32,
    monitor_width: u32,
    monitor_top: i32,
    dpi: f64,
    margin_y: f64,
) -> PhysicalPoint {
    PhysicalPoint {
        x: clamp_x_physical(requested_x, window_width, monitor_x, monitor_width),
        y: safe_restore_y_physical(monitor_top, dpi, margin_y),
    }
}

fn physical_size_for_state(
    window: &WebviewWindow,
    state: &IslandWindowState,
) -> Result<(u32, u32, f64), String> {
    let dpi = window.scale_factor().map_err(|error| error.to_string())?;
    let logical = logical_size_for_state(state);
    let dimension = |value: f64| (value * dpi).round().clamp(1.0, i32::MAX as f64) as u32;
    Ok((dimension(logical.width), dimension(logical.height), dpi))
}

fn target_window_frame(
    window: &WebviewWindow,
    state: &IslandWindowState,
    anchor: Option<PhysicalGeometrySnapshot>,
    intent: GeometryIntent,
) -> Result<PhysicalWindowFrame, String> {
    let (width, height, dpi) = physical_size_for_state(window, state)?;
    let current_position = anchor
        .map(|snapshot| snapshot.position)
        .or_else(|| {
            window.outer_position().ok().map(|position| PhysicalPoint {
                x: position.x,
                y: position.y,
            })
        })
        .unwrap_or_default();

    let position = if matches!(intent, GeometryIntent::Tuck) || state.is_tucked {
        let monitor = monitor_for_placement(
            window,
            state.saved_visible_placement.as_ref(),
            MonitorPreference::CurrentFirst,
        )?;
        let requested_x = state
            .saved_visible_placement
            .as_ref()
            .map(|saved| saved.position.x)
            .unwrap_or(current_position.x);
        PhysicalPoint {
            x: clamp_x_to_monitor(requested_x, width, &monitor),
            y: tucked_y_physical(monitor.position().y, height, monitor.scale_factor()),
        }
    } else if matches!(intent, GeometryIntent::RestoreVisible) {
        let monitor = monitor_for_placement(
            window,
            state.saved_visible_placement.as_ref(),
            MonitorPreference::SavedFirst,
        )?;
        let requested_x = state
            .saved_visible_placement
            .as_ref()
            .map(|saved| saved.position.x)
            .unwrap_or(current_position.x);
        restore_target_physical(
            requested_x,
            width,
            monitor.position().x,
            monitor.size().width,
            monitor.position().y,
            monitor.scale_factor(),
            state.margin_y,
        )
    } else if state.saved_visible_placement.is_some() {
        let previous_width = anchor
            .map(|snapshot| snapshot.width)
            .or_else(|| window.outer_size().ok().map(|size| size.width))
            .unwrap_or(width);
        let center_x = current_position.x as i64 + previous_width as i64 / 2;
        PhysicalPoint {
            x: (center_x - width as i64 / 2).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            y: current_position.y,
        }
    } else {
        let monitor = window
            .primary_monitor()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "primary monitor not found".to_string())?;
        top_center_physical(
            monitor.size().width,
            monitor.size().height,
            width,
            height,
            monitor.position().x,
            monitor.position().y,
            monitor.scale_factor(),
            state.margin_y,
        )
    };

    Ok(PhysicalWindowFrame {
        position,
        width,
        height,
        corner_radius: physical_corner_radius(state, dpi),
    })
}

fn current_window_frame(
    window: &WebviewWindow,
    state: &IslandWindowState,
) -> Result<PhysicalWindowFrame, String> {
    let geometry = snapshot_physical_geometry(window)?;
    let dpi = window.scale_factor().map_err(|error| error.to_string())?;
    let fallback_radius = physical_corner_radius(state, dpi);
    let observed_radius = CURRENT_CORNER_RADIUS.load(Ordering::SeqCst);
    Ok(PhysicalWindowFrame {
        position: geometry.position,
        width: geometry.width,
        height: geometry.height,
        corner_radius: if observed_radius > 0 {
            observed_radius
        } else {
            fallback_radius
        },
    })
}

fn apply_current_window_region(
    window: &WebviewWindow,
    state: &IslandWindowState,
) -> Result<(), String> {
    let size = window.outer_size().map_err(|error| error.to_string())?;
    let dpi = window.scale_factor().map_err(|error| error.to_string())?;
    apply_window_region(
        window,
        size.width,
        size.height,
        physical_corner_radius(state, dpi),
    )
}

#[cfg(target_os = "windows")]
fn apply_window_region(
    window: &WebviewWindow,
    width: u32,
    height: u32,
    corner_radius: i32,
) -> Result<(), String> {
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn, HGDIOBJ};

    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    let right = width.min((i32::MAX - 1) as u32) as i32 + 1;
    let bottom = height.min((i32::MAX - 1) as u32) as i32 + 1;
    let diameter = corner_radius.max(1).saturating_mul(2);
    let region = unsafe { CreateRoundRectRgn(0, 0, right, bottom, diameter, diameter) };
    if region.is_invalid() {
        return Err(format!(
            "window_region stage=create error={}",
            windows_core::Error::from_win32()
        ));
    }

    if unsafe { SetWindowRgn(hwnd, Some(region), true) } == 0 {
        let error = windows_core::Error::from_win32();
        unsafe {
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
        return Err(format!("window_region stage=apply error={error}"));
    }
    CURRENT_CORNER_RADIUS.store(corner_radius.max(1), Ordering::SeqCst);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn apply_window_region(
    _window: &WebviewWindow,
    _width: u32,
    _height: u32,
    corner_radius: i32,
) -> Result<(), String> {
    CURRENT_CORNER_RADIUS.store(corner_radius.max(1), Ordering::SeqCst);
    Ok(())
}

fn apply_physical_window_frame(
    window: &WebviewWindow,
    frame: PhysicalWindowFrame,
) -> Result<(), String> {
    let _guard = ProgrammaticMoveGuard::enter();

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOREDRAW, SWP_NOZORDER,
        };
        let hwnd = window.hwnd().map_err(|error| error.to_string())?;
        unsafe {
            SetWindowPos(
                hwnd,
                None,
                frame.position.x,
                frame.position.y,
                frame.width.min(i32::MAX as u32) as i32,
                frame.height.min(i32::MAX as u32) as i32,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOREDRAW | SWP_NOZORDER,
            )
        }
        .map_err(|error| format!("window_geometry stage=set_bounds error={error}"))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        window
            .set_size(tauri::PhysicalSize::new(frame.width, frame.height))
            .map_err(|error| error.to_string())?;
        window
            .set_position(tauri::PhysicalPosition::new(
                frame.position.x,
                frame.position.y,
            ))
            .map_err(|error| error.to_string())?;
    }

    apply_window_region(window, frame.width, frame.height, frame.corner_radius)
}

fn apply_geometry(window: &WebviewWindow, state: IslandWindowState) -> Result<(), String> {
    apply_geometry_with_anchor(window, state, None, GeometryIntent::PreserveAnchor)
}

fn apply_geometry_with_anchor(
    window: &WebviewWindow,
    state: IslandWindowState,
    anchor: Option<PhysicalGeometrySnapshot>,
    intent: GeometryIntent,
) -> Result<(), String> {
    let frame = target_window_frame(window, &state, anchor, intent)?;
    apply_physical_window_frame(window, frame)?;
    if !state.is_tucked {
        remember_visible_placement(window, frame.position)?;
    }

    if !window.is_visible().map_err(|error| error.to_string())? {
        show_borderless_window(window)?;
    }
    Ok(())
}

fn restore_physical_geometry(
    window: &WebviewWindow,
    snapshot: PhysicalGeometrySnapshot,
    state: &IslandWindowState,
) -> Result<(), String> {
    let dpi = window.scale_factor().map_err(|error| error.to_string())?;
    apply_physical_window_frame(
        window,
        PhysicalWindowFrame {
            position: snapshot.position,
            width: snapshot.width,
            height: snapshot.height,
            corner_radius: physical_corner_radius(state, dpi),
        },
    )?;
    if !state.is_tucked {
        remember_visible_placement(window, snapshot.position)?;
    }
    Ok(())
}

fn restore_committed_mode_geometry(
    window: &WebviewWindow,
    committed: CommittedModeGeometry,
) -> Result<(), String> {
    apply_physical_window_frame(window, committed.frame)?;
    if committed.remember_visible_position {
        remember_visible_placement(window, committed.frame.position)?;
    }
    Ok(())
}

fn execute_state_transition_with_rollback<Update, Apply, Rollback, Commit>(
    old: IslandWindowState,
    update: Update,
    apply: Apply,
    rollback: Rollback,
    commit: Commit,
    field: &str,
) -> Result<(), String>
where
    Update: FnOnce(&mut IslandWindowState),
    Apply: FnOnce(&IslandWindowState) -> Result<(), String>,
    Rollback: FnOnce() -> Result<(), String>,
    Commit: FnOnce(&IslandWindowState) -> Result<(), String>,
{
    let mut candidate = old.clone();
    update(&mut candidate);

    if let Err(error) = apply(&candidate) {
        let rollback = rollback();
        return Err(format_transition_error(
            field,
            "candidate_geometry",
            error,
            rollback,
        ));
    }

    if let Err(error) = commit(&candidate) {
        let rollback = rollback();
        return Err(format_transition_error(field, "commit", error, rollback));
    }

    Ok(())
}

fn format_transition_error(
    field: &str,
    stage: &str,
    error: String,
    rollback: Result<(), String>,
) -> String {
    match rollback {
        Ok(()) => format!(
            "state_transition field={field} stage={stage} error={error} rollback=ok"
        ),
        Err(rollback_error) => format!(
            "state_transition field={field} stage={stage} error={error} rollback_error={rollback_error}"
        ),
    }
}

fn compensate_after_tuck_emit_failure<Emit, RestoreGeometry, RestoreState>(
    emit: Emit,
    restore_geometry: RestoreGeometry,
    restore_state: RestoreState,
) -> Result<(), String>
where
    Emit: FnOnce() -> Result<(), String>,
    RestoreGeometry: FnOnce() -> Result<(), String>,
    RestoreState: FnOnce() -> Result<(), String>,
{
    if let Err(error) = emit() {
        let geometry = restore_geometry();
        let state = restore_state();
        return match (geometry, state) {
            (Ok(()), Ok(())) => Err(format!(
                "state_transition field=tucked stage=emit error={error} compensation=ok"
            )),
            (geometry, state) => Err(format!(
                "state_transition field=tucked stage=emit error={error} compensation_geometry={geometry:?} compensation_state={state:?}"
            )),
        };
    }
    Ok(())
}

fn transition_window_state<Update, Commit>(
    app: &AppHandle,
    field: &str,
    update: Update,
    commit: Commit,
) -> Result<(), String>
where
    Update: FnOnce(&mut IslandWindowState),
    Commit: FnOnce(&mut IslandWindowState, &IslandWindowState),
{
    let _transition = state_transition().lock().map_err(|_| {
        format!("state_transition field={field} stage=serialize error=lock_poisoned")
    })?;
    let window = main_window(app)
        .map_err(|error| format!("state_transition field={field} stage=window error={error}"))?;
    let original_geometry = snapshot_physical_geometry(&window).map_err(|error| {
        format!("state_transition field={field} stage=geometry_snapshot error={error}")
    })?;
    let old = state_snapshot()
        .map_err(|error| format!("state_transition field={field} stage=snapshot error={error}"))?;

    execute_state_transition_with_rollback(
        old.clone(),
        update,
        |state| {
            apply_geometry_with_anchor(
                &window,
                state.clone(),
                Some(original_geometry),
                GeometryIntent::PreserveAnchor,
            )
        },
        || restore_physical_geometry(&window, original_geometry, &old),
        |candidate| {
            let mut current = window_state()
                .lock()
                .map_err(|_| "lock_poisoned".to_string())?;
            commit(&mut current, candidate);
            Ok(())
        },
        field,
    )
}

fn transition_window_width(
    app: &AppHandle,
    target_mode: IslandMode,
    requested_width: f64,
    fixed_edge: FixedHorizontalEdge,
) -> Result<f64, String> {
    let _transition = state_transition().lock().map_err(|_| {
        "state_transition field=width stage=serialize error=lock_poisoned".to_string()
    })?;
    let window = main_window(app)
        .map_err(|error| format!("state_transition field=width stage=window error={error}"))?;
    let original_geometry = snapshot_physical_geometry(&window).map_err(|error| {
        format!("state_transition field=width stage=geometry_snapshot error={error}")
    })?;
    let old = state_snapshot()
        .map_err(|error| format!("state_transition field=width stage=snapshot error={error}"))?;
    let work_area = work_area_for_window(&window)?;
    let dpi = window.scale_factor().map_err(|error| error.to_string())?;
    let width = clamp_width_to_work_area(
        target_mode,
        requested_width,
        old.scale,
        dpi,
        work_area.width,
        old.margin_y,
    );

    if target_mode != old.mode {
        let mut current = window_state().lock().map_err(|_| {
            "state_transition field=width stage=commit error=lock_poisoned".to_string()
        })?;
        match target_mode {
            IslandMode::Collapsed => current.collapsed_width = width,
            IslandMode::Expanded => current.expanded_width = width,
        }
        return Ok(width);
    }

    execute_state_transition_with_rollback(
        old.clone(),
        |candidate| match target_mode {
            IslandMode::Collapsed => candidate.collapsed_width = width,
            IslandMode::Expanded => candidate.expanded_width = width,
        },
        |candidate| {
            let mut frame = target_window_frame(
                &window,
                candidate,
                Some(original_geometry),
                GeometryIntent::PreserveAnchor,
            )?;
            frame.position.x = resized_x_for_fixed_edge(
                original_geometry.position.x,
                original_geometry.width,
                frame.width,
                fixed_edge,
            );
            frame.position.x =
                clamp_x_physical(frame.position.x, frame.width, work_area.x, work_area.width);
            apply_physical_window_frame(&window, frame)?;
            if !candidate.is_tucked {
                remember_visible_placement(&window, frame.position)?;
            }
            Ok(())
        },
        || restore_physical_geometry(&window, original_geometry, &old),
        |candidate| {
            let mut current = window_state()
                .lock()
                .map_err(|_| "lock_poisoned".to_string())?;
            match target_mode {
                IslandMode::Collapsed => current.collapsed_width = candidate.collapsed_width,
                IslandMode::Expanded => current.expanded_width = candidate.expanded_width,
            }
            Ok(())
        },
        "width",
    )?;
    Ok(width)
}

fn resolve_client_area_animation_preference(preference: Result<bool, ()>) -> bool {
    preference.unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn native_client_area_animations_enabled() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };

    let mut enabled = 0i32;
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut i32).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    let preference = match result {
        Ok(()) => Ok(enabled != 0),
        Err(error) => {
            log::warn!(
                target: "aisland::window",
                "window_animation stage=accessibility_query status=fallback_reduced_motion error={error}"
            );
            Err(())
        }
    };
    resolve_client_area_animation_preference(preference)
}

#[cfg(not(target_os = "windows"))]
fn native_client_area_animations_enabled() -> bool {
    true
}

fn transition_window_mode(
    app: &AppHandle,
    mode: IslandMode,
    generation: u64,
    animation_spec: WindowAnimationSpec,
) -> Result<(), String> {
    let _transition = state_transition().lock().map_err(|_| {
        "state_transition field=mode stage=serialize error=lock_poisoned".to_string()
    })?;
    require_current_mode_request(animation_generation_is_current(
        generation,
        MODE_ANIMATION_GENERATION.load(Ordering::SeqCst),
    ))?;

    let window = main_window(app)
        .map_err(|error| format!("state_transition field=mode stage=window error={error}"))?;
    let original_geometry = repair_pending_mode_geometry_before_snapshot(
        mode_geometry_repair_obligation(),
        |committed| restore_committed_mode_geometry(&window, committed),
        || {
            snapshot_physical_geometry(&window).map_err(|error| {
                format!("state_transition field=mode stage=geometry_snapshot error={error}")
            })
        },
    )?;
    let old = state_snapshot()
        .map_err(|error| format!("state_transition field=mode stage=snapshot error={error}"))?;
    let mut candidate = old.clone();
    candidate.mode = mode;
    let work_area = work_area_for_window(&window)?;
    let dpi = window.scale_factor().map_err(|error| error.to_string())?;
    match mode {
        IslandMode::Collapsed => {
            candidate.collapsed_width = clamp_width_to_work_area(
                mode,
                candidate.collapsed_width,
                candidate.scale,
                dpi,
                work_area.width,
                candidate.margin_y,
            );
        }
        IslandMode::Expanded => {
            candidate.expanded_width = clamp_width_to_work_area(
                mode,
                candidate.expanded_width,
                candidate.scale,
                dpi,
                work_area.width,
                candidate.margin_y,
            );
        }
    }
    let start = current_window_frame(&window, &old)?;
    let end = target_window_frame(
        &window,
        &candidate,
        Some(original_geometry),
        GeometryIntent::PreserveAnchor,
    )?;
    let animated = native_client_area_animations_enabled() && start != end;
    let committed_geometry = CommittedModeGeometry {
        frame: start,
        remember_visible_position: !old.is_tucked,
    };
    let _animation_move_guard = ProgrammaticMoveGuard::enter();

    let outcome = match drive_window_animation(
        start,
        end,
        animated,
        animation_spec,
        || {
            animation_generation_is_current(
                generation,
                MODE_ANIMATION_GENERATION.load(Ordering::SeqCst),
            )
        },
        |frame| apply_physical_window_frame(&window, frame),
        std::thread::sleep,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            let rollback = rollback_committed_mode_geometry_or_record(
                mode_geometry_repair_obligation(),
                committed_geometry,
                || restore_committed_mode_geometry(&window, committed_geometry),
            );
            return Err(format_transition_error(
                "mode",
                "candidate_geometry",
                error,
                rollback,
            ));
        }
    };

    if outcome == WindowAnimationOutcome::Superseded {
        return reject_superseded_mode_request_with_rollback(|| {
            rollback_committed_mode_geometry_or_record(
                mode_geometry_repair_obligation(),
                committed_geometry,
                || restore_committed_mode_geometry(&window, committed_geometry),
            )
        });
    }
    if !animation_generation_is_current(
        generation,
        MODE_ANIMATION_GENERATION.load(Ordering::SeqCst),
    ) {
        return reject_superseded_mode_request_with_rollback(|| {
            rollback_committed_mode_geometry_or_record(
                mode_geometry_repair_obligation(),
                committed_geometry,
                || restore_committed_mode_geometry(&window, committed_geometry),
            )
        });
    }

    let completion = complete_current_mode_request(
        mode_commit_gate(),
        &MODE_ANIMATION_GENERATION,
        generation,
        || {
            if let Err(error) = apply_physical_window_frame(&window, end) {
                let rollback = rollback_committed_mode_geometry_or_record(
                    mode_geometry_repair_obligation(),
                    committed_geometry,
                    || restore_committed_mode_geometry(&window, committed_geometry),
                );
                return Err(format_transition_error(
                    "mode",
                    "candidate_geometry",
                    error,
                    rollback,
                ));
            }
            let finish = (|| {
                if !candidate.is_tucked {
                    remember_visible_placement(&window, end.position)?;
                }
                if !window.is_visible().map_err(|error| error.to_string())? {
                    show_borderless_window(&window)?;
                }
                Ok(())
            })();
            if let Err(error) = finish {
                let rollback = rollback_committed_mode_geometry_or_record(
                    mode_geometry_repair_obligation(),
                    committed_geometry,
                    || restore_committed_mode_geometry(&window, committed_geometry),
                );
                return Err(format_transition_error(
                    "mode",
                    "candidate_geometry",
                    error,
                    rollback,
                ));
            }

            let mut current = window_state().lock().map_err(|_| {
                let rollback = rollback_committed_mode_geometry_or_record(
                    mode_geometry_repair_obligation(),
                    committed_geometry,
                    || restore_committed_mode_geometry(&window, committed_geometry),
                );
                format_transition_error("mode", "commit", "lock_poisoned".to_string(), rollback)
            })?;
            current.mode = candidate.mode;
            current.collapsed_width = candidate.collapsed_width;
            current.expanded_width = candidate.expanded_width;
            Ok(())
        },
    );
    match completion {
        Err(error) if error == SUPERSEDED_MODE_ERROR => {
            reject_superseded_mode_request_with_rollback(|| {
                rollback_committed_mode_geometry_or_record(
                    mode_geometry_repair_obligation(),
                    committed_geometry,
                    || restore_committed_mode_geometry(&window, committed_geometry),
                )
            })
        }
        other => other,
    }
}

fn transition_tucked_state(app: &AppHandle, tucked: bool) -> Result<(), String> {
    let _transition = state_transition().lock().map_err(|_| {
        "state_transition field=tucked stage=serialize error=lock_poisoned".to_string()
    })?;
    let window = main_window(app)
        .map_err(|error| format!("state_transition field=tucked stage=window error={error}"))?;
    let original_geometry = snapshot_physical_geometry(&window).map_err(|error| {
        format!("state_transition field=tucked stage=geometry_snapshot error={error}")
    })?;
    let old = state_snapshot()
        .map_err(|error| format!("state_transition field=tucked stage=snapshot error={error}"))?;

    if old.is_tucked == tucked {
        return Ok(());
    }

    let saved_placement = if tucked {
        let monitor = monitor_for_placement(
            &window,
            old.saved_visible_placement.as_ref(),
            MonitorPreference::CurrentFirst,
        )?;
        Some(SavedPlacement {
            position: PhysicalPoint {
                x: original_geometry.position.x,
                y: safe_restore_y_physical(
                    monitor.position().y,
                    monitor.scale_factor(),
                    old.margin_y,
                ),
            },
            monitor_name: monitor.name().cloned(),
            dpi: monitor.scale_factor(),
        })
    } else {
        old.saved_visible_placement.clone()
    };
    let intent = if tucked {
        GeometryIntent::Tuck
    } else {
        GeometryIntent::RestoreVisible
    };

    execute_state_transition_with_rollback(
        old.clone(),
        |candidate| {
            candidate.is_tucked = tucked;
            candidate.saved_visible_placement = saved_placement;
        },
        |state| apply_geometry_with_anchor(&window, state.clone(), Some(original_geometry), intent),
        || restore_physical_geometry(&window, original_geometry, &old),
        |candidate| {
            let mut current = window_state()
                .lock()
                .map_err(|_| "lock_poisoned".to_string())?;
            current.is_tucked = candidate.is_tucked;
            current.saved_visible_placement = candidate.saved_visible_placement.clone();
            Ok(())
        },
        "tucked",
    )?;

    let geometry_rollback_state = old.clone();
    compensate_after_tuck_emit_failure(
        || {
            window
                .emit("island-tucked-changed", tucked)
                .map_err(|error| error.to_string())
        },
        || restore_physical_geometry(&window, original_geometry, &geometry_rollback_state),
        || {
            let mut current = window_state()
                .lock()
                .map_err(|_| "lock_poisoned".to_string())?;
            *current = old;
            Ok(())
        },
    )
}

fn handle_geometry_failure(window: &WebviewWindow, mut message: String) {
    log::error!(target: "aisland::window", "{message}");
    set_last_rasterization_error(Some(message.clone()));

    let needs_show = match window.is_visible() {
        Ok(visible) => !visible,
        Err(error) => {
            message.push_str(&format!("; visibility_check_error={error}"));
            set_last_rasterization_error(Some(message.clone()));
            true
        }
    };

    if needs_show {
        if let Err(error) = show_borderless_window(window) {
            message.push_str(&format!("; recovery_show_error={error}; action=exit"));
            log::error!(target: "aisland::window", "{message}");
            set_last_rasterization_error(Some(message));
            window.app_handle().exit(1);
        }
    }
}

fn schedule_pending_geometry_retry(window: WebviewWindow) {
    let should_start = match dpi_retry_state().lock() {
        Ok(mut state) => state.start_worker_if_idle(),
        Err(_) => {
            log::error!(
                target: "aisland::window",
                "window_geometry action=dpi_retry error=lock_poisoned"
            );
            false
        }
    };
    if !should_start {
        return;
    }

    std::thread::spawn(move || {
        let mut observed_generation = 0;
        let mut attempts = 0;
        loop {
            let decision = match dpi_retry_state().lock() {
                Ok(mut state) => {
                    state.worker_attempt_or_exit(&mut observed_generation, &mut attempts)
                }
                Err(_) => DpiWorkerDecision::Exit,
            };
            let attempt = match decision {
                DpiWorkerDecision::Retry(attempt) => attempt,
                DpiWorkerDecision::Exit => {
                    log::warn!(
                        target: "aisland::window",
                        "window_geometry action=dpi_retry exhausted=true"
                    );
                    return;
                }
            };
            std::thread::sleep(std::time::Duration::from_millis(16 * attempt as u64));
            schedule_geometry_after_rasterization(window.clone());
            let pending = match dpi_retry_state().lock() {
                Ok(mut state) => {
                    let pending = state.has_pending_geometry();
                    if !pending {
                        state.worker_running = false;
                    }
                    pending
                }
                Err(_) => false,
            };
            if !pending {
                return;
            }
        }
    });
}

fn apply_current_geometry(window: &WebviewWindow, prior_error: Option<String>) {
    let _transition = match state_transition().try_lock() {
        Ok(transition) => transition,
        Err(_) => {
            schedule_pending_geometry_retry(window.clone());
            return;
        }
    };

    if let Err(error) = state_snapshot().and_then(|state| apply_geometry(window, state)) {
        let message = match prior_error {
            Some(prior_error) => format!("{prior_error}; geometry stage=apply error={error}"),
            None => format!("geometry stage=apply error={error}"),
        };
        handle_geometry_failure(window, message);
    } else if let Ok(mut state) = dpi_retry_state().lock() {
        state.mark_applied();
    }
}

#[cfg(target_os = "windows")]
fn controller_error(stage: &str, error: windows_core::Error, target_scale: f64) -> String {
    format!(
        "rasterization stage={stage} hresult=0x{:08X} target_scale={target_scale:.6}",
        error.code().0 as u32
    )
}

#[cfg(target_os = "windows")]
fn configure_rasterization(
    webview: tauri::webview::PlatformWebview,
    target_scale: f64,
) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Controller3, COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS,
    };
    use windows_core::Interface;

    if !target_scale.is_finite() || target_scale <= 0.0 {
        return Err(format!(
            "rasterization stage=validate target_scale={target_scale:.6} error=invalid_scale"
        ));
    }

    unsafe {
        let controller = webview.controller();
        let controller3: ICoreWebView2Controller3 = controller
            .cast()
            .map_err(|error| controller_error("cast_controller3", error, target_scale))?;
        controller3
            .SetBoundsMode(COREWEBVIEW2_BOUNDS_MODE_USE_RAW_PIXELS)
            .map_err(|error| controller_error("set_bounds_mode", error, target_scale))?;
        if let Err(error) = controller3.SetShouldDetectMonitorScaleChanges(false) {
            let _ = controller3.SetShouldDetectMonitorScaleChanges(true);
            return Err(controller_error(
                "disable_monitor_scale_detection",
                error,
                target_scale,
            ));
        }
        if let Err(error) = controller3.SetRasterizationScale(target_scale) {
            let restore = controller3.SetShouldDetectMonitorScaleChanges(true).err();
            let mut message = controller_error("set_rasterization_scale", error, target_scale);
            if let Some(restore_error) = restore {
                message.push_str(&format!(
                    "; restore_monitor_scale_detection_hresult=0x{:08X}",
                    restore_error.code().0 as u32
                ));
            }
            return Err(message);
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn schedule_geometry_after_rasterization(window: WebviewWindow) {
    let closure_window = window.clone();
    let fallback_window = window.clone();
    if let Err(error) = window.with_webview(move |webview| {
        let result = closure_window
            .scale_factor()
            .map_err(|error| format!("rasterization stage=read_scale error={error}"))
            .and_then(|target_scale| configure_rasterization(webview, target_scale));

        let prior_error = result.err();
        if let Some(error) = &prior_error {
            log::warn!(target: "aisland::window", "{error}");
            set_last_rasterization_error(Some(error.clone()));
        } else {
            set_last_rasterization_error(None);
        }
        apply_current_geometry(&closure_window, prior_error);
    }) {
        let message = format!("rasterization stage=schedule error={error}");
        log::error!(target: "aisland::window", "{message}");
        set_last_rasterization_error(Some(message.clone()));
        apply_current_geometry(&fallback_window, Some(message));
    }
}

#[cfg(not(target_os = "windows"))]
fn schedule_geometry_after_rasterization(window: WebviewWindow) {
    apply_current_geometry(&window, None);
}

struct TauriApplicationLifecycleActions<'a> {
    app: &'a AppHandle,
    window: Option<&'a tauri::Window>,
}

impl ApplicationLifecycleActions for TauriApplicationLifecycleActions<'_> {
    fn hide_to_tray(&mut self) {
        if let Some(window) = self.window {
            if let Err(error) = window.hide() {
                log_lifecycle_error("close_hide", error);
            }
        }
    }

    fn await_shutdown(&mut self) {
        let services = self.app.state::<Arc<AppServices>>();
        if let Err(error) = tauri::async_runtime::block_on(services.shutdown()) {
            log_lifecycle_error("shutdown", error.message_key);
        }
    }
}

fn reassert_borderless_for_main_window(app: &AppHandle, trigger: &str) {
    let Some(webview) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    if let Err(error) = enforce_borderless_window(&webview) {
        log::error!(
            target: "aisland::window",
            "window_chrome action=reassert_borderless trigger={trigger} error={error}"
        );
    }
}

fn handle_window_event(window: &tauri::Window, event: &WindowEvent) {
    match event {
        WindowEvent::CloseRequested { api, .. } if window.label() == WINDOW_LABEL => {
            api.prevent_close();
            let mut actions = TauriApplicationLifecycleActions {
                app: window.app_handle(),
                window: Some(window),
            };
            handle_application_lifecycle_event(
                ApplicationLifecycleEvent::CloseRequested,
                &mut actions,
            );
        }
        WindowEvent::Moved(position) if window.label() == WINDOW_LABEL => {
            let Some(webview) = window.app_handle().get_webview_window(WINDOW_LABEL) else {
                return;
            };
            let Ok(state) = state_snapshot() else {
                return;
            };
            if should_ignore_moved_event(&state) {
                return;
            }
            let Ok(Some(monitor)) = webview.current_monitor() else {
                return;
            };
            if state.mode == IslandMode::Collapsed
                && should_tuck_physical(position.y, monitor.position().y, monitor.scale_factor())
            {
                if let Err(error) = transition_tucked_state(window.app_handle(), true) {
                    log::error!(
                        target: "aisland::window",
                        "window_geometry action=auto_tuck error={error}"
                    );
                }
            } else if let Err(error) = remember_visible_placement(
                &webview,
                PhysicalPoint {
                    x: position.x,
                    y: position.y,
                },
            ) {
                log::error!(
                    target: "aisland::window",
                    "window_geometry action=remember_visible_placement error={error}"
                );
            }
        }
        WindowEvent::ScaleFactorChanged { .. } if window.label() == WINDOW_LABEL => {
            reassert_borderless_for_main_window(window.app_handle(), "scale_factor_changed");
            if let Ok(mut state) = dpi_retry_state().lock() {
                state.note_dpi_event();
            }
            if let Some(webview) = window.app_handle().get_webview_window(WINDOW_LABEL) {
                schedule_geometry_after_rasterization(webview);
            }
        }
        WindowEvent::Focused(focused) if window.label() == WINDOW_LABEL => {
            let trigger = if *focused { "focused" } else { "focus_lost" };
            reassert_borderless_for_main_window(window.app_handle(), trigger);
        }
        WindowEvent::Resized(_) if window.label() == WINDOW_LABEL => {
            reassert_borderless_for_main_window(window.app_handle(), "resized");
        }
        _ => {}
    }
}

#[tauri::command]
async fn set_island_mode(
    app: AppHandle,
    mode: String,
    motion: Option<String>,
) -> Result<(), String> {
    let mode = IslandMode::from_value(&mode)?;
    let animation_spec = window_animation_spec(motion.as_deref())?;
    let generation = issue_mode_request_generation(mode_commit_gate(), &MODE_ANIMATION_GENERATION)?;

    tauri::async_runtime::spawn_blocking(move || {
        transition_window_mode(&app, mode, generation, animation_spec)
    })
    .await
    .map_err(|error| format!("state_transition field=mode stage=join error={error}"))?
}

#[tauri::command]
fn set_island_scale(app: AppHandle, scale: f64) -> Result<(), String> {
    let scale = clamp_scale(scale);
    let window = main_window(&app)?;
    let work_area = work_area_for_window(&window)?;
    let dpi = window.scale_factor().map_err(|error| error.to_string())?;
    transition_window_state(
        &app,
        "scale",
        |candidate| {
            candidate.scale = scale;
            candidate.collapsed_width = clamp_width_to_work_area(
                IslandMode::Collapsed,
                candidate.collapsed_width,
                scale,
                dpi,
                work_area.width,
                candidate.margin_y,
            );
            candidate.expanded_width = clamp_width_to_work_area(
                IslandMode::Expanded,
                candidate.expanded_width,
                scale,
                dpi,
                work_area.width,
                candidate.margin_y,
            );
        },
        |current, candidate| {
            current.scale = candidate.scale;
            current.collapsed_width = candidate.collapsed_width;
            current.expanded_width = candidate.expanded_width;
        },
    )
}

fn apply_native_window_material(
    material: NativeWindowMaterial,
    apply_material: impl FnOnce(NativeWindowMaterial) -> Result<(), String>,
    enforce_borderless: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    apply_material(material)?;
    enforce_borderless()
}

fn apply_island_glass_transparency(
    window: &WebviewWindow,
    transparency: i32,
) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        apply_native_window_material(
            native_window_material_for_glass_transparency(transparency),
            |material| {
                match material {
                    NativeWindowMaterial::Clear => window_vibrancy::clear_acrylic(window),
                }
                .map_err(|error| error.to_string())
            },
            || enforce_borderless_window(window),
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, transparency);
        Ok(())
    }
}

#[tauri::command]
fn set_island_glass_transparency(app: AppHandle, transparency: i32) -> Result<(), String> {
    apply_island_glass_transparency(&main_window(&app)?, transparency)
}

#[tauri::command]
fn set_island_expanded_height(app: AppHandle, height: f64) -> Result<(), String> {
    let height = if height.is_finite() {
        height.clamp(DEFAULT_EXPANDED_HEIGHT, MAX_EXPANDED_HEIGHT)
    } else {
        DEFAULT_EXPANDED_HEIGHT
    };
    transition_window_state(
        &app,
        "expanded_height",
        |candidate| candidate.expanded_height = height,
        |current, candidate| current.expanded_height = candidate.expanded_height,
    )
}

#[tauri::command]
fn set_island_width(
    app: AppHandle,
    mode: String,
    width: f64,
    fixed_edge: String,
) -> Result<f64, String> {
    transition_window_width(
        &app,
        IslandMode::from_value(&mode)?,
        width,
        FixedHorizontalEdge::from_value(&fixed_edge)?,
    )
}

#[tauri::command]
fn set_island_tucked(app: AppHandle, tucked: bool) -> Result<(), String> {
    transition_tucked_state(&app, tucked)
}

#[tauri::command]
fn start_island_drag(app: AppHandle) -> Result<(), String> {
    main_window(&app)?
        .start_dragging()
        .map_err(|error| error.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitialState {
    scale: f64,
    dpi: f64,
    mode: &'static str,
    collapsed_width: f64,
    expanded_width: f64,
    expanded_height: f64,
    tucked: bool,
    rasterization_error: Option<String>,
}

#[tauri::command]
fn get_initial_state(app: AppHandle) -> Result<InitialState, String> {
    let window = main_window(&app)?;
    let dpi = window.scale_factor().map_err(|error| error.to_string())?;
    let state = state_snapshot()?;
    Ok(InitialState {
        scale: state.scale,
        dpi,
        mode: match state.mode {
            IslandMode::Collapsed => "collapsed",
            IslandMode::Expanded => "expanded",
        },
        collapsed_width: state.collapsed_width,
        expanded_width: state.expanded_width,
        expanded_height: state.expanded_height,
        tucked: state.is_tucked,
        rasterization_error: latest_rasterization_error(),
    })
}

#[tauri::command]
fn get_pending_tray_navigation() -> Option<PendingTrayNavigation> {
    TRAY_NAVIGATION_STATE.pending()
}

#[tauri::command]
fn acknowledge_tray_navigation(sequence: u64) {
    TRAY_NAVIGATION_STATE.acknowledge(sequence);
}

fn build_tray_menu<R: Runtime, M: Manager<R>>(
    manager: &M,
    labels: TrayLabels,
) -> tauri::Result<Menu<R>> {
    let show_item = MenuItem::with_id(manager, "show", labels.show_hide, true, None::<&str>)?;
    let settings_item =
        MenuItem::with_id(manager, "settings", labels.settings, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(manager, "quit", labels.quit, true, None::<&str>)?;
    Menu::with_items(manager, &[&show_item, &settings_item, &quit_item])
}

fn replace_tray_menu(app: &AppHandle, labels: TrayLabels) -> Result<(), String> {
    let menu = build_tray_menu(app, labels).map_err(|error| error.to_string())?;
    let tray = app
        .tray_by_id("main-tray")
        .ok_or_else(|| "tray icon not found".to_string())?;
    tray.set_menu(Some(menu)).map_err(|error| error.to_string())
}

#[tauri::command]
fn set_ui_language(
    app: AppHandle,
    services: tauri::State<'_, Arc<AppServices>>,
    language: String,
) -> Result<(), String> {
    let candidate = parse_ui_language(&language)?;
    commit_native_language_change(
        native_ui_language_state(),
        &services.settings,
        candidate,
        native_now_millis()?,
        |labels| replace_tray_menu(&app, labels),
    )
}

#[cfg(windows)]
fn open_windows_target(target: &std::ffi::OsStr) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("open target failed: {error}"))
}

#[cfg(not(windows))]
fn open_windows_target(_target: &std::ffi::OsStr) -> Result<(), String> {
    Err("opening project links is only supported on Windows".to_string())
}

#[tauri::command]
fn open_aisland_github() -> Result<(), String> {
    open_windows_target(std::ffi::OsStr::new(AISLAND_GITHUB_URL))
}

#[tauri::command]
fn open_project_readme(app: AppHandle) -> Result<(), String> {
    let bundled = app
        .path()
        .resource_dir()
        .map_err(|error| format!("resolve README resource failed: {error}"))?
        .join("README.md");
    let development = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("README.md");
    let candidate = if bundled.is_file() {
        bundled
    } else {
        development
    };
    let readme = candidate
        .canonicalize()
        .map_err(|error| format!("resolve README file failed: {error}"))?;
    if readme.file_name().and_then(|name| name.to_str()) != Some("README.md") {
        return Err("resolved README file has an unexpected name".to_string());
    }
    open_windows_target(readme.as_os_str())
}

fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let window = main_window(app)?;
    show_borderless_window(&window)?;
    window.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_island_to_tray(app: AppHandle) -> Result<(), String> {
    if app.tray_by_id("main-tray").is_none() {
        return Err("tray icon not found".to_string());
    }
    main_window(&app)?.hide().map_err(|error| error.to_string())
}

fn toggle_main_window(app: &AppHandle) -> Result<(), String> {
    let window = main_window(app)?;
    if window.is_visible().map_err(|error| error.to_string())? {
        window.hide().map_err(|error| error.to_string())
    } else {
        show_borderless_window(&window)?;
        window.set_focus().map_err(|error| error.to_string())
    }
}

fn log_lifecycle_error(action: &str, error: impl std::fmt::Display) {
    log::error!(target: "aisland::lifecycle", "action={action} error={error}");
}

fn current_unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

macro_rules! registered_handler_from_all_boundaries {
    (
        $($note_wire_name:ident => $note_implementation:ident),+ $(,)?
        ;
        $($clipboard_wire_name:ident => $clipboard_implementation:ident),+ $(,)?
        ;
        $($monitor_wire_name:ident => $monitor_implementation:ident),+ $(,)?
        ;
        $($notification_wire_name:ident => $notification_implementation:ident),+ $(,)?
    ) => {
        tauri::generate_handler![
            set_ui_language,
            open_aisland_github,
            open_project_readme,
            set_island_mode,
            set_island_scale,
            set_island_glass_transparency,
            set_island_expanded_height,
            set_island_width,
            set_island_tucked,
            start_island_drag,
            get_initial_state,
            hide_island_to_tray,
            get_pending_tray_navigation,
            acknowledge_tray_navigation,
            commands::foundation::getAppSnapshot,
            commands::foundation::listServiceHealth,
            commands::foundation::getDiagnostics,
            commands::foundation::checkStorageIntegrity,
            commands::settings_updates::get_general_settings,
            commands::settings_updates::save_general_settings,
            commands::settings_updates::check_for_update,
            commands::settings_updates::install_update,
            commands::agents::getAgentsSnapshot,
            commands::agents::installAgentIntegration,
            commands::agents::repairAgentIntegration,
            commands::agents::uninstallAgentIntegration,
            commands::agent_profiles::list_agent_integration_profiles,
            commands::agent_profiles::discover_agent_integration_candidates,
            commands::agent_profiles::get_agent_profiles_snapshot,
            commands::agent_profiles::save_agent_integration_profile,
            commands::agent_profiles::install_agent_integration_profile,
            commands::agent_profiles::repair_agent_integration_profile,
            commands::agent_profiles::uninstall_agent_integration_profile,
            commands::agent_profiles::delete_agent_integration_profile,
            commands::reminders::listReminderRules,
            commands::reminders::saveReminderRule,
            commands::reminders::deleteReminderRule,
            commands::reminders::replayReminderDeliveries,
            commands::reminders::commitReminderReplayCursor,
            commands::reminders::reloadReminderAlertGroup,
            commands::reminders::acknowledgeReminder,
            commands::reminders::completeReminder,
            commands::reminders::snoozeReminder,
            commands::reminders::getPendingReminderNavigation,
            commands::reminders::acknowledgeReminderNavigation,
            $(commands::notes::$note_implementation),+,
            $(commands::clipboard::$clipboard_implementation),+,
            $(commands::monitor::$monitor_implementation),+,
            $(commands::notifications::$notification_implementation),+
        ]
    };
}

macro_rules! registered_handler_from_monitor_boundaries {
    (
        $($note_wire_name:ident => $note_implementation:ident),+ $(,)?
        ;
        $($clipboard_wire_name:ident => $clipboard_implementation:ident),+ $(,)?
        ;
        $($monitor_wire_name:ident => $monitor_implementation:ident),+ $(,)?
    ) => {
        commands::notification_command_manifest!(
            registered_handler_from_all_boundaries;
            $($note_wire_name => $note_implementation),+
            ;
            $($clipboard_wire_name => $clipboard_implementation),+
            ;
            $($monitor_wire_name => $monitor_implementation),+
        )
    };
}

macro_rules! registered_handler_from_notes_and_clipboard {
    (
        $($note_wire_name:ident => $note_implementation:ident),+ $(,)?
        ;
        $($clipboard_wire_name:ident => $clipboard_implementation:ident),+ $(,)?
    ) => {
        commands::monitor_command_manifest!(
            registered_handler_from_monitor_boundaries;
            $($note_wire_name => $note_implementation),+
            ;
            $($clipboard_wire_name => $clipboard_implementation),+
        )
    };
}

macro_rules! registered_handler_from_notes {
    ($($note_wire_name:ident => $note_implementation:ident),+ $(,)?) => {
        commands::clipboard_command_manifest!(
            registered_handler_from_notes_and_clipboard;
            $($note_wire_name => $note_implementation),+
        )
    };
}

macro_rules! registered_handler {
    () => {
        commands::note_command_manifest!(registered_handler_from_notes)
    };
}

pub fn run() {
    tauri::Builder::default()
        .plugin(logging::plugin())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log::info!(target: "aisland::lifecycle", "event=second_instance action=show_main_window");
            if let Err(error) = show_main_window(app) {
                log_lifecycle_error("single_instance_show", error);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(registered_handler!())
        .setup(|app| {
            logging::install_panic_hook();
            let log_dir = app
                .path()
                .app_log_dir()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            log::info!(
                target: "aisland::lifecycle",
                "event=startup version={} log_dir={}",
                app.package_info().version,
                log_dir.display()
            );

            let services = AppServices::new(app.handle()).map_err(|error| {
                log::error!(
                    target: "aisland::startup",
                    "stage=services status=failed error={}",
                    error.message_key
                );
                std::io::Error::other(error.message_key)
            })?;
            let product_settings = Arc::new(
                services::product_settings::ProductSettingsService::new(
                    services.settings.clone(),
                    Arc::new(services::product_settings::TauriAutostartPort::new(
                        app.handle().clone(),
                    )),
                ),
            );
            if let Err(error) = product_settings.reconcile_startup(current_unix_millis()) {
                log::warn!(
                    target: "aisland::autostart",
                    "stage=startup_reconcile status=degraded error={}",
                    error.message_key
                );
            }
            let app_updates = Arc::new(services::app_updates::AppUpdateService::new(Arc::new(
                services::app_updates::TauriUpdaterPort::new(app.handle().clone()),
            )));
            app.manage(services.clone());
            app.manage(product_settings);
            app.manage(app_updates);
            log::info!(target: "aisland::startup", "stage=services status=ready");
            let restored_profiles = services
                .restore_agent_profiles_once()
                .map_err(|error| {
                    log::error!(
                        target: "aisland::startup",
                        "stage=agent_profiles status=failed error={}",
                        error.message_key
                    );
                    std::io::Error::other(error.message_key)
                })?;
            log::info!(
                target: "aisland::startup",
                "stage=agent_profiles status=ready restored={restored_profiles}"
            );
            #[cfg(windows)]
            services
                .start_optional_modules_once(app.handle().clone())
                .map_err(|error| {
                    log::error!(
                        target: "aisland::startup",
                        "stage=optional_modules status=failed error={}",
                        error.message_key
                    );
                    std::io::Error::other(error.message_key)
                })?;
            #[cfg(windows)]
            log::info!(
                target: "aisland::startup",
                "stage=optional_modules status=ready"
            );
            #[cfg(windows)]
            services
                .start_notification_history_worker_once(app.handle().clone())
                .map_err(|error| {
                    log::error!(
                        target: "aisland::startup",
                        "stage=notification_history_worker status=failed error={}",
                        error.message_key
                    );
                    std::io::Error::other(error.message_key)
                })?;
            #[cfg(windows)]
            log::info!(
                target: "aisland::startup",
                "stage=notification_history_worker status=ready"
            );
            services
                .start_reminder_worker_once()
                .map_err(|error| {
                    log::error!(
                        target: "aisland::startup",
                        "stage=reminder_worker status=failed error={}",
                        error.message_key
                    );
                    std::io::Error::other(error.message_key)
                })?;
            log::info!(
                target: "aisland::startup",
                "stage=reminder_worker status=ready"
            );
            services
                .start_reminder_channel_worker_once()
                .map_err(|error| {
                    log::error!(
                        target: "aisland::startup",
                        "stage=reminder_channel_worker status=failed error={}",
                        error.message_key
                    );
                    std::io::Error::other(error.message_key)
                })?;
            log::info!(
                target: "aisland::startup",
                "stage=reminder_channel_worker status=ready"
            );
            let status_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| std::io::Error::other(error.to_string()))?
                .join("agent-status");
            services
                .start_agent_status_watcher_once(status_dir.clone())
                .map_err(|error| {
                    log::error!(
                        target: "aisland::startup",
                        "stage=agent_status_watcher status=failed error={}",
                        error.message_key
                    );
                    std::io::Error::other(error.message_key)
                })?;
            log::info!(
                target: "aisland::startup",
                "stage=agent_status_watcher status=ready directory={}",
                status_dir.display()
            );
            let initial_language = current_native_ui_language().map_err(std::io::Error::other)?;
            let menu = build_tray_menu(app, tray_labels(initial_language))?;

            TrayIconBuilder::with_id("main-tray")
                .icon(
                    app.default_window_icon()
                        .ok_or_else(|| std::io::Error::other("default window icon missing"))?
                        .clone(),
                )
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        log::info!(target: "aisland::lifecycle", "source=tray action=toggle");
                        if let Err(error) = toggle_main_window(app) {
                            log_lifecycle_error("tray_menu_toggle", error);
                        }
                    }
                    "settings" => {
                        log::info!(target: "aisland::lifecycle", "source=tray action=open_settings");
                        TRAY_NAVIGATION_STATE.request();
                        if let Err(error) = show_main_window(app) {
                            log_lifecycle_error("tray_settings_show", error);
                        }
                        if let Err(error) = app.emit_to(WINDOW_LABEL, "tray-navigate", "settings") {
                            log_lifecycle_error("tray_settings_emit", error);
                        }
                    }
                    "quit" => {
                        log::info!(target: "aisland::lifecycle", "source=tray action=quit");
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        log::info!(target: "aisland::lifecycle", "source=tray_icon action=toggle");
                        if let Err(error) = toggle_main_window(tray.app_handle()) {
                            log_lifecycle_error("tray_left_toggle", error);
                        }
                    }
                })
                .build(app)?;

            if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
                #[cfg(target_os = "windows")]
                {
                    if let Err(error) = state_snapshot()
                        .and_then(|state| apply_current_window_region(&window, &state))
                    {
                        log::warn!(
                            target: "aisland::window",
                            "window_effect effect=rounded_region status=failed error={error}"
                        );
                    }
                    if let Err(error) =
                        apply_island_glass_transparency(&window, DEFAULT_GLASS_TRANSPARENCY)
                    {
                        log::warn!(
                            target: "aisland::window",
                            "window_effect effect=acrylic status=failed error={error}"
                        );
                    }
                }
                enforce_borderless_window(&window).map_err(std::io::Error::other)?;
                schedule_geometry_after_rasterization(window);
            } else {
                log::warn!(
                    target: "aisland::startup",
                    "stage=main_window status=missing"
                );
            }
            log::info!(target: "aisland::lifecycle", "event=startup status=ready");
            Ok(())
        })
        .on_window_event(handle_window_event)
        .build(tauri::generate_context!())
        .expect("error while building AIsland")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                log::info!(target: "aisland::lifecycle", "event=shutdown status=started");
                let mut actions = TauriApplicationLifecycleActions {
                    app: app_handle,
                    window: None,
                };
                handle_application_lifecycle_event(ApplicationLifecycleEvent::Exit, &mut actions);
                log::info!(target: "aisland::lifecycle", "event=shutdown status=completed");
                logging::flush();
            }
        });
}

#[cfg(test)]
mod native_language_tests {
    use super::*;
    use crate::repositories::app_settings::AppSettingsRepository;
    use crate::storage::Storage;
    use std::cell::Cell;
    use std::sync::Arc;

    #[test]
    fn native_startup_reads_the_durable_locale_before_the_react_restore() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let path = directory.keep();
        let settings = AppSettingsRepository::new(Arc::new(Storage::open(&path).unwrap()));
        settings.put("ui.locale", &"en-US", None, 1).unwrap();

        assert_eq!(
            persisted_native_ui_language(&settings).unwrap(),
            UiLanguage::EnUs
        );
    }

    #[test]
    fn failed_tray_change_keeps_native_and_durable_locale_at_the_same_committed_value() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let path = directory.keep();
        let settings = AppSettingsRepository::new(Arc::new(Storage::open(&path).unwrap()));
        settings.put("ui.locale", &"zh-CN", None, 1).unwrap();
        let state = Mutex::new(UiLanguage::ZhCn);

        let error = commit_native_language_change(&state, &settings, UiLanguage::EnUs, 2, |_| {
            Err("tray rejected replacement".into())
        })
        .expect_err("a failed tray replacement must not persist an uncommitted locale");

        assert!(error.contains("stage=menu"));
        assert_eq!(*state.lock().unwrap(), UiLanguage::ZhCn);
        assert_eq!(
            settings
                .get::<String>("ui.locale")
                .unwrap()
                .map(|row| row.0),
            Some("zh-CN".into())
        );
    }

    #[test]
    fn matching_native_locale_backfills_missing_durable_locale_for_the_next_restart() {
        let directory = tempfile::tempdir().expect("temporary settings directory");
        let path = directory.keep();
        let settings = AppSettingsRepository::new(Arc::new(Storage::open(&path).unwrap()));
        let state = Mutex::new(UiLanguage::EnUs);
        let menu_calls = Cell::new(0);

        commit_native_language_change(&state, &settings, UiLanguage::EnUs, 2, |_| {
            menu_calls.set(menu_calls.get() + 1);
            Ok(())
        })
        .unwrap();

        assert_eq!(menu_calls.get(), 0);
        assert_eq!(
            settings
                .get::<String>("ui.locale")
                .unwrap()
                .map(|row| row.0),
            Some("en-US".into())
        );
    }

    #[test]
    fn parses_only_supported_ui_languages() {
        assert_eq!(parse_ui_language("zh-CN"), Ok(UiLanguage::ZhCn));
        assert_eq!(parse_ui_language("en-US"), Ok(UiLanguage::EnUs));

        let error = parse_ui_language("fr-FR").expect_err("unknown language must be rejected");
        assert!(error.contains("native_language stage=parse"));
    }

    #[test]
    fn supplies_localized_labels_for_every_tray_action() {
        assert_eq!(
            tray_labels(UiLanguage::ZhCn),
            TrayLabels {
                show_hide: "显示/隐藏",
                settings: "设置",
                quit: "退出",
            }
        );
        assert_eq!(
            tray_labels(UiLanguage::EnUs),
            TrayLabels {
                show_hide: "Show/Hide",
                settings: "Settings",
                quit: "Quit",
            }
        );
    }

    #[test]
    fn failed_menu_adapter_keeps_the_confirmed_language() {
        let state = Mutex::new(UiLanguage::ZhCn);

        let error = apply_native_language_change(&state, UiLanguage::EnUs, |_| {
            Err("tray refused the replacement menu".to_string())
        })
        .expect_err("a menu failure must not commit the candidate language");

        assert!(error.contains("native_language stage=menu"));
        assert_eq!(*state.lock().expect("state lock"), UiLanguage::ZhCn);
    }

    #[test]
    fn successful_change_commits_once_and_repeating_it_is_idempotent() {
        let state = Mutex::new(UiLanguage::ZhCn);
        let adapter_calls = Cell::new(0);

        apply_native_language_change(&state, UiLanguage::EnUs, |labels| {
            adapter_calls.set(adapter_calls.get() + 1);
            assert_eq!(labels, tray_labels(UiLanguage::EnUs));
            Ok(())
        })
        .expect("a successful menu update should commit the language");
        apply_native_language_change(&state, UiLanguage::EnUs, |_| {
            adapter_calls.set(adapter_calls.get() + 1);
            Ok(())
        })
        .expect("repeating the confirmed language should be a no-op");

        assert_eq!(*state.lock().expect("state lock"), UiLanguage::EnUs);
        assert_eq!(adapter_calls.get(), 1);
    }
}

#[cfg(test)]
mod main_window_chrome_tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct DecorationSpy(Cell<usize>);

    impl WindowDecorationPort for DecorationSpy {
        fn set_borderless(&self) -> Result<(), String> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn runtime_main_window_is_forced_borderless_before_it_is_shown() {
        let spy = DecorationSpy(Cell::new(0));

        enforce_borderless_window(&spy).unwrap();

        assert_eq!(spy.0.get(), 1);
    }

    #[test]
    fn glass_transparency_reasserts_borderless_after_native_material_change() {
        let calls = RefCell::new(Vec::new());

        apply_native_window_material(
            NativeWindowMaterial::Clear,
            |_| {
                calls.borrow_mut().push("material");
                Ok(())
            },
            || {
                calls.borrow_mut().push("borderless");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(*calls.borrow(), ["material", "borderless"]);
    }
}

#[cfg(test)]
mod reminder_alert_descriptor_tests {
    #[test]
    fn updater_plugin_always_has_a_deserializable_fail_closed_config_object() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert!(config["plugins"]["updater"].is_object());
        assert!(config["plugins"]["updater"]["endpoints"].is_array());
        assert!(config["plugins"]["updater"]["pubkey"].is_string());
    }

    #[test]
    fn static_alert_window_and_capability_match_the_task_8_boundary() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let windows = config["app"]["windows"].as_array().unwrap();
        let alert = windows
            .iter()
            .find(|window| window["label"] == "reminder-alert")
            .expect("Task 8 requires a static reminder-alert window");
        assert_eq!(alert["visible"], false);
        assert_eq!(alert["alwaysOnTop"], true);
        assert_eq!(alert["focus"], false);
        assert_eq!(alert["width"], 360);
        assert_eq!(alert["height"], 220);
        assert_eq!(alert["transparent"], true);
        assert_eq!(alert["decorations"], false);
        assert_eq!(alert["resizable"], false);
        assert_eq!(alert["skipTaskbar"], true);

        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        assert_eq!(
            capability["windows"],
            serde_json::json!(["main", "reminder-alert"])
        );
        assert!(capability["permissions"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("notification:default")));
    }
}

#[cfg(test)]
mod tray_navigation_tests {
    use super::*;

    #[test]
    fn request_before_listener_remains_pending_for_replay() {
        let state = TrayNavigationState::default();

        let sequence = state.request();
        let pending = state
            .pending()
            .expect("settings request should remain pending");

        assert_eq!(sequence, 1);
        assert_eq!(pending.page, "settings");
        assert_eq!(pending.sequence, 1);
    }

    #[test]
    fn acknowledging_older_sequence_preserves_newer_request() {
        let state = TrayNavigationState::default();
        let first = state.request();
        let second = state.request();

        state.acknowledge(first);

        assert_eq!(second, 2);
        assert_eq!(state.pending().map(|pending| pending.sequence), Some(2));
    }

    #[test]
    fn acknowledging_latest_sequence_clears_pending() {
        let state = TrayNavigationState::default();
        let latest = state.request();

        state.acknowledge(latest);

        assert!(state.pending().is_none());
    }
}

#[cfg(test)]
mod transition_tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn failed_tuck_emit_compensates_committed_geometry_and_state() {
        let original = PhysicalPoint { x: 680, y: 12 };
        let current_geometry = RefCell::new(PhysicalPoint { x: 680, y: -48 });
        let current_tucked = Cell::new(true);

        let error = compensate_after_tuck_emit_failure(
            || Err("event delivery failed".to_string()),
            || {
                *current_geometry.borrow_mut() = original;
                Ok(())
            },
            || {
                current_tucked.set(false);
                Ok(())
            },
        )
        .expect_err("an emit failure must compensate the committed tuck transition");

        assert!(error.contains("stage=emit"));
        assert_eq!(*current_geometry.borrow(), original);
        assert!(!current_tucked.get());
    }

    #[test]
    fn automatic_tuck_prefers_the_current_monitor_over_stale_saved_monitor() {
        assert_eq!(
            select_monitor_candidate(true, true, true, MonitorPreference::CurrentFirst),
            Some(MonitorCandidate::Current),
        );
        assert_eq!(
            select_monitor_candidate(true, true, true, MonitorPreference::SavedFirst),
            Some(MonitorCandidate::Saved),
        );
        assert_eq!(
            select_monitor_candidate(false, false, true, MonitorPreference::CurrentFirst),
            Some(MonitorCandidate::Primary),
        );
    }

    #[test]
    fn dpi_retry_is_bounded_but_a_new_event_reopens_the_latest_geometry_attempt() {
        let mut state = DpiRetryState::default();
        state.note_dpi_event();
        let mut observed_generation = 0;
        let mut attempts = 0;

        for expected in 1..=MAX_DPI_RETRY_ATTEMPTS {
            assert_eq!(
                state.next_retry_attempt(&mut observed_generation, &mut attempts),
                Some(expected),
            );
        }
        assert_eq!(
            state.next_retry_attempt(&mut observed_generation, &mut attempts),
            None
        );

        state.note_dpi_event();
        assert_eq!(
            state.next_retry_attempt(&mut observed_generation, &mut attempts),
            Some(1),
        );
    }

    #[test]
    fn dpi_worker_exit_releases_the_worker_slot_before_a_new_generation_can_start() {
        let mut state = DpiRetryState::default();
        state.note_dpi_event();
        assert!(state.start_worker_if_idle());
        let mut observed_generation = state.latest_generation;
        let mut attempts = MAX_DPI_RETRY_ATTEMPTS;

        assert_eq!(
            state.worker_attempt_or_exit(&mut observed_generation, &mut attempts),
            DpiWorkerDecision::Exit,
        );
        assert!(!state.worker_running);

        state.note_dpi_event();
        assert!(state.start_worker_if_idle());
    }

    #[test]
    fn tuck_geometry_helpers_clamp_x_and_restore_to_the_selected_monitor() {
        assert_eq!(clamp_x_physical(-50, 320, -100, 1920), -50);
        assert_eq!(clamp_x_physical(-500, 320, -100, 1920), -100);
        assert_eq!(clamp_x_physical(1900, 320, -100, 1920), 1500);

        assert_eq!(
            restore_target_physical(1800, 320, -100, 1920, -200, 2.0, 12.0),
            PhysicalPoint { x: 1500, y: -176 },
        );
    }

    #[test]
    fn failed_tuck_candidate_restores_original_geometry_without_committing_tucked_state() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct TestGeometry {
            position: PhysicalPoint,
            size: (u32, u32),
        }

        let old_state = IslandWindowState::default();
        let original = TestGeometry {
            position: PhysicalPoint { x: 680, y: 12 },
            size: (320, 58),
        };
        let current = RefCell::new(original);
        let committed = Cell::new(false);

        let error = execute_state_transition_with_rollback(
            old_state,
            |candidate| candidate.is_tucked = true,
            |_| {
                current.borrow_mut().position = PhysicalPoint { x: 680, y: -48 };
                Err("tuck move failed".to_string())
            },
            || {
                *current.borrow_mut() = original;
                Ok(())
            },
            |_| {
                committed.set(true);
                Ok(())
            },
            "tucked",
        )
        .expect_err("tuck geometry failure must roll back");

        assert!(error.contains("stage=candidate_geometry"));
        assert_eq!(*current.borrow(), original);
        assert!(!committed.get());
    }

    #[test]
    fn failed_untuck_candidate_keeps_tucked_geometry_and_state() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct TestGeometry {
            position: PhysicalPoint,
            size: (u32, u32),
        }

        let old_state = IslandWindowState {
            is_tucked: true,
            ..IslandWindowState::default()
        };
        let original = TestGeometry {
            position: PhysicalPoint { x: 680, y: -48 },
            size: (320, 58),
        };
        let current = RefCell::new(original);
        let committed = Cell::new(false);

        let error = execute_state_transition_with_rollback(
            old_state,
            |candidate| candidate.is_tucked = false,
            |_| {
                current.borrow_mut().position = PhysicalPoint { x: 680, y: 12 };
                Err("untuck move failed".to_string())
            },
            || {
                *current.borrow_mut() = original;
                Ok(())
            },
            |_| {
                committed.set(true);
                Ok(())
            },
            "tucked",
        )
        .expect_err("untuck geometry failure must roll back");

        assert!(error.contains("stage=candidate_geometry"));
        assert_eq!(*current.borrow(), original);
        assert!(!committed.get());
    }

    #[test]
    fn programmatic_or_stale_moved_events_do_not_start_recursive_tuck_or_replace_saved_placement() {
        let saved = SavedPlacement {
            position: PhysicalPoint { x: 680, y: 12 },
            monitor_name: Some("secondary".to_string()),
            dpi: 2.0,
        };
        let tucked = IslandWindowState {
            is_tucked: true,
            saved_visible_placement: Some(saved.clone()),
            ..IslandWindowState::default()
        };

        assert!(should_ignore_moved_event(&tucked));
        assert_eq!(tucked.saved_visible_placement, Some(saved));

        let guard = ProgrammaticMoveGuard::enter();
        assert!(should_ignore_moved_event(&IslandWindowState::default()));
        drop(guard);
        assert!(!should_ignore_moved_event(&IslandWindowState::default()));
    }

    #[test]
    fn failed_candidate_geometry_rolls_back_without_committing() {
        let old = IslandWindowState::default();
        let applied_modes = RefCell::new(Vec::new());
        let committed = Cell::new(false);

        let error = execute_state_transition_with_rollback(
            old,
            |candidate| candidate.mode = IslandMode::Expanded,
            |state| {
                applied_modes.borrow_mut().push(state.mode);
                if state.mode == IslandMode::Expanded {
                    Err("candidate rejected".to_string())
                } else {
                    Ok(())
                }
            },
            || {
                applied_modes.borrow_mut().push(IslandMode::Collapsed);
                Ok(())
            },
            |_| {
                committed.set(true);
                Ok(())
            },
            "mode",
        )
        .expect_err("candidate geometry must fail");

        assert!(error.contains("stage=candidate_geometry"));
        assert_eq!(
            applied_modes.into_inner(),
            vec![IslandMode::Expanded, IslandMode::Collapsed]
        );
        assert!(!committed.get());
    }

    #[test]
    fn failed_commit_rolls_geometry_back_to_the_old_state() {
        let old = IslandWindowState::default();
        let applied_scales = RefCell::new(Vec::new());

        let error = execute_state_transition_with_rollback(
            old,
            |candidate| candidate.scale = 1.4,
            |state| {
                applied_scales.borrow_mut().push(state.scale);
                Ok(())
            },
            || {
                applied_scales.borrow_mut().push(1.0);
                Ok(())
            },
            |_| Err("state commit rejected".to_string()),
            "scale",
        )
        .expect_err("commit must fail");

        assert!(error.contains("stage=commit"));
        assert_eq!(applied_scales.into_inner(), vec![1.4, 1.0]);
    }

    #[test]
    fn partial_candidate_geometry_failure_restores_exact_physical_snapshot() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct TestGeometry {
            position: (i32, i32),
            size: (u32, u32),
        }

        let old_state = IslandWindowState::default();
        let original = TestGeometry {
            position: (1680, 18),
            size: (480, 87),
        };
        let current = RefCell::new(original);
        let committed = Cell::new(false);

        let error = execute_state_transition_with_rollback(
            old_state,
            |candidate| candidate.mode = IslandMode::Expanded,
            |_| {
                current.borrow_mut().size = (840, 459);
                Err("position update failed after resize".to_string())
            },
            || {
                *current.borrow_mut() = original;
                Ok(())
            },
            |_| {
                committed.set(true);
                Ok(())
            },
            "mode",
        )
        .expect_err("candidate geometry must fail after mutating size");

        assert!(error.contains("stage=candidate_geometry"));
        assert_eq!(*current.borrow(), original);
        assert!(!committed.get());
    }

    #[test]
    fn newer_mode_request_cancels_the_old_animation_without_applying_its_final_frame() {
        let start = crate::window::PhysicalWindowFrame {
            position: PhysicalPoint { x: 100, y: 12 },
            width: 248,
            height: 46,
            corner_radius: 16,
        };
        let end = crate::window::PhysicalWindowFrame {
            position: PhysicalPoint { x: -56, y: 12 },
            width: 560,
            height: 306,
            corner_radius: 24,
        };
        let current = Cell::new(true);
        let applied = RefCell::new(Vec::new());

        let outcome = drive_window_animation(
            start,
            end,
            true,
            window_animation_spec(None).unwrap(),
            || current.get(),
            |frame| {
                applied.borrow_mut().push(frame);
                current.set(false);
                Ok(())
            },
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome, WindowAnimationOutcome::Superseded);
        assert_eq!(applied.borrow().len(), 1);
        assert_ne!(applied.borrow().last().copied(), Some(end));
    }

    #[test]
    fn superseded_mode_command_rejects_instead_of_confirming_an_uncommitted_mode() {
        assert_eq!(
            require_current_mode_request(false).unwrap_err(),
            "state_transition field=mode stage=superseded error=newer_request"
        );
        assert_eq!(require_current_mode_request(true), Ok(()));
    }

    #[test]
    fn superseded_partial_frame_is_restored_before_a_following_failed_request_snapshots_geometry() {
        let committed_geometry = crate::window::PhysicalWindowFrame {
            position: PhysicalPoint { x: 100, y: 12 },
            width: 248,
            height: 46,
            corner_radius: 16,
        };
        let requested_geometry = crate::window::PhysicalWindowFrame {
            position: PhysicalPoint { x: -56, y: 12 },
            width: 560,
            height: 306,
            corner_radius: 24,
        };
        let physical_geometry = Cell::new(committed_geometry);
        let current = Cell::new(true);

        let outcome = drive_window_animation(
            committed_geometry,
            requested_geometry,
            true,
            window_animation_spec(None).unwrap(),
            || current.get(),
            |frame| {
                physical_geometry.set(frame);
                current.set(false);
                Ok(())
            },
            |_| {},
        )
        .unwrap();

        assert_eq!(outcome, WindowAnimationOutcome::Superseded);
        assert_ne!(physical_geometry.get(), committed_geometry);
        assert_ne!(physical_geometry.get(), requested_geometry);

        let error = reject_superseded_mode_request_with_rollback(|| {
            physical_geometry.set(committed_geometry);
            Ok(())
        })
        .unwrap_err();
        assert_eq!(
            error,
            "state_transition field=mode stage=superseded error=newer_request"
        );
        assert_eq!(physical_geometry.get(), committed_geometry);

        let following_request_snapshot = physical_geometry.get();
        physical_geometry.set(requested_geometry);
        physical_geometry.set(following_request_snapshot);
        assert_eq!(physical_geometry.get(), committed_geometry);
    }

    #[test]
    fn failed_superseded_rollback_blocks_the_next_snapshot_until_committed_geometry_is_repaired() {
        let committed = PhysicalWindowFrame {
            position: PhysicalPoint { x: 100, y: 12 },
            width: 248,
            height: 46,
            corner_radius: 16,
        };
        let intermediate = PhysicalWindowFrame {
            position: PhysicalPoint { x: 30, y: 12 },
            width: 388,
            height: 162,
            corner_radius: 20,
        };
        let requested = PhysicalWindowFrame {
            position: PhysicalPoint { x: -56, y: 12 },
            width: 560,
            height: 306,
            corner_radius: 24,
        };
        let repair = Mutex::new(None);
        let physical = Cell::new(intermediate);

        let rollback = rollback_committed_mode_geometry_or_record(
            &repair,
            CommittedModeGeometry {
                frame: committed,
                remember_visible_position: true,
            },
            || Err("native rollback failed".to_string()),
        );
        assert_eq!(rollback.unwrap_err(), "native rollback failed");

        let snapshot_calls = Cell::new(0);
        let blocked = repair_pending_mode_geometry_before_snapshot(
            &repair,
            |_| Err("native repair still failing".to_string()),
            || {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Ok(physical.get())
            },
        )
        .unwrap_err();
        assert!(blocked.contains("stage=repair_pending"));
        assert_eq!(snapshot_calls.get(), 0);
        assert_eq!(physical.get(), intermediate);

        let following_request_snapshot = repair_pending_mode_geometry_before_snapshot(
            &repair,
            |obligation| {
                physical.set(obligation.frame);
                Ok(())
            },
            || {
                snapshot_calls.set(snapshot_calls.get() + 1);
                Ok(physical.get())
            },
        )
        .unwrap();
        assert_eq!(following_request_snapshot, committed);
        assert_eq!(snapshot_calls.get(), 1);

        physical.set(requested);
        physical.set(following_request_snapshot);
        assert_eq!(physical.get(), committed);
    }

    #[test]
    fn generation_issue_and_final_mode_commit_share_one_linearization_gate() {
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;
        use std::time::Duration;

        let gate = Mutex::new(());
        let generations = AtomicU64::new(0);
        let first = issue_mode_request_generation(&gate, &generations).unwrap();
        let second = issue_mode_request_generation(&gate, &generations).unwrap();
        let committed = AtomicBool::new(false);
        let error = complete_current_mode_request(&gate, &generations, first, || {
            committed.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap_err();
        assert_eq!(
            error,
            "state_transition field=mode stage=superseded error=newer_request"
        );
        assert!(!committed.load(Ordering::SeqCst));
        assert_eq!(second, 2);

        let gate = Arc::new(Mutex::new(()));
        let generations = Arc::new(AtomicU64::new(0));
        let first = issue_mode_request_generation(&gate, &generations).unwrap();
        let committed = Arc::new(AtomicBool::new(false));
        let (commit_entered_tx, commit_entered_rx) = mpsc::sync_channel(0);
        let (release_commit_tx, release_commit_rx) = mpsc::sync_channel(0);
        let commit_thread = {
            let gate = Arc::clone(&gate);
            let generations = Arc::clone(&generations);
            let committed = Arc::clone(&committed);
            std::thread::spawn(move || {
                complete_current_mode_request(&gate, &generations, first, || {
                    commit_entered_tx.send(()).unwrap();
                    release_commit_rx.recv().unwrap();
                    committed.store(true, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        commit_entered_rx.recv().unwrap();

        let (issue_started_tx, issue_started_rx) = mpsc::sync_channel(0);
        let (issued_tx, issued_rx) = mpsc::sync_channel(0);
        let issue_thread = {
            let gate = Arc::clone(&gate);
            let generations = Arc::clone(&generations);
            std::thread::spawn(move || {
                issue_started_tx.send(()).unwrap();
                let generation = issue_mode_request_generation(&gate, &generations).unwrap();
                issued_tx.send(generation).unwrap();
            })
        };
        issue_started_rx.recv().unwrap();
        assert!(issued_rx.recv_timeout(Duration::from_millis(50)).is_err());

        release_commit_tx.send(()).unwrap();
        assert_eq!(commit_thread.join().unwrap(), Ok(()));
        assert_eq!(issued_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
        issue_thread.join().unwrap();
        assert!(committed.load(Ordering::SeqCst));
    }

    #[test]
    fn unknown_native_animation_preference_disables_motion() {
        assert!(!resolve_client_area_animation_preference(Err(())));
        assert!(resolve_client_area_animation_preference(Ok(true)));
        assert!(!resolve_client_area_animation_preference(Ok(false)));
    }

    #[test]
    fn mode_command_completion_linearizes_the_final_native_frame_and_state_commit() {
        let start = crate::window::PhysicalWindowFrame {
            position: PhysicalPoint { x: 100, y: 12 },
            width: 248,
            height: 46,
            corner_radius: 16,
        };
        let end = crate::window::PhysicalWindowFrame {
            position: PhysicalPoint { x: -56, y: 12 },
            width: 560,
            height: 306,
            corner_radius: 24,
        };
        let elapsed = Cell::new(0_u64);
        let applied = RefCell::new(Vec::new());

        let outcome = drive_window_animation(
            start,
            end,
            true,
            window_animation_spec(None).unwrap(),
            || true,
            |frame| {
                applied.borrow_mut().push(frame);
                Ok(())
            },
            |duration| {
                elapsed.set(elapsed.get() + duration.as_millis() as u64);
            },
        )
        .unwrap();

        assert_eq!(outcome, WindowAnimationOutcome::Applied);
        assert_eq!(elapsed.get(), crate::window::WINDOW_ANIMATION_DURATION_MS);
        assert_ne!(applied.borrow().last().copied(), Some(end));

        let gate = Mutex::new(());
        let generations = AtomicU64::new(1);
        let committed = Cell::new(false);
        complete_current_mode_request(&gate, &generations, 1, || {
            applied.borrow_mut().push(end);
            committed.set(true);
            Ok(())
        })
        .unwrap();

        assert_eq!(applied.borrow().last().copied(), Some(end));
        assert!(committed.get());
    }

    #[test]
    fn reduced_motion_defers_the_exact_final_frame_to_the_same_commit_gate() {
        let start = crate::window::PhysicalWindowFrame {
            position: PhysicalPoint { x: 100, y: 12 },
            width: 248,
            height: 46,
            corner_radius: 16,
        };
        let end = crate::window::PhysicalWindowFrame {
            position: PhysicalPoint { x: -56, y: 12 },
            width: 560,
            height: 306,
            corner_radius: 24,
        };
        let applied = RefCell::new(Vec::new());
        let sleep_calls = Cell::new(0);

        let outcome = drive_window_animation(
            start,
            end,
            false,
            window_animation_spec(None).unwrap(),
            || true,
            |frame| {
                applied.borrow_mut().push(frame);
                Ok(())
            },
            |_| sleep_calls.set(sleep_calls.get() + 1),
        )
        .unwrap();

        assert_eq!(outcome, WindowAnimationOutcome::Applied);
        assert!(applied.borrow().is_empty());
        assert_eq!(sleep_calls.get(), 0);

        let gate = Mutex::new(());
        let generations = AtomicU64::new(1);
        complete_current_mode_request(&gate, &generations, 1, || {
            applied.borrow_mut().push(end);
            Ok(())
        })
        .unwrap();
        assert_eq!(applied.into_inner(), vec![end]);
    }
}
