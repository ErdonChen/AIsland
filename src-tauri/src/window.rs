pub const COLLAPSED_WIDTH: f64 = 248.0;
pub const COLLAPSED_HEIGHT: f64 = 46.0;
pub const EXPANDED_WIDTH: f64 = 560.0;
pub const MIN_COLLAPSED_WIDTH: f64 = 248.0;
pub const MAX_COLLAPSED_WIDTH: f64 = 720.0;
pub const MIN_EXPANDED_WIDTH: f64 = 420.0;
pub const MAX_EXPANDED_WIDTH: f64 = 960.0;
pub const DEFAULT_EXPANDED_HEIGHT: f64 = 306.0;
pub const MAX_EXPANDED_HEIGHT: f64 = 640.0;
pub const DEFAULT_MARGIN_Y: f64 = 12.0;
pub const DEFAULT_SCALE: f64 = 1.0;
pub const MIN_SCALE: f64 = 0.8;
pub const MAX_SCALE: f64 = 2.2;
pub const TUCKED_VISIBLE_EDGE: f64 = 10.0;
pub const TUCK_THRESHOLD_Y: f64 = 2.0;
pub const COLLAPSED_CORNER_RADIUS: f64 = COLLAPSED_HEIGHT / 2.0;
pub const EXPANDED_CORNER_RADIUS: f64 = 24.0;
pub const WINDOW_ANIMATION_DURATION_MS: u64 = 420;
pub const WINDOW_ANIMATION_FRAME_MS: u64 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAnimationMotion {
    Elastic,
    Smooth,
    Swift,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowAnimationSpec {
    pub motion: WindowAnimationMotion,
    pub duration_ms: u64,
}

pub fn window_animation_spec(value: Option<&str>) -> Result<WindowAnimationSpec, String> {
    let motion = match value.unwrap_or("elastic") {
        "elastic" => WindowAnimationMotion::Elastic,
        "smooth" => WindowAnimationMotion::Smooth,
        "swift" => WindowAnimationMotion::Swift,
        _ => return Err("invalid animation motion".to_string()),
    };
    Ok(WindowAnimationSpec {
        motion,
        duration_ms: match motion {
            WindowAnimationMotion::Elastic => WINDOW_ANIMATION_DURATION_MS,
            WindowAnimationMotion::Smooth => 520,
            WindowAnimationMotion::Swift => 180,
        },
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationLifecycleEvent {
    CloseRequested,
    Exit,
}

pub trait ApplicationLifecycleActions {
    fn hide_to_tray(&mut self);
    fn await_shutdown(&mut self);
}

pub fn handle_application_lifecycle_event(
    event: ApplicationLifecycleEvent,
    actions: &mut impl ApplicationLifecycleActions,
) {
    match event {
        ApplicationLifecycleEvent::CloseRequested => actions.hide_to_tray(),
        ApplicationLifecycleEvent::Exit => actions.await_shutdown(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IslandMode {
    #[default]
    Collapsed,
    Expanded,
}

impl IslandMode {
    pub fn from_value(value: &str) -> Result<Self, String> {
        match value {
            "collapsed" => Ok(Self::Collapsed),
            "expanded" => Ok(Self::Expanded),
            _ => Err(format!("Unsupported island mode: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogicalWindowSize {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicalPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalWindowFrame {
    pub position: PhysicalPoint,
    pub width: u32,
    pub height: u32,
    pub corner_radius: i32,
}

impl PhysicalWindowFrame {
    pub fn top_center_x(self) -> i64 {
        self.position.x as i64 + self.width as i64 / 2
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedPlacement {
    pub position: PhysicalPoint,
    pub monitor_name: Option<String>,
    pub dpi: f64,
}

#[derive(Clone, Debug)]
pub struct IslandWindowState {
    pub mode: IslandMode,
    pub is_tucked: bool,
    pub scale: f64,
    pub collapsed_width: f64,
    pub expanded_width: f64,
    pub expanded_height: f64,
    pub margin_y: f64,
    pub saved_visible_placement: Option<SavedPlacement>,
}

impl Default for IslandWindowState {
    fn default() -> Self {
        Self {
            mode: IslandMode::Collapsed,
            is_tucked: false,
            scale: DEFAULT_SCALE,
            collapsed_width: COLLAPSED_WIDTH,
            expanded_width: EXPANDED_WIDTH,
            expanded_height: DEFAULT_EXPANDED_HEIGHT,
            margin_y: DEFAULT_MARGIN_Y,
            saved_visible_placement: None,
        }
    }
}

pub fn clamp_scale(scale: f64) -> f64 {
    if scale.is_finite() {
        scale.clamp(MIN_SCALE, MAX_SCALE)
    } else {
        DEFAULT_SCALE
    }
}

pub fn clamp_mode_width(mode: IslandMode, width: f64) -> f64 {
    let (minimum, maximum, fallback) = match mode {
        IslandMode::Collapsed => (MIN_COLLAPSED_WIDTH, MAX_COLLAPSED_WIDTH, COLLAPSED_WIDTH),
        IslandMode::Expanded => (MIN_EXPANDED_WIDTH, MAX_EXPANDED_WIDTH, EXPANDED_WIDTH),
    };
    if width.is_finite() {
        width.clamp(minimum, maximum)
    } else {
        fallback
    }
}

pub fn clamp_width_to_work_area(
    mode: IslandMode,
    width: f64,
    scale: f64,
    dpi: f64,
    work_area_physical_width: u32,
    margin: f64,
) -> f64 {
    let scale = clamp_scale(scale);
    let dpi = if dpi.is_finite() && dpi > 0.0 {
        dpi
    } else {
        1.0
    };
    let margin = if margin.is_finite() {
        margin.max(0.0)
    } else {
        0.0
    };
    let available_logical = work_area_physical_width as f64 / dpi - margin * 2.0;
    let maximum_for_work_area = (available_logical / scale).max(0.0);
    let minimum = match mode {
        IslandMode::Collapsed => MIN_COLLAPSED_WIDTH,
        IslandMode::Expanded => MIN_EXPANDED_WIDTH,
    };
    clamp_mode_width(mode, width)
        .min(maximum_for_work_area)
        .max(minimum)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedHorizontalEdge {
    Left,
    Right,
}

impl FixedHorizontalEdge {
    pub fn from_value(value: &str) -> Result<Self, String> {
        match value {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            _ => Err(format!("Unsupported fixed horizontal edge: {value}")),
        }
    }
}

pub fn resized_x_for_fixed_edge(
    current_x: i32,
    current_width: u32,
    new_width: u32,
    fixed_edge: FixedHorizontalEdge,
) -> i32 {
    match fixed_edge {
        FixedHorizontalEdge::Left => current_x,
        FixedHorizontalEdge::Right => (current_x as i64 + current_width as i64 - new_width as i64)
            .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeWindowMaterial {
    Clear,
}

pub fn native_window_material_for_glass_transparency(_transparency: i32) -> NativeWindowMaterial {
    NativeWindowMaterial::Clear
}

pub fn collapsed_size(scale: f64) -> LogicalWindowSize {
    collapsed_size_with_width(scale, COLLAPSED_WIDTH)
}

pub fn collapsed_size_with_width(scale: f64, width: f64) -> LogicalWindowSize {
    let scale = clamp_scale(scale);
    LogicalWindowSize {
        width: clamp_mode_width(IslandMode::Collapsed, width) * scale,
        height: COLLAPSED_HEIGHT * scale,
    }
}

pub fn expanded_size(scale: f64, expanded_height: f64) -> LogicalWindowSize {
    expanded_size_with_width(scale, EXPANDED_WIDTH, expanded_height)
}

pub fn expanded_size_with_width(scale: f64, width: f64, expanded_height: f64) -> LogicalWindowSize {
    let scale = clamp_scale(scale);
    let height = if expanded_height.is_finite() {
        expanded_height.clamp(DEFAULT_EXPANDED_HEIGHT, MAX_EXPANDED_HEIGHT)
    } else {
        DEFAULT_EXPANDED_HEIGHT
    };
    LogicalWindowSize {
        width: clamp_mode_width(IslandMode::Expanded, width) * scale,
        height: height * scale,
    }
}

pub fn logical_size_for_state(state: &IslandWindowState) -> LogicalWindowSize {
    match state.mode {
        IslandMode::Collapsed => collapsed_size_with_width(state.scale, state.collapsed_width),
        IslandMode::Expanded => {
            expanded_size_with_width(state.scale, state.expanded_width, state.expanded_height)
        }
    }
}

pub fn physical_corner_radius(state: &IslandWindowState, dpi: f64) -> i32 {
    let base = match state.mode {
        IslandMode::Collapsed => COLLAPSED_CORNER_RADIUS,
        IslandMode::Expanded => EXPANDED_CORNER_RADIUS,
    };
    let dpi = if dpi.is_finite() && dpi > 0.0 {
        dpi
    } else {
        1.0
    };
    (base * clamp_scale(state.scale) * dpi)
        .round()
        .clamp(1.0, i32::MAX as f64) as i32
}

fn ease_out_back(progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    let shifted = progress - 1.0;
    const OVERSHOOT: f64 = 0.65;
    1.0 + (OVERSHOOT + 1.0) * shifted.powi(3) + OVERSHOOT * shifted.powi(2)
}

fn lerp_i32(start: i32, end: i32, progress: f64) -> i32 {
    (start as f64 + (end as f64 - start as f64) * progress)
        .round()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn lerp_u32(start: u32, end: u32, progress: f64) -> u32 {
    (start as f64 + (end as f64 - start as f64) * progress)
        .round()
        .clamp(1.0, u32::MAX as f64) as u32
}

fn interpolate_window_frame(
    start: PhysicalWindowFrame,
    end: PhysicalWindowFrame,
    progress: f64,
) -> PhysicalWindowFrame {
    PhysicalWindowFrame {
        position: PhysicalPoint {
            x: lerp_i32(start.position.x, end.position.x, progress),
            y: lerp_i32(start.position.y, end.position.y, progress),
        },
        width: lerp_u32(start.width, end.width, progress),
        height: lerp_u32(start.height, end.height, progress),
        corner_radius: lerp_i32(start.corner_radius, end.corner_radius, progress).max(1),
    }
}

pub fn eased_window_frame(
    start: PhysicalWindowFrame,
    end: PhysicalWindowFrame,
    elapsed_ms: u64,
    duration_ms: u64,
) -> PhysicalWindowFrame {
    if elapsed_ms >= duration_ms || duration_ms == 0 {
        return end;
    }
    let progress = ease_out_back(elapsed_ms as f64 / duration_ms as f64);
    interpolate_window_frame(start, end, progress)
}

pub fn eased_window_frame_with_spec(
    start: PhysicalWindowFrame,
    end: PhysicalWindowFrame,
    elapsed_ms: u64,
    spec: WindowAnimationSpec,
) -> PhysicalWindowFrame {
    if elapsed_ms >= spec.duration_ms || spec.duration_ms == 0 {
        return end;
    }
    let linear = (elapsed_ms as f64 / spec.duration_ms as f64).clamp(0.0, 1.0);
    let progress = match spec.motion {
        WindowAnimationMotion::Elastic => ease_out_back(linear),
        WindowAnimationMotion::Smooth => 1.0 - (1.0 - linear).powi(3),
        WindowAnimationMotion::Swift => 1.0 - (1.0 - linear).powi(4),
    };
    interpolate_window_frame(start, end, progress)
}

pub fn animation_frame_times(animated: bool) -> Vec<u64> {
    animation_frame_times_for(animated, WINDOW_ANIMATION_DURATION_MS)
}

pub fn animation_frame_times_for(animated: bool, duration_ms: u64) -> Vec<u64> {
    if !animated {
        return vec![duration_ms];
    }
    let mut frames = Vec::new();
    let mut elapsed = WINDOW_ANIMATION_FRAME_MS;
    while elapsed < duration_ms {
        frames.push(elapsed);
        elapsed = elapsed.saturating_add(WINDOW_ANIMATION_FRAME_MS);
    }
    frames.push(duration_ms);
    frames
}

pub fn animation_generation_is_current(generation: u64, current_generation: u64) -> bool {
    generation == current_generation
}

#[allow(clippy::too_many_arguments)]
pub fn top_center_physical(
    monitor_width: u32,
    _monitor_height: u32,
    window_width: u32,
    _window_height: u32,
    monitor_x: i32,
    monitor_y: i32,
    dpi: f64,
    margin_y: f64,
) -> PhysicalPoint {
    let centered_x = monitor_x as i64 + (monitor_width as i64 - window_width as i64).max(0) / 2;
    PhysicalPoint {
        x: centered_x.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        y: monitor_y.saturating_add((margin_y * dpi).round() as i32),
    }
}

pub fn tucked_y_physical(monitor_top: i32, window_height: u32, dpi: f64) -> i32 {
    let visible_physical = (TUCKED_VISIBLE_EDGE * dpi).round() as i32;
    monitor_top
        .saturating_sub(window_height.min(i32::MAX as u32) as i32)
        .saturating_add(visible_physical)
}

pub fn safe_restore_y_physical(monitor_top: i32, dpi: f64, margin_y: f64) -> i32 {
    monitor_top.saturating_add((margin_y * dpi).round() as i32)
}

pub fn should_tuck_physical(window_y: i32, monitor_top: i32, dpi: f64) -> bool {
    let threshold = (TUCK_THRESHOLD_Y * dpi).round() as i32;
    window_y <= monitor_top.saturating_add(threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct LifecycleSpy {
        hide_calls: usize,
        shutdown_calls: usize,
    }

    impl ApplicationLifecycleActions for LifecycleSpy {
        fn hide_to_tray(&mut self) {
            self.hide_calls += 1;
        }

        fn await_shutdown(&mut self) {
            self.shutdown_calls += 1;
        }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn close_request_handler_hides_without_shutdown() {
        let mut actions = LifecycleSpy::default();

        handle_application_lifecycle_event(ApplicationLifecycleEvent::CloseRequested, &mut actions);

        assert_eq!(actions.hide_calls, 1);
        assert_eq!(actions.shutdown_calls, 0);
    }

    #[test]
    fn exit_handler_awaits_shutdown_without_hiding() {
        let mut actions = LifecycleSpy::default();

        handle_application_lifecycle_event(ApplicationLifecycleEvent::Exit, &mut actions);

        assert_eq!(actions.hide_calls, 0);
        assert_eq!(actions.shutdown_calls, 1);
    }

    #[test]
    fn logical_sizes_keep_fractional_values_until_tauri_converts_to_physical() {
        let collapsed = collapsed_size(0.85);
        assert_close(collapsed.width, 210.8);
        assert_close(collapsed.height, 39.1);

        let large = collapsed_size(1.30);
        assert_close(large.width, 322.4);
        assert_close(large.height, 59.8);

        let expanded = expanded_size(0.85, 306.0);
        assert_close(expanded.width, 476.0);
        assert_close(expanded.height, 260.1);
    }

    #[test]
    fn window_widths_are_independent_and_follow_each_mode_range() {
        let defaults = IslandWindowState::default();
        assert_close(defaults.collapsed_width, 248.0);
        assert_close(defaults.expanded_width, 560.0);

        assert_close(clamp_mode_width(IslandMode::Collapsed, 100.0), 248.0);
        assert_close(clamp_mode_width(IslandMode::Collapsed, 900.0), 720.0);
        assert_close(clamp_mode_width(IslandMode::Expanded, 100.0), 420.0);
        assert_close(clamp_mode_width(IslandMode::Expanded, 1_200.0), 960.0);

        let collapsed = IslandWindowState {
            collapsed_width: 420.0,
            expanded_width: 880.0,
            ..IslandWindowState::default()
        };
        assert_close(logical_size_for_state(&collapsed).width, 420.0);

        let expanded = IslandWindowState {
            mode: IslandMode::Expanded,
            collapsed_width: 420.0,
            expanded_width: 880.0,
            ..IslandWindowState::default()
        };
        assert_close(logical_size_for_state(&expanded).width, 880.0);
    }

    #[test]
    fn monitor_work_area_caps_scaled_width_before_the_window_can_leave_screen() {
        assert_close(
            clamp_width_to_work_area(IslandMode::Expanded, 960.0, 2.2, 1.0, 1920, 12.0),
            861.8181818181818,
        );
        assert_close(
            clamp_width_to_work_area(IslandMode::Collapsed, 720.0, 1.0, 1.5, 1920, 12.0),
            720.0,
        );
    }

    #[test]
    fn horizontal_resize_keeps_the_opposite_edge_fixed() {
        assert_eq!(
            FixedHorizontalEdge::from_value("left"),
            Ok(FixedHorizontalEdge::Left)
        );
        assert_eq!(
            FixedHorizontalEdge::from_value("right"),
            Ok(FixedHorizontalEdge::Right)
        );
        assert!(FixedHorizontalEdge::from_value("center").is_err());
        assert_eq!(
            resized_x_for_fixed_edge(100, 560, 720, FixedHorizontalEdge::Left),
            100
        );
        assert_eq!(
            resized_x_for_fixed_edge(100, 560, 720, FixedHorizontalEdge::Right),
            -60
        );
    }

    #[test]
    fn clamp_scale_handles_bounds_and_non_finite_values() {
        assert_close(clamp_scale(0.5), 0.8);
        assert_close(clamp_scale(3.0), 2.2);
        assert_close(clamp_scale(2.0), 2.0);
        assert_close(clamp_scale(1.15), 1.15);
        assert_close(clamp_scale(f64::NAN), 1.0);
    }

    #[test]
    fn native_window_background_is_clear_at_every_glass_transparency() {
        for transparency in [-50, 0, 58, 99, 100, 160] {
            assert_eq!(
                native_window_material_for_glass_transparency(transparency),
                NativeWindowMaterial::Clear
            );
        }
    }

    #[test]
    fn expanded_height_falls_back_to_default_for_nan() {
        assert_close(expanded_size(1.0, f64::NAN).height, 306.0);
    }

    #[test]
    fn mode_from_value_rejects_unknown_values() {
        assert_eq!(
            IslandMode::from_value("collapsed"),
            Ok(IslandMode::Collapsed)
        );
        assert_eq!(IslandMode::from_value("expanded"), Ok(IslandMode::Expanded));
        assert!(IslandMode::from_value("bogus").is_err());
    }

    #[test]
    fn top_center_uses_monitor_physical_origin_and_dpi_margin() {
        let point = top_center_physical(1920, 1080, 560, 306, 0, 0, 1.0, 12.0);
        assert_eq!(point, PhysicalPoint { x: 680, y: 12 });

        let second = top_center_physical(2560, 1440, 840, 459, 1920, -200, 1.5, 12.0);
        assert_eq!(second, PhysicalPoint { x: 2780, y: -182 });
    }

    #[test]
    fn tuck_leaves_exactly_ten_logical_pixels_visible() {
        assert_eq!(tucked_y_physical(0, 87, 1.5), -72);
        assert_eq!(tucked_y_physical(-200, 116, 2.0), -296);
    }

    #[test]
    fn restore_position_is_below_tuck_threshold() {
        assert_eq!(safe_restore_y_physical(0, 1.5, 12.0), 18);
        assert_eq!(safe_restore_y_physical(-200, 2.0, 12.0), -176);
    }

    #[test]
    fn tuck_threshold_is_relative_to_each_monitor_and_dpi() {
        assert!(should_tuck_physical(2, 0, 1.0));
        assert!(!should_tuck_physical(3, 0, 1.0));
        assert!(should_tuck_physical(-196, -200, 2.0));
        assert!(!should_tuck_physical(-195, -200, 2.0));
    }

    #[test]
    fn native_corner_radius_tracks_mode_application_scale_and_monitor_dpi() {
        let collapsed = IslandWindowState::default();
        assert_eq!(physical_corner_radius(&collapsed, 1.0), 23);

        let expanded = IslandWindowState {
            mode: IslandMode::Expanded,
            scale: 1.25,
            ..IslandWindowState::default()
        };
        assert_eq!(physical_corner_radius(&expanded, 1.5), 45);
    }

    #[test]
    fn eased_native_frame_keeps_the_top_center_anchor_and_radius_in_sync() {
        let start = PhysicalWindowFrame {
            position: PhysicalPoint { x: 100, y: 12 },
            width: 248,
            height: 46,
            corner_radius: 23,
        };
        let end = PhysicalWindowFrame {
            position: PhysicalPoint { x: -56, y: 12 },
            width: 560,
            height: 306,
            corner_radius: 24,
        };

        let halfway = eased_window_frame(
            start,
            end,
            WINDOW_ANIMATION_DURATION_MS / 2,
            WINDOW_ANIMATION_DURATION_MS,
        );
        assert_eq!(halfway.width, 546);
        assert_eq!(halfway.height, 295);
        assert_eq!(halfway.corner_radius, 24);
        assert_eq!(halfway.position.y, 12);
        assert!((halfway.top_center_x() - start.top_center_x()).abs() <= 1);

        let rebound = eased_window_frame(
            start,
            end,
            WINDOW_ANIMATION_DURATION_MS * 3 / 4,
            WINDOW_ANIMATION_DURATION_MS,
        );
        assert!(rebound.width > end.width);
        assert!(rebound.height > end.height);

        assert_eq!(
            eased_window_frame(
                start,
                end,
                WINDOW_ANIMATION_DURATION_MS,
                WINDOW_ANIMATION_DURATION_MS,
            ),
            end
        );
    }

    #[test]
    fn reduced_motion_uses_one_exact_final_frame() {
        assert_eq!(
            animation_frame_times(false),
            vec![WINDOW_ANIMATION_DURATION_MS]
        );

        let animated = animation_frame_times(true);
        assert_eq!(animated.last().copied(), Some(WINDOW_ANIMATION_DURATION_MS));
        assert!(animated.len() > 2);
        assert!(animated.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn native_window_animation_matches_each_user_selected_motion() {
        assert_eq!(
            window_animation_spec(Some("elastic")).unwrap(),
            WindowAnimationSpec {
                motion: WindowAnimationMotion::Elastic,
                duration_ms: 420,
            }
        );
        assert_eq!(
            window_animation_spec(Some("smooth")).unwrap(),
            WindowAnimationSpec {
                motion: WindowAnimationMotion::Smooth,
                duration_ms: 520,
            }
        );
        assert_eq!(
            window_animation_spec(Some("swift")).unwrap(),
            WindowAnimationSpec {
                motion: WindowAnimationMotion::Swift,
                duration_ms: 180,
            }
        );
        assert!(window_animation_spec(Some("unknown")).is_err());

        let start = PhysicalWindowFrame {
            position: PhysicalPoint { x: 100, y: 12 },
            width: 248,
            height: 46,
            corner_radius: 23,
        };
        let end = PhysicalWindowFrame {
            position: PhysicalPoint { x: -56, y: 12 },
            width: 560,
            height: 306,
            corner_radius: 24,
        };
        let frames = ["elastic", "smooth", "swift"].map(|motion| {
            let spec = window_animation_spec(Some(motion)).unwrap();
            eased_window_frame_with_spec(start, end, spec.duration_ms * 3 / 4, spec)
        });
        assert!(frames[0].width > end.width, "elastic visibly rebounds");
        assert!(
            frames[1].width < frames[2].width,
            "smooth reveals more gently than swift"
        );
        assert_ne!(frames[0], frames[1]);
        assert_ne!(frames[1], frames[2]);
    }

    #[test]
    fn smooth_and_swift_native_geometry_never_overshoot_the_target() {
        let start = PhysicalWindowFrame {
            position: PhysicalPoint { x: 100, y: 12 },
            width: 248,
            height: 46,
            corner_radius: 23,
        };
        let end = PhysicalWindowFrame {
            position: PhysicalPoint { x: -56, y: 12 },
            width: 560,
            height: 306,
            corner_radius: 24,
        };

        for motion in ["smooth", "swift"] {
            let spec = window_animation_spec(Some(motion)).unwrap();
            let frame = eased_window_frame_with_spec(start, end, spec.duration_ms * 3 / 4, spec);
            assert!(
                frame.width >= start.width && frame.width <= end.width,
                "{motion}"
            );
            assert!(
                frame.height >= start.height && frame.height <= end.height,
                "{motion}"
            );
        }
    }

    #[test]
    fn only_the_latest_animation_generation_may_commit() {
        let current_generation = 8;
        assert!(!animation_generation_is_current(7, current_generation));
        assert!(animation_generation_is_current(8, current_generation));
    }
}
