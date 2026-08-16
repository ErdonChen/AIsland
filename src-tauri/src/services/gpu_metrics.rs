use crate::services::system_metrics::MetricFault;
use std::time::{Duration, Instant};

const GPU_RETRY_INTERVAL: Duration = Duration::from_secs(60);

pub trait GpuMetricsSource: Send {
    fn capture_percent(&mut self) -> Result<Option<f64>, MetricFault>;
}

fn aggregate_gpu_instances(instances: &[(&str, f64)]) -> Result<Option<f64>, MetricFault> {
    let mut found = false;
    let mut total = 0.0;
    for (name, value) in instances
        .iter()
        .copied()
        .filter(|(name, _)| is_tracked_engine(name))
    {
        let _ = name;
        if !value.is_finite() || value < 0.0 {
            return Err(fault("gpu", "counterInvalid"));
        }
        found = true;
        total += value;
        if !total.is_finite() {
            return Err(fault("gpu", "counterInvalid"));
        }
    }
    Ok(found.then(|| total.clamp(0.0, 100.0)))
}

fn is_tracked_engine(instance_name: &str) -> bool {
    matches!(
        instance_name
            .rsplit_once("engtype_")
            .map(|(_, suffix)| suffix),
        Some("3D" | "Compute_0" | "Compute_1" | "Compute")
    )
}

#[derive(Default)]
struct RetryState {
    last_attempt: Option<Instant>,
}

impl RetryState {
    fn should_initialize(&self, now: Instant) -> bool {
        self.last_attempt
            .and_then(|last| now.checked_duration_since(last))
            .is_none_or(|elapsed| elapsed >= GPU_RETRY_INTERVAL)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum InitializationOutcome {
    Ready,
    Deferred,
    Unavailable,
}

fn initialize_query<T, E>(
    query: &mut Option<T>,
    retry: &mut RetryState,
    now: Instant,
    opener: impl FnOnce() -> Result<T, E>,
) -> InitializationOutcome {
    if query.is_some() {
        return InitializationOutcome::Ready;
    }
    if !retry.should_initialize(now) {
        return InitializationOutcome::Deferred;
    }
    retry.last_attempt = Some(now);
    match opener() {
        Ok(opened) => {
            *query = Some(opened);
            InitializationOutcome::Ready
        }
        Err(_) => InitializationOutcome::Unavailable,
    }
}

#[cfg(windows)]
pub struct WindowsGpuMetricsSource {
    query: Option<PdhGpuQuery>,
    retry: RetryState,
}

#[cfg(windows)]
impl WindowsGpuMetricsSource {
    pub fn new() -> Self {
        Self {
            query: None,
            retry: RetryState::default(),
        }
    }

    fn capture_at(&mut self, now: Instant) -> Result<Option<f64>, MetricFault> {
        if initialize_query(&mut self.query, &mut self.retry, now, PdhGpuQuery::open)
            != InitializationOutcome::Ready
        {
            return Ok(None);
        }
        let result = self.query.as_ref().expect("GPU query initialized").read();
        match result {
            Ok(instances) => {
                let refs = instances
                    .iter()
                    .map(|(name, value)| (name.as_str(), *value))
                    .collect::<Vec<_>>();
                aggregate_gpu_instances(&refs)
            }
            Err(error) => {
                self.query = None;
                self.retry.last_attempt = Some(now);
                Err(error)
            }
        }
    }
}

#[cfg(windows)]
impl Default for WindowsGpuMetricsSource {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl GpuMetricsSource for WindowsGpuMetricsSource {
    fn capture_percent(&mut self) -> Result<Option<f64>, MetricFault> {
        self.capture_at(Instant::now())
    }
}

#[cfg(windows)]
struct PdhGpuQuery {
    query: windows::Win32::System::Performance::PDH_HQUERY,
    counter: windows::Win32::System::Performance::PDH_HCOUNTER,
}

#[cfg(windows)]
// SAFETY: the query and counter are exclusively owned by this value and all
// access is serialized by the sampler's GPU source mutex.
unsafe impl Send for PdhGpuQuery {}

#[cfg(windows)]
impl PdhGpuQuery {
    fn open() -> Result<Self, MetricFault> {
        use windows::core::{w, PCWSTR};
        use windows::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhCollectQueryData, PdhOpenQueryW, PDH_HCOUNTER, PDH_HQUERY,
        };
        let mut query = PDH_HQUERY::default();
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            return Err(fault("gpu", "queryOpenFailed"));
        }
        let guard = PdhQueryGuard(query);
        let mut counter = PDH_HCOUNTER::default();
        if unsafe {
            PdhAddEnglishCounterW(
                query,
                w!(r"\GPU Engine(*)\Utilization Percentage"),
                0,
                &mut counter,
            )
        } != 0
        {
            return Err(fault("gpu", "counterUnavailable"));
        }
        if unsafe { PdhCollectQueryData(query) } != 0 {
            return Err(fault("gpu", "counterUnavailable"));
        }
        let query = guard.into_inner();
        Ok(Self { query, counter })
    }

    fn read(&self) -> Result<Vec<(String, f64)>, MetricFault> {
        use windows::Win32::System::Performance::{
            PdhCollectQueryData, PdhGetFormattedCounterArrayW, PDH_CSTATUS_NEW_DATA,
            PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
        };
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return Err(fault("gpu", "queryFailed"));
        }
        let mut buffer_bytes = 0_u32;
        let mut item_count = 0_u32;
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut buffer_bytes,
                &mut item_count,
                None,
            )
        };
        if status != PDH_MORE_DATA {
            return if item_count == 0 {
                Ok(Vec::new())
            } else {
                Err(fault("gpu", "queryFailed"))
            };
        }
        let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let capacity = (buffer_bytes as usize).div_ceil(item_size).max(1);
        let mut buffer = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); capacity];
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut buffer_bytes,
                &mut item_count,
                Some(buffer.as_mut_ptr()),
            )
        };
        if status != 0 || item_count as usize > buffer.len() {
            return Err(fault("gpu", "queryFailed"));
        }
        buffer
            .iter()
            .take(item_count as usize)
            .map(|item| {
                if !matches!(
                    item.FmtValue.CStatus,
                    PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
                ) {
                    return Err(fault("gpu", "counterInvalid"));
                }
                let name = unsafe { wide_string(item.szName.0) }?;
                let value = unsafe { item.FmtValue.Anonymous.doubleValue };
                Ok((name, value))
            })
            .collect()
    }
}

#[cfg(windows)]
impl Drop for PdhGpuQuery {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Performance::PdhCloseQuery(self.query);
        }
    }
}

#[cfg(windows)]
struct PdhQueryGuard(windows::Win32::System::Performance::PDH_HQUERY);

#[cfg(windows)]
impl PdhQueryGuard {
    fn into_inner(self) -> windows::Win32::System::Performance::PDH_HQUERY {
        let query = self.0;
        std::mem::forget(self);
        query
    }
}

#[cfg(windows)]
impl Drop for PdhQueryGuard {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Performance::PdhCloseQuery(self.0);
        }
    }
}

#[cfg(windows)]
unsafe fn wide_string(pointer: *mut u16) -> Result<String, MetricFault> {
    if pointer.is_null() {
        return Err(fault("gpu", "counterInvalid"));
    }
    let mut length = 0_usize;
    while length < 32_768 && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    if length == 32_768 {
        return Err(fault("gpu", "counterInvalid"));
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, length) })
        .map_err(|_| fault("gpu", "counterInvalid"))
}

fn fault(metric: &'static str, reason_code: &'static str) -> MetricFault {
    MetricFault {
        metric,
        reason_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_3d_and_compute_instances_sums_and_clamps() {
        let value = aggregate_gpu_instances(&[
            ("pid_1_engtype_3D", 65.0),
            ("pid_2_engtype_Compute_0", 20.0),
            ("pid_3_engtype_Compute_1", 15.0),
            ("pid_4_engtype_Compute", 10.0),
            ("pid_5_engtype_Copy", 99.0),
        ])
        .unwrap();
        assert_eq!(value, Some(100.0));
    }

    #[test]
    fn engine_type_matching_rejects_compute_2_and_longer_near_matches() {
        let value = aggregate_gpu_instances(&[
            ("pid_1_engtype_Compute_2", 35.0),
            ("pid_2_engtype_Compute_0_extra", 25.0),
            ("pid_3_engtype_ComputeExtra", 20.0),
            ("pid_4_engtype_3D_extra", 15.0),
        ])
        .unwrap();

        assert_eq!(value, None);
    }

    #[test]
    fn missing_is_none_but_negative_nan_and_infinity_are_faults() {
        assert_eq!(
            aggregate_gpu_instances(&[("pid_1_engtype_Copy", 50.0)]).unwrap(),
            None
        );
        for invalid in [-1.0, f64::NAN, f64::INFINITY] {
            let fault = aggregate_gpu_instances(&[("pid_1_engtype_3D", invalid)]).unwrap_err();
            assert_eq!(fault.metric, "gpu");
            assert_eq!(fault.reason_code, "counterInvalid");
        }
    }

    #[test]
    fn failed_initialization_retries_no_more_than_once_per_sixty_seconds() {
        let start = Instant::now();
        let state = RetryState {
            last_attempt: Some(start),
        };
        assert!(!state.should_initialize(start + Duration::from_secs(59)));
        assert!(state.should_initialize(start + Duration::from_secs(60)));
    }

    #[test]
    fn failed_initialization_does_not_reinvoke_opener_before_sixty_seconds() {
        use std::cell::Cell;

        let start = Instant::now();
        let attempts = Cell::new(0_u32);
        let mut query = None;
        let mut retry = RetryState::default();
        let mut open = || {
            attempts.set(attempts.get() + 1);
            Err::<u8, ()>(())
        };

        assert_eq!(
            initialize_query(&mut query, &mut retry, start, &mut open),
            InitializationOutcome::Unavailable
        );
        assert_eq!(
            initialize_query(
                &mut query,
                &mut retry,
                start + Duration::from_secs(59),
                &mut open,
            ),
            InitializationOutcome::Deferred
        );
        assert_eq!(attempts.get(), 1);
        assert_eq!(
            initialize_query(
                &mut query,
                &mut retry,
                start + Duration::from_secs(60),
                || {
                    attempts.set(attempts.get() + 1);
                    Ok::<u8, ()>(7)
                },
            ),
            InitializationOutcome::Ready
        );
        assert_eq!(attempts.get(), 2);
        assert_eq!(query, Some(7));
    }
}
