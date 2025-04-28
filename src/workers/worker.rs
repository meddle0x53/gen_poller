use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

use crate::workers::events::Hub;

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

    async fn run(&mut self, hub: Arc<Hub>, mut rx: Receiver<WorkerEnvelope<Self::Message>>) {
        loop {
            tokio::select! {
                Some(msg) = rx.recv() => {
                    match msg {
                        WorkerEnvelope::Control(ctrl_msg) => {
                            self.handle_control_message(ctrl_msg).await;
                            if let ControlMessage::Shutdown = ctrl_msg {
                                break;
                            }
                        }
                        WorkerEnvelope::Custom(custom_msg) => {
                            self.handle_message(custom_msg, hub.clone()).await;
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
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DummyWorker {
        pub name: String,
        pub hub: Arc<Hub>,
        pub state: Arc<WorkerState>,
    }

    struct WorkerState {
        pub message_count: AtomicUsize,
        pub shutdown_called: AtomicUsize,
    }

    #[async_trait]
    impl Worker for DummyWorker {
        type Message = String;

        fn build_from_config(name: &str, _config: &HashMap<String, serde_json::Value>) -> Self {
            DummyWorker {
                name: name.to_string(),
                hub: Arc::new(Hub::new(32)),
                state: Arc::new(WorkerState {
                    message_count: AtomicUsize::new(0),
                    shutdown_called: AtomicUsize::new(0),
                }),
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
    async fn test_worker_handles_custom_messages() {
        let mut worker = DummyWorker::build_from_config("test_worker", &HashMap::new());

        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let hub = Arc::new(Hub::new(32));

        let worker_state = worker.state.clone();
        let hub = worker.hub.clone();

        let worker_task = tokio::spawn(async move {
            let mut worker = worker; // rebind mutable
            worker.run(hub, rx).await;
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
        });

        let hub = Arc::new(Hub::new(32));
        let mut worker = DummyWorker {
            name: "shutdown_only".to_string(),
            hub: hub.clone(),
            state: worker_state.clone(),
        };

        let (tx, rx) = tokio::sync::mpsc::channel(32);

        let worker_task = tokio::spawn(async move {
            worker.run(hub, rx).await;
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
        });

        let hub = Arc::new(Hub::new(32));
        let mut worker = DummyWorker {
            name: "multiple_shutdowns".to_string(),
            hub: hub.clone(),
            state: worker_state.clone(),
        };

        let (tx, rx) = tokio::sync::mpsc::channel(32);

        let worker_task = tokio::spawn(async move {
            worker.run(hub, rx).await;
        });

        // Send shutdown twice
        tx.send(WorkerEnvelope::Control(ControlMessage::Shutdown))
            .await
            .unwrap();
        tx.send(WorkerEnvelope::Control(ControlMessage::Shutdown))
            .await
            .unwrap();

        println!("Hmm? {:?}", worker_task.is_finished());
        worker_task.await.unwrap();

        assert_eq!(worker_state.shutdown_called.load(Ordering::SeqCst), 1);
    }
}
