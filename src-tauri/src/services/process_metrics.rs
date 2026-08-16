use crate::contracts::ProcessWatch;
use crate::domain::monitor::NewProcessSample;
use crate::services::system_metrics::MetricFault;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

const FILETIME_TICKS_PER_SECOND: f64 = 10_000_000.0;

pub trait ProcessMetricsSource: Send {
    fn capture(
        &mut self,
        watches: &[ProcessWatch],
        elapsed: Duration,
        sampled_at: i64,
    ) -> Result<Vec<NewProcessSample>, MetricFault>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessSkip {
    pub watch_id: String,
    pub skipped_count: u64,
    pub sampled_at: i64,
}

pub(crate) type ProcessSkipCollector = Arc<Mutex<Vec<ProcessSkip>>>;

#[derive(Clone, Debug)]
struct RawProcess {
    pid: u32,
    base_name: String,
    creation_time: u64,
    kernel_user_total: u64,
    working_set_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct ProcessBaseline {
    creation_time: u64,
    kernel_user_total: u64,
}

#[derive(Default)]
struct ProcessAccumulator {
    baselines: HashMap<u32, ProcessBaseline>,
}

impl ProcessAccumulator {
    fn map(
        &mut self,
        snapshot: Vec<RawProcess>,
        watches: &[ProcessWatch],
        elapsed: Duration,
        logical_processors: u32,
    ) -> Result<Vec<NewProcessSample>, MetricFault> {
        let elapsed_seconds = elapsed.as_secs_f64();
        if elapsed_seconds <= 0.0 || logical_processors == 0 {
            return Err(fault("processes", "clockInvalid"));
        }

        let mut enabled = BTreeMap::<String, Vec<Uuid>>::new();
        for watch in watches.iter().filter(|watch| watch.enabled) {
            let id = Uuid::parse_str(&watch.id).map_err(|_| fault("processes", "watchInvalid"))?;
            enabled
                .entry(watch.process_name.to_lowercase())
                .or_default()
                .push(id);
        }
        for ids in enabled.values_mut() {
            ids.sort_unstable();
        }

        let mut present = HashSet::new();
        let mut rows = Vec::new();
        for process in snapshot {
            let Some(watch_ids) = enabled.get(&process.base_name.to_lowercase()) else {
                continue;
            };
            present.insert(process.pid);
            let prior = self.baselines.insert(
                process.pid,
                ProcessBaseline {
                    creation_time: process.creation_time,
                    kernel_user_total: process.kernel_user_total,
                },
            );
            let Some(prior) = prior else { continue };
            if prior.creation_time != process.creation_time
                || process.kernel_user_total < prior.kernel_user_total
            {
                continue;
            }
            let delta_seconds = (process.kernel_user_total - prior.kernel_user_total) as f64
                / FILETIME_TICKS_PER_SECOND;
            let cpu_percent = (delta_seconds / elapsed_seconds / f64::from(logical_processors)
                * 100.0)
                .clamp(0.0, 100.0);
            let pid = i64::from(process.pid);
            let memory_bytes = i64::try_from(process.working_set_bytes)
                .map_err(|_| fault("processes", "counterInvalid"))?;
            for watch_id in watch_ids {
                rows.push(NewProcessSample {
                    process_watch_id: *watch_id,
                    pid,
                    process_name: process.base_name.clone(),
                    cpu_percent,
                    memory_bytes,
                });
            }
        }
        self.baselines.retain(|pid, _| present.contains(pid));
        rows.sort_by(|left, right| {
            right
                .cpu_percent
                .total_cmp(&left.cpu_percent)
                .then_with(|| right.memory_bytes.cmp(&left.memory_bytes))
                .then_with(|| left.pid.cmp(&right.pid))
                .then_with(|| left.process_watch_id.cmp(&right.process_watch_id))
        });
        Ok(rows)
    }
}

#[cfg(windows)]
pub struct WindowsProcessMetricsSource {
    accumulator: ProcessAccumulator,
    logical_processors: u32,
    skips: ProcessSkipCollector,
}

#[cfg(windows)]
impl WindowsProcessMetricsSource {
    pub fn new() -> Self {
        Self::with_skip_collector(Arc::new(Mutex::new(Vec::new())))
    }

    pub(crate) fn with_skip_collector(skips: ProcessSkipCollector) -> Self {
        let logical_processors =
            unsafe { windows::Win32::System::Threading::GetActiveProcessorCount(u16::MAX) };
        Self {
            accumulator: ProcessAccumulator::default(),
            logical_processors,
            skips,
        }
    }
}

#[cfg(windows)]
impl ProcessMetricsSource for WindowsProcessMetricsSource {
    fn capture(
        &mut self,
        watches: &[ProcessWatch],
        elapsed: Duration,
        sampled_at: i64,
    ) -> Result<Vec<NewProcessSample>, MetricFault> {
        if self.logical_processors == 0 {
            return Err(fault("processes", "processorCountUnavailable"));
        }
        let snapshot = read_process_snapshot(watches, sampled_at, &self.skips)?;
        self.accumulator
            .map(snapshot, watches, elapsed, self.logical_processors)
    }
}

#[cfg(windows)]
fn read_process_snapshot(
    watches: &[ProcessWatch],
    sampled_at: i64,
    skips: &ProcessSkipCollector,
) -> Result<Vec<RawProcess>, MetricFault> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let enabled = watches.iter().filter(|watch| watch.enabled).fold(
        BTreeMap::<String, Vec<&ProcessWatch>>::new(),
        |mut map, watch| {
            map.entry(watch.process_name.to_lowercase())
                .or_default()
                .push(watch);
            map
        },
    );
    if enabled.is_empty() {
        return Ok(Vec::new());
    }

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|_| fault("processes", "snapshotFailed"))?;
    let _snapshot_guard = HandleGuard(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Process32FirstW(snapshot, &mut entry) }.is_err() {
        return Err(fault("processes", "snapshotFailed"));
    }

    let mut rows = Vec::new();
    let mut skipped = BTreeMap::<String, u64>::new();
    loop {
        let end = entry
            .szExeFile
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(entry.szExeFile.len());
        let base_name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if let Some(matched) = enabled.get(&base_name.to_lowercase()) {
            collect_process_read(
                read_process(entry.th32ProcessID, base_name),
                matched,
                &mut rows,
                &mut skipped,
            )
            .map_err(|_| fault("processes", "queryFailed"))?;
        }
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
            break;
        }
    }
    if !skipped.is_empty() {
        let mut collector = skips
            .lock()
            .map_err(|_| fault("processes", "lockPoisoned"))?;
        collector.extend(
            skipped
                .into_iter()
                .map(|(watch_id, skipped_count)| ProcessSkip {
                    watch_id,
                    skipped_count,
                    sampled_at,
                }),
        );
    }
    Ok(rows)
}

#[cfg(windows)]
#[derive(Debug, PartialEq, Eq)]
enum ProcessReadFault {
    AccessDenied,
    Gone,
    QueryFailed,
}

#[cfg(windows)]
fn collect_process_read(
    result: Result<RawProcess, ProcessReadFault>,
    matched: &[&ProcessWatch],
    rows: &mut Vec<RawProcess>,
    skipped: &mut BTreeMap<String, u64>,
) -> Result<(), ProcessReadFault> {
    match result {
        Ok(process) => rows.push(process),
        Err(ProcessReadFault::AccessDenied) => {
            for watch in matched {
                *skipped.entry(watch.id.clone()).or_default() += 1;
            }
        }
        Err(ProcessReadFault::Gone) => {}
        Err(ProcessReadFault::QueryFailed) => return Err(ProcessReadFault::QueryFailed),
    }
    Ok(())
}

#[cfg(windows)]
fn process_open_access() -> windows::Win32::System::Threading::PROCESS_ACCESS_RIGHTS {
    use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;

    PROCESS_QUERY_LIMITED_INFORMATION
}

#[cfg(windows)]
fn classify_memory_query_error(error: windows::Win32::Foundation::WIN32_ERROR) -> ProcessReadFault {
    if error == windows::Win32::Foundation::ERROR_ACCESS_DENIED {
        ProcessReadFault::AccessDenied
    } else {
        ProcessReadFault::QueryFailed
    }
}

#[cfg(windows)]
fn read_process(pid: u32, base_name: String) -> Result<RawProcess, ProcessReadFault> {
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetProcessTimes, OpenProcess};

    let handle = unsafe { OpenProcess(process_open_access(), false, pid) }
        .map_err(classify_process_error)?;
    let _guard = HandleGuard(handle);
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }
        .map_err(classify_process_error)?;
    let mut memory = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    if !unsafe {
        K32GetProcessMemoryInfo(
            handle,
            &mut memory,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    }
    .as_bool()
    {
        return Err(classify_memory_query_error(unsafe { GetLastError() }));
    }
    let kernel_user_total = filetime_value(kernel)
        .checked_add(filetime_value(user))
        .ok_or(ProcessReadFault::QueryFailed)?;
    Ok(RawProcess {
        pid,
        base_name,
        creation_time: filetime_value(creation),
        kernel_user_total,
        working_set_bytes: memory.WorkingSetSize as u64,
    })
}

#[cfg(windows)]
fn classify_process_error(error: windows::core::Error) -> ProcessReadFault {
    let code = error.code().0 as u32;
    if code == 0x8007_0005 {
        ProcessReadFault::AccessDenied
    } else if matches!(code, 0x8007_0006 | 0x8007_0057 | 0x8007_0074) {
        ProcessReadFault::Gone
    } else {
        ProcessReadFault::QueryFailed
    }
}

#[cfg(windows)]
fn filetime_value(value: windows::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[cfg(windows)]
struct HandleGuard(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for HandleGuard {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
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

    fn watch(id: &str, name: &str, enabled: bool) -> ProcessWatch {
        ProcessWatch {
            id: id.into(),
            process_name: name.into(),
            enabled,
            revision: 0,
            updated_at: 0,
        }
    }

    fn raw(pid: u32, name: &str, created: u64, total: u64, memory: u64) -> RawProcess {
        RawProcess {
            pid,
            base_name: name.into(),
            creation_time: created,
            kernel_user_total: total,
            working_set_bytes: memory,
        }
    }

    #[test]
    fn enabled_base_names_match_case_insensitively_and_keep_each_pid() {
        let id = "00000000-0000-0000-0000-000000000001";
        let watches = [
            watch(id, "Editor.EXE", true),
            watch("00000000-0000-0000-0000-000000000002", "off.exe", false),
        ];
        let mut accumulator = ProcessAccumulator::default();
        accumulator
            .map(
                vec![
                    raw(7, "editor.exe", 1, 10_000_000, 70),
                    raw(8, "EDITOR.EXE", 2, 20_000_000, 80),
                    raw(9, "off.exe", 3, 1, 90),
                ],
                &watches,
                Duration::from_secs(1),
                2,
            )
            .unwrap();
        let rows = accumulator
            .map(
                vec![
                    raw(7, "editor.exe", 1, 30_000_000, 700),
                    raw(8, "EDITOR.EXE", 2, 30_000_000, 800),
                    raw(9, "off.exe", 3, 2, 900),
                ],
                &watches,
                Duration::from_secs(1),
                2,
            )
            .unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.pid, row.cpu_percent, row.memory_bytes))
                .collect::<Vec<_>>(),
            vec![(7, 100.0, 700), (8, 50.0, 800)]
        );
        assert!(rows
            .iter()
            .all(|row| row.process_watch_id.to_string() == id));
    }

    #[test]
    fn pid_reuse_and_counter_reset_become_new_baselines_and_absent_pids_are_removed() {
        let watches = [watch(
            "00000000-0000-0000-0000-000000000001",
            "app.exe",
            true,
        )];
        let mut accumulator = ProcessAccumulator::default();
        accumulator
            .map(
                vec![raw(1, "app.exe", 10, 10, 1)],
                &watches,
                Duration::from_secs(1),
                1,
            )
            .unwrap();
        assert!(accumulator
            .map(
                vec![raw(1, "app.exe", 11, 20, 2)],
                &watches,
                Duration::from_secs(1),
                1,
            )
            .unwrap()
            .is_empty());
        assert!(accumulator
            .map(
                vec![raw(1, "app.exe", 11, 19, 3)],
                &watches,
                Duration::from_secs(1),
                1,
            )
            .unwrap()
            .is_empty());
        accumulator
            .map(Vec::new(), &watches, Duration::from_secs(1), 1)
            .unwrap();
        assert!(accumulator.baselines.is_empty());
    }

    #[test]
    fn rows_sort_by_cpu_then_memory_descending_then_pid_ascending() {
        let watches = [watch(
            "00000000-0000-0000-0000-000000000001",
            "app.exe",
            true,
        )];
        let mut accumulator = ProcessAccumulator::default();
        accumulator
            .map(
                vec![
                    raw(3, "app.exe", 3, 0, 3),
                    raw(2, "app.exe", 2, 0, 5),
                    raw(1, "app.exe", 1, 0, 5),
                ],
                &watches,
                Duration::from_secs(1),
                1,
            )
            .unwrap();
        let rows = accumulator
            .map(
                vec![
                    raw(3, "app.exe", 3, 10_000_000, 30),
                    raw(2, "app.exe", 2, 20_000_000, 50),
                    raw(1, "app.exe", 1, 20_000_000, 50),
                ],
                &watches,
                Duration::from_secs(2),
                1,
            )
            .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.pid).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[cfg(windows)]
    #[test]
    fn access_denied_is_classified_for_safe_skip_counting() {
        let error =
            windows::core::Error::from_hresult(windows::core::HRESULT(0x8007_0005_u32 as i32));
        assert_eq!(
            classify_process_error(error),
            ProcessReadFault::AccessDenied
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_read_requests_only_limited_query_access() {
        assert_eq!(
            process_open_access(),
            windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_read_memory_access_denied_uses_per_watch_safe_skip() {
        let watched = watch(
            "00000000-0000-0000-0000-000000000001",
            "protected.exe",
            true,
        );
        let mut rows = Vec::new();
        let mut skipped = BTreeMap::new();

        collect_process_read(
            Err(classify_memory_query_error(
                windows::Win32::Foundation::ERROR_ACCESS_DENIED,
            )),
            &[&watched],
            &mut rows,
            &mut skipped,
        )
        .unwrap();

        assert!(rows.is_empty());
        assert_eq!(
            skipped,
            BTreeMap::from([("00000000-0000-0000-0000-000000000001".to_string(), 1,)])
        );
    }
}
