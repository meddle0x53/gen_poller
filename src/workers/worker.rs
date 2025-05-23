use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

use crate::workers::events::Hub;

#[derive(Debug)]
pub enum WorkerEnvelope<M> {
    Control(ControlMessage),
    Custom(M),
}

#[derive(Debug, Clone)]
pub enum ControlMessage {
    Shutdown,
    // later: Pause, Resume, Restart, etc.
}

pub struct WorkerHandle<Message> {
    pub handle: JoinHandle<()>,
    pub sender: Sender<WorkerEnvelope<Message>>,
}

#[async_trait]
pub trait Worker: Send + Sync + 'static {
    type Message: Send + 'static;

    /// Build from config
    fn build_from_config(name: &str, config: &HashMap<String, serde_json::Value>) -> Self;

    /// Called when a user-defined message arrives
    async fn handle_message(&mut self, msg: Self::Message, hub: Arc<Hub>);

    async fn on_start(&mut self, _hub: Arc<Hub>) {}

    async fn shutdown(&mut self) {
        tracing::warn!("Worker shutting down (default behavior).");
    }

    async fn handle_control_message(&mut self, control: ControlMessage) {
        match control {
            ControlMessage::Shutdown => {
                self.shutdown().await;
            }
        }
    }

    /// Async periodic task hook (optional, default is no-op)
    async fn poll(&mut self, hub: Arc<Hub>, config: &HashMap<String, serde_json::Value>);

    /// Returns optional duration for polling interval
    fn polling_interval(&self) -> Option<std::time::Duration> {
        None
    }

    async fn run(
        &mut self,
        hub: Arc<Hub>,
        mut rx: Receiver<WorkerEnvelope<Self::Message>>,
        config: HashMap<String, serde_json::Value>,
    ) {
        let mut ticker = if let Some(interval) = self.polling_interval() {
            Some(tokio::time::interval(interval))
        } else {
            None
        };

        self.on_start(hub.clone()).await;

        loop {
            tokio::select! {
                biased;

                // Periodic work
                _ = async {
                    if let Some(t) = &mut ticker {
                        t.tick().await;
                        true
                    } else {
                        tokio::time::sleep(std::time::Duration::MAX).await;
                        false
                    }
                }, if ticker.is_some() => {
                    self.poll(hub.clone(), &config).await;
                }

                msg = rx.recv() => {
                    match msg {
                        Some(WorkerEnvelope::Control(ctrl_msg)) => {
                            self.handle_control_message(ctrl_msg).await;
                            if let ControlMessage::Shutdown = ctrl_msg {
                                break;
                            }
                        }
                        Some(WorkerEnvelope::Custom(custom_msg)) => {
                            self.handle_message(custom_msg, hub.clone()).await;
                        }
                        None => {
                            tracing::warn!("Worker channel closed, exiting.");
                            break;
                        }
                    }
                }
                // Periodic timers or other worker-specific logic can be added here too
            }
        }

        tracing::warn!("Worker run loop exited.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub struct DummyWorker {
        pub name: String,
        pub hub: Arc<Hub>,
        pub state: Arc<WorkerState>,
        pub interval: Option<std::time::Duration>,
    }

    #[derive(Default)]
    struct WorkerState {
        pub message_count: AtomicUsize,
        pub shutdown_called: AtomicUsize,
        pub poll_count: AtomicUsize,
    }

    #[async_trait]
    impl Worker for DummyWorker {
        type Message = String;

        fn build_from_config(name: &str, config: &HashMap<String, serde_json::Value>) -> Self {
            let interval = config
                .get("interval")
                .and_then(|val| val.as_str().and_then(|s| humantime::parse_duration(s).ok()));

            DummyWorker {
                name: name.to_string(),
                hub: Arc::new(Hub::new(32)),
                state: Arc::new(WorkerState::default()),
                interval,
            }
        }

        fn polling_interval(&self) -> Option<tokio::time::Duration> {
            self.interval
        }

        async fn poll(&mut self, _hub: Arc<Hub>, config: &HashMap<String, serde_json::Value>) {
            self.state.poll_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn handle_message(&mut self, _msg: Self::Message, _hub: Arc<Hub>) {
            self.state.message_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn shutdown(&mut self) {
            self.state.shutdown_called.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_worker_handles_custom_messages() {
        let worker = DummyWorker::build_from_config("test_worker", &HashMap::new());

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let hub = Arc::new(Hub::new(32));

        let worker_state = worker.state.clone();
        let hub = worker.hub.clone();

        let worker_task = tokio::spawn(async move {
            let mut worker = worker; // rebind mutable
            worker.run(hub, rx, HashMap::new()).await;
        });

        // Send some messages
        tx.send(WorkerEnvelope::Custom("msg1".to_string()))
            .await
            .unwrap();
        tx.send(WorkerEnvelope::Custom("msg2".to_string()))
            .await
            .unwrap();
        tx.send(WorkerEnvelope::Control(ControlMessage::Shutdown))
            .await
            .unwrap();

        worker_task.await.unwrap();

        assert_eq!(worker_state.message_count.load(Ordering::SeqCst), 2);
        assert_eq!(worker_state.shutdown_called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_worker_shutdown_only() {
        let worker_state = Arc::new(WorkerState {
            message_count: AtomicUsize::new(0),
            shutdown_called: AtomicUsize::new(0),
            poll_count: AtomicUsize::new(0),
        });

        let hub = Arc::new(Hub::new(32));
        let mut worker = DummyWorker {
            interval: None,
            name: "shutdown_only".to_string(),
            hub: hub.clone(),
            state: worker_state.clone(),
        };

        let (tx, rx) = tokio::sync::mpsc::channel(32);

        let worker_task = tokio::spawn(async move {
            worker.run(hub, rx, HashMap::new()).await;
        });

        tx.send(WorkerEnvelope::Control(ControlMessage::Shutdown))
            .await
            .unwrap();

        worker_task.await.unwrap();

        assert_eq!(worker_state.message_count.load(Ordering::SeqCst), 0);
        assert_eq!(worker_state.shutdown_called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_worker_multiple_shutdowns() {
        let worker_state = Arc::new(WorkerState {
            message_count: AtomicUsize::new(0),
            shutdown_called: AtomicUsize::new(0),
            poll_count: AtomicUsize::new(0),
        });

        let hub = Arc::new(Hub::new(32));
        let mut worker = DummyWorker {
            interval: None,
            name: "multiple_shutdowns".to_string(),
            hub: hub.clone(),
            state: worker_state.clone(),
        };

        let (tx, rx) = tokio::sync::mpsc::channel(32);

        let worker_task = tokio::spawn(async move {
            worker.run(hub, rx, HashMap::new()).await;
        });

        // Send shutdown twice
        tx.send(WorkerEnvelope::Control(ControlMessage::Shutdown))
            .await
            .unwrap();
        tx.send(WorkerEnvelope::Control(ControlMessage::Shutdown))
            .await
            .unwrap();

        worker_task.await.unwrap();

        assert_eq!(worker_state.shutdown_called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_worker_handles_no_messages() {
        let worker_state = Arc::new(WorkerState {
            message_count: AtomicUsize::new(0),
            shutdown_called: AtomicUsize::new(0),
            poll_count: AtomicUsize::new(0),
        });

        let hub = Arc::new(Hub::new(32));
        let mut worker = DummyWorker {
            interval: None,
            name: "no_messages".to_string(),
            hub: hub.clone(),
            state: worker_state.clone(),
        };

        let (_tx, rx) = tokio::sync::mpsc::channel(32);
        drop(_tx); // close the channel

        let worker_task = tokio::spawn(async move {
            worker.run(hub, rx, HashMap::new()).await;
        });

        worker_task.await.unwrap();

        // nothing was received, so these should be zero
        assert_eq!(worker_state.message_count.load(Ordering::SeqCst), 0);
        assert_eq!(worker_state.shutdown_called.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_worker_poll_triggered() {
        let mut config = HashMap::new();
        config.insert(
            "interval".to_string(),
            serde_json::Value::String("100ms".into()),
        );

        let worker = DummyWorker::build_from_config("polling_worker", &config);
        let worker_state = worker.state.clone();
        let hub = worker.hub.clone();

        let (_tx, rx) = tokio::sync::mpsc::channel(32);

        let handle = tokio::spawn(async move {
            let mut w = worker;
            tokio::select! {
                _ = w.run(hub, rx, config) => {},
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(350)) => {}, // run for 3 ticks
            }
        });

        handle.await.unwrap();

        assert!(worker_state.poll_count.load(Ordering::SeqCst) >= 3);
    }
}
