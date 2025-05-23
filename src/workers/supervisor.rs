use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time::Duration;

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
                let handle =
                    Supervisor::build_worker(worker, sup.hub.clone(), config.clone()).await;
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

    async fn build_worker(
        mut worker: W,
        hub: Arc<Hub>,
        config_map: HashMap<String, serde_json::Value>,
    ) -> WorkerHandle<W::Message> {
        let (tx, rx) = mpsc::channel(32);

        let config = config_map.clone();
        let handle = tokio::spawn(async move {
            worker.run(hub, rx, config).await;
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
                            old.handle.abort();
                            let _ = old.handle.await;
                        }

                        if let Some(config) = sup.configs.get(&name) {
                            let worker = W::build_from_config(&name, config);
                            let handle = Supervisor::build_worker(worker, sup.hub.clone(), config.clone()).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Clone)]
    pub struct DummyWorker {
        pub name: String,
        pub hub: Arc<Hub>,
        pub state: Arc<DummyWorkerState>,
        pub interval: Option<Duration>,
        pub crash_on_poll: bool,
        pub has_crashed_ptr: Option<Arc<AtomicBool>>,
    }

    #[derive(Default)]
    pub struct DummyWorkerState {
        pub poll_count: AtomicUsize,
        pub shutdown_called: AtomicUsize,
        pub message_count: AtomicUsize,
        pub build_count: AtomicUsize,
        pub has_crashed: AtomicBool,
    }

    #[async_trait]
    impl Worker for DummyWorker {
        type Message = String;

        fn build_from_config(name: &str, config: &HashMap<String, serde_json::Value>) -> Self {
            let interval = config
                .get("interval")
                .and_then(|v| v.as_str().and_then(|s| humantime::parse_duration(s).ok()));

            let crash_on_poll = config
                .get("crash_on_poll")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let has_crashed_ptr = config
                .get("crash_ptr")
                .and_then(|v| v.as_u64())
                .map(|raw| unsafe { Arc::from_raw(raw as *const AtomicBool) });

            DummyWorker {
                name: name.to_string(),
                hub: Arc::new(Hub::new(32)),
                state: Arc::new(DummyWorkerState {
                    poll_count: AtomicUsize::new(0),
                    shutdown_called: AtomicUsize::new(0),
                    message_count: AtomicUsize::new(0),
                    build_count: AtomicUsize::new(0),
                    has_crashed: AtomicBool::new(false),
                }),
                interval,
                has_crashed_ptr,
                crash_on_poll,
            }
        }

        fn polling_interval(&self) -> Option<Duration> {
            self.interval
        }

        async fn poll(&mut self, _hub: Arc<Hub>, _cfg: &HashMap<String, serde_json::Value>) {
            self.state.poll_count.fetch_add(1, Ordering::SeqCst);

            if self.crash_on_poll {
                if let Some(flag) = &self.has_crashed_ptr {
                    if !flag.swap(true, Ordering::SeqCst) {
                        panic!("Simulated crash");
                    }
                } else {
                    panic!("Simulated crash");
                }
            }
        }

        async fn handle_message(&mut self, _msg: Self::Message, _hub: Arc<Hub>) {
            self.state.message_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn shutdown(&mut self) {
            self.state.shutdown_called.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_supervisor_starts_worker_and_sends_message() {
        let mut config = HashMap::new();
        config.insert(
            "interval".to_string(),
            serde_json::Value::String("50ms".into()),
        );

        let mut configs = HashMap::new();
        configs.insert("worker1".to_string(), config.clone());

        let shutdown = watch::channel(()).1;
        let supervisor = Supervisor::<DummyWorker>::start_all(configs.clone(), shutdown).await;

        let sup = supervisor.lock().await;
        sup.send("worker1", "hello".to_string()).await;

        tokio::time::sleep(Duration::from_millis(100)).await;

        let handle = sup.workers.get("worker1").unwrap();
        assert!(!handle.handle.is_finished());
    }

    #[tokio::test]
    async fn test_supervisor_restarts_crashed_worker() {
        let crash_flag = Arc::new(AtomicBool::new(false));
        let ptr_val = Arc::as_ptr(&crash_flag) as usize;

        let cloned = Arc::clone(&crash_flag);
        std::mem::forget(cloned);

        let mut config = HashMap::new();
        config.insert("interval".into(), serde_json::Value::String("50ms".into()));
        config.insert("crash_on_poll".into(), serde_json::Value::Bool(true));
        config.insert(
            "crash_ptr".into(),
            serde_json::Value::Number(ptr_val.into()),
        );

        let mut configs = HashMap::new();
        configs.insert("crasher".to_string(), config.clone());

        let (_tx, shutdown_rx) = watch::channel(());

        let sup = Supervisor::<DummyWorker>::start_all(configs, shutdown_rx).await;

        // wait for crash + monitor cycle (interval + monitor delay)
        tokio::time::sleep(Duration::from_secs(12)).await;

        let guard = sup.lock().await;
        let worker = guard.workers.get("crasher").unwrap();

        assert!(!worker.handle.is_finished()); // it's restarted
    }
}
