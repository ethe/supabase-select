use crate::config::{UploadConfig, WatchConfig};
use crate::spool::SpoolEntry;
use anyhow::Result;
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Client, Method, StatusCode};
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::fs::File;
use tokio::time::sleep;
use tokio_util::io::ReaderStream;

const MAX_ATTEMPTS: usize = 6;
const BASE_DELAY_MS: u64 = 500;
const MAX_DELAY_MS: u64 = 30_000;

#[derive(Debug, Clone)]
pub struct UploadClient {
    pub client: Client,
    pub config: Arc<WatchConfig>,
}

#[derive(Debug, Clone)]
pub struct UploadRequest {
    pub object_path: String,
    pub local_path: std::path::PathBuf,
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
}

#[derive(Debug)]
struct AttemptError {
    error: anyhow::Error,
    retryable: bool,
}

impl AttemptError {
    fn fatal<E: Into<anyhow::Error>>(err: E) -> Self {
        Self {
            error: err.into(),
            retryable: false,
        }
    }

    fn retryable<E: Into<anyhow::Error>>(err: E) -> Self {
        Self {
            error: err.into(),
            retryable: true,
        }
    }
}

impl UploadClient {
    pub fn new(config: Arc<WatchConfig>) -> Result<Self> {
        let client = Client::builder()
            .user_agent("agent-uploader/0.1")
            .pool_max_idle_per_host(12)
            .build()?;
        Ok(Self { client, config })
    }

    pub async fn upload(&self, request: UploadRequest) -> Result<()> {
        match self.config.upload {
            UploadConfig::DryRun => {
                tracing::info!(
                    object = tracing::field::display(&request.object_path),
                    "dry-run: skipping upload"
                );
                return Ok(());
            }
            _ => {}
        }

        let mut delay = Duration::from_millis(BASE_DELAY_MS);
        for attempt in 0..MAX_ATTEMPTS {
            match self.try_upload(&request).await {
                Ok(_) => return Ok(()),
                Err(err) => {
                    let attempts_left = MAX_ATTEMPTS - attempt - 1;
                    if err.retryable && attempts_left > 0 {
                        tracing::warn!(
                            error = %err.error,
                            attempt = attempt + 1,
                            "upload failed, retrying"
                        );
                        sleep(delay).await;
                        delay = std::cmp::min(delay * 2, Duration::from_millis(MAX_DELAY_MS));
                        continue;
                    } else {
                        return Err(err.error);
                    }
                }
            }
        }
        unreachable!("retry loop should return before exhausting attempts");
    }

    pub async fn upload_spool_entry(&self, entry: &SpoolEntry) -> Result<()> {
        let request = UploadRequest::from_entry(entry);
        self.upload(request).await
    }

    async fn try_upload(&self, request: &UploadRequest) -> std::result::Result<(), AttemptError> {
        let object_path = sanitize_object_path(&request.object_path);
        let (method, url) = match &self.config.upload {
            UploadConfig::DryRun => unreachable!(),
            UploadConfig::Supabase { base_url, .. } => {
                let url = format!(
                    "{}/storage/v1/object/{}/{}",
                    base_url.trim_end_matches('/'),
                    self.config.bucket,
                    object_path
                );
                (Method::POST, url)
            }
            UploadConfig::Presigned { base_url } => {
                let url = format!("{}/{}", base_url.trim_end_matches('/'), object_path);
                (Method::PUT, url)
            }
        };

        let metadata = fs::metadata(&request.local_path)
            .await
            .map_err(|err| AttemptError::fatal(err))?;
        let len = metadata.len();
        let file = File::open(&request.local_path)
            .await
            .map_err(|err| AttemptError::fatal(err))?;
        let stream = ReaderStream::new(file);
        let body = reqwest::Body::wrap_stream(stream);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&len.to_string()).map_err(AttemptError::fatal)?,
        );
        if let Some(content_type) = &request.content_type {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(content_type).map_err(AttemptError::fatal)?,
            );
        }
        if let Some(encoding) = &request.content_encoding {
            headers.insert(
                header::CONTENT_ENCODING,
                HeaderValue::from_str(encoding).map_err(AttemptError::fatal)?,
            );
        }

        let mut req = self.client.request(method, url).headers(headers).body(body);

        if let UploadConfig::Supabase { api_key, .. } = &self.config.upload {
            req = req
                .header(header::AUTHORIZATION, format!("Bearer {api_key}"))
                .header("x-upsert", "true");
        }

        let response = req.send().await;
        let response = match response {
            Ok(resp) => resp,
            Err(err) => {
                if err.is_timeout() || err.is_connect() || err.is_request() {
                    return Err(AttemptError::retryable(err));
                }
                return Err(AttemptError::fatal(err));
            }
        };

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unavailable>".to_string());
        let err = anyhow::anyhow!(
            "upload failed with status {} for {}: {}",
            status,
            request.object_path,
            text
        );

        if should_retry_status(status) {
            Err(AttemptError::retryable(err))
        } else {
            Err(AttemptError::fatal(err))
        }
    }
}

fn sanitize_object_path(path: &str) -> String {
    path.trim_start_matches('/').to_string()
}

fn should_retry_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
        || status.is_server_error()
}

impl UploadRequest {
    pub fn from_entry(entry: &SpoolEntry) -> Self {
        Self {
            object_path: entry.metadata.remote_path.clone(),
            local_path: entry.data_path.clone(),
            content_type: entry.metadata.content_type.clone(),
            content_encoding: entry.metadata.content_encoding.clone(),
        }
    }
}
