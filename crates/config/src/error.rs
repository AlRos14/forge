use std::{fmt, io, path::PathBuf};

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    InvalidEnv {
        key: &'static str,
        value: String,
        message: String,
    },
    InvalidConfig {
        message: String,
    },
    Write {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "failed to read config {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "failed to parse config {}: {source}", path.display())
            }
            Self::InvalidEnv {
                key,
                value,
                message,
            } => {
                write!(f, "invalid environment variable {key}={value:?}: {message}")
            }
            Self::InvalidConfig { message } => write!(f, "invalid config: {message}"),
            Self::Write { path, source } => {
                write!(f, "failed to write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::InvalidEnv { .. } | Self::InvalidConfig { .. } => None,
            Self::Write { source, .. } => Some(source),
        }
    }
}
