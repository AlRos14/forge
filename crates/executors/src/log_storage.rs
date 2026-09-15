//! Lossless execution-log segments. The base path is the active JSONL file;
//! immutable segments live in `<base>.d`. No segment is evicted.
use crate::LogEntry;
use flate2::{read::MultiGzDecoder, write::GzEncoder, Compression};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

type FileLock = Mutex<()>;
static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<FileLock>>>> = OnceLock::new();

fn file_lock(path: &Path) -> Arc<FileLock> {
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    locks.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(Mutex::new(()));
    locks.insert(path.to_owned(), Arc::downgrade(&lock));
    lock
}

pub(crate) fn locked<T>(path: &Path, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let lock = file_lock(path);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    operation()
}

pub(crate) async fn blocking<T: Send + 'static>(
    path: &Path,
    operation: impl FnOnce(&Path) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || locked(&path, || operation(&path)))
        .await
        .map_err(io::Error::other)?
}

pub(crate) fn segment_dir(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".d");
    PathBuf::from(name)
}

fn segments(path: &Path) -> io::Result<BTreeMap<u64, PathBuf>> {
    let mut segments = BTreeMap::new();
    let entries = match fs::read_dir(segment_dir(path)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(segments),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(index) = name
            .strip_suffix(".jsonl.gz")
            .or_else(|| name.strip_suffix(".jsonl"))
            .and_then(|index| index.parse::<u64>().ok())
        else {
            continue;
        };
        // A crash after publishing gzip but before removing the plain segment
        // leaves both. They represent the same segment: read it exactly once.
        if name.ends_with(".gz") || !segments.contains_key(&index) {
            segments.insert(index, entry.path());
        }
    }
    Ok(segments)
}

pub(crate) fn parts(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut parts: Vec<_> = segments(path)?.into_values().collect();
    if path.exists() {
        parts.push(path.to_owned());
    }
    if parts.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "execution log not found",
        ));
    }
    Ok(parts)
}

pub(crate) fn visit_part(
    path: &Path,
    visitor: &mut impl FnMut(LogEntry) -> bool,
) -> io::Result<bool> {
    let file = File::open(path)?;
    let reader: Box<dyn BufRead> = if path.extension().is_some_and(|ext| ext == "gz") {
        Box::new(BufReader::new(MultiGzDecoder::new(file)))
    } else {
        Box::new(BufReader::new(file))
    };
    for line in reader.lines() {
        if let Ok(entry) = serde_json::from_str::<LogEntry>(&line?) {
            if !visitor(entry) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(crate) fn visit(path: &Path, mut visitor: impl FnMut(LogEntry) -> bool) -> io::Result<()> {
    for part in parts(path)? {
        if !visit_part(&part, &mut visitor)? {
            break;
        }
    }
    Ok(())
}

pub(crate) fn next_sequence(path: &Path) -> io::Result<u64> {
    let parts = match parts(path) {
        Ok(parts) => parts,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    for part in parts.iter().rev() {
        let mut last = None;
        visit_part(part, &mut |entry| {
            last = Some(last.map_or(entry.sequence, |previous: u64| previous.max(entry.sequence)));
            true
        })?;
        if let Some(last) = last {
            return Ok(last.saturating_add(1));
        }
    }
    Ok(0)
}

fn compress(plain: &Path) -> io::Result<()> {
    let compressed = plain.with_extension("jsonl.gz");
    let temporary = plain.with_extension("jsonl.gz.tmp");
    if !compressed.exists() {
        let mut source = File::open(plain)?;
        let mut encoder = GzEncoder::new(File::create(&temporary)?, Compression::default());
        io::copy(&mut source, &mut encoder)?;
        let mut file = encoder.finish()?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, &compressed)?;
        sync_directory(plain.parent().expect("segment parent"))?;
    }
    fs::remove_file(plain)?;
    Ok(())
}

/// Seal before compressing: an interrupted compression always leaves either a
/// complete plain segment or a complete gzip segment available to readers.
pub(crate) fn seal(path: &Path) -> io::Result<()> {
    let dir = segment_dir(path);
    let existing = segments(path)?;
    let has_active_data = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata.len() > 0,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "execution log is not a file",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if has_active_data {
        fs::create_dir_all(&dir)?;
        let index = existing.keys().next_back().map_or(0, |index| index + 1);
        let plain = dir.join(format!("{index:020}.jsonl"));
        File::open(path)?.sync_all()?;
        fs::rename(path, &plain)?;
        sync_directory(&dir)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        File::create(path)?;
        compress(&plain)?;
    }
    // Finish compression left interrupted by a process restart.
    if dir.exists() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                compress(&entry.path())?;
            }
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Relocate an inactive log bundle. Publish a complete destination before
/// removing any source files, including when moving across filesystems.
pub(crate) fn relocate(source: &Path, destination: &Path) -> io::Result<()> {
    if source == destination {
        return Ok(());
    }
    let source_parts = match parts(source) {
        Ok(parts) => parts,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let destination_parts = match parts(destination) {
        Ok(parts) => parts,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    if !destination_parts.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination execution log already exists",
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    // Preserve exact encoded bytes; do not parse/rewrite historical records.
    for part in &source_parts {
        let target = if part == source {
            destination.to_owned()
        } else {
            let dir = segment_dir(destination);
            fs::create_dir_all(&dir)?;
            dir.join(part.file_name().expect("segment filename"))
        };
        if let Err(error) = fs::copy(part, &target).and_then(|_| File::open(&target)?.sync_all()) {
            // Source is still complete. Keep it authoritative on failure.
            let _ = fs::remove_file(destination);
            let _ = fs::remove_dir_all(segment_dir(destination));
            return Err(error);
        }
    }
    // Failure to clean up a source copy must not discard the complete target.
    // Callers update the stored path to the complete destination after success.
    for part in source_parts {
        let _ = fs::remove_file(part);
    }
    let _ = fs::remove_dir(segment_dir(source));
    Ok(())
}
