use crate::state::BranchInfo;

use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub struct BranchEvent {
    pub repo: String,
    pub branch: String,
    pub info: BranchInfo,
}

#[derive(Clone, Debug)]
pub struct PubSubHub {
    sender: broadcast::Sender<BranchEvent>,
}

impl PubSubHub {
    pub fn new(buffer: usize) -> Self {
        let (sender, _) = broadcast::channel(buffer);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BranchEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: BranchEvent) {
        let _ = self.sender.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::state::BranchInfo;
    use chrono::Utc;
    use tokio::time::{Duration, timeout};

    fn dummy_event(repo: &str, branch: &str, hash: &str) -> BranchEvent {
        BranchEvent {
            repo: repo.to_string(),
            branch: branch.to_string(),
            info: BranchInfo {
                created_at: Utc::now(),
                git_hash: hash.to_string(),
                url: "test".to_string()
            },
        }
    }

    #[tokio::test]
    async fn subscriber_receives_published_event() {
        let hub = PubSubHub::new(16);
        let mut rx = hub.subscribe();

        let event = dummy_event("repo1", "feature/x", "abc123");
        hub.publish(event.clone());

        let received = timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("Timed out waiting for event")
            .expect("Channel closed");

        assert_eq!(received.repo, "repo1");
        assert_eq!(received.branch, "feature/x");
        assert_eq!(received.info.git_hash, "abc123");
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_event() {
        let hub = PubSubHub::new(16);
        let mut rx1 = hub.subscribe();
        let mut rx2 = hub.subscribe();

        let event = dummy_event("repo2", "dev", "def456");
        hub.publish(event.clone());

        let ev1 = timeout(Duration::from_secs(1), rx1.recv())
            .await
            .unwrap()
            .unwrap();
        let ev2 = timeout(Duration::from_secs(1), rx2.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(ev1.repo, event.repo);
        assert_eq!(ev2.branch, event.branch);
    }

    #[tokio::test]
    async fn slow_subscriber_may_miss_messages() {
        let hub = PubSubHub::new(1); // Small buffer to force overwrite
        let mut rx = hub.subscribe();

        hub.publish(dummy_event("repo", "branch1", "h1"));
        hub.publish(dummy_event("repo", "branch2", "h2")); // overwrites previous

        match rx.recv().await {
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => (),
            Ok(_) => panic!("Expected Lagged error"),
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn subscribe_after_publish_does_not_receive_old() {
        let hub = PubSubHub::new(16);

        // Publish before subscribing
        hub.publish(dummy_event("repo", "branch-old", "abc"));

        // New subscriber won't get past messages
        let mut rx = hub.subscribe();

        let res = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(res.is_err(), "Subscriber should not receive old messages");
    }

    #[tokio::test]
    async fn receiving_from_closed_channel_returns_error() {
        let hub = PubSubHub::new(8);
        let mut rx = hub.subscribe();

        // drop the hub so the sender is dropped internally
        drop(hub);

        match rx.recv().await {
            Err(tokio::sync::broadcast::error::RecvError::Closed) => (),
            Ok(_) => panic!("Expected channel to be closed"),
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn dropping_receiver_unsubscribes_from_channel() {
        let hub = PubSubHub::new(8);

        let rx = hub.subscribe();
        drop(rx); // unsubscribe

        // There’s no direct observable effect, but this ensures no panic
        // and the hub still works for others
        let mut rx2 = hub.subscribe();
        let event = dummy_event("repo", "branch", "hash");

        hub.publish(event.clone());

        let received = tokio::time::timeout(Duration::from_secs(1), rx2.recv())
            .await
            .expect("Expected message")
            .expect("Receiver failed");

        assert_eq!(received.branch, "branch");
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashSet;
    use tokio::runtime::Runtime;

    proptest! {
        #[test]
        fn subscribers_receive_all_events_without_lag(event_count in 1..50usize, subscriber_count in 1..10usize) {
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                let hub = PubSubHub::new(event_count * 2); // large enough buffer

                let mut receivers = Vec::new();
                for _ in 0..subscriber_count {
                    receivers.push(hub.subscribe());
                }

                // Send N events
                for i in 0..event_count {
                    hub.publish(BranchEvent {
                        repo: format!("repo{}", i),
                        branch: format!("branch{}", i),
                        info: BranchInfo {
                            created_at: chrono::Utc::now(),
                            git_hash: format!("hash{}", i),
                            url: "test".to_string()
                        },
                    });
                }

                // Now verify all subscribers received all events
                for mut rx in receivers {
                    let mut seen = HashSet::new();
                    for _ in 0..event_count {
                        let ev = rx.recv().await.expect("Subscriber missed message");
                        seen.insert(ev.repo);
                    }
                    assert_eq!(seen.len(), event_count);
                }
            });
        }
    }
}
