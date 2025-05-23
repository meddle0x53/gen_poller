use std::sync::Arc;

use futures::future::BoxFuture;
use regex::Regex;

use crate::workers::events::PollingEvent;


pub type PollingHandlerFn<T> = Arc<dyn Fn(&str, &str, &T) + Send + Sync>;
pub type PollingHandlerFutureFn<T> = Arc<dyn Fn(&str, &str, &T) -> BoxFuture<'static, ()> + Send + Sync>;

pub enum PollingHandler<T> {
    Sync(PollingHandlerFn<T>),
    Async(PollingHandlerFutureFn<T>),
}

pub fn async_handler<T, F, Fut>(handler: F) -> PollingHandlerFutureFn<T>
where
    T: Send + Sync + 'static,
    F: Fn(&str, &str, &T) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    Arc::new(move |topic, kind, payload| Box::pin(handler(topic, kind, payload)))
}

pub async fn spawn_generic_subscriber<T>(
    mut rx: tokio::sync::broadcast::Receiver<PollingEvent>,
    topic_pattern: &str,
    kind_pattern: &str,
    handler: PollingHandler<T>,
)
where
    T: for<'de> serde::Deserialize<'de> + Send + Sync + 'static,
{
    let topic_re = Regex::new(topic_pattern).expect("Invalid topic regex");
    let kind_re = Regex::new(kind_pattern).expect("Invalid kind regex");

    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if topic_re.is_match(&event.topic) && kind_re.is_match(&event.kind) {
                if let Ok(payload) = serde_json::from_value::<T>(event.payload.clone()) {
                    match &handler {
                        PollingHandler::Sync(f) => f(&event.topic, &event.kind, &payload),
                        PollingHandler::Async(f) => f(&event.topic, &event.kind, &payload).await,
                    }
                } else {
                    tracing::warn!("Failed to deserialize payload for event: {:?}", event);
                }
            }
        }
    });
}

