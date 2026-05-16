use std::{
    env, io,
    path::{Path, PathBuf},
};

use crate::errors::{ApiError, ApiResult};

pub(crate) fn canonical_directory(input: &str) -> ApiResult<PathBuf> {
    let resolved = resolve_path_input(input)?;
    let canonical = match resolved.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ApiError::not_found("path", resolved.to_string_lossy()));
        }
        Err(error) => return Err(ApiError::bad_request(error.to_string())),
    };

    if !canonical.is_dir() {
        return Err(ApiError::bad_request_with_code(
            "fs.not_a_directory",
            "path must be an existing directory",
        ));
    }

    Ok(canonical)
}

fn resolve_path_input(input: &str) -> ApiResult<PathBuf> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("path must not be empty"));
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
            .map_err(|error| ApiError::internal(format!("failed to read current dir: {error}")))
    }
}

fn expand_home(input: &str) -> ApiResult<Option<PathBuf>> {
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
        return Err(ApiError::bad_request_with_code(
            "fs.unsupported_path",
            "only ~ or ~/... are supported",
        ));
    }

    Ok(None)
}

fn home_dir() -> ApiResult<PathBuf> {
    if let Some(path) = env::var_os("HOME").map(PathBuf::from) {
        return Ok(path);
    }

    if let Some(path) = env::var_os("USERPROFILE").map(PathBuf::from) {
        return Ok(path);
    }

    match (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
        (Some(drive), Some(path)) => Ok(Path::new(&drive).join(path)),
        _ => Err(ApiError::internal("failed to resolve home directory")),
    }
}
