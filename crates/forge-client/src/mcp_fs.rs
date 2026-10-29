use std::{
    io,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};

/// A client configuration file constrained to a canonical root.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    pub root: PathBuf,
    pub relative: PathBuf,
}

impl ConfigFile {
    pub fn path(&self) -> PathBuf {
        self.root.join(&self.relative)
    }
}

pub fn read_text(file: &ConfigFile) -> Result<Option<String>> {
    imp::read_text(file)
}

pub fn write_text(file: &ConfigFile, contents: &[u8]) -> Result<()> {
    imp::write_text(file, contents)
}

fn relative_components(relative: &Path) -> Result<Vec<&std::ffi::OsStr>> {
    use std::path::Component;

    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) if !value.is_empty() => components.push(value),
            _ => {
                return Err(anyhow!(
                    "MCP configuration path must stay inside its configured root"
                ));
            }
        }
    }
    if components.is_empty() {
        return Err(anyhow!("MCP configuration path must name a file"));
    }
    Ok(components)
}

fn canonical_root(root: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(root).with_context(|| {
        format!(
            "failed to resolve MCP configuration root {}",
            root.display()
        )
    })?;
    if !canonical.is_dir() {
        return Err(anyhow!(
            "MCP configuration root {} is not a directory",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn io_error(error: io::Error) -> anyhow::Error {
    if error.kind() == io::ErrorKind::PermissionDenied {
        anyhow!("MCP configuration path must not traverse symlinks")
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

    use anyhow::{anyhow, Result};
    use rustix::{
        fs::{fchmod, mkdirat, open, openat, Mode, OFlags},
        io::Errno,
    };

    use super::{canonical_root, io_error, relative_components, ConfigFile};

    const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::CLOEXEC)
        .union(OFlags::NOFOLLOW);

    fn free(error: Errno) -> anyhow::Error {
        match error {
            Errno::LOOP | Errno::NOTDIR => {
                anyhow!("MCP configuration path must not traverse symlinks")
            }
            _ => io_error(io::Error::from_raw_os_error(error.raw_os_error())),
        }
    }

    fn open_root(root: &std::path::Path) -> Result<rustix::fd::OwnedFd> {
        open(root, DIRECTORY_FLAGS, Mode::empty()).map_err(free)
    }

    fn open_directory(
        parent: &rustix::fd::OwnedFd,
        name: &OsStr,
        create: bool,
    ) -> Result<Option<rustix::fd::OwnedFd>> {
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

    pub(super) fn read_text(file: &ConfigFile) -> Result<Option<String>> {
        let root = canonical_root(&file.root)?;
        let components = relative_components(&file.relative)?;
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

    pub(super) fn write_text(file: &ConfigFile, contents: &[u8]) -> Result<()> {
        let root = canonical_root(&file.root)?;
        let components = relative_components(&file.relative)?;
        let (file_name, parents) = components
            .split_last()
            .expect("relative components are non-empty");
        let mut directory = open_root(&root)?;
        for name in parents {
            let next = open_directory(&directory, name, true)?
                .ok_or_else(|| anyhow!("MCP configuration directory disappeared"))?;
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

    use anyhow::{anyhow, Result};

    use super::{canonical_root, io_error, relative_components, ConfigFile};

    fn checked_path(file: &ConfigFile) -> Result<(PathBuf, PathBuf)> {
        let root = canonical_root(&file.root)?;
        let components = relative_components(&file.relative)?;
        let mut path = root.clone();
        for component in components {
            path.push(component);
        }
        if !path.starts_with(&root) {
            return Err(anyhow!(
                "MCP configuration path must stay inside its configured root"
            ));
        }
        Ok((root, path))
    }

    fn reject_symlink_components(root: &std::path::Path, path: &std::path::Path) -> Result<()> {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| anyhow!("MCP configuration path escaped its root"))?;
        let mut current = root.to_path_buf();
        for component in relative.components() {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(anyhow!("MCP configuration path must not traverse symlinks"));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(())
    }

    pub(super) fn read_text(file: &ConfigFile) -> Result<Option<String>> {
        let (root, path) = checked_path(file)?;
        reject_symlink_components(&root, &path)?;
        match fs::read_to_string(&path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }

    pub(super) fn write_text(file: &ConfigFile, contents: &[u8]) -> Result<()> {
        let (root, path) = checked_path(file)?;
        reject_symlink_components(&root, &path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        reject_symlink_components(&root, &path)?;
        fs::write(&path, contents).map_err(io_error)
    }
}
