use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Debug, Clone)]
pub struct SessionEvent {
    pub raw: Vec<u8>,
    pub json: Option<Value>,
    pub timestamp: OffsetDateTime,
    pub unix_ts: i64,
    pub event_type: Option<String>,
    pub checkpoint: Option<CheckpointTrigger>,
}

#[derive(Debug)]
pub struct TailReader {
    path: PathBuf,
    file: File,
    offset: u64,
    carry: Vec<u8>,
}

#[derive(Debug)]
pub struct TailBatch {
    pub events: Vec<SessionEvent>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct CheckpointTrigger {
    pub label: Option<String>,
    pub git_commit: Option<String>,
    pub branch: Option<String>,
    pub payload: Option<Value>,
}

impl TailReader {
    pub async fn new(path: PathBuf) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .open(&path)
            .await
            .with_context(|| format!("failed to open session file {}", path.display()))?;
        Ok(Self {
            path,
            file,
            offset: 0,
            carry: Vec::new(),
        })
    }

    pub async fn poll(&mut self) -> Result<Option<TailBatch>> {
        let metadata = match fs::metadata(&self.path).await {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(err) => return Err(err.into()),
        };
        let len = metadata.len();
        let mut truncated = false;
        if len < self.offset {
            self.reset().await?;
            truncated = true;
        }
        if len == self.offset && !truncated {
            return Ok(None);
        }
        let to_read = len - self.offset;
        let mut buf = vec![0u8; to_read as usize];
        self.file.seek(SeekFrom::Start(self.offset)).await?;
        self.file.read_exact(&mut buf).await?;
        self.offset = len;

        let mut data = Vec::new();
        if !self.carry.is_empty() {
            data.extend_from_slice(&self.carry);
            self.carry.clear();
        }
        data.extend_from_slice(&buf);

        let mut start = 0usize;
        let mut events = Vec::new();
        for (idx, byte) in data.iter().enumerate() {
            if *byte == b'\n' {
                let mut line = data[start..idx].to_vec();
                if line.ends_with(b"\r") {
                    line.pop();
                }
                if !line.is_empty() {
                    events.push(SessionEvent::from_line(line));
                }
                start = idx + 1;
            }
        }
        if start < data.len() {
            self.carry = data[start..].to_vec();
        }
        if !truncated && events.is_empty() {
            return Ok(None);
        }
        Ok(Some(TailBatch { events, truncated }))
    }

    pub async fn reset(&mut self) -> Result<()> {
        self.file = OpenOptions::new()
            .read(true)
            .open(&self.path)
            .await
            .with_context(|| format!("failed to reopen session file {}", self.path.display()))?;
        self.offset = 0;
        self.carry.clear();
        Ok(())
    }
}

impl SessionEvent {
    pub fn from_line(raw: Vec<u8>) -> Self {
        match serde_json::from_slice::<Value>(&raw) {
            Ok(value) => {
                let timestamp = extract_timestamp(&value).unwrap_or_else(OffsetDateTime::now_utc);
                let event_type = extract_event_type(&value);
                let checkpoint = extract_checkpoint(event_type.as_deref(), &value);
                Self {
                    raw,
                    json: Some(value),
                    timestamp,
                    unix_ts: timestamp.unix_timestamp(),
                    event_type,
                    checkpoint,
                }
            }
            Err(_) => {
                let timestamp = OffsetDateTime::now_utc();
                Self {
                    raw,
                    json: None,
                    timestamp,
                    unix_ts: timestamp.unix_timestamp(),
                    event_type: None,
                    checkpoint: None,
                }
            }
        }
    }
}

pub fn poll_interval(duration: Duration) -> tokio::time::Interval {
    tokio::time::interval(duration)
}

fn extract_timestamp(value: &Value) -> Option<OffsetDateTime> {
    match value {
        Value::Object(map) => map
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|ts| OffsetDateTime::parse(ts, &Rfc3339).ok()),
        _ => None,
    }
}

fn extract_event_type(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => map
            .get("type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

fn extract_checkpoint(event_type: Option<&str>, value: &Value) -> Option<CheckpointTrigger> {
    if event_type != Some("compacted") {
        return None;
    }
    let payload = match value {
        Value::Object(map) => map,
        _ => return None,
    };

    let checkpoint_obj = payload
        .get("checkpoint")
        .and_then(|v| v.as_object())
        .cloned()
        .or_else(|| payload.get("detail").and_then(|v| v.as_object()).cloned())
        .unwrap_or_else(Map::new);

    let git_commit = checkpoint_obj
        .get("git_commit")
        .or_else(|| checkpoint_obj.get("git"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let branch = checkpoint_obj
        .get("branch")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let label = checkpoint_obj
        .get("label")
        .or_else(|| checkpoint_obj.get("summary"))
        .or_else(|| payload.get("note"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| Some("compacted".to_string()));

    Some(CheckpointTrigger {
        label,
        git_commit,
        branch,
        payload: Some(Value::Object(checkpoint_obj.clone())),
    })
}
