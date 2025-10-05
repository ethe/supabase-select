use crate::config::WatchConfig;
use crate::util::ensure_dir;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

pub const MANIFEST_FILENAME: &str = "manifest.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default = "default_version")]
    pub version: u32,
    pub sid: String,
    #[serde(default = "default_created_at", with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default = "default_updated_at", with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(default)]
    pub segments: Vec<SegmentEntry>,
    #[serde(default)]
    pub checkpoints: Vec<ManifestCheckpoint>,
    #[serde(default)]
    pub active_seq: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SegmentEntry {
    pub seq: u32,
    pub path: String,
    pub first_ts: i64,
    pub last_ts: i64,
    pub lines: u64,
    pub bytes_uncompressed: u64,
    pub bytes_gzip: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManifestCheckpoint {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub seq: u32,
    pub line_idx: u64,
    pub ts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SegmentStats {
    pub first_ts: i64,
    pub last_ts: i64,
    pub lines: u64,
    pub bytes_uncompressed: u64,
    pub bytes_gzip: u64,
    pub checksum: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ManifestStore {
    path: PathBuf,
}

impl Manifest {
    pub fn new(config: &WatchConfig) -> Self {
        Self {
            version: default_version(),
            sid: config.sid.clone(),
            created_at: config.created_at,
            updated_at: config.created_at,
            segments: Vec::new(),
            checkpoints: Vec::new(),
            active_seq: 1,
        }
    }

    pub fn latest_seq(&self) -> u32 {
        self.segments.last().map(|s| s.seq + 1).unwrap_or(1)
    }

    pub fn manifest_path(prefix: &str) -> String {
        format!("{}/{}", prefix.trim_end_matches('/'), MANIFEST_FILENAME)
    }

    pub fn load_or_new(path: &Path, config: &WatchConfig) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new(config));
        }
        let data = std::fs::read(path)
            .with_context(|| format!("failed to read manifest from {}", path.display()))?;
        let mut manifest: Manifest = serde_json::from_slice(&data)
            .with_context(|| format!("invalid manifest json at {}", path.display()))?;
        if manifest.version == 0 {
            manifest.version = default_version();
        }
        if manifest.updated_at < manifest.created_at {
            manifest.updated_at = manifest.created_at;
        }
        if manifest.segments.is_empty() {
            manifest.active_seq = manifest.active_seq.max(1);
        } else {
            manifest.active_seq = manifest.segments.last().map(|seg| seg.seq + 1).unwrap_or(1);
        }
        Ok(manifest)
    }

    pub fn add_segment(&mut self, segment: SegmentEntry) {
        self.active_seq = segment.seq + 1;
        self.segments.push(segment);
        self.touch_updated();
    }

    pub fn add_checkpoint(&mut self, checkpoint: ManifestCheckpoint) {
        self.checkpoints.push(checkpoint);
        self.touch_updated();
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        serde_json::to_writer_pretty(&mut buf, self).context("failed to serialize manifest")?;
        Ok(buf)
    }

    fn touch_updated(&mut self) {
        self.updated_at = OffsetDateTime::now_utc();
    }
}

impl SegmentEntry {
    pub fn new(seq: u32, path: String, stats: SegmentStats) -> Self {
        Self {
            seq,
            path,
            first_ts: stats.first_ts,
            last_ts: stats.last_ts,
            lines: stats.lines,
            bytes_uncompressed: stats.bytes_uncompressed,
            bytes_gzip: stats.bytes_gzip,
            checksum: stats.checksum,
        }
    }
}

fn default_created_at() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn default_updated_at() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn default_version() -> u32 {
    1
}

impl ManifestStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_or_new(&self, config: &WatchConfig) -> Result<Manifest> {
        Manifest::load_or_new(&self.path, config)
    }

    pub fn save(&self, manifest: &Manifest) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            ensure_dir(parent)?;
        }
        let tmp = self.path.with_extension("tmp");
        let bytes = manifest.to_bytes()?;
        std::fs::write(&tmp, &bytes)
            .with_context(|| format!("failed to write manifest temp file {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("failed to persist manifest to {}", self.path.display()))?;
        Ok(())
    }
}
