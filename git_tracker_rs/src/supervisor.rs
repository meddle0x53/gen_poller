use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Duration;

use crate::config::PartialRepoConfig;
use crate::config::RepoConfig;
use crate::events::PubSubHub;
use crate::tracker::{TrackerMessage, tracker_loop};

pub struct RepoHandle {
    pub config: RepoConfig,
    pub handle: JoinHandle<()>,
    pub sender: mpsc::Sender<TrackerMessage>,
}

pub struct Supervisor {
    trackers: HashMap<String, RepoHandle>,
    pub hub: Arc<PubSubHub>,
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            trackers: HashMap::new(),
            hub: Arc::new(PubSubHub::new(32)),
        }
    }

    pub async fn send_check_now(&self, name: &str) {
        if let Some(handle) = self.trackers.get(name) {
            let _ = handle.sender.send(TrackerMessage::CheckNow).await;
        }
    }

    pub async fn send_shutdown(&self, name: &str) {
        if let Some(handle) = self.trackers.get(name) {
            tracing::warn!("Sending shutdown to {}", name);
            let _ = handle.sender.send(TrackerMessage::Shutdown).await;
        }
    }

    pub async fn update_config(&self, config: PartialRepoConfig) {
        if let Some(handle) = self.trackers.get(&config.name) {
            let _ = handle
                .sender
                .send(TrackerMessage::UpdateConfigPartial(config))
                .await;
        }
    }

    pub async fn shutdown_all(&mut self) {
        for (name, handle) in &self.trackers {
            tracing::warn!("Sending shutdown to {}", name);
            let _ = handle.sender.send(TrackerMessage::Shutdown).await;
        }
    }

    pub async fn monitor(supervisor: Arc<Mutex<Supervisor>>, mut shutdown: watch::Receiver<()>) {
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    tracing::warn!("[monitor] Received shutdown signal. Exiting monitor.");
                    break;
                }

                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    let mut crashed = vec![];

                    let mut mut_sup = supervisor.lock().await;
                    for (name, handle) in &mut_sup.trackers {
                        if handle.handle.is_finished() {
                            tracing::warn!("Tracker '{}' has stopped. Scheduling restart.", name);
                            crashed.push(name.clone());
                        }
                    }

                    for name in crashed {
                        tracing::info!("About to restart {}.", name);
                        if let Some(old) = mut_sup.trackers.remove(&name) {
                            let _ = old.handle.await;

                            let tracker = Supervisor::build_tracker(old.config, mut_sup.hub.clone()).await;
                            mut_sup.insert_tracker(name, tracker)
                        }
                    }
                }
            }
        }
    }

    pub async fn wait_for_all(&mut self) {
        for (name, handle) in self.trackers.drain() {
            tracing::warn!("[{}] Waiting for task to finish...", name);
            let _ = handle.handle.await;
        }
    }

    pub async fn start_all(
        configs: Vec<RepoConfig>,
        shutdown_rx: watch::Receiver<()>,
    ) -> Arc<Mutex<Self>> {
        let supervisor = Arc::new(Mutex::new(Supervisor::new()));

        for config in configs {
            let name = config.name.clone();

            let tracker =
                Supervisor::build_tracker(config, supervisor.lock().await.hub.clone()).await;
            supervisor.lock().await.insert_tracker(name, tracker);
        }

        // Spawn monitor
        let supervisor_clone = supervisor.clone();
        tokio::spawn(async move {
            Supervisor::monitor(supervisor_clone, shutdown_rx).await;
        });

        supervisor
    }

    fn insert_tracker(&mut self, name: String, handle: RepoHandle) {
        self.trackers.insert(name, handle);
    }

    async fn build_tracker(config: RepoConfig, hub: Arc<PubSubHub>) -> RepoHandle {
        let (tx, rx) = mpsc::channel(32);
        let handle = tokio::spawn(tracker_loop(config.clone(), rx, hub));

        RepoHandle {
            config,
            sender: tx,
            handle,
        }
    }
}
