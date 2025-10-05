use crate::util::{expand_path, generate_sid};
use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::time::Duration;
use time::Duration as TimeDuration;
use time::OffsetDateTime;
use uuid::Uuid;

const DEFAULT_SEG_BYTES: usize = 8 * 1024 * 1024; // 8 MiB
const DEFAULT_SEG_LINES: usize = 10_000;
const DEFAULT_SEG_WALL_MS: u64 = 600_000; // 10 minutes
const DEFAULT_POLL_MS: u64 = 500;
const DEFAULT_CONCURRENCY: usize = 2;
const DEFAULT_ROOT_PREFIX: &str = "sessions";
const DEFAULT_UI_PORT: u16 = 4333;

#[derive(Debug, Parser)]
#[command(name = "agent-uploader", version, about = "Tail Codex sessions and mirror them to Supabase Storage", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Tail a session history file and mirror segments to Supabase Storage
    Watch(WatchArgs),
    /// Reconstruct a session.jsonl locally from remote Storage assets
    Reload(ReloadArgs),
    /// Stream a session to stdout using remote Storage assets
    Replay(ReplayArgs),
    /// Serve the local web UI bundle for browsing sessions
    Host(HostArgs),
    /// Print the CLI version information
    Version,
}

#[derive(Debug, Clone, Args)]
pub struct WatchArgs {
    /// Path to the session history file emitted by the coding agent CLI (NDJSON)
    #[arg(long = "file", env = "AGENT_SESSION_FILE")]
    pub session_file: PathBuf,

    /// Supabase Storage bucket name
    #[arg(long, env = "SUPABASE_BUCKET", default_value = "sessions")]
    pub bucket: String,

    /// Session identifier. Use the value "auto" to auto-generate
    #[arg(long, env = "AGENT_SID", default_value = "auto")]
    pub sid: String,

    /// Root prefix prepended before the session id when storing objects
    #[arg(long, default_value = DEFAULT_ROOT_PREFIX)]
    pub root_prefix: String,

    /// Maximum uncompressed bytes per segment before rotation
    #[arg(long = "seg-bytes", default_value_t = DEFAULT_SEG_BYTES)]
    pub seg_bytes: usize,

    /// Maximum lines per segment before rotation
    #[arg(long = "seg-lines", default_value_t = DEFAULT_SEG_LINES)]
    pub seg_lines: usize,

    /// Maximum wall-clock duration per segment (ms)
    #[arg(long = "seg-ms", default_value_t = DEFAULT_SEG_WALL_MS)]
    pub seg_ms: u64,

    /// Poll interval in milliseconds for tailing the session file
    #[arg(long = "poll-ms", default_value_t = DEFAULT_POLL_MS)]
    pub poll_ms: u64,

    /// Directory used to spool pending uploads when offline
    #[arg(long = "spool-dir")]
    pub spool_dir: Option<PathBuf>,

    /// Number of concurrent uploads to Storage
    #[arg(long, default_value_t = DEFAULT_CONCURRENCY)]
    pub concurrency: usize,

    /// Verbose logging (sets RUST_LOG=debug if unset)
    #[arg(long)]
    pub verbose: bool,

    /// Dry-run mode. Tail and rotate locally but skip remote uploads
    #[arg(long)]
    pub dry_run: bool,

    /// Disable gzip compression for closed segments
    #[arg(long = "no-gzip")]
    pub no_gzip: bool,

    /// Override Supabase REST endpoint (https://<project>.supabase.co)
    #[arg(long = "supabase-url", env = "SUPABASE_URL")]
    pub supabase_url: Option<String>,

    /// Service or anon key for Supabase Storage REST
    #[arg(long = "supabase-key", env = "SUPABASE_KEY")]
    pub supabase_key: Option<String>,

    /// Optional presigned upload URL template; bypasses Supabase REST
    #[arg(long = "upload-url")]
    pub upload_url: Option<String>,

    /// Optional path to write manifests locally before upload
    #[arg(long = "state-dir")]
    pub state_dir: Option<PathBuf>,

    /// Disable the embedded web UI server
    #[arg(long = "ui-disable")]
    pub ui_disable: bool,

    /// Bind address for the embedded web UI
    #[arg(long = "ui-bind", env = "AGENT_UI_BIND", default_value = "127.0.0.1")]
    pub ui_bind: String,

    /// TCP port for the embedded web UI
    #[arg(long = "ui-port", env = "AGENT_UI_PORT", default_value_t = DEFAULT_UI_PORT)]
    pub ui_port: u16,

    /// Directory containing the built web UI assets (defaults to ./frontend/dist)
    #[arg(long = "ui-dist", env = "AGENT_UI_DIST")]
    pub ui_dist: Option<PathBuf>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ReloadArgs {
    /// Output path to write the reconstructed session file
    #[arg(long = "to")]
    pub output: Option<PathBuf>,

    /// Remote session identifier
    #[arg(long, env = "AGENT_SID")]
    pub sid: Option<String>,

    /// Optional checkpoint id (or "latest") to stop replay
    #[arg(long = "checkpoint", default_value = "latest")]
    pub checkpoint: String,
}

#[derive(Debug, Clone, Args, Default)]
pub struct ReplayArgs {
    /// Remote session identifier
    #[arg(long, env = "AGENT_SID")]
    pub sid: Option<String>,

    /// Optional checkpoint id (or "latest")
    #[arg(long = "checkpoint", default_value = "latest")]
    pub checkpoint: String,
}

#[derive(Debug, Clone, Args, Default)]
pub struct HostArgs {
    /// Directory containing static web assets to serve
    #[arg(long = "web-dir")]
    pub web_dir: Option<PathBuf>,

    /// Port to bind the HTTP server
    #[arg(long, default_value_t = 4333)]
    pub port: u16,

    /// Automatically open the default browser when the server starts
    #[arg(long = "open")]
    pub open_browser: bool,

    /// Supabase project URL used by the hosted UI
    #[arg(long = "supabase-url", env = "SUPABASE_URL")]
    pub supabase_url: Option<String>,

    /// Supabase anon key surfaced to the hosted UI
    #[arg(long = "supabase-anon-key", env = "SUPABASE_ANON_KEY")]
    pub supabase_anon_key: Option<String>,

    /// Storage bucket expected by the hosted UI
    #[arg(long = "bucket", env = "SUPABASE_BUCKET", default_value = "sessions")]
    pub bucket: String,
}

#[derive(Debug, Clone)]
pub struct WatchConfig {
    pub session_file: PathBuf,
    pub bucket: String,
    pub sid: String,
    pub root_prefix: String,
    pub rotate: RotatePolicy,
    pub poll_interval: Duration,
    pub spool_dir: PathBuf,
    pub concurrency: usize,
    pub verbose: bool,
    pub dry_run: bool,
    pub gzip_enabled: bool,
    pub upload: UploadConfig,
    pub manifest_state_dir: PathBuf,
    pub created_at: OffsetDateTime,
    pub ui: UiConfig,
}

#[derive(Debug, Clone)]
pub struct RotatePolicy {
    pub max_bytes: usize,
    pub max_lines: usize,
    pub max_wall: TimeDuration,
}

#[derive(Debug, Clone)]
pub struct UiConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    pub dist_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum UploadConfig {
    Supabase { base_url: String, api_key: String },
    Presigned { base_url: String },
    DryRun,
}

impl Cli {
    pub fn parse_args() -> Self {
        Cli::parse()
    }
}

impl WatchConfig {
    pub fn from_args(args: WatchArgs) -> Result<Self> {
        Self::try_from_args(args)
    }

    fn try_from_args(args: WatchArgs) -> Result<Self> {
        let session_file = expand_path(&args.session_file)?;
        let spool_dir = match args.spool_dir {
            Some(path) => expand_path(&path)?,
            None => default_spool_dir()?,
        };

        let manifest_state_dir = match args.state_dir {
            Some(path) => expand_path(&path)?,
            None => spool_dir.join("state"),
        };

        let ui_dist = match args.ui_dist {
            Some(path) => Some(expand_path(&path)?),
            None => default_ui_dist()?,
        };

        let rotate = RotatePolicy {
            max_bytes: args.seg_bytes,
            max_lines: args.seg_lines,
            max_wall: TimeDuration::milliseconds(args.seg_ms as i64),
        };

        if rotate.max_bytes == 0 {
            bail!("seg-bytes must be greater than 0");
        }

        if rotate.max_lines == 0 {
            bail!("seg-lines must be greater than 0");
        }

        if rotate.max_wall.is_negative() || rotate.max_wall.is_zero() {
            bail!("seg-ms must be greater than 0");
        }

        let poll_interval = Duration::from_millis(args.poll_ms);
        if poll_interval.is_zero() {
            bail!("poll-ms must be greater than 0");
        }

        let sid = if args.sid.trim().eq_ignore_ascii_case("auto") {
            match derive_sid_from_session_file(&session_file) {
                Some(derived) => derived,
                None => generate_sid(),
            }
        } else {
            sanitize_sid(&args.sid)?
        };
        let sid = sanitize_sid(&sid)?;

        let upload = if args.dry_run {
            UploadConfig::DryRun
        } else if let Some(url) = args.upload_url {
            UploadConfig::Presigned { base_url: url }
        } else {
            let base_url = args
                .supabase_url
                .clone()
                .context("supabase-url is required unless --upload-url or --dry-run is set")?;
            let api_key = args
                .supabase_key
                .clone()
                .context("supabase-key is required unless --upload-url or --dry-run is set")?;
            UploadConfig::Supabase { base_url, api_key }
        };

        let created_at = OffsetDateTime::now_utc();

        let ui = UiConfig {
            enabled: !args.ui_disable,
            bind: args.ui_bind,
            port: args.ui_port,
            dist_dir: ui_dist,
        };

        Ok(Self {
            session_file,
            bucket: args.bucket,
            sid,
            root_prefix: args.root_prefix,
            rotate,
            poll_interval,
            spool_dir,
            concurrency: args.concurrency.max(1),
            verbose: args.verbose,
            dry_run: args.dry_run,
            gzip_enabled: !args.no_gzip,
            upload,
            manifest_state_dir,
            created_at,
            ui,
        })
    }

    pub fn object_prefix(&self) -> String {
        format!("{}/{}", self.root_prefix.trim_end_matches('/'), self.sid)
    }
}

fn default_spool_dir() -> Result<PathBuf> {
    let home =
        directories::BaseDirs::new().context("unable to determine home directory for spool dir")?;
    Ok(home.home_dir().join(".agent-uploader").join("spool"))
}

fn default_ui_dist() -> Result<Option<PathBuf>> {
    let current =
        std::env::current_dir().context("failed to determine current directory for ui assets")?;
    let candidate = current.join("frontend").join("dist");
    if candidate.exists() {
        Ok(Some(candidate))
    } else {
        Ok(None)
    }
}

fn sanitize_sid(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("sid cannot be empty");
    }
    if trimmed.chars().any(|c| c.is_whitespace()) {
        bail!("sid cannot contain whitespace");
    }
    Ok(trimmed.to_string())
}

impl UploadConfig {
    pub fn prefers_supabase(&self) -> bool {
        matches!(self, UploadConfig::Supabase { .. })
    }
}

fn derive_sid_from_session_file(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    if let Some(uuid) = extract_uuid(stem.as_ref()) {
        return Some(uuid);
    }
    let name = path.file_name()?.to_string_lossy();
    extract_uuid(name.as_ref())
}

fn extract_uuid(candidate: &str) -> Option<String> {
    if let Ok(uuid) = Uuid::parse_str(candidate) {
        return Some(uuid.to_string());
    }
    if candidate.len() < 36 {
        return None;
    }
    for start in (0..=candidate.len().saturating_sub(36)).rev() {
        let end = start + 36;
        if let Some(slice) = candidate.get(start..end) {
            if Uuid::parse_str(slice).is_ok() {
                return Some(slice.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_sid_rejects_whitespace() {
        assert!(sanitize_sid("bad id").is_err());
        assert!(sanitize_sid(" ").is_err());
        assert!(sanitize_sid("good-id").is_ok());
    }

    #[test]
    fn derive_sid_from_rollout_filename() {
        let path = PathBuf::from(
            "/tmp/rollout-2025-10-04T15-16-09-0199b14b-f650-7c52-93bd-b226acca5ff5.jsonl",
        );
        let derived = derive_sid_from_session_file(&path).expect("uuid expected");
        assert_eq!(derived, "0199b14b-f650-7c52-93bd-b226acca5ff5");
    }
}
