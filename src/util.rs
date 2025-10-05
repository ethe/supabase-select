use anyhow::{Context, Result, bail};
use path_absolutize::Absolutize;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;
use uuid::Uuid;

static SID_FORMAT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]Z");

pub fn expand_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Ok(path.to_path_buf());
    }
    let path_str = path.to_string_lossy();
    let expanded = if let Some(stripped) = path_str.strip_prefix('~') {
        let base = directories::BaseDirs::new()
            .context("unable to resolve home directory for path expansion")?
            .home_dir()
            .to_path_buf();
        let stripped = stripped.trim_start_matches(|c| c == '/' || c == '\\');
        if stripped.is_empty() {
            base
        } else {
            base.join(stripped)
        }
    } else {
        path.to_path_buf()
    };
    expanded
        .absolutize()
        .map(|p| p.to_path_buf())
        .context("failed to absolutize path")
}

pub fn generate_sid() -> String {
    let ts = OffsetDateTime::now_utc()
        .format(SID_FORMAT)
        .unwrap_or_else(|_| "unknown".to_string());
    let uuid = Uuid::new_v4().to_string();
    let suffix = uuid.split('-').next().unwrap_or("sid");
    format!("{}-{}", ts, suffix)
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    if path.exists() {
        if path.is_dir() {
            return Ok(());
        }
        bail!("{} exists but is not a directory", path.display());
    }
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory {}", path.display()))?;
    Ok(())
}
