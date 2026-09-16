//! What a sync pass did.
//!
//! A pass used to return `()` on success, so JNI, `aw-sync status`, and the
//! daemon all had to invent a story (a fixed string, a boolean, or silence).
//! `SyncReport` is that story, persisted locally — never in the Syncthing
//! folder, which would leak one device's last pass onto every peer.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[cfg(feature = "cli")]
use clap::ValueEnum;

#[derive(PartialEq, Eq, Copy, Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    Push,
    Pull,
    Both,
}

impl SyncMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncMode::Push => "push",
            SyncMode::Pull => "pull",
            SyncMode::Both => "both",
        }
    }
}

/// Facts about one completed (or failed-and-aborted) sync pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncReport {
    pub started: DateTime<Utc>,
    pub finished: DateTime<Utc>,
    pub mode: SyncMode,
    /// One entry per peer considered on pull, including skipped and failed.
    pub peers: Vec<PeerReport>,
    /// Buckets written to this device's staging db on push.
    pub pushed: Vec<BucketReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerReport {
    pub device_id: String,
    pub hostname: String,
    pub path: PathBuf,
    pub outcome: PeerOutcome,
    pub buckets: Vec<BucketReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PeerOutcome {
    Imported,
    Skipped { reason: String },
    Failed { error: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BucketReport {
    pub bucket_id: String,
    pub events_new: i64,
    pub resumed_at: Option<DateTime<Utc>>,
}

impl SyncReport {
    pub fn new(mode: SyncMode) -> Self {
        let now = Utc::now();
        Self {
            started: now,
            finished: now,
            mode,
            peers: vec![],
            pushed: vec![],
        }
    }

    pub fn finish(&mut self) {
        self.finished = Utc::now();
    }

    /// Fold another pass into this one (pull then push, or per-peer pull_all).
    pub fn merge(&mut self, other: SyncReport) {
        self.started = self.started.min(other.started);
        self.finished = self.finished.max(other.finished);
        if self.mode != other.mode {
            self.mode = SyncMode::Both;
        }
        self.peers.extend(other.peers);
        self.pushed.extend(other.pushed);
    }

    pub fn events_new(&self) -> i64 {
        self.peers
            .iter()
            .flat_map(|p| p.buckets.iter())
            .map(|b| b.events_new)
            .sum::<i64>()
            + self.pushed.iter().map(|b| b.events_new).sum::<i64>()
    }

    pub fn events_pulled(&self) -> i64 {
        self.peers
            .iter()
            .flat_map(|p| p.buckets.iter())
            .map(|b| b.events_new)
            .sum()
    }

    pub fn events_pushed(&self) -> i64 {
        self.pushed.iter().map(|b| b.events_new).sum()
    }

    pub fn peers_imported(&self) -> usize {
        self.peers
            .iter()
            .filter(|p| matches!(p.outcome, PeerOutcome::Imported))
            .count()
    }

    pub fn peers_skipped(&self) -> usize {
        self.peers
            .iter()
            .filter(|p| matches!(p.outcome, PeerOutcome::Skipped { .. }))
            .count()
    }

    pub fn peers_failed(&self) -> usize {
        self.peers
            .iter()
            .filter(|p| matches!(p.outcome, PeerOutcome::Failed { .. }))
            .count()
    }

    pub fn summary_message(&self) -> String {
        let pulled = self.events_pulled();
        let pushed = self.events_pushed();
        let imported = self.peers_imported();
        let skipped = self.peers_skipped();
        let failed = self.peers_failed();
        let peer_n = self.peers.len();
        match self.mode {
            SyncMode::Pull => format!(
                "Pulled {pulled} new events from {imported}/{peer_n} peers \
                 ({skipped} skipped, {failed} failed)"
            ),
            SyncMode::Push => format!(
                "Pushed {pushed} new events across {} bucket(s)",
                self.pushed.len()
            ),
            SyncMode::Both => format!(
                "Synced {pulled} events in from {imported}/{peer_n} peers \
                 ({skipped} skipped, {failed} failed); pushed {pushed} events"
            ),
        }
    }

    /// JSON `SyncInterface.performSyncAsync` already parses (`success` + `message`),
    /// plus the counts that ActivityWatch/aw-android#274 can render without a
    /// JNI-side redesign.
    #[cfg(any(target_os = "android", test))]
    pub fn to_jni_json(&self) -> String {
        serde_json::json!({
            "success": true,
            "message": self.summary_message(),
            "started": self.started.to_rfc3339(),
            "finished": self.finished.to_rfc3339(),
            "mode": self.mode,
            "events_new": self.events_new(),
            "events_pulled": self.events_pulled(),
            "events_pushed": self.events_pushed(),
            "peers_imported": self.peers_imported(),
            "peers_skipped": self.peers_skipped(),
            "peers_failed": self.peers_failed(),
            "peers": self.peers,
            "pushed": self.pushed,
        })
        .to_string()
    }
}

impl PeerReport {
    pub fn imported(
        device_id: String,
        hostname: String,
        path: PathBuf,
        buckets: Vec<BucketReport>,
    ) -> Self {
        Self {
            device_id,
            hostname,
            path,
            outcome: PeerOutcome::Imported,
            buckets,
        }
    }

    pub fn skipped(device_id: String, hostname: String, path: PathBuf, reason: String) -> Self {
        Self {
            device_id,
            hostname,
            path,
            outcome: PeerOutcome::Skipped { reason },
            buckets: vec![],
        }
    }

    pub fn failed(device_id: String, hostname: String, path: PathBuf, error: String) -> Self {
        Self {
            device_id,
            hostname,
            path,
            outcome: PeerOutcome::Failed { error },
            buckets: vec![],
        }
    }
}

impl fmt::Display for SyncReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Last pass")?;
        writeln!(f, "---------")?;
        writeln!(f, "started:  {}", self.started.to_rfc3339())?;
        writeln!(f, "finished: {}", self.finished.to_rfc3339())?;
        writeln!(f, "mode:     {}", self.mode.as_str())?;
        writeln!(
            f,
            "peers:    {} imported, {} skipped, {} failed",
            self.peers_imported(),
            self.peers_skipped(),
            self.peers_failed()
        )?;
        writeln!(
            f,
            "events:   {} pulled, {} pushed ({} total)",
            self.events_pulled(),
            self.events_pushed(),
            self.events_new()
        )?;
        if !self.peers.is_empty() {
            writeln!(f, "peers:")?;
            for peer in &self.peers {
                writeln!(f, "  {}", format_peer_line(peer))?;
            }
        }
        if !self.pushed.is_empty() {
            writeln!(f, "pushed:")?;
            for bucket in &self.pushed {
                writeln!(
                    f,
                    "  {}  +{} events{}",
                    bucket.bucket_id,
                    bucket.events_new,
                    match bucket.resumed_at {
                        Some(t) => format!("  resumed {}", t.to_rfc3339()),
                        None => String::new(),
                    }
                )?;
            }
        }
        Ok(())
    }
}

fn format_peer_line(peer: &PeerReport) -> String {
    let host = if peer.hostname.is_empty() {
        "?".to_string()
    } else {
        peer.hostname.clone()
    };
    let did = if peer.device_id.is_empty() {
        "?".to_string()
    } else {
        peer.device_id.clone()
    };
    let events: i64 = peer.buckets.iter().map(|b| b.events_new).sum();
    match &peer.outcome {
        PeerOutcome::Imported => format!("{host} / {did}  imported  +{events} events"),
        PeerOutcome::Skipped { reason } => {
            format!("{host} / {did}  skipped  {reason}")
        }
        PeerOutcome::Failed { error } => format!("{host} / {did}  failed  {error}"),
    }
}

/// Override with `AW_SYNC_LAST_REPORT` (absolute path) in tests.
pub fn last_report_path() -> Result<PathBuf, Box<dyn Error>> {
    if let Ok(p) = std::env::var("AW_SYNC_LAST_REPORT") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let dir = aw_server::dirs::get_data_dir().map_err(|_| "Could not get data dir")?;
    Ok(dir.join("aw-sync").join("last-sync-report.json"))
}

pub fn persist_last_report(report: &SyncReport) -> Result<PathBuf, Box<dyn Error>> {
    let path = last_report_path()?;
    persist_last_report_to(report, &path)?;
    Ok(path)
}

pub fn persist_last_report_to(report: &SyncReport, path: &Path) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(report)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Persist, but a write failure must not fail a pass that already succeeded.
pub fn persist_last_report_warn(report: &SyncReport) {
    match persist_last_report(report) {
        Ok(path) => debug!("Wrote last sync report to {}", path.display()),
        Err(e) => warn!("Failed to persist last sync report: {e}"),
    }
}

pub fn load_last_report() -> Result<Option<SyncReport>, Box<dyn Error>> {
    load_last_report_from(&last_report_path()?)
}

pub fn load_last_report_from(path: &Path) -> Result<Option<SyncReport>, Box<dyn Error>> {
    if !path.exists() {
        return Ok(None);
    }
    let body = fs::read_to_string(path)?;
    Ok(Some(serde_json::from_str(&body)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_report_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "aw-sync-report-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn roundtrip_json_and_counts() {
        let mut report = SyncReport::new(SyncMode::Both);
        report.peers.push(PeerReport::imported(
            "dev-a".into(),
            "laptop".into(),
            PathBuf::from("/sync/laptop/dev-a/test.db"),
            vec![BucketReport {
                bucket_id: "aw-watcher-window".into(),
                events_new: 10,
                resumed_at: None,
            }],
        ));
        report.peers.push(PeerReport {
            device_id: "dev-a".into(),
            hostname: "old-name".into(),
            path: PathBuf::from("/sync/old-name/dev-a/test.db"),
            outcome: PeerOutcome::Skipped {
                reason: "duplicate device_id dev-a; kept larger db".into(),
            },
            buckets: vec![],
        });
        report.pushed.push(BucketReport {
            bucket_id: "aw-watcher-afk".into(),
            events_new: 3,
            resumed_at: None,
        });
        report.finish();

        assert_eq!(report.events_pulled(), 10);
        assert_eq!(report.events_pushed(), 3);
        assert_eq!(report.events_new(), 13);
        assert_eq!(report.peers_imported(), 1);
        assert_eq!(report.peers_skipped(), 1);
        assert_eq!(report.peers_failed(), 0);

        let path = temp_report_path();
        persist_last_report_to(&report, &path).unwrap();
        let loaded = load_last_report_from(&path).unwrap().unwrap();
        let _ = fs::remove_file(&path);
        assert_eq!(loaded.peers_imported(), 1);
        assert_eq!(loaded.peers_skipped(), 1);
        assert_eq!(loaded.pushed[0].events_new, 3);
        assert!(loaded.summary_message().contains("10 events"));
        let json = loaded.to_jni_json();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"peers_skipped\":1"));
    }

    #[test]
    fn load_missing_is_none() {
        let path = temp_report_path();
        let _ = fs::remove_file(&path);
        assert!(load_last_report_from(&path).unwrap().is_none());
    }

    #[test]
    fn display_includes_skipped_reason() {
        let mut report = SyncReport::new(SyncMode::Pull);
        report.peers.push(PeerReport {
            device_id: "abc".into(),
            hostname: "phone".into(),
            path: PathBuf::from("/sync/phone/abc/test.db"),
            outcome: PeerOutcome::Skipped {
                reason: "duplicate device_id abc; kept larger db".into(),
            },
            buckets: vec![],
        });
        let text = report.to_string();
        assert!(text.contains("Last pass"));
        assert!(text.contains("skipped  duplicate device_id abc"));
    }
}
