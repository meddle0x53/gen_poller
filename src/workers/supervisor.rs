use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Duration;

use crate::config::PartialRepoConfig;
use crate::config::RepoConfig;
use crate::workers::events::Hub;
use crate::workers::worker::{ControlMessage, Worker, WorkerEnvelope, WorkerHandle};

pub struct Supervisor<W>
where
    W: Worker,
{
    workers: HashMap<String, WorkerHandle<W::Message>>,
    configs: HashMap<String, HashMap<String, serde_json::Value>>,
    pub hub: Arc<Hub>,
}

impl<W> Supervisor<W>
where
    W: Worker,
{
    pub fn new() -> Self {
        Self {
            workers: HashMap::new(),
            configs: HashMap::new(),
            hub: Arc::new(Hub::new(32)),
        }
    }

    pub async fn send(&self, name: &str, msg: W::Message) {
        if let Some(handle) = self.workers.get(name) {
            let _ = handle.sender.send(WorkerEnvelope::Custom(msg)).await;
        }
    }

    pub async fn shutdown_all(&mut self) {
        for (name, handle) in &self.workers {
            tracing::warn!("Sending shutdown to {}", name);
            let _ = handle
                .sender
                .send(WorkerEnvelope::Control(ControlMessage::Shutdown))
                .await;
        }
    }

    pub async fn wait_for_all(&mut self) {
        for (name, handle) in self.workers.drain() {
            tracing::warn!("[{}] Waiting for task to finish...", name);
            let _ = handle.handle.await;
        }
    }

    pub async fn start_all(
        configs: HashMap<String, HashMap<String, serde_json::Value>>,
        shutdown_rx: watch::Receiver<()>,
    ) -> Arc<Mutex<Self>> {
        let supervisor = Arc::new(Mutex::new(Supervisor::new()));

        {
            let mut sup = supervisor.lock().await;
            for (name, config) in &configs {
                let worker = W::build_from_config(name, config);
                let handle = Supervisor::build_worker(worker, sup.hub.clone()).await;
                sup.insert_worker(name.clone(), handle);
            }
            sup.configs = configs;
        }

        let supervisor_clone = supervisor.clone();
        tokio::spawn(async move {
            Supervisor::monitor(supervisor_clone, shutdown_rx).await;
        });

        supervisor
    }

    async fn build_worker(mut worker: W, hub: Arc<Hub>) -> WorkerHandle<W::Message> {
        let (tx, rx) = mpsc::channel(32);
        let handle = tokio::spawn(async move {
            worker.run(hub, rx).await;
        });
        WorkerHandle { sender: tx, handle }
    }

    fn insert_worker(&mut self, name: String, handle: WorkerHandle<W::Message>) {
        self.workers.insert(name, handle);
    }

    async fn monitor(supervisor: Arc<Mutex<Self>>, mut shutdown: watch::Receiver<()>) {
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    tracing::warn!("[monitor] Received shutdown signal. Exiting monitor.");
                    break;
                }

                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    let mut crashed = vec![];

                    {
                        let sup = supervisor.lock().await;
                        for (name, handle) in &sup.workers {
                            if handle.handle.is_finished() {
                                tracing::warn!("Worker '{}' has stopped. Scheduling restart.", name);
                                crashed.push(name.clone());
                            }
                        }
                    }

                    for name in crashed {
                        let mut sup = supervisor.lock().await;
                        if let Some(old) = sup.workers.remove(&name) {
                            let _ = old.handle.await;
                        }

                        if let Some(config) = sup.configs.get(&name) {
                            let worker = W::build_from_config(&name, config);
                            let handle = Supervisor::build_worker(worker, sup.hub.clone()).await;
                            sup.insert_worker(name.clone(), handle);
                            tracing::info!("Worker '{}' restarted.", name);
                        } else {
                            tracing::error!("Missing config for crashed worker '{}'", name);
                        }
                    }
                }
            }
        }
    }
}
