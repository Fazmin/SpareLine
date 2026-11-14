//! Control-panel IPC DTOs (server Tauri backend ⇄ panel UI).
//!
//! Privacy rule: nothing in this module can describe a conversation. The panel sees
//! accounts, groups, devices, connection health and aggregate counters only.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Timestamp;
use crate::model::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelState {
    pub needs_setup: bool,
    pub unlocked: bool,
    pub server_name: String,
    pub suggested_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupInput {
    pub server_name: String,
    pub admin_password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupResult {
    /// Shown once. Lets the admin reset the panel password.
    pub recovery_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    Running,
    Degraded,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCheck {
    pub id: String,
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    /// Optional one-click fix (e.g. add firewall rule).
    pub fix: Option<FixAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixAction {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StatPoint {
    /// Start of the one-minute bucket.
    pub ts: Timestamp,
    pub connections: u32,
    pub messages: u32,
    pub auth_failures: u32,
    pub errors: u32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SystemStats {
    pub cpu_percent: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub process_mem: u64,
    pub disk_free: u64,
    pub disk_total: u64,
    pub db_size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overview {
    pub status: ServerStatus,
    pub server_name: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub fingerprint: String,
    pub started_at: Timestamp,
    pub users_total: u32,
    pub users_online: u32,
    pub devices_total: u32,
    /// Devices seen in the last 7 days: "ready for an outage".
    pub devices_ready: u32,
    pub connections: u32,
    pub messages_last_hour: u64,
    pub messages_last_24h: u64,
    pub series: Vec<StatPoint>,
    pub system: SystemStats,
    pub cert_not_after: Timestamp,
    pub health: Vec<HealthCheck>,
    pub debug_mode_until: Option<Timestamp>,
    pub mdns_active: bool,
    pub active_broadcasts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    Active,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRow {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
    pub status: UserStatus,
    pub groups: Vec<GroupRef>,
    pub devices: u32,
    pub online: bool,
    pub last_seen: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRow {
    pub id: DeviceId,
    pub name: String,
    pub platform: String,
    pub client_version: String,
    pub created_at: Timestamp,
    pub last_seen: Option<Timestamp>,
    pub revoked: bool,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRow {
    /// First characters of the token hash, for identification only.
    pub id: String,
    pub device_id: DeviceId,
    pub created_at: Timestamp,
    pub last_used: Timestamp,
    pub expires_at: Timestamp,
    pub ip: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserDetail {
    pub user: UserRow,
    pub devices: Vec<DeviceRow>,
    pub sessions: Vec<SessionRow>,
    pub recent_events: Vec<ConnectionEvent>,
    pub health_reports: Vec<HealthReportRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReportRow {
    pub received_at: Timestamp,
    pub device_id: DeviceId,
    pub report: ClientHealthReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRow {
    pub id: GroupId,
    pub name: String,
    pub description: String,
    pub member_count: u32,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRef {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDetail {
    pub group: GroupRow,
    pub members: Vec<UserRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteKind {
    NewUser,
    Setup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteStatus {
    Pending,
    Used,
    Expired,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteRow {
    pub id: Uuid,
    pub kind: InviteKind,
    pub groups: Vec<GroupRef>,
    pub target_user: Option<UserRef>,
    pub used_by: Option<UserRef>,
    pub note: String,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub status: InviteStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInvitesInput {
    pub count: u32,
    pub group_ids: Vec<GroupId>,
    pub expires_in_secs: i64,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedInvite {
    pub row: InviteRow,
    /// Display form `SPL-XXXX-...`. Shown once; the server keeps only derived keys.
    pub code: String,
    /// Ready-to-send text with the code, host hints and a `spareline://` link.
    pub message: String,
    pub link: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BroadcastInput {
    pub title: String,
    pub body: String,
    pub severity: Severity,
    pub expires_in_secs: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BroadcastRow {
    pub broadcast: Broadcast,
    pub active: bool,
    pub ended_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub seq: u64,
    pub ts: Timestamp,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
    /// `key=value` pairs (already redacted).
    pub fields: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogQuery {
    pub min_level: Option<LogLevel>,
    pub target: Option<String>,
    pub text: Option<String>,
    /// Only entries with `seq > after_seq`.
    pub after_seq: Option<u64>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionRow {
    pub conn_id: u64,
    pub user: UserRef,
    pub device_id: DeviceId,
    pub device_name: String,
    pub ip: String,
    pub connected_at: Timestamp,
    pub rtt_ms: Option<u32>,
    pub client_version: String,
    pub platform: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnEventKind {
    TcpAccepted,
    TlsFailed,
    Connected,
    Disconnected,
    AuthFailed,
    Registered,
    ProtocolMismatch,
    RateLimited,
    SessionRevoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionEvent {
    pub ts: Timestamp,
    pub ip: String,
    pub user: Option<UserRef>,
    pub device_id: Option<DeviceId>,
    pub kind: ConnEventKind,
    /// Plain-language reason (no content).
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfTestStep {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    pub millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRow {
    pub ts: Timestamp,
    pub action: String,
    pub target: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSettings {
    pub server_name: String,
    pub port: u16,
    pub envelope_retention_days: u32,
    pub log_retention_days: u32,
    pub backup_keep: u32,
    pub session_idle_days: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupRow {
    pub file_name: String,
    pub created_at: Timestamp,
    pub size: u64,
}

/// Standby copies: an encrypted copy of this server kept somewhere else, so another
/// computer can take over if this one fails.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StandbyStatus {
    pub enabled: bool,
    /// Where copies go (empty when off).
    pub folder: String,
    pub interval_minutes: u32,
    pub keep: u32,
    /// A standby password has been set.
    pub has_password: bool,
    pub last_copy: Option<StandbyCopyRow>,
    /// The most recent attempt failed, and why.
    pub last_error: Option<String>,
    /// The folder is on the same disk as this server's data, so it wouldn't survive
    /// a disk failure.
    pub same_disk: bool,
    /// Copies of this server in the folder, newest first.
    pub copies: Vec<StandbyCopyRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandbyCopyRow {
    pub file_name: String,
    pub created_at: Timestamp,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandbySetupInput {
    pub folder: String,
    pub interval_minutes: u32,
    pub keep: u32,
    /// Empty keeps the current standby password.
    pub password: String,
}

/// What a standby copy holds, shown before restoring it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandbyInfo {
    pub server_name: String,
    pub server_id: Uuid,
    pub taken_at: Timestamp,
    pub fingerprint: String,
    pub port: u16,
    pub people: u32,
    /// The old server's network addresses when the copy was taken.
    pub addresses: Vec<String>,
    pub app_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreResult {
    pub info: StandbyInfo,
    /// This computer's network addresses.
    pub addresses: Vec<String>,
}
