use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::BufReader, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub name: String,
    pub url: String,
    pub github_token: Option<String>,

    #[serde(with = "humantime_serde")]
    pub interval: Duration, // e.g., "10s", "5m", etc.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialRepoConfig {
    pub name: String,

    #[serde(default)]
    pub interval: Option<Duration>,

    #[serde(default)]
    pub github_token: Option<Option<String>>, // double Option for clearing value

    #[serde(default)]
    pub url: Option<String>,
}

pub fn load_repo_configs(path: &str) -> Result<Vec<RepoConfig>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let configs = serde_json::from_reader(reader)?;

    Ok(configs)
}
