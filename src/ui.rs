use crate::config::{UploadConfig, WatchConfig};
use crate::manifest::Manifest;
use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use flate2::read::GzDecoder;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tower::ServiceBuilder;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

const MAX_LINES_DEFAULT: usize = 5000;

#[derive(Clone)]
struct UiState {
    storage: Option<Arc<StorageInspector>>,
    root_prefix: String,
    max_lines: usize,
}

impl UiState {
    fn new(storage: Option<Arc<StorageInspector>>, config: &WatchConfig) -> Self {
        Self {
            storage,
            root_prefix: config.root_prefix.trim_end_matches('/').to_string(),
            max_lines: MAX_LINES_DEFAULT,
        }
    }
}

#[derive(Clone)]
struct StorageInspector {
    client: Client,
    base_url: String,
    api_key: String,
    bucket: String,
}

impl StorageInspector {
    fn new(base_url: String, api_key: String, bucket: String) -> Result<Self> {
        let client = Client::builder()
            .user_agent("agent-uploader/ui/0.1")
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            base_url,
            api_key,
            bucket,
        })
    }

    async fn list_session_manifests(&self, root_prefix: &str) -> Result<Vec<Manifest>> {
        let mut manifests = Vec::new();
        for sid in self.list_sessions(root_prefix).await? {
            match self.fetch_manifest(root_prefix, &sid).await {
                Ok(manifest) => manifests.push(manifest),
                Err(err) => {
                    tracing::warn!(session = %sid, error = %err, "failed to fetch manifest");
                }
            }
        }
        Ok(manifests)
    }

    async fn list_sessions(&self, root_prefix: &str) -> Result<Vec<String>> {
        let url = format!(
            "{}/storage/v1/object/list/{}",
            self.base_url.trim_end_matches('/'),
            self.bucket
        );
        let prefix = format!("{}/", root_prefix.trim_start_matches('/'));
        let body = serde_json::json!({
            "prefix": prefix,
            "limit": 1000,
            "offset": 0,
            "sortBy": { "column": "name", "order": "asc" },
            "depth": 2
        });
        let response = self
            .client
            .post(url)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("failed to list sessions: {} {}", status, text);
        }

        let text = response.text().await?;
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("failed to parse storage list payload: {text}"))?;
        let objects = match value {
            Value::Array(array) => array,
            Value::Object(obj) => obj
                .get("data")
                .and_then(|data| data.as_array())
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        };

        let mut result = Vec::new();
        for item in objects {
            let Some(name) = item.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let candidate = if let Some(stripped) = name.strip_prefix(&prefix) {
                stripped
            } else {
                name
            };
            if candidate.ends_with("manifest.json") {
                if let Some((sid, _)) = candidate.split_once('/') {
                    if !sid.is_empty() {
                        result.push(sid.to_string());
                    }
                }
            }
        }
        result.sort();
        result.dedup();
        Ok(result)
    }

    async fn fetch_manifest(&self, root_prefix: &str, sid: &str) -> Result<Manifest> {
        let object_path = format!(
            "{}/{}/{}",
            root_prefix.trim_start_matches('/'),
            sid,
            crate::manifest::MANIFEST_FILENAME
        );
        let bytes = self.fetch_object_bytes(&object_path).await?;
        let manifest: Manifest = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse manifest for {sid}"))?;
        Ok(manifest)
    }

    async fn fetch_segment_lines(
        &self,
        root_prefix: &str,
        sid: &str,
        path: &str,
    ) -> Result<Vec<Value>> {
        let object_path = format!("{}/{}/{}", root_prefix.trim_start_matches('/'), sid, path);
        let bytes = self.fetch_object_bytes(&object_path).await?;
        let raw = if path.ends_with(".gz") {
            let mut decoder = GzDecoder::new(bytes.as_slice());
            let mut out = Vec::new();
            use std::io::Read;
            decoder.read_to_end(&mut out)?;
            out
        } else {
            bytes
        };
        parse_ndjson_lines(&raw)
    }

    async fn fetch_object_bytes(&self, object_path: &str) -> Result<Vec<u8>> {
        let url = format!(
            "{}/storage/v1/object/{}/{}",
            self.base_url.trim_end_matches('/'),
            self.bucket,
            object_path.trim_start_matches('/')
        );
        let response = self
            .client
            .get(url)
            .header("authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "failed to fetch object {}: {} {}",
                object_path,
                status,
                text
            );
        }
        Ok(response.bytes().await?.to_vec())
    }
}

#[derive(Serialize)]
struct SessionsResponse {
    sessions: Vec<SessionPayload>,
}

#[derive(Serialize)]
struct SessionPayload {
    sid: String,
    manifest: Manifest,
}

#[derive(Deserialize)]
struct ReplayQuery {
    seq: Option<u32>,
    line_idx: Option<u64>,
    max_lines: Option<usize>,
}

#[derive(Serialize)]
struct ReplayResponse {
    lines: Vec<Value>,
}

pub struct UiHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl UiHandle {
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.join.await;
    }
}

pub async fn spawn(config: Arc<WatchConfig>) -> Result<Option<UiHandle>> {
    if !config.ui.enabled {
        return Ok(None);
    }

    let state = build_state(&config)?;
    let Some(dist_dir) = config.ui.dist_dir.clone() else {
        tracing::warn!("web ui disabled: no dist directory provided or found");
        return Ok(None);
    };
    if !dist_dir.exists() {
        tracing::warn!(path = %dist_dir.display(), "web ui disabled: dist directory missing");
        return Ok(None);
    }

    let addr: SocketAddr = format!("{}:{}", config.ui.bind, config.ui.port)
        .parse()
        .context("invalid ui bind address")?;

    let router = build_router(state.clone(), dist_dir.clone());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("failed to bind ui listener")?;
    let local_addr = listener
        .local_addr()
        .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
    tracing::info!(address = %local_addr, "ui available");

    let (tx, mut rx) = watch::channel(false);
    let server =
        axum::serve(listener, router.into_make_service()).with_graceful_shutdown(async move {
            let _ = rx.changed().await;
        });

    let handle = tokio::spawn(async move {
        if let Err(err) = server.await {
            tracing::error!(error = %err, "ui server terminated");
        }
    });

    Ok(Some(UiHandle {
        shutdown: tx,
        join: handle,
    }))
}

fn build_state(config: &Arc<WatchConfig>) -> Result<UiState> {
    let storage = match &config.upload {
        UploadConfig::Supabase { base_url, api_key } => Some(Arc::new(StorageInspector::new(
            base_url.clone(),
            api_key.clone(),
            config.bucket.clone(),
        )?)),
        _ => None,
    };
    Ok(UiState::new(storage, config))
}

fn build_router(state: UiState, dist_dir: PathBuf) -> Router {
    let api_state = Arc::new(state);
    let static_service =
        ServeDir::new(&dist_dir).not_found_service(ServeFile::new(dist_dir.join("index.html")));

    Router::new()
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions/:sid/replay", get(replay_session))
        .with_state(api_state)
        .nest_service("/", static_service)
        .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()))
}

async fn list_sessions(State(state): State<Arc<UiState>>) -> Response {
    let Some(storage) = state.storage.clone() else {
        return JsonError::service_unavailable("Supabase access not configured").into_response();
    };

    match storage.list_session_manifests(&state.root_prefix).await {
        Ok(manifests) => {
            let sessions = manifests
                .into_iter()
                .map(|manifest| SessionPayload {
                    sid: manifest.sid.clone(),
                    manifest,
                })
                .collect();
            Json(SessionsResponse { sessions }).into_response()
        }
        Err(err) => JsonError::internal(err).into_response(),
    }
}

async fn replay_session(
    State(state): State<Arc<UiState>>,
    Path(sid): Path<String>,
    Query(params): Query<ReplayQuery>,
) -> Response {
    let Some(storage) = state.storage.clone() else {
        return JsonError::service_unavailable("Supabase access not configured").into_response();
    };

    let target_seq = params.seq.unwrap_or(1);
    let target_line_idx = params.line_idx.unwrap_or(0);
    let max_lines = params.max_lines.unwrap_or(state.max_lines);

    match storage.fetch_manifest(&state.root_prefix, &sid).await {
        Ok(manifest) => match collect_lines(
            &storage,
            &state.root_prefix,
            &sid,
            &manifest,
            target_seq,
            target_line_idx,
            max_lines,
        )
        .await
        {
            Ok(lines) => Json(ReplayResponse { lines }).into_response(),
            Err(err) => JsonError::internal(err).into_response(),
        },
        Err(err) => JsonError::internal(err).into_response(),
    }
}

async fn collect_lines(
    storage: &StorageInspector,
    root_prefix: &str,
    sid: &str,
    manifest: &Manifest,
    target_seq: u32,
    target_line_idx: u64,
    max_lines: usize,
) -> Result<Vec<Value>> {
    let mut lines = Vec::new();
    for segment in &manifest.segments {
        if segment.seq < target_seq {
            let mut seg_lines = storage
                .fetch_segment_lines(root_prefix, sid, &segment.path)
                .await?;
            lines.append(&mut seg_lines);
        } else if segment.seq == target_seq {
            let mut seg_lines = storage
                .fetch_segment_lines(root_prefix, sid, &segment.path)
                .await?;
            let cutoff = (target_line_idx as usize + 1).min(seg_lines.len());
            seg_lines.truncate(cutoff);
            lines.append(&mut seg_lines);
            break;
        }
    }
    if lines.len() > max_lines {
        let start = lines.len() - max_lines;
        return Ok(lines[start..].to_vec());
    }
    Ok(lines)
}

fn parse_ndjson_lines(bytes: &[u8]) -> Result<Vec<Value>> {
    let mut lines = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_slice(line)?;
        lines.push(value);
    }
    Ok(lines)
}

struct JsonError {
    status: StatusCode,
    message: String,
}

impl JsonError {
    fn internal<E: std::fmt::Display>(err: E) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }

    fn service_unavailable(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: msg.into(),
        }
    }
}

impl IntoResponse for JsonError {
    fn into_response(self) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        let body = serde_json::json!({ "error": self.message });
        (self.status, headers, body.to_string()).into_response()
    }
}
