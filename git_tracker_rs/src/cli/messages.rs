use crate::config::PartialRepoConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum ControlCommand {
    CheckNow(String),
    Shutdown(String),
    ShutdownAll,
    UpdateConfigPartial(PartialRepoConfig),
}
