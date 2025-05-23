use crate::cli::control::start_control_interface;

use crate::config::load_repo_configs;
use crate::subscribers::BranchHandler;
use crate::supervisor::Supervisor;

use std::sync::Arc;

use tokio::sync::Mutex;

pub async fn start(handlers: Vec<(&str, &str, BranchHandler)>, run_control_api: bool) -> (Arc<Mutex<Supervisor>>, tokio::sync::watch::Sender<()>) {
    let configs = load_repo_configs("config/repos.json").expect("Failed to load config");

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(());
    let supervisor = Supervisor::start_all(configs, shutdown_rx).await;

    if run_control_api {
        tokio::spawn(start_control_interface(supervisor.clone()));
    }

    for (repo_pattern, branch_pattern, handler) in handlers {
        crate::subscribers::spawn_handler_subscriber(
            supervisor.lock().await.hub.clone(),
            repo_pattern,
            branch_pattern,
            handler
        ).await;
    }

    tracing::info!("[Git Tracker Runner :: start] Git Tracker Runner started.");

    (supervisor, shutdown_tx)
}

pub async fn stop(runner : (Arc<Mutex<Supervisor>>, tokio::sync::watch::Sender<()>)) {
    let (supervisor, shutdown_tx) = runner;

    // Trigger shutdown
    let _ = shutdown_tx.send(()); // stop monitor
    supervisor.lock().await.shutdown_all().await; // tell trackers to shut down
    supervisor.lock().await.wait_for_all().await; // wait for all to finish

    tracing::warn!("[Git Tracker Runner :: start] Git Tracker Runner stopped.");
}
