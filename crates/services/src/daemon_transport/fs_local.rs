use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use async_trait::async_trait;

use crate::daemon_transport::providers::FilesystemProvider;
use crate::{Result, ServiceError};

const SKIP_NAMES: &[&str] = &[
    ".Trashes",
    ".Spotlight-V100",
    ".fseventsd",
    "Library",
    "$RECYCLE.BIN",
    "System Volume Information",
    "AppData",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    ".git",
];

#[derive(Debug, Default)]
pub struct EmbeddedFilesystemProvider;

impl EmbeddedFilesystemProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl FilesystemProvider for EmbeddedFilesystemProvider {
    async fn list(&self, params: api_types::FsListParams) -> Result<api_types::FsListResult> {
        let path = canonical_directory(&params.path)?;
        let mut entries = Vec::new();

        for entry in fs::read_dir(&path).map_err(io_error)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if SKIP_NAMES.contains(&name.as_str()) {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            let entry_path = entry.path();
            let is_dir = file_type.is_dir();
            entries.push(api_types::FsEntry {
                name,
                path: canonical_or_absolute(&entry_path)
                    .to_string_lossy()
                    .into_owned(),
                is_dir,
                is_git_repo: is_dir && git::is_git_repo(&entry_path).await,
            });
        }

        entries.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| {
                    left.name
                        .to_ascii_lowercase()
                        .cmp(&right.name.to_ascii_lowercase())
                })
                .then_with(|| left.name.cmp(&right.name))
        });

        Ok(api_types::FsListResult {
            path: path.to_string_lossy().into_owned(),
            entries,
        })
    }

    async fn branches(
        &self,
        params: api_types::FsBranchesParams,
    ) -> Result<api_types::FsBranchesResult> {
        let path = canonical_directory(&params.path)?;
        if !git::is_git_repo(&path).await {
            return Err(ServiceError::invalid_operation(
                "path is not a git repository",
            ));
        }

        let branches = git::list_branches(&path)
            .await
            .map_err(|error| match error {
                git::GitError::CommandFailed { .. } => {
                    ServiceError::invalid_operation(format!("failed to list branches: {error}"))
                }
                other => {
                    ServiceError::invalid_operation(format!("failed to list branches: {other}"))
                }
            })?;

        Ok(api_types::FsBranchesResult {
            branches: branches.branches,
            default_branch: branches.default_branch,
            origin_url: branches.origin_url,
        })
    }
}

fn canonical_directory(input: &str) -> Result<PathBuf> {
    let resolved = resolve_path_input(input)?;
    let canonical = match resolved.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ServiceError::not_found(
                "path",
                resolved.to_string_lossy().into_owned(),
            ));
        }
        Err(error) => return Err(ServiceError::invalid_operation(error.to_string())),
    };

    if !canonical.is_absolute() {
        return Err(ServiceError::invalid_operation("path must be absolute"));
    }

    if !canonical.is_dir() {
        return Err(ServiceError::invalid_operation(
            "path must be an existing directory",
        ));
    }

    Ok(canonical)
}

fn resolve_path_input(input: &str) -> Result<PathBuf> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ServiceError::invalid_operation("path must not be empty"));
    }

    if let Some(path) = expand_home(trimmed)? {
        return Ok(path);
    }

    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        Ok(path)
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| {
                ServiceError::invalid_operation(format!("failed to read current dir: {error}"))
            })
    }
}

fn expand_home(input: &str) -> Result<Option<PathBuf>> {
    if input == "~" {
        return Ok(Some(home_dir()?));
    }

    if let Some(remainder) = input.strip_prefix("~/") {
        return Ok(Some(home_dir()?.join(remainder)));
    }

    if let Some(remainder) = input.strip_prefix("~\\") {
        return Ok(Some(home_dir()?.join(remainder)));
    }

    if input.starts_with('~') {
        return Err(ServiceError::invalid_operation(
            "only ~ or ~/... are supported",
        ));
    }

    Ok(None)
}

fn home_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("HOME").map(PathBuf::from) {
        return Ok(path);
    }

    if let Some(path) = env::var_os("USERPROFILE").map(PathBuf::from) {
        return Ok(path);
    }

    match (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
        (Some(drive), Some(path)) => Ok(Path::new(&drive).join(path)),
        _ => Err(ServiceError::invalid_operation(
            "failed to resolve home directory",
        )),
    }
}

fn canonical_or_absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn io_error(error: io::Error) -> ServiceError {
    ServiceError::invalid_operation(error.to_string())
}
