use crate::config::{RotatePolicy, WatchConfig};
use crate::manifest::{ManifestCheckpoint, SegmentEntry, SegmentStats};
use crate::spool::SpoolLayout;
use crate::tail::{CheckpointTrigger, SessionEvent};
use crate::util::ensure_dir;
use anyhow::{Context, Result};
use async_compression::tokio::write::GzipEncoder;
use serde::Serialize;
use serde_json::{self, Value};
use std::convert::TryInto;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;

pub const SEGMENT_PREFIX: &str = "session";
const CHECKPOINT_ID_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]Z");

#[derive(Debug, Clone)]
pub struct SegmentFileSet {
    pub seq: u32,
    pub active_path: PathBuf,
    pub compressed_path: PathBuf,
    pub queued_raw_path: PathBuf,
    pub active_name: String,
    pub compressed_name: String,
    pub remote_active: String,
    pub remote_compressed: String,
    pub manifest_path: String,
}

#[derive(Debug)]
pub struct SegmentWriter {
    config: Arc<WatchConfig>,
    spool: SpoolLayout,
    policy: RotatePolicy,
    wall_duration: Duration,
    seq: u32,
    fileset: SegmentFileSet,
    file: Option<File>,
    opened_at: Instant,
    lines: u64,
    bytes: u64,
    first_ts: Option<i64>,
    last_ts: Option<i64>,
    pending_checkpoint: Option<PendingCheckpoint>,
    gzip_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct SegmentClosed {
    pub entry: SegmentEntry,
    pub stats: SegmentStats,
    pub files: SegmentFileSet,
    pub checkpoint: Option<PendingCheckpoint>,
    pub upload_local_path: PathBuf,
    pub upload_remote_path: String,
    pub content_encoding: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingCheckpoint {
    pub id: String,
    pub seq: u32,
    pub line_idx: u64,
    pub timestamp: OffsetDateTime,
    pub label: Option<String>,
    pub git_commit: Option<String>,
    pub branch: Option<String>,
    pub file_path: PathBuf,
    pub remote_path: String,
    pub manifest_path: String,
    pub payload: Option<Value>,
}

#[derive(Debug, Serialize)]
struct CheckpointFileRecord {
    id: String,
    seq: u32,
    line_idx: u64,
    ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<Value>,
}

impl PendingCheckpoint {
    pub fn manifest_entry(&self) -> ManifestCheckpoint {
        ManifestCheckpoint {
            id: self.id.clone(),
            label: self.label.clone(),
            seq: self.seq,
            line_idx: self.line_idx,
            ts: self.timestamp.unix_timestamp(),
            git: self.git_commit.clone(),
            branch: self.branch.clone(),
        }
    }

    pub fn file_bytes(&self) -> Result<Vec<u8>> {
        let record = CheckpointFileRecord {
            id: self.id.clone(),
            seq: self.seq,
            line_idx: self.line_idx,
            ts: self.timestamp.unix_timestamp(),
            label: self.label.clone(),
            git: self.git_commit.clone(),
            branch: self.branch.clone(),
            payload: self.payload.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&record)?;
        Ok(bytes)
    }
}

impl SegmentWriter {
    pub async fn new(
        config: Arc<WatchConfig>,
        spool: SpoolLayout,
        starting_seq: u32,
    ) -> Result<Self> {
        spool.ensure()?;
        let prefix = config.object_prefix();
        let fileset = SegmentFileSet::new(&spool, &prefix, starting_seq)?;
        let file = open_segment_file(&fileset.active_path).await?;
        let policy = RotatePolicy {
            max_bytes: config.rotate.max_bytes,
            max_lines: config.rotate.max_lines,
            max_wall: config.rotate.max_wall,
        };
        let wall_duration = policy
            .max_wall
            .try_into()
            .unwrap_or(Duration::from_secs(600));
        let gzip_enabled = config.gzip_enabled;
        Ok(Self {
            config: config.clone(),
            spool,
            policy,
            wall_duration,
            seq: starting_seq,
            fileset,
            file: Some(file),
            opened_at: Instant::now(),
            lines: 0,
            bytes: 0,
            first_ts: None,
            last_ts: None,
            pending_checkpoint: None,
            gzip_enabled,
        })
    }

    pub async fn append(&mut self, event: &SessionEvent) -> Result<Option<SegmentClosed>> {
        self.write_event(event).await?;
        self.lines += 1;
        self.bytes += event.raw.len() as u64 + 1;
        let ts = event.unix_ts;
        if self.first_ts.is_none() {
            self.first_ts = Some(ts);
        }
        self.last_ts = Some(ts);

        if let Some(trigger) = event.checkpoint.as_ref() {
            let pending = self.build_pending_checkpoint(event, trigger)?;
            self.pending_checkpoint = Some(pending);
            let closed = self.rotate().await?;
            self.start_next_segment().await?;
            return Ok(Some(closed));
        }

        if self.should_rotate() {
            let closed = self.rotate().await?;
            self.start_next_segment().await?;
            return Ok(Some(closed));
        }

        Ok(None)
    }

    pub async fn force_rotate(&mut self) -> Result<Option<SegmentClosed>> {
        if self.lines == 0 {
            return Ok(None);
        }
        let closed = self.rotate().await?;
        self.start_next_segment().await?;
        Ok(Some(closed))
    }

    pub fn gzip_enabled(&self) -> bool {
        self.gzip_enabled
    }

    fn should_rotate(&self) -> bool {
        if self.lines == 0 {
            return false;
        }
        if self.bytes as usize >= self.policy.max_bytes {
            return true;
        }
        if self.lines as usize >= self.policy.max_lines {
            return true;
        }
        if self.opened_at.elapsed() >= self.wall_duration {
            return true;
        }
        false
    }

    async fn write_event(&mut self, event: &SessionEvent) -> Result<()> {
        let file = self
            .file
            .as_mut()
            .context("segment writer missing active file handle")?;
        file.write_all(&event.raw).await?;
        file.write_all(b"\n").await?;
        Ok(())
    }

    async fn rotate(&mut self) -> Result<SegmentClosed> {
        let mut file = self
            .file
            .take()
            .context("segment writer missing file during rotate")?;
        file.flush().await?;
        drop(file);

        let fileset = self.fileset.clone();
        let (bytes_gzip, upload_local_path, upload_remote_path, manifest_path, content_encoding) =
            if self.gzip_enabled {
                let gzip_bytes = gzip_file(&fileset.active_path, &fileset.compressed_path).await?;
                if !self.config.dry_run {
                    let _ = fs::remove_file(&fileset.active_path).await;
                }
                (
                    gzip_bytes,
                    fileset.compressed_path.clone(),
                    fileset.remote_compressed.clone(),
                    fileset.manifest_path.clone(),
                    Some("gzip".to_string()),
                )
            } else {
                move_to_queue(&fileset.active_path, &fileset.queued_raw_path).await?;
                (
                    self.bytes,
                    fileset.queued_raw_path.clone(),
                    fileset.remote_active.clone(),
                    format!("segments/{}", fileset.active_name),
                    None,
                )
            };
        let checkpoint = self.pending_checkpoint.take();
        let stats = SegmentStats {
            first_ts: self.first_ts.unwrap_or(0),
            last_ts: self.last_ts.unwrap_or(self.first_ts.unwrap_or(0)),
            lines: self.lines,
            bytes_uncompressed: self.bytes,
            bytes_gzip,
            checksum: None,
        };
        let entry = SegmentEntry::new(self.seq, manifest_path.clone(), stats.clone());

        Ok(SegmentClosed {
            entry,
            stats,
            files: fileset,
            checkpoint,
            upload_local_path,
            upload_remote_path,
            content_encoding,
        })
    }

    async fn start_next_segment(&mut self) -> Result<()> {
        self.seq += 1;
        self.bytes = 0;
        self.lines = 0;
        self.first_ts = None;
        self.last_ts = None;
        self.pending_checkpoint = None;
        self.opened_at = Instant::now();
        let prefix = self.config.object_prefix();
        self.fileset = SegmentFileSet::new(&self.spool, &prefix, self.seq)?;
        let next_file = open_segment_file(&self.fileset.active_path).await?;
        self.file = Some(next_file);
        Ok(())
    }

    fn build_pending_checkpoint(
        &self,
        event: &SessionEvent,
        trigger: &CheckpointTrigger,
    ) -> Result<PendingCheckpoint> {
        let id = format_checkpoint_id(event.timestamp, self.seq, self.lines);
        let file_name = format!("{}.json", id);
        let manifest_path = format!("checkpoints/{}", file_name);
        let remote_path = format!(
            "{}/{}",
            self.config.object_prefix().trim_end_matches('/'),
            manifest_path
        );
        let file_path = self.spool.queued_checkpoint_path(&file_name);
        Ok(PendingCheckpoint {
            id,
            seq: self.seq,
            line_idx: self.lines,
            timestamp: event.timestamp,
            label: trigger.label.clone(),
            git_commit: trigger.git_commit.clone(),
            branch: trigger.branch.clone(),
            file_path,
            remote_path,
            manifest_path,
            payload: trigger.payload.clone().or_else(|| event.json.clone()),
        })
    }
}

impl SegmentFileSet {
    pub fn new(spool: &SpoolLayout, prefix: &str, seq: u32) -> Result<Self> {
        let file_stem = format!("{}-{:06}", SEGMENT_PREFIX, seq);
        let active_name = format!("{}.jsonl", file_stem);
        let compressed_name = format!("{}.jsonl.gz", file_stem);
        let active_path = spool.active_segment_path(&active_name);
        let compressed_path = spool.queued_segment_path(&compressed_name);
        let queued_raw_path = spool.queued_raw_segment_path(&active_name);
        if let Some(parent) = active_path.parent() {
            ensure_dir(parent)?;
        }
        if let Some(parent) = compressed_path.parent() {
            ensure_dir(parent)?;
        }
        if let Some(parent) = queued_raw_path.parent() {
            ensure_dir(parent)?;
        }
        let prefix_trimmed = prefix.trim_end_matches('/');
        let manifest_path = format!("segments/{}", compressed_name);
        let remote_active = format!("{}/segments/{}", prefix_trimmed, active_name);
        let remote_compressed = format!("{}/{}", prefix_trimmed, manifest_path);
        Ok(Self {
            seq,
            active_path,
            compressed_path,
            queued_raw_path,
            active_name,
            compressed_name,
            remote_active,
            remote_compressed,
            manifest_path,
        })
    }
}

async fn open_segment_file(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .write(true)
        .open(path)
        .await
        .with_context(|| format!("failed to open segment file {}", path.display()))?;
    Ok(file)
}

async fn gzip_file(source: &Path, dest: &Path) -> Result<u64> {
    if let Some(parent) = dest.parent() {
        ensure_dir(parent)?;
    }
    let mut reader = File::open(source)
        .await
        .with_context(|| format!("failed to open {} for compression", source.display()))?;
    let dest_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dest)
        .await
        .with_context(|| format!("failed to create gzip {}", dest.display()))?;
    let mut encoder = GzipEncoder::new(dest_file);
    tokio::io::copy(&mut reader, &mut encoder).await?;
    encoder.shutdown().await?;
    let meta = fs::metadata(dest).await?;
    Ok(meta.len())
}

async fn move_to_queue(source: &Path, dest: &Path) -> Result<()> {
    if source == dest {
        return Ok(());
    }
    match fs::rename(source, dest).await {
        Ok(_) => Ok(()),
        Err(err) => {
            if err.kind() == ErrorKind::NotFound {
                return Ok(());
            }
            tokio::fs::copy(source, dest).await.with_context(|| {
                format!("failed to copy {} to {}", source.display(), dest.display())
            })?;
            tokio::fs::remove_file(source)
                .await
                .with_context(|| format!("failed to remove {} after copy", source.display()))?;
            Ok(())
        }
    }
}

fn format_checkpoint_id(timestamp: OffsetDateTime, seq: u32, line_idx: u64) -> String {
    let base = timestamp
        .format(CHECKPOINT_ID_FORMAT)
        .unwrap_or_else(|_| timestamp.unix_timestamp().to_string());
    format!("{base}-s{seq:06}-l{line_idx:06}")
}
