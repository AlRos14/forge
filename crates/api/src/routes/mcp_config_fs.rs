use std::{
    io,
    path::{Path, PathBuf},
};

use crate::errors::{ApiError, ApiResult};

/// A configuration path constrained to an allowed root.
///
/// The path is always `root.join(relative)`, and `relative` must contain only
/// normal components. Reads and writes never accept an absolute path or `..`.
#[derive(Debug, Clone)]
pub(crate) struct ConfigTarget {
    pub(crate) root: PathBuf,
    pub(crate) relative: PathBuf,
    pub(crate) path: PathBuf,
}

pub(crate) fn read_text(target: &ConfigTarget) -> ApiResult<Option<String>> {
    imp::read_text(target)
}

pub(crate) fn write_text(target: &ConfigTarget, contents: &[u8]) -> ApiResult<()> {
    imp::write_text(target, contents)
}

fn relative_components(relative: &Path) -> ApiResult<Vec<&std::ffi::OsStr>> {
    use std::path::Component;

    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) if !value.is_empty() => components.push(value),
            _ => {
                return Err(ApiError::forbidden_with_code(
                    "config_path_escape",
                    "MCP configuration path must stay inside its configured root",
                ));
            }
        }
    }
    if components.is_empty() {
        return Err(ApiError::forbidden_with_code(
            "config_path_escape",
            "MCP configuration path must name a file",
        ));
    }
    Ok(components)
}

fn canonical_root(root: &Path) -> ApiResult<PathBuf> {
    let canonical = match std::fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ApiError::bad_request_with_code(
                "config_root_unavailable",
                "MCP configuration root does not exist",
            ));
        }
        Err(error) => return Err(error.into()),
    };
    if !canonical.is_dir() {
        return Err(ApiError::bad_request_with_code(
            "config_root_unavailable",
            "MCP configuration root must be a directory",
        ));
    }
    Ok(canonical)
}

fn io_error(error: io::Error) -> ApiError {
    if error.kind() == io::ErrorKind::PermissionDenied {
        ApiError::forbidden_with_code(
            "config_path_symlink",
            "MCP configuration path must not traverse symlinks",
        )
    } else {
        error.into()
    }
}

#[cfg(unix)]
mod imp {
    use std::{
        ffi::OsStr,
        io::{self, Read, Write},
    };

    use rustix::{
        fs::{fchmod, mkdirat, open, openat, Mode, OFlags},
        io::Errno,
    };

    use super::{canonical_root, io_error, relative_components, ConfigTarget};
    use crate::errors::{ApiError, ApiResult};

    const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::CLOEXEC)
        .union(OFlags::NOFOLLOW);

    fn free(error: Errno) -> ApiError {
        match error {
            Errno::LOOP | Errno::NOTDIR => ApiError::forbidden_with_code(
                "config_path_symlink",
                "MCP configuration path must contain only real directories",
            ),
            _ => io_error(io::Error::from_raw_os_error(error.raw_os_error())),
        }
    }

    fn open_root(root: &std::path::Path) -> ApiResult<rustix::fd::OwnedFd> {
        open(root, DIRECTORY_FLAGS, Mode::empty()).map_err(free)
    }

    fn open_directory(
        parent: &rustix::fd::OwnedFd,
        name: &OsStr,
        create: bool,
    ) -> ApiResult<Option<rustix::fd::OwnedFd>> {
        if create {
            match mkdirat(parent, name, Mode::RWXU) {
                Ok(()) => {}
                Err(Errno::EXIST) => {}
                Err(error) => return Err(free(error)),
            }
        }
        match openat(parent, name, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(fd) => Ok(Some(fd)),
            Err(Errno::NOENT) if !create => Ok(None),
            Err(error) => Err(free(error)),
        }
    }

    pub(super) fn read_text(target: &ConfigTarget) -> ApiResult<Option<String>> {
        let root = canonical_root(&target.root)?;
        let components = relative_components(&target.relative)?;
        let (file_name, parents) = components
            .split_last()
            .expect("relative components are non-empty");
        let mut directory = open_root(&root)?;
        for name in parents {
            let Some(next) = open_directory(&directory, name, false)? else {
                return Ok(None);
            };
            directory = next;
        }

        let fd = match openat(
            &directory,
            *file_name,
            OFlags::RDONLY
                .union(OFlags::CLOEXEC)
                .union(OFlags::NOFOLLOW),
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return Err(free(error)),
        };
        let mut file = std::fs::File::from(fd);
        let mut contents = String::new();
        file.read_to_string(&mut contents).map_err(io_error)?;
        Ok(Some(contents))
    }

    pub(super) fn write_text(target: &ConfigTarget, contents: &[u8]) -> ApiResult<()> {
        let root = canonical_root(&target.root)?;
        let components = relative_components(&target.relative)?;
        let (file_name, parents) = components
            .split_last()
            .expect("relative components are non-empty");
        let mut directory = open_root(&root)?;
        for name in parents {
            let next = open_directory(&directory, name, true)?.ok_or_else(|| {
                ApiError::internal("MCP configuration directory disappeared during creation")
            })?;
            directory = next;
        }

        let fd = openat(
            &directory,
            *file_name,
            OFlags::WRONLY
                .union(OFlags::CREATE)
                .union(OFlags::TRUNC)
                .union(OFlags::CLOEXEC)
                .union(OFlags::NOFOLLOW),
            Mode::RUSR.union(Mode::WUSR),
        )
        .map_err(free)?;
        fchmod(&fd, Mode::RUSR.union(Mode::WUSR))
            .map_err(|error| io_error(io::Error::from_raw_os_error(error.raw_os_error())))?;
        let mut file = std::fs::File::from(fd);
        file.write_all(contents).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        Ok(())
    }
}

#[cfg(not(unix))]
mod imp {
    use std::{fs, io, path::PathBuf};

    use super::{canonical_root, io_error, relative_components, ConfigTarget};
    use crate::errors::{ApiError, ApiResult};

    fn checked_path(target: &ConfigTarget) -> ApiResult<(PathBuf, PathBuf)> {
        let root = canonical_root(&target.root)?;
        let components = relative_components(&target.relative)?;
        let mut path = root.clone();
        for component in components {
            path.push(component);
        }
        if !path.starts_with(&root) {
            return Err(ApiError::forbidden_with_code(
                "config_path_escape",
                "MCP configuration path must stay inside its configured root",
            ));
        }
        Ok((root, path))
    }

    fn reject_symlink_components(root: &std::path::Path, path: &std::path::Path) -> ApiResult<()> {
        let relative = path.strip_prefix(root).map_err(|_| {
            ApiError::forbidden_with_code(
                "config_path_escape",
                "MCP configuration path must stay inside its configured root",
            )
        })?;
        let mut current = root.to_path_buf();
        for component in relative.components() {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(ApiError::forbidden_with_code(
                        "config_path_symlink",
                        "MCP configuration path must not traverse symlinks",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(())
    }

    pub(super) fn read_text(target: &ConfigTarget) -> ApiResult<Option<String>> {
        let (root, path) = checked_path(target)?;
        reject_symlink_components(&root, &path)?;
        match fs::read_to_string(&path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }

    pub(super) fn write_text(target: &ConfigTarget, contents: &[u8]) -> ApiResult<()> {
        let (root, path) = checked_path(target)?;
        reject_symlink_components(&root, &path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        reject_symlink_components(&root, &path)?;
        fs::write(&path, contents).map_err(io_error)
    }
}
