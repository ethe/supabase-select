use crate::config::WatchConfig;
use crate::util::ensure_dir;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use tokio::fs;

pub const META_EXTENSION: &str = "meta.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpoolItemKind {
    Segment,
    Manifest,
    Checkpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolMetadata {
    pub remote_path: String,
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
    pub created_at: OffsetDateTime,
    pub kind: SpoolItemKind,
}

#[derive(Debug, Clone)]
pub struct SpoolEntry {
    pub data_path: PathBuf,
    pub metadata_path: PathBuf,
    pub metadata: SpoolMetadata,
}

#[derive(Debug, Clone)]
pub struct SpoolLayout {
    pub root: PathBuf,
    pub active_dir: PathBuf,
    pub queue_dir: PathBuf,
    pub manifest_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SpoolQueue {
    layout: SpoolLayout,
}

impl SpoolLayout {
    pub fn new(root: PathBuf) -> Self {
        let active_dir = root.join("active");
        let queue_dir = root.join("queue");
        let manifest_dir = root.join("manifests");
        Self {
            root,
            active_dir,
            queue_dir,
            manifest_dir,
        }
    }

    pub fn from_config(config: &WatchConfig) -> Self {
        Self::new(config.spool_dir.clone())
    }

    pub fn manifest_state_path(&self, sid: &str) -> PathBuf {
        self.manifest_dir.join(format!("{sid}.json"))
    }

    pub fn active_segment_path(&self, name: &str) -> PathBuf {
        self.active_dir.join(name)
    }

    pub fn queued_segment_path(&self, name: &str) -> PathBuf {
        self.queue_dir.join(name)
    }

    pub fn queued_raw_segment_path(&self, name: &str) -> PathBuf {
        self.queue_dir.join(name)
    }

    pub fn queue_manifest_path(&self) -> PathBuf {
        self.queue_dir.join("manifest.json")
    }

    pub fn queued_checkpoint_path(&self, name: &str) -> PathBuf {
        self.queue_dir.join(name)
    }

    pub fn ensure(&self) -> Result<()> {
        ensure_dir(&self.root)?;
        ensure_dir(&self.active_dir)?;
        ensure_dir(&self.queue_dir)?;
        ensure_dir(&self.manifest_dir)?;
        Ok(())
    }

    pub fn metadata_path(&self, data_path: &Path) -> PathBuf {
        let base = data_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("segment");
        let meta_name = format!("{base}.{}", META_EXTENSION);
        if let Some(parent) = data_path.parent() {
            parent.join(&meta_name)
        } else {
            PathBuf::from(meta_name)
        }
    }
}

impl SpoolQueue {
    pub fn new(layout: SpoolLayout) -> Self {
        Self { layout }
    }

    pub fn layout(&self) -> &SpoolLayout {
        &self.layout
    }

    pub async fn enqueue(&self, data_path: &Path, metadata: &SpoolMetadata) -> Result<()> {
        let meta_path = self.layout.metadata_path(data_path);
        if fs::metadata(data_path).await.is_err() {
            anyhow::bail!("spool enqueue missing data file {}", data_path.display());
        }
        if let Some(parent) = meta_path.parent() {
            ensure_dir(parent)?;
        }
        let tmp = meta_path.with_extension("tmp");
        let payload = serde_json::to_vec(metadata)?;
        fs::write(&tmp, payload).await?;
        fs::rename(&tmp, &meta_path).await?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<SpoolEntry>> {
        let mut entries = Vec::new();
        let mut dir = fs::read_dir(&self.layout.queue_dir).await?;
        let suffix = format!(".{}", META_EXTENSION);
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !name.ends_with(&suffix) {
                continue;
            }
            let data_name = &name[..name.len() - suffix.len()];
            let data_path = path.with_file_name(data_name);
            if fs::metadata(&data_path).await.is_err() {
                continue;
            }
            let data = fs::read(&path).await?;
            let metadata: SpoolMetadata = serde_json::from_slice(&data)?;
            entries.push(SpoolEntry {
                data_path,
                metadata_path: path.clone(),
                metadata,
            });
        }
        entries.sort_by(|a, b| a.metadata.created_at.cmp(&b.metadata.created_at));
        Ok(entries)
    }

    pub async fn mark_uploaded(&self, entry: &SpoolEntry) -> Result<()> {
        if fs::metadata(&entry.data_path).await.is_ok() {
            fs::remove_file(&entry.data_path).await?;
        }
        if fs::metadata(&entry.metadata_path).await.is_ok() {
            fs::remove_file(&entry.metadata_path).await?;
        }
        Ok(())
    }
}
