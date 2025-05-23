use crate::events::PubSubHub;
use crate::state::BranchInfo;

use futures::future::BoxFuture;
use std::sync::Arc;

use colored::*;
use regex::Regex;

pub type BranchHandlerFn = Arc<dyn Fn(&str, &str, &BranchInfo) + Send + Sync>;
pub type BranchHandlerFutureFn = Arc<dyn Fn(&str, &str, &BranchInfo) -> BoxFuture<'static, ()> + Send + Sync>;

pub enum BranchHandler {
    Sync(BranchHandlerFn),
    Async(BranchHandlerFutureFn),
}

#[allow(dead_code)]
pub async fn log_branch_subscriber(
    hub: Arc<PubSubHub>,
    repo_pattern: &str,
    branch_pattern: &str,
    color: Color,
) {
    let mut rx = hub.subscribe();
    let repo_re = Regex::new(repo_pattern).unwrap();
    let branch_re = Regex::new(branch_pattern).unwrap();

    while let Ok(event) = rx.recv().await {
        if repo_re.is_match(&event.repo) && branch_re.is_match(&event.branch) {
            println!(
                "{}",
                format!(
                    "[MATCH] repo={} branch={} hash={}",
                    event.repo, event.branch, event.info.git_hash
                )
                .color(color)
            );
        }
    }
}

pub async fn spawn_handler_subscriber(
    hub: Arc<PubSubHub>,
    repo_pattern: &str,
    branch_pattern: &str,
    handler: BranchHandler,
) {
    let mut rx = hub.subscribe();
    let repo_re = Regex::new(repo_pattern).expect("Invalid repo regex");
    let branch_re = Regex::new(branch_pattern).expect("Invalid branch regex");

    tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if repo_re.is_match(&event.repo) && branch_re.is_match(&event.branch) {
                match &handler {
                    BranchHandler::Sync(f) => f(&event.repo, &event.branch, &event.info),
                    BranchHandler::Async(f) => {
                        f(&event.repo, &event.branch, &event.info).await;
                    }
                }
            }
        }
    });
}

pub fn async_handler<F, Fut>(handler: F) -> BranchHandlerFutureFn
where
    F: Fn(&str, &str, &BranchInfo) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    Arc::new(move |repo, branch, info| Box::pin(handler(repo, branch, info)))
}
