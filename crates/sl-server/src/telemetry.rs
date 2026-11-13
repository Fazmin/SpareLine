//! Logs and metrics.
//!
//! - Rolling daily JSON log files in `logs/` (retention configurable).
//! - An in-memory ring buffer that feeds the control panel's live log view.
//! - A reloadable filter so "Debug mode" can raise networking verbosity at runtime and
//!   switch itself off again.
//! - Per-minute counters for the dashboard.
//!
//! Privacy: nothing here ever sees message content. Payloads are `Bytes`, whose Debug
//! output is only a length, and handlers never log request bodies.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use parking_lot::Mutex;
use sl_proto::Timestamp;
use sl_proto::panel::{LogEntry, LogLevel, LogQuery, StatPoint};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::Registry;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, reload};

use crate::config::Paths;

const RING_CAPACITY: usize = 10_000;
const MINUTES_KEPT: usize = 24 * 60;

pub const NORMAL_FILTER: &str =
    "info,sl_server=info,spareline_server=info,hyper=warn,tungstenite=warn,mdns_sd=warn";
pub const DEBUG_FILTER: &str = "info,sl_server=debug,sl_server::ws=trace,sl_server::auth=debug,sl_server::tls=trace,sl_server::mdns=debug,sl_server::delivery=debug,spareline_server=debug,mdns_sd=info";

/// Shared telemetry handles given to the engine.
#[derive(Clone)]
pub struct Telemetry {
    pub logs: Arc<LogHub>,
    pub metrics: Arc<Metrics>,
}

impl Telemetry {
    /// Telemetry not connected to a global subscriber (tests, embedding).
    pub fn detached() -> Self {
        Self {
            logs: Arc::new(LogHub::new(None)),
            metrics: Arc::new(Metrics::default()),
        }
    }
}

/// Keeps the non-blocking file writer alive. Hold it for the life of the process.
pub struct TelemetryGuard {
    _file: Option<tracing_appender::non_blocking::WorkerGuard>,
}

/// Install the global tracing subscriber. Call once per process.
pub fn init(
    paths: &Paths,
    log_retention_days: u32,
    console: bool,
) -> anyhow::Result<(Telemetry, TelemetryGuard)> {
    std::fs::create_dir_all(&paths.logs)?;
    let (filter, handle) = reload::Layer::new(EnvFilter::new(NORMAL_FILTER));
    let metrics = Arc::new(Metrics::default());
    let logs = Arc::new(LogHub::new(Some(handle)));

    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("spareline-server")
        .filename_suffix("log")
        .max_log_files(log_retention_days.max(1) as usize)
        .build(&paths.logs)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(writer)
        .with_current_span(false);
    let console_layer =
        console.then(|| tracing_subscriber::fmt::layer().with_target(true).compact());
    let ring = RingLayer {
        hub: logs.clone(),
        metrics: metrics.clone(),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .with(ring)
        .try_init()?;

    Ok((
        Telemetry { logs, metrics },
        TelemetryGuard { _file: Some(guard) },
    ))
}

// -------------------------------------------------------------------------------------
// Log ring buffer
// -------------------------------------------------------------------------------------

pub struct LogHub {
    ring: Mutex<VecDeque<LogEntry>>,
    seq: AtomicU64,
    tx: broadcast::Sender<LogEntry>,
    reload: Option<reload::Handle<EnvFilter, Registry>>,
    debug_until: Mutex<Option<Timestamp>>,
}

impl LogHub {
    fn new(reload: Option<reload::Handle<EnvFilter, Registry>>) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            ring: Mutex::new(VecDeque::with_capacity(RING_CAPACITY)),
            seq: AtomicU64::new(0),
            tx,
            reload,
            debug_until: Mutex::new(None),
        }
    }

    fn push(&self, mut entry: LogEntry) {
        entry.seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        {
            let mut ring = self.ring.lock();
            if ring.len() == RING_CAPACITY {
                ring.pop_front();
            }
            ring.push_back(entry.clone());
        }
        let _ = self.tx.send(entry);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.tx.subscribe()
    }

    pub fn query(&self, q: &LogQuery) -> Vec<LogEntry> {
        let ring = self.ring.lock();
        let limit = q.limit.unwrap_or(500).min(RING_CAPACITY as u32) as usize;
        let text = q
            .text
            .as_ref()
            .map(|t| t.to_lowercase())
            .filter(|t| !t.is_empty());
        let target = q.target.as_ref().filter(|t| !t.is_empty());
        let mut out: Vec<LogEntry> = ring
            .iter()
            .rev()
            .filter(|e| q.after_seq.is_none_or(|s| e.seq > s))
            .filter(|e| q.min_level.is_none_or(|l| e.level >= l))
            .filter(|e| target.is_none_or(|t| e.target.contains(t.as_str())))
            .filter(|e| {
                text.as_ref().is_none_or(|t| {
                    e.message.to_lowercase().contains(t) || e.fields.to_lowercase().contains(t)
                })
            })
            .take(limit)
            .cloned()
            .collect();
        out.reverse();
        out
    }

    /// Plain-text dump of the most recent entries (for diagnostic reports).
    pub fn dump(&self, max: usize) -> String {
        let ring = self.ring.lock();
        let mut out = String::new();
        let skip = ring.len().saturating_sub(max);
        for e in ring.iter().skip(skip) {
            let _ = writeln!(
                out,
                "{} {:?} {} {} {}",
                e.ts, e.level, e.target, e.message, e.fields
            );
        }
        out
    }

    /// Enable debug verbosity until `until` (unix ms), or disable with `None`.
    pub fn set_debug_until(&self, until: Option<Timestamp>) {
        *self.debug_until.lock() = until;
        if let Some(h) = &self.reload {
            let filter = if until.is_some() {
                DEBUG_FILTER
            } else {
                NORMAL_FILTER
            };
            if let Err(e) = h.modify(|f| *f = EnvFilter::new(filter)) {
                tracing::warn!(error = %e, "could not change log filter");
            }
        }
        match until {
            Some(t) => tracing::info!(until = t, "debug mode enabled"),
            None => tracing::info!("debug mode disabled"),
        }
    }

    pub fn debug_until(&self) -> Option<Timestamp> {
        *self.debug_until.lock()
    }

    /// Turn debug mode off if its time is up. Returns true if it was switched off.
    pub fn expire_debug(&self, now: Timestamp) -> bool {
        let until = *self.debug_until.lock();
        if let Some(t) = until
            && now >= t
        {
            self.set_debug_until(None);
            return true;
        }
        false
    }
}

struct RingLayer {
    hub: Arc<LogHub>,
    metrics: Arc<Metrics>,
}

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let level = match *meta.level() {
            Level::TRACE => LogLevel::Trace,
            Level::DEBUG => LogLevel::Debug,
            Level::INFO => LogLevel::Info,
            Level::WARN => LogLevel::Warn,
            Level::ERROR => LogLevel::Error,
        };
        if level == LogLevel::Error {
            self.metrics.record_error();
        }
        let mut v = FieldVisitor::default();
        event.record(&mut v);
        self.hub.push(LogEntry {
            seq: 0,
            ts: sl_proto::now_ms(),
            level,
            target: meta.target().to_string(),
            message: v.message,
            fields: v.fields,
        });
    }
}

#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: String,
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, "{}={} ", field.name(), value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, "{}={:?} ", field.name(), value);
        }
    }
}

// -------------------------------------------------------------------------------------
// Metrics
// -------------------------------------------------------------------------------------

#[derive(Default)]
pub struct Metrics {
    minutes: Mutex<VecDeque<StatPoint>>,
    connections: AtomicU32,
    total_messages: AtomicU64,
    total_bytes: AtomicU64,
}

impl Metrics {
    fn bump(&self, f: impl FnOnce(&mut StatPoint)) {
        let now = sl_proto::now_ms();
        let minute = now - now.rem_euclid(60_000);
        let mut m = self.minutes.lock();
        Self::roll(&mut m, minute, self.connections.load(Ordering::Relaxed));
        if let Some(last) = m.back_mut() {
            f(last);
        }
    }

    fn roll(m: &mut VecDeque<StatPoint>, minute: Timestamp, conns: u32) {
        match m.back() {
            Some(last) if last.ts == minute => {}
            Some(last) if last.ts < minute => {
                let mut t = last.ts + 60_000;
                // Fill gaps (cap to the window size).
                if (minute - t) / 60_000 > MINUTES_KEPT as i64 {
                    t = minute - (MINUTES_KEPT as i64 - 1) * 60_000;
                }
                while t <= minute {
                    m.push_back(StatPoint {
                        ts: t,
                        connections: conns,
                        ..Default::default()
                    });
                    t += 60_000;
                }
            }
            Some(_) => {}
            None => m.push_back(StatPoint {
                ts: minute,
                connections: conns,
                ..Default::default()
            }),
        }
        while m.len() > MINUTES_KEPT {
            m.pop_front();
        }
    }

    pub fn record_message(&self, bytes: usize) {
        self.total_messages.fetch_add(1, Ordering::Relaxed);
        self.total_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.bump(|p| p.messages += 1);
    }

    pub fn record_auth_failure(&self) {
        self.bump(|p| p.auth_failures += 1);
    }

    pub fn record_error(&self) {
        self.bump(|p| p.errors += 1);
    }

    pub fn set_connections(&self, n: u32) {
        self.connections.store(n, Ordering::Relaxed);
        self.bump(|p| p.connections = p.connections.max(n));
    }

    pub fn connections(&self) -> u32 {
        self.connections.load(Ordering::Relaxed)
    }

    pub fn total_messages(&self) -> u64 {
        self.total_messages.load(Ordering::Relaxed)
    }

    /// Last 24 hours of one-minute buckets (gaps filled).
    pub fn series(&self) -> Vec<StatPoint> {
        let now = sl_proto::now_ms();
        let minute = now - now.rem_euclid(60_000);
        let mut m = self.minutes.lock();
        Self::roll(&mut m, minute, self.connections.load(Ordering::Relaxed));
        m.iter().copied().collect()
    }

    pub fn messages_since(&self, since: Timestamp) -> u64 {
        self.minutes
            .lock()
            .iter()
            .filter(|p| p.ts >= since)
            .map(|p| p.messages as u64)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_bucket_and_sum() {
        let m = Metrics::default();
        m.record_message(10);
        m.record_message(10);
        m.record_auth_failure();
        let s = m.series();
        assert!(!s.is_empty());
        let last = s.last().copied().unwrap_or_default();
        assert_eq!(last.messages, 2);
        assert_eq!(last.auth_failures, 1);
        assert_eq!(m.messages_since(0), 2);
    }

    #[test]
    fn log_query_filters() {
        let hub = LogHub::new(None);
        for (i, lvl) in [LogLevel::Info, LogLevel::Warn, LogLevel::Error]
            .into_iter()
            .enumerate()
        {
            hub.push(LogEntry {
                seq: 0,
                ts: i as i64,
                level: lvl,
                target: "sl_server::ws".into(),
                message: format!("event {i}"),
                fields: String::new(),
            });
        }
        let warn_up = hub.query(&LogQuery {
            min_level: Some(LogLevel::Warn),
            ..Default::default()
        });
        assert_eq!(warn_up.len(), 2);
        let text = hub.query(&LogQuery {
            text: Some("EVENT 2".into()),
            ..Default::default()
        });
        assert_eq!(text.len(), 1);
        let after = hub.query(&LogQuery {
            after_seq: Some(2),
            ..Default::default()
        });
        assert_eq!(after.len(), 1);
    }
}
