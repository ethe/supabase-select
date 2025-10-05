pub mod config;
pub mod manifest;
pub mod segment;
pub mod spool;
pub mod tail;
pub mod ui;
pub mod upload;
pub mod util;
pub mod watch;

pub use config::{Cli, Command, HostArgs, ReloadArgs, ReplayArgs, WatchArgs, WatchConfig};

pub type Result<T> = anyhow::Result<T>;
