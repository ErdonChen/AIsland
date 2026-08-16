use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct NewMonitorSample {
    pub cpu_percent: f64,
    pub memory_used_bytes: i64,
    pub memory_total_bytes: i64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub network_rx_bps: f64,
    pub network_tx_bps: f64,
    pub gpu_percent: Option<f64>,
    pub sampled_at: i64,
}

#[derive(Clone, Debug)]
pub struct NewProcessSample {
    pub process_watch_id: Uuid,
    pub pid: i64,
    pub process_name: String,
    pub cpu_percent: f64,
    pub memory_bytes: i64,
}

#[derive(Clone, Debug)]
pub struct ThresholdBreachUpdate {
    pub threshold_id: Uuid,
    pub breach_started_at: i64,
    pub last_triggered_at: Option<i64>,
    pub cleared_at: Option<i64>,
    pub reminder_delivery_id: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThresholdBreach {
    pub id: String,
    pub threshold_id: String,
    pub breach_started_at: i64,
    pub last_triggered_at: Option<i64>,
    pub cleared_at: Option<i64>,
    pub reminder_delivery_id: Option<String>,
}
