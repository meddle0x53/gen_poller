use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct PollingEvent {
    pub topic: String, // or repo name, etc.
    pub kind: String,  // or branch name, etc.
    pub payload: serde_json::Value, // or Box<dyn Any> if needed
}

#[derive(Clone, Debug)]
pub struct Hub {
    sender: broadcast::Sender<PollingEvent>,
}

impl Hub {
    pub fn new(buffer: usize) -> Self {
        let (sender, _) = broadcast::channel(buffer);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PollingEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: PollingEvent) {
        let _ = self.sender.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn test_publish_and_subscribe() {
        let hub = Hub::new(16);
        let mut rx = hub.subscribe();

        let event = PollingEvent {
            topic: "test_topic".into(),
            kind: "test_kind".into(),
            payload: json!({"key": "value"}),
        };

        hub.publish(event.clone());

        let received = timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("timed out")
            .expect("receive failed");

        assert_eq!(received.topic, event.topic);
        assert_eq!(received.kind, event.kind);
        assert_eq!(received.payload, event.payload);
    }

    #[tokio::test]
    async fn test_multiple_subscribers_receive_event() {
        let hub = Hub::new(16);
        let mut rx1 = hub.subscribe();
        let mut rx2 = hub.subscribe();

        let event = PollingEvent {
            topic: "multi".into(),
            kind: "broadcast".into(),
            payload: json!({"multi": true}),
        };

        hub.publish(event.clone());

        let e1 = rx1.recv().await.expect("rx1 failed");
        let e2 = rx2.recv().await.expect("rx2 failed");

        assert_eq!(e1.topic, "multi");
        assert_eq!(e2.topic, "multi");
    }
}
