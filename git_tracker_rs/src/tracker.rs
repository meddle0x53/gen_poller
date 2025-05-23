use crate::config::{PartialRepoConfig, RepoConfig};
use crate::events::BranchEvent;
use crate::events::PubSubHub;
use crate::state::{BranchInfo, RepoState};

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc::Receiver;
use tokio::time::interval;

use anyhow::{Context, Result};
use chrono::Utc;
use git2::{BranchType, RemoteCallbacks, Repository};

use tracing::instrument;

#[derive(Debug)]
pub enum TrackerMessage {
    CheckNow,
    UpdateConfigPartial(PartialRepoConfig),
    Shutdown,
}

fn fetch_origin(repo: &Repository) -> Result<()> {
    let mut remote = repo
        .find_remote("origin")
        .or_else(|_| repo.remote_anonymous("origin"))?;

    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|_url, _username, _allowed| git2::Cred::default());

    let mut options = git2::FetchOptions::new();
    options.remote_callbacks(callbacks);

    remote.fetch(
        &["refs/heads/*:refs/remotes/origin/*"],
        Some(&mut options),
        None,
    )?;
    Ok(())
}

fn list_branches(repo: &Repository) -> Result<HashMap<String, String>, anyhow::Error> {
    let mut result = HashMap::new();

    for branch in repo.branches(Some(BranchType::Remote))? {
        let (branch, _) = branch?;
        if let Some(name) = branch.name()? {
            if let Some(target) = branch.get().target() {
                result.insert(name.to_string(), target.to_string());
            }
        }
    }

    Ok(result)
}

/// Clone or open the repo, fetch, and return a list of branch names
pub async fn fetch_and_list_branches(
    config: &RepoConfig,
) -> anyhow::Result<Vec<(String, BranchInfo)>> {
    let local_path = format!("repos/{}", &config.name);

    if let Some(token) = &config.github_token {
        tracing::debug!("Configured a token : {}. To be implemented", token)
    }

    if !Path::new(&local_path).exists() {
        tracing::info!("Cloning repo '{}' into {:?}", config.url, config.name);
        tokio::task::spawn_blocking({
            let config = config.clone();

            move || {
                Repository::clone(&config.url, &local_path)?;
                Ok::<_, anyhow::Error>(())
            }
        })
        .await??;
    }

    let repo_url = config.url.clone();
    let local_name = config.name.clone();
    let branches = tokio::task::spawn_blocking({
        let local_path = format!("repos/{}", &config.name);

        move || {
            let repo = Repository::open(&local_path)?;
            fetch_origin(&repo)?;
            let current = list_branches(&repo)?;

            let mut state = RepoState::load(&local_name)?;
            let mut new_branches = vec![];

            //let new_branches: Vec<_> = current.difference(known).cloned().collect();
            for (name, hash) in &current {
                if !state.branches.contains_key(name) {
                    let info = BranchInfo {
                        created_at: Utc::now(),
                        git_hash: hash.clone(),
                        url: repo_url.clone()
                    };
                    state.branches.insert(name.clone(), info.clone());
                    new_branches.push((name.clone(), info));
                }
            }

            if !new_branches.is_empty() {
                state.save(&local_name)?;
            }

            Ok::<Vec<(std::string::String, BranchInfo)>, anyhow::Error>(new_branches)
        }
    })
    .await??;

    Ok(branches)
}

pub async fn run_repo_tracker(config: RepoConfig, hub: Arc<PubSubHub>) -> anyhow::Result<()> {
    let new_branches = fetch_and_list_branches(&config)
        .await
        .context("Failed to fetch branches")?;

    if !new_branches.is_empty() {
        tracing::info!("[{}] New branches: {:?}", config.name, new_branches);
        // TODO
        for (branch_name, branch_info) in new_branches {
            hub.publish(BranchEvent {
                repo: config.name.clone(),
                branch: branch_name.clone(),
                info: branch_info.clone(),
            });
        }
    } else {
        tracing::debug!("No new Branches for '{}'.", config.name);
    }

    Ok(())
}

#[instrument(skip(config))]
pub async fn tracker_loop(
    mut config: RepoConfig,
    mut rx: Receiver<TrackerMessage>,
    hub: Arc<PubSubHub>,
) {
    tracing::info!("Tracker started for '{}'", config.name);

    let mut ticker = interval(config.interval);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                tracing::info!("[{}] Running scheduled check", config.name);
                if let Err(e) = run_repo_tracker(config.clone(), hub.clone()).await {
                    tracing::error!("[{}] Tracker error: {:?}", config.name, e);
                }
            }

            msg = rx.recv() => match msg {
                Some(TrackerMessage::CheckNow) => {
                    tracing::info!("[{}] Received CheckNow", config.name);
                    let _ = run_repo_tracker(config.clone(), hub.clone()).await;
                }

                Some(TrackerMessage::UpdateConfigPartial(update)) => {
                    tracing::info!("[{}] Updating config fields", config.name);

                    if let Some(new_interval) = update.interval {
                        ticker = interval(new_interval);
                        config.interval = new_interval;
                    }

                    if let Some(Some(new_token)) = update.github_token {
                        config.github_token = Some(new_token);
                    } else if let Some(None) = update.github_token {
                        config.github_token = None;
                    }

                    if let Some(new_url) = update.url {
                        config.url = new_url;
                    }
                }

                Some(TrackerMessage::Shutdown) => {
                    tracing::info!("[{}] Received Shutdown", config.name);
                    break;
                }

                None => {
                    tracing::info!("[{}] Channel closed — shutting down", config.name);
                    break;
                }
            }
        }
    }

    tracing::warn!("[{}] Tracker exited", config.name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::PubSubHub;
    use crate::state::RepoState;
    use git2::{Repository, Signature};
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn setup_bare_origin_with_branch(name: &str) -> PathBuf {
        let origin_path = Path::new("test-origins").join(name);
        std::fs::create_dir_all(&origin_path).unwrap();
        let _origin = Repository::init_bare(&origin_path).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let work = Repository::init(tmp.path()).unwrap();

        let sig = Signature::now("Test", "test@example.com").unwrap();
        let tree_id = {
            let mut index = work.index().unwrap();
            index.write_tree().unwrap()
        };
        let tree = work.find_tree(tree_id).unwrap();

        let commit = work
            .commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
            .unwrap();
        let commit = work.find_commit(commit).unwrap();

        work.branch("feature/test-branch", &commit, false).unwrap();

        // Push the branch to the bare origin
        work.remote("origin", origin_path.to_str().unwrap())
            .unwrap();
        let mut remote = work.find_remote("origin").unwrap();

        remote
            .push(&["refs/heads/master:refs/heads/master"], None)
            .unwrap();
        remote
            .push(
                &["refs/heads/feature/test-branch:refs/heads/feature/test-branch"],
                None,
            )
            .unwrap();

        origin_path
    }

    fn create_local_clone_from_origin(name: &str, origin_path: &Path) -> RepoConfig {
        let local_path = Path::new("repos").join(name);
        if local_path.exists() {
            std::fs::remove_dir_all(&local_path).unwrap();
        }

        let repo = Repository::init(&local_path).unwrap();
        repo.remote("origin", origin_path.to_str().unwrap())
            .unwrap();

        RepoConfig {
            name: name.to_string(),
            url: origin_path.to_str().unwrap().to_string(), // unused due to pre-existing repo
            github_token: None,
            interval: Duration::from_secs(1),
        }
    }

    fn test_config(name: &str, url_path: String) -> RepoConfig {
        let file_url = format!("file://{}", url_path);
        RepoConfig {
            name: name.to_string(),
            url: file_url,
            github_token: None,
            interval: Duration::from_secs(2),
        }
    }

    fn clean_test_dirs(name: &str) {
        let _ = std::fs::remove_dir_all(format!("repos/{}", name));
        let _ = std::fs::remove_file(format!("state/{}.json", name));
        let _ = std::fs::remove_dir_all(format!("test-origins/{}", name));
        let _ = std::fs::remove_dir_all(format!("test-workdirs/{}", name));
    }

    #[tokio::test]
    async fn fetch_detects_new_branch_and_persists_state() {
        clean_test_dirs("testrepo");

        let origin = setup_bare_origin_with_branch("testrepo");
        let config = create_local_clone_from_origin("testrepo", &origin);

        let new_branches = fetch_and_list_branches(&config)
            .await
            .expect("fetch should succeed");

        assert_eq!(new_branches.len(), 2);
        assert!(
            ["origin/feature/test-branch", "origin/master"]
                .iter()
                .any(|&s| s == new_branches[0].0)
        );
        assert!(
            ["origin/feature/test-branch", "origin/master"]
                .iter()
                .any(|&s| s == new_branches[1].0)
        );

        let state = RepoState::load("testrepo").unwrap();
        assert!(state.branches.contains_key("origin/feature/test-branch"));

        clean_test_dirs("testrepo");
    }

    #[tokio::test]
    async fn tracker_loop_handles_checknow_and_shutdown() {
        clean_test_dirs("looprepo");

        let origin = setup_bare_origin_with_branch("looprepo");
        let config = create_local_clone_from_origin("looprepo", &origin);

        let config = test_config("looprepo", config.url);
        let (tx, rx) = mpsc::channel(4);
        let hub = Arc::new(PubSubHub::new(8));
        let mut sub = hub.subscribe();

        let task = tokio::spawn(tracker_loop(config.clone(), rx, hub.clone()));

        tx.send(TrackerMessage::CheckNow).await.unwrap();

        let msg = tokio::time::timeout(Duration::from_secs(2), sub.recv())
            .await
            .expect("Expected branch event")
            .expect("Receive failed");

        assert_eq!(msg.repo, "looprepo");
        assert!(
            ["origin/feature/test-branch", "origin/master"]
                .iter()
                .any(|&s| s == msg.branch)
        );

        let msg2 = tokio::time::timeout(Duration::from_secs(2), sub.recv())
            .await
            .expect("Expected branch event")
            .expect("Receive failed");

        assert_eq!(msg2.repo, "looprepo");
        assert!(
            ["origin/feature/test-branch", "origin/master"]
                .iter()
                .any(|&s| s == msg2.branch)
        );

        assert_ne!(msg.branch, msg2.branch);

        tx.send(TrackerMessage::Shutdown).await.unwrap();
        task.await.unwrap();

        clean_test_dirs("looprepo");
    }
}
