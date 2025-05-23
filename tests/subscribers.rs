use gen_poller::subscribers::*;
use gen_poller::workers::events::{PollingEvent, Hub};

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::broadcast;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Deserialize, Serialize)]
struct TestPayload {
    message: String,
}

#[tokio::test]
async fn test_sync_subscriber_receives_and_parses_event() {
    let hub = Hub::new(16);
    let mut rx = hub.subscribe();

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);

    let handler = PollingHandler::Sync(Arc::new(move |_topic, _kind, payload: &TestPayload| {
        if payload.message == "ping" {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        }
    }));

    spawn_generic_subscriber::<TestPayload>(rx, "test", "kind", handler).await;

    let event = PollingEvent {
        topic: "test".into(),
        kind: "kind".into(),
        payload: json!({ "message": "ping" }),
    };

    hub.publish(event);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_async_subscriber_receives_event() {
    let hub = Hub::new(16);
    let mut rx = hub.subscribe();

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);

    let async_fn = move |_topic: &str, _kind: &str, _payload: &TestPayload| {
        let counter = Arc::clone(&counter_clone);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    };

    let handler = PollingHandler::Async(async_handler(async_fn));

    spawn_generic_subscriber::<TestPayload>(rx, "test", "kind", handler).await;

    let event = PollingEvent {
        topic: "test".into(),
        kind: "kind".into(),
        payload: json!({ "message": "ping" }),
    };

    hub.publish(event);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(counter.load(Ordering::SeqCst), 1);
}
