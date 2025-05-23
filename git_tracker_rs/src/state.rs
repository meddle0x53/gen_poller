use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchInfo {
    pub created_at: DateTime<Utc>,
    pub git_hash: String,
    pub url: String
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoState {
    pub branches: HashMap<String, BranchInfo>,
}

impl RepoState {
    pub fn new() -> Self {
        Self {
            branches: HashMap::new(),
        }
    }

    pub fn load(repo_name: &str) -> anyhow::Result<Self> {
        let path = format!("state/{}.json", repo_name);

        if std::path::Path::new(&path).exists() {
            let data = std::fs::read_to_string(path)?;

            Ok(serde_json::from_str(&data)?)
        } else {
            Ok(Self::new())
        }
    }

    pub fn save(&self, repo_name: &str) -> anyhow::Result<()> {
        let path = format!("state/{}.json", repo_name);
        let data = serde_json::to_string_pretty(self)?;

        std::fs::create_dir_all("state")?;
        std::fs::write(path, data)?;

        Ok(())
    }
}
