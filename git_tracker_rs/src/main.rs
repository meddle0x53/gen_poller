mod cli;
mod config;
mod events;
mod state;
mod subscribers;
mod supervisor;
mod tracker;
mod runner;

use crate::subscribers::BranchHandler;
use crate::subscribers::BranchHandlerFn;

use std::sync::Arc;

use colored::*;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

fn init_logging() -> WorkerGuard {
    let file_appender = rolling::daily("logs", "git_tracker.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = fmt::layer().with_writer(file_writer);
    let stdout_layer = fmt::layer(); // defaults to stdout

    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(stdout_layer)
        .with(file_layer)
        .init();

    guard
}

#[tokio::main]
async fn main() {
    let _guard = init_logging();

    let handler : BranchHandlerFn = Arc::new(|repo, branch, info| {
        println!(
            "{}",
            format!(
                "[SUBSCRIBER] {}: new branch '{}' @ {}",
                repo, branch, info.git_hash
            )
            .bright_cyan()
        );
    });
    let runner = runner::start(vec![(".*", ".*", BranchHandler::Sync(handler))], true).await;

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await.unwrap();
    tracing::warn!("[main] Ctrl+C received, beginning shutdown");

    runner::stop(runner).await;
}
