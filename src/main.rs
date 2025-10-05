use agent_uploader::Result;
use agent_uploader::config::{Cli, Command, WatchArgs, WatchConfig};
use agent_uploader::{ui, watch};
use clap::Parser;
use std::sync::Arc;

fn init_tracing(verbose: bool) {
    if std::env::var("RUST_LOG").is_err() {
        let level = if verbose { "debug" } else { "info" };
        unsafe {
            std::env::set_var("RUST_LOG", format!("agent_uploader={level},info"));
        }
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .with_level(true)
        .try_init();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Watch(args) => run_watch(args).await,
        Command::Reload(_) => anyhow::bail!("reload subcommand not implemented yet"),
        Command::Replay(_) => anyhow::bail!("replay subcommand not implemented yet"),
        Command::Host(_) => anyhow::bail!("host subcommand not implemented yet"),
        Command::Version => {
            println!("agent-uploader {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

async fn run_watch(args: WatchArgs) -> Result<()> {
    let config = Arc::new(WatchConfig::from_args(args)?);
    init_tracing(config.verbose);
    tracing::info!(
        sid = tracing::field::display(&config.sid),
        "starting agent-uploader watch"
    );

    let ui_handle = ui::spawn(config.clone()).await?;

    let result = watch::run(config.clone()).await;

    if let Some(handle) = ui_handle {
        handle.shutdown().await;
    }

    match result {
        Ok(()) => {
            tracing::info!("agent-uploader exiting normally");
            Ok(())
        }
        Err(err) => {
            tracing::error!(error = %err, "agent-uploader watch terminated with error");
            Err(err)
        }
    }
}
