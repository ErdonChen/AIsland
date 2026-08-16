use crate::domain::monitor::{NewMonitorSample, NewProcessSample};
use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetricFault {
    pub metric: &'static str,
    pub reason_code: &'static str,
}

#[derive(Clone, Debug)]
pub struct CoreMetricCapture {
    pub sample: NewMonitorSample,
    pub processes: Vec<NewProcessSample>,
}

pub trait CoreMetricsSource: Send {
    fn capture(
        &mut self,
        monotonic_now: Instant,
        unix_now: i64,
    ) -> Result<CoreMetricCapture, MetricFault>;
}

#[derive(Clone, Copy, Debug)]
struct RawCoreCounters {
    idle: u64,
    kernel: u64,
    user: u64,
    memory_total: u64,
    memory_available: u64,
    disk_read_bytes: u64,
    disk_write_bytes: u64,
    network_receive_bytes: u64,
    network_send_bytes: u64,
}

#[derive(Default)]
struct CounterAccumulator {
    previous: Option<(RawCoreCounters, Instant)>,
}

impl CounterAccumulator {
    fn map(
        &mut self,
        raw: RawCoreCounters,
        monotonic_now: Instant,
        unix_now: i64,
    ) -> Result<CoreMetricCapture, MetricFault> {
        if raw.memory_total == 0 || raw.memory_available > raw.memory_total {
            return Err(fault("memory", "counterInvalid"));
        }
        let Some((previous, previous_at)) = self.previous else {
            self.previous = Some((raw, monotonic_now));
            return Err(fault("core", "baselinePending"));
        };
        let elapsed = monotonic_now.checked_duration_since(previous_at);
        let Some(elapsed_seconds) = elapsed.map(|value| value.as_secs_f64()) else {
            return Err(fault("core", "clockInvalid"));
        };
        if elapsed_seconds <= 0.0 {
            return Err(fault("core", "clockInvalid"));
        }

        if raw.disk_read_bytes < previous.disk_read_bytes
            || raw.disk_write_bytes < previous.disk_write_bytes
            || raw.network_receive_bytes < previous.network_receive_bytes
            || raw.network_send_bytes < previous.network_send_bytes
        {
            self.previous = Some((raw, monotonic_now));
            return Err(fault("io", "counterReset"));
        }

        let Some(idle_delta) = raw.idle.checked_sub(previous.idle) else {
            return Err(fault("cpu", "counterInvalid"));
        };
        let Some(kernel_delta) = raw.kernel.checked_sub(previous.kernel) else {
            return Err(fault("cpu", "counterInvalid"));
        };
        let Some(user_delta) = raw.user.checked_sub(previous.user) else {
            return Err(fault("cpu", "counterInvalid"));
        };
        let Some(total_delta) = kernel_delta.checked_add(user_delta) else {
            return Err(fault("cpu", "counterInvalid"));
        };
        if total_delta == 0 || idle_delta > total_delta {
            return Err(fault("cpu", "counterInvalid"));
        }

        let cpu_percent =
            ((1.0 - idle_delta as f64 / total_delta as f64) * 100.0).clamp(0.0, 100.0);
        let to_rate = |current: u64, prior: u64| (current - prior) as f64 / elapsed_seconds;
        self.previous = Some((raw, monotonic_now));
        Ok(CoreMetricCapture {
            sample: NewMonitorSample {
                cpu_percent,
                memory_used_bytes: i64::try_from(raw.memory_total - raw.memory_available)
                    .map_err(|_| fault("memory", "counterInvalid"))?,
                memory_total_bytes: i64::try_from(raw.memory_total)
                    .map_err(|_| fault("memory", "counterInvalid"))?,
                disk_read_bps: to_rate(raw.disk_read_bytes, previous.disk_read_bytes),
                disk_write_bps: to_rate(raw.disk_write_bytes, previous.disk_write_bytes),
                network_rx_bps: to_rate(raw.network_receive_bytes, previous.network_receive_bytes),
                network_tx_bps: to_rate(raw.network_send_bytes, previous.network_send_bytes),
                gpu_percent: None,
                sampled_at: unix_now,
            },
            processes: Vec::new(),
        })
    }
}

fn fault(metric: &'static str, reason_code: &'static str) -> MetricFault {
    MetricFault {
        metric,
        reason_code,
    }
}

#[cfg(windows)]
pub struct WindowsCoreMetricsSource {
    accumulator: CounterAccumulator,
    disk: PdhDiskCounters,
}

#[cfg(windows)]
impl WindowsCoreMetricsSource {
    pub fn new() -> Result<Self, MetricFault> {
        Ok(Self {
            accumulator: CounterAccumulator::default(),
            disk: PdhDiskCounters::open()?,
        })
    }

    fn read_raw(&self) -> Result<RawCoreCounters, MetricFault> {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        use windows::Win32::System::Threading::GetSystemTimes;

        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe {
            GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user))
                .map_err(|_| fault("cpu", "queryFailed"))?;
        }
        let mut memory = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        unsafe {
            GlobalMemoryStatusEx(&mut memory).map_err(|_| fault("memory", "queryFailed"))?;
        }
        let (disk_read_bytes, disk_write_bytes) = self.disk.read()?;
        let (network_receive_bytes, network_send_bytes) = read_network_octets()?;
        Ok(RawCoreCounters {
            idle: filetime_value(idle),
            kernel: filetime_value(kernel),
            user: filetime_value(user),
            memory_total: memory.ullTotalPhys,
            memory_available: memory.ullAvailPhys,
            disk_read_bytes,
            disk_write_bytes,
            network_receive_bytes,
            network_send_bytes,
        })
    }
}

#[cfg(windows)]
impl CoreMetricsSource for WindowsCoreMetricsSource {
    fn capture(
        &mut self,
        monotonic_now: Instant,
        unix_now: i64,
    ) -> Result<CoreMetricCapture, MetricFault> {
        let raw = self.read_raw()?;
        self.accumulator.map(raw, monotonic_now, unix_now)
    }
}

#[cfg(windows)]
fn filetime_value(value: windows::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[cfg(windows)]
struct PdhDiskCounters {
    query: windows::Win32::System::Performance::PDH_HQUERY,
    read: windows::Win32::System::Performance::PDH_HCOUNTER,
    write: windows::Win32::System::Performance::PDH_HCOUNTER,
}

#[cfg(windows)]
// SAFETY: `PdhDiskCounters` exclusively owns its query and counter handles. The
// containing metrics source moves as one value between executor threads, never
// shares the handles concurrently, and serializes every PDH call through the
// sampler worker before closing the query in `Drop`.
unsafe impl Send for PdhDiskCounters {}

#[cfg(windows)]
impl PdhDiskCounters {
    fn open() -> Result<Self, MetricFault> {
        use windows::core::{w, PCWSTR};
        use windows::Win32::System::Performance::{
            PdhAddEnglishCounterW, PdhOpenQueryW, PDH_HCOUNTER, PDH_HQUERY,
        };
        let mut query = PDH_HQUERY::default();
        if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut query) } != 0 {
            return Err(fault("disk", "queryOpenFailed"));
        }
        let guard = PdhQueryGuard(query);
        let mut read = PDH_HCOUNTER::default();
        if unsafe {
            PdhAddEnglishCounterW(
                query,
                w!(r"\PhysicalDisk(_Total)\Disk Read Bytes/sec"),
                0,
                &mut read,
            )
        } != 0
        {
            return Err(fault("disk", "counterOpenFailed"));
        }
        let mut write = PDH_HCOUNTER::default();
        if unsafe {
            PdhAddEnglishCounterW(
                query,
                w!(r"\PhysicalDisk(_Total)\Disk Write Bytes/sec"),
                0,
                &mut write,
            )
        } != 0
        {
            return Err(fault("disk", "counterOpenFailed"));
        }
        let query = guard.into_inner();
        Ok(Self { query, read, write })
    }

    fn read(&self) -> Result<(u64, u64), MetricFault> {
        use windows::Win32::System::Performance::{
            PdhCollectQueryData, PdhGetRawCounterValue, PDH_RAW_COUNTER,
        };
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return Err(fault("disk", "queryFailed"));
        }
        let read = raw_counter(self.read)?;
        let write = raw_counter(self.write)?;

        fn raw_counter(
            counter: windows::Win32::System::Performance::PDH_HCOUNTER,
        ) -> Result<u64, MetricFault> {
            let mut value = PDH_RAW_COUNTER::default();
            if unsafe { PdhGetRawCounterValue(counter, None, &mut value) } != 0
                || !matches!(value.CStatus, 0 | 1)
            {
                return Err(fault("disk", "queryFailed"));
            }
            u64::try_from(value.FirstValue).map_err(|_| fault("disk", "counterInvalid"))
        }

        Ok((read, write))
    }
}

#[cfg(windows)]
impl Drop for PdhDiskCounters {
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
        let value = self.0;
        std::mem::forget(self);
        value
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
fn read_network_octets() -> Result<(u64, u64), MetricFault> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_TABLE2,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    let mut table = std::ptr::null_mut::<MIB_IF_TABLE2>();
    if unsafe { GetIfTable2(&mut table) }.0 != 0 || table.is_null() {
        return Err(fault("network", "queryFailed"));
    }
    let guard = MibTableGuard(table.cast());
    let table_ref = unsafe { &*table };
    let rows = unsafe {
        std::slice::from_raw_parts(table_ref.Table.as_ptr(), table_ref.NumEntries as usize)
    };
    let mut received = 0_u64;
    let mut sent = 0_u64;
    for row in rows
        .iter()
        .filter(|row| row.OperStatus == IfOperStatusUp && row.Type != IF_TYPE_SOFTWARE_LOOPBACK)
    {
        received = received
            .checked_add(row.InOctets)
            .ok_or_else(|| fault("network", "counterInvalid"))?;
        sent = sent
            .checked_add(row.OutOctets)
            .ok_or_else(|| fault("network", "counterInvalid"))?;
    }
    drop(guard);
    Ok((received, sent))
}

#[cfg(windows)]
struct MibTableGuard(*const std::ffi::c_void);

#[cfg(windows)]
impl Drop for MibTableGuard {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::NetworkManagement::IpHelper::FreeMibTable(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::AppErrorCode;
    use std::time::Duration;

    fn raw(
        idle: u64,
        kernel: u64,
        user: u64,
        disk_read_bytes: u64,
        network_receive_bytes: u64,
    ) -> RawCoreCounters {
        RawCoreCounters {
            idle,
            kernel,
            user,
            memory_total: 1_000,
            memory_available: 250,
            disk_read_bytes,
            disk_write_bytes: 0,
            network_receive_bytes,
            network_send_bytes: 0,
        }
    }

    #[test]
    fn first_capture_is_a_baseline_then_cpu_memory_and_rates_use_monotonic_deltas() {
        let mut accumulator = CounterAccumulator::default();
        let start = Instant::now();
        let first = accumulator.map(raw(100, 300, 100, 1_000, 2_000), start, 10);
        assert_eq!(first.unwrap_err().reason_code, "baselinePending");

        let capture = accumulator
            .map(
                raw(120, 360, 140, 1_600, 3_200),
                start + Duration::from_millis(500),
                9_999,
            )
            .unwrap();
        assert_eq!(capture.sample.cpu_percent, 80.0);
        assert_eq!(capture.sample.memory_used_bytes, 750);
        assert_eq!(capture.sample.memory_total_bytes, 1_000);
        assert_eq!(capture.sample.disk_read_bps, 1_200.0);
        assert_eq!(capture.sample.network_rx_bps, 2_400.0);
        assert_eq!(capture.sample.sampled_at, 9_999);
    }

    #[test]
    fn zero_or_backward_cpu_total_is_rejected_without_losing_the_last_good_baseline() {
        let mut accumulator = CounterAccumulator::default();
        let start = Instant::now();
        let _ = accumulator.map(raw(10, 20, 10, 100, 100), start, 1);
        let fault = accumulator
            .map(raw(11, 19, 10, 110, 110), start + Duration::from_secs(1), 2)
            .unwrap_err();
        assert_eq!(fault.metric, "cpu");
        assert_eq!(fault.reason_code, "counterInvalid");
        let valid = accumulator
            .map(raw(12, 24, 12, 120, 120), start + Duration::from_secs(2), 3)
            .unwrap();
        assert!((valid.sample.cpu_percent - 66.666_666_666_666_67).abs() < f64::EPSILON);
    }

    #[test]
    fn counter_reset_establishes_a_new_baseline_and_rejects_exactly_one_sample() {
        let mut accumulator = CounterAccumulator::default();
        let start = Instant::now();
        let _ = accumulator.map(raw(1, 2, 1, 1_000, 1_000), start, 1);
        let _ = accumulator
            .map(
                raw(2, 4, 2, 1_100, 1_100),
                start + Duration::from_secs(1),
                2,
            )
            .unwrap();
        let reset = accumulator
            .map(raw(3, 6, 3, 50, 60), start + Duration::from_secs(2), 3)
            .unwrap_err();
        assert_eq!(reset.reason_code, "counterReset");
        let after = accumulator
            .map(raw(4, 8, 4, 70, 100), start + Duration::from_secs(3), 4)
            .unwrap();
        assert_eq!(after.sample.disk_read_bps, 20.0);
        assert_eq!(after.sample.network_rx_bps, 40.0);
    }

    #[test]
    fn a_fresh_generation_source_never_reuses_the_previous_generation_baseline() {
        let start = Instant::now();
        let mut generation_one = CounterAccumulator::default();
        let _ = generation_one.map(raw(10, 20, 10, 100, 100), start, 1);
        generation_one
            .map(raw(20, 40, 20, 200, 200), start + Duration::from_secs(1), 2)
            .unwrap();

        let mut generation_two = CounterAccumulator::default();
        let first = generation_two
            .map(raw(30, 60, 30, 300, 300), start + Duration::from_secs(2), 3)
            .unwrap_err();
        assert_eq!(first.metric, "core");
        assert_eq!(first.reason_code, "baselinePending");
    }

    #[test]
    fn source_unavailable_error_code_is_the_sampler_boundary_contract() {
        let error = crate::services::monitor_sampler::metric_fault_error(MetricFault {
            metric: "network",
            reason_code: "queryFailed",
        });
        assert_eq!(error.code, AppErrorCode::SourceUnavailable);
        assert_eq!(error.message_key, "errors.sourceUnavailable");
    }
}
