use crate::manifest::{Manifest, ManifestStore};
use crate::segment::{PendingCheckpoint, SegmentClosed, SegmentWriter};
use crate::spool::{SpoolItemKind, SpoolLayout, SpoolMetadata, SpoolQueue};
use crate::tail::{TailBatch, TailReader};
use crate::upload::UploadClient;
use crate::util::ensure_dir;
use crate::{Result, WatchConfig};
use futures::stream::{self, StreamExt, TryStreamExt};
use std::path::Path;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::signal;

pub async fn run(config: Arc<WatchConfig>) -> Result<()> {
    let spool_layout = SpoolLayout::from_config(&config);
    spool_layout.ensure()?;
    ensure_dir(&config.manifest_state_dir)?;

    let manifest_state_path = config
        .manifest_state_dir
        .join(format!("{}.json", config.sid));
    let manifest_store = ManifestStore::new(manifest_state_path);
    let mut manifest = manifest_store.load_or_new(&config)?;

    let starting_seq = manifest.active_seq;
    let mut tail_reader = TailReader::new(config.session_file.clone()).await?;
    let mut segment_writer =
        SegmentWriter::new(config.clone(), spool_layout.clone(), starting_seq).await?;
    let spool_queue = Arc::new(SpoolQueue::new(spool_layout.clone()));
    let uploader = Arc::new(UploadClient::new(config.clone())?);
    let manifest_remote_path = Manifest::manifest_path(&config.object_prefix());
    let manifest_upload_path = spool_layout.queue_manifest_path();
    let concurrency = config.concurrency.max(1);

    if let Err(err) = drain_spool(spool_queue.clone(), uploader.clone(), concurrency).await {
        tracing::warn!(error = %err, "failed to drain existing spool entries at startup");
    }

    let mut interval = crate::tail::poll_interval(config.poll_interval);

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                tracing::info!("shutdown signal received");
                finalize(&mut segment_writer, &mut manifest, &manifest_store, &spool_queue, &manifest_upload_path, &manifest_remote_path, &uploader, concurrency).await?;
                break;
            }
            _ = interval.tick() => {
                if let Some(batch) = tail_reader.poll().await? {
                    handle_batch(
                        batch,
                        &mut segment_writer,
                        &mut manifest,
                        &manifest_store,
                        &spool_queue,
                        &manifest_upload_path,
                        &manifest_remote_path,
                        &uploader,
                        concurrency,
                    ).await?;
                }
            }
        }
    }

    Ok(())
}

async fn handle_batch(
    batch: TailBatch,
    segment_writer: &mut SegmentWriter,
    manifest: &mut Manifest,
    manifest_store: &ManifestStore,
    spool_queue: &Arc<SpoolQueue>,
    manifest_upload_path: &Path,
    manifest_remote_path: &str,
    uploader: &Arc<UploadClient>,
    concurrency: usize,
) -> Result<()> {
    if batch.truncated {
        if let Some(closed) = segment_writer.force_rotate().await? {
            finalize_segment(
                closed,
                manifest,
                manifest_store,
                spool_queue,
                manifest_upload_path,
                manifest_remote_path,
                uploader,
                concurrency,
            )
            .await?;
        }
    }

    for event in batch.events {
        if let Some(closed) = segment_writer.append(&event).await? {
            finalize_segment(
                closed,
                manifest,
                manifest_store,
                spool_queue,
                manifest_upload_path,
                manifest_remote_path,
                uploader,
                concurrency,
            )
            .await?;
        }
    }
    Ok(())
}

async fn finalize(
    segment_writer: &mut SegmentWriter,
    manifest: &mut Manifest,
    manifest_store: &ManifestStore,
    spool_queue: &Arc<SpoolQueue>,
    manifest_upload_path: &Path,
    manifest_remote_path: &str,
    uploader: &Arc<UploadClient>,
    concurrency: usize,
) -> Result<()> {
    if let Some(closed) = segment_writer.force_rotate().await? {
        finalize_segment(
            closed,
            manifest,
            manifest_store,
            spool_queue,
            manifest_upload_path,
            manifest_remote_path,
            uploader,
            concurrency,
        )
        .await?;
    } else {
        queue_manifest(
            manifest,
            manifest_store,
            spool_queue,
            manifest_upload_path,
            manifest_remote_path,
        )
        .await?;
    }

    if let Err(err) = drain_spool(spool_queue.clone(), uploader.clone(), concurrency).await {
        tracing::warn!(error = %err, "failed to upload all queued items during shutdown");
    }

    Ok(())
}

async fn finalize_segment(
    closed: SegmentClosed,
    manifest: &mut Manifest,
    manifest_store: &ManifestStore,
    spool_queue: &Arc<SpoolQueue>,
    manifest_upload_path: &Path,
    manifest_remote_path: &str,
    uploader: &Arc<UploadClient>,
    concurrency: usize,
) -> Result<()> {
    let checkpoint = closed.checkpoint.clone();
    manifest.add_segment(closed.entry.clone());
    if let Some(ref cp) = checkpoint {
        manifest.add_checkpoint(cp.manifest_entry());
    }

    let content_type = if closed.content_encoding.is_some() {
        "application/octet-stream"
    } else {
        "application/x-ndjson"
    };
    let segment_metadata = SpoolMetadata {
        remote_path: closed.upload_remote_path.clone(),
        content_type: Some(content_type.to_string()),
        content_encoding: closed.content_encoding.clone(),
        created_at: OffsetDateTime::now_utc(),
        kind: SpoolItemKind::Segment,
    };
    spool_queue
        .enqueue(&closed.upload_local_path, &segment_metadata)
        .await?;

    if let Some(cp) = checkpoint {
        queue_checkpoint(&cp, spool_queue).await?;
    }

    queue_manifest(
        manifest,
        manifest_store,
        spool_queue,
        manifest_upload_path,
        manifest_remote_path,
    )
    .await?;

    if let Err(err) = drain_spool(spool_queue.clone(), uploader.clone(), concurrency).await {
        tracing::warn!(error = %err, "upload failed; data will remain in spool");
    }
    Ok(())
}

async fn queue_manifest(
    manifest: &Manifest,
    manifest_store: &ManifestStore,
    spool_queue: &Arc<SpoolQueue>,
    manifest_upload_path: &Path,
    manifest_remote_path: &str,
) -> Result<()> {
    manifest_store.save(manifest)?;
    let bytes = manifest.to_bytes()?;
    tokio::fs::write(manifest_upload_path, &bytes).await?;
    let manifest_metadata = SpoolMetadata {
        remote_path: manifest_remote_path.to_string(),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        created_at: OffsetDateTime::now_utc(),
        kind: SpoolItemKind::Manifest,
    };
    spool_queue
        .enqueue(manifest_upload_path, &manifest_metadata)
        .await?;
    Ok(())
}

async fn queue_checkpoint(
    checkpoint: &PendingCheckpoint,
    spool_queue: &Arc<SpoolQueue>,
) -> Result<()> {
    if let Some(parent) = checkpoint.file_path.parent() {
        ensure_dir(parent)?;
    }
    let bytes = checkpoint.file_bytes()?;
    tokio::fs::write(&checkpoint.file_path, &bytes).await?;
    let metadata = SpoolMetadata {
        remote_path: checkpoint.remote_path.clone(),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        created_at: OffsetDateTime::now_utc(),
        kind: SpoolItemKind::Checkpoint,
    };
    spool_queue.enqueue(&checkpoint.file_path, &metadata).await
}

async fn drain_spool(
    spool_queue: Arc<SpoolQueue>,
    uploader: Arc<UploadClient>,
    concurrency: usize,
) -> Result<()> {
    let entries = spool_queue.list().await?;
    if entries.is_empty() {
        return Ok(());
    }
    stream::iter(entries.into_iter().map(|entry| {
        let queue = spool_queue.clone();
        let client = uploader.clone();
        async move {
            client.upload_spool_entry(&entry).await?;
            queue.mark_uploaded(&entry).await?;
            Ok::<_, anyhow::Error>(())
        }
    }))
    .buffer_unordered(concurrency)
    .try_collect::<Vec<_>>()
    .await?;
    Ok(())
}
