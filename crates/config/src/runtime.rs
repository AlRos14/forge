use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const SERVER_STATE_FILE: &str = "server.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerState {
    pub bind: String,
    pub server_url: String,
}

impl ServerState {
    #[must_use]
    pub fn new(bind: impl Into<String>, server_url: impl Into<String>) -> Self {
        Self {
            bind: bind.into(),
            server_url: server_url.into(),
        }
    }
}

#[must_use]
pub fn server_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SERVER_STATE_FILE)
}

pub fn read_server_state(data_dir: &Path) -> io::Result<Option<ServerState>> {
    let path = server_state_path(data_dir);
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)?;
    serde_json::from_str(&contents)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn write_server_state(data_dir: &Path, state: &ServerState) -> io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let path = server_state_path(data_dir);
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let contents = serde_json::to_vec_pretty(state)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&temp, contents)?;
    fs::rename(temp, path)
}
