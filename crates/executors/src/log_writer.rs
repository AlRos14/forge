use crate::{
    log_schema::{LogEntry, LogKind, LogStream},
    log_storage,
};
use std::{
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use tokio::sync::mpsc;

/// Rotate the active JSONL segment after 10 MiB; retain compressed segments.
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;

pub struct LogWriter {
    path: PathBuf,
    execution_id: String,
    sequence: u64,
    segment_bytes: u64,
    initialization_error: Option<io::Error>,
    log_sender: Option<mpsc::UnboundedSender<LogEntry>>,
}

impl LogWriter {
    pub fn new(path: impl Into<PathBuf>, execution_id: String, segment_bytes: u64) -> Self {
        let path = path.into();
        let sequence = log_storage::locked(&path, || log_storage::next_sequence(&path));
        let (sequence, initialization_error) = match sequence {
            Ok(sequence) => (sequence, None),
            Err(error) => (0, Some(error)),
        };
        Self {
            path,
            execution_id,
            sequence,
            segment_bytes: segment_bytes.max(1),
            initialization_error,
            log_sender: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn set_log_sender(&mut self, sender: mpsc::UnboundedSender<LogEntry>) {
        self.log_sender = Some(sender);
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Move the complete inactive log, including compressed segments.
    pub async fn relocate(source: &Path, destination: &Path) -> io::Result<()> {
        if source == destination {
            return Ok(());
        }
        let source = source.to_owned();
        let destination = destination.to_owned();
        let (first, second) = if source < destination {
            (source.clone(), destination.clone())
        } else {
            (destination.clone(), source.clone())
        };
        log_storage::blocking(&first, move |_| {
            log_storage::locked(&second, || log_storage::relocate(&source, &destination))
        })
        .await
    }

    /// Compress the final partial segment, including existing single-file logs.
    /// Safe to repeat or to append to the same logical log after completion.
    pub async fn compact(path: &Path) -> io::Result<()> {
        log_storage::blocking(path, log_storage::seal).await
    }

    pub async fn write(
        &mut self,
        kind: LogKind,
        stream: LogStream,
        payload: serde_json::Value,
    ) -> io::Result<()> {
        if let Some(error) = &self.initialization_error {
            return Err(io::Error::new(error.kind(), error.to_string()));
        }
        let entry = LogEntry {
            schema_version: 1,
            sequence: self.sequence,
            timestamp: chrono::Utc::now().to_rfc3339(),
            execution_id: self.execution_id.clone(),
            kind,
            stream,
            payload,
            truncated: false,
        };
        let mut line = serde_json::to_vec(&entry).map_err(io::Error::other)?;
        line.push(b'\n');
        // Activity delivery is independent of persistence. A storage failure is
        // returned to the executor explicitly, never hidden as a missing heartbeat.
        if let Some(sender) = &self.log_sender {
            let _ = sender.send(entry);
        }
        self.sequence += 1;
        let segment_bytes = self.segment_bytes;
        log_storage::blocking(&self.path, move |path| {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes = match std::fs::metadata(path) {
                Ok(metadata) => metadata.len(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
                Err(e) => return Err(e),
            };
            // An oversized individual event remains intact in its own segment.
            if bytes > 0 && bytes.saturating_add(line.len() as u64) > segment_bytes {
                log_storage::seal(path)?;
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(path)?;
            // A crash may leave an unterminated record. Keep its bytes, but
            // separate the next event so it does not get swallowed by that line.
            if file.metadata()?.len() > 0 {
                file.seek(SeekFrom::End(-1))?;
                let mut last = [0];
                file.read_exact(&mut last)?;
                if last[0] != b'\n' {
                    file.write_all(b"\n")?;
                }
            }
            file.write_all(&line)?;
            if line.len() as u64 >= segment_bytes {
                log_storage::seal(path)?;
            }
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LogReader;
    use serde_json::json;

    #[tokio::test]
    async fn rotation_compression_restart_and_live_activity_are_lossless() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("execution.jsonl");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut writer = LogWriter::new(&path, "execution".into(), 1024);
        writer.set_log_sender(tx);
        let mut expected = Vec::new();
        for i in 0..40 {
            writer
                .write(
                    LogKind::Assistant,
                    LogStream::Main,
                    json!({"text": "repeated text ".repeat(25), "index": i}),
                )
                .await
                .unwrap();
            expected.push(rx.recv().await.unwrap());
        }
        writer
            .write(
                LogKind::SessionInfo,
                LogStream::Heartbeat,
                json!({"type": "codex_turn_heartbeat"}),
            )
            .await
            .unwrap();
        expected.push(rx.recv().await.unwrap());
        assert_eq!(expected.last().unwrap().stream, LogStream::Heartbeat);
        LogWriter::compact(&path).await.unwrap();
        LogWriter::compact(&path).await.unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        let compressed_bytes: u64 = std::fs::read_dir(log_storage::segment_dir(&path))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                assert_eq!(entry.path().extension().unwrap(), "gz");
                entry.metadata().unwrap().len()
            })
            .sum();
        let raw_bytes: usize = expected
            .iter()
            .map(|e| serde_json::to_vec(e).unwrap().len() + 1)
            .sum();
        assert!(compressed_bytes < raw_bytes as u64 / 2);
        let mut actual = Vec::new();
        let mut cursor = 0;
        loop {
            let page = LogReader::read(&path, cursor, 7).await.unwrap();
            cursor = page.next_sequence.unwrap();
            actual.extend(page.entries);
            if !page.has_more {
                break;
            }
        }
        assert_eq!(actual, expected);
        assert_eq!(
            LogReader::tail(&path, 3).await.unwrap().entries,
            expected[38..]
        );
        let mut resumed = LogWriter::new(&path, "execution".into(), 1024);
        assert_eq!(resumed.sequence(), 41);
        resumed
            .write(
                LogKind::Assistant,
                LogStream::Main,
                json!({"text": "after restart"}),
            )
            .await
            .unwrap();
        assert_eq!(
            LogReader::read(&path, 41, 10).await.unwrap().entries[0].payload["text"],
            "after restart"
        );
    }

    #[tokio::test]
    async fn relocating_a_log_preserves_compressed_history_and_active_events() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("old.jsonl");
        let destination = dir.path().join("durable/execution.jsonl");
        let mut writer = LogWriter::new(&source, "execution".into(), 500);
        for i in 0..12 {
            writer
                .write(LogKind::Assistant, LogStream::Main, json!({"text": i}))
                .await
                .unwrap();
        }
        let expected = LogReader::read(&source, 0, 20).await.unwrap().entries;
        LogWriter::relocate(&source, &destination).await.unwrap();
        assert_eq!(
            LogReader::read(&destination, 0, 20).await.unwrap().entries,
            expected
        );
        assert!(!source.exists());
        assert!(!log_storage::segment_dir(&source).exists());
        LogWriter::relocate(&destination, &destination)
            .await
            .unwrap();
        assert_eq!(
            LogWriter::new(&destination, "execution".into(), 500).sequence(),
            12
        );
    }

    #[tokio::test]
    async fn tail_bounds_long_turns_without_removing_older_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long-turn.jsonl");
        let mut writer = LogWriter::new(&path, "execution".into(), 4096);
        writer
            .write(
                LogKind::User,
                LogStream::Main,
                json!({"text": "original request"}),
            )
            .await
            .unwrap();
        for i in 0..1100 {
            writer
                .write(
                    LogKind::AssistantDelta,
                    LogStream::Main,
                    json!({"delta": i}),
                )
                .await
                .unwrap();
        }
        let tail = LogReader::tail(&path, 200).await.unwrap();
        assert_eq!(tail.entries.len(), 200);
        assert!(tail.has_more);
        assert_eq!(tail.entries[0].sequence, 901);
        assert_eq!(
            LogReader::read(&path, 0, 1).await.unwrap().entries[0].payload["text"],
            "original request"
        );
    }

    #[tokio::test]
    async fn reader_and_rotation_can_run_concurrently_without_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent.jsonl");
        std::fs::write(&path, "").unwrap();
        let writer_path = path.clone();
        let writer = tokio::spawn(async move {
            let mut writer = LogWriter::new(writer_path, "execution".into(), 500);
            for i in 0..60 {
                writer
                    .write(LogKind::Stdout, LogStream::Main, json!({"line": i}))
                    .await
                    .unwrap();
            }
        });
        let mut cursor = 0;
        while cursor < 60 {
            let page = LogReader::read(&path, cursor, 3).await.unwrap();
            for entry in page.entries {
                assert_eq!(entry.sequence, cursor);
                cursor += 1;
            }
            tokio::task::yield_now().await;
        }
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn storage_failure_does_not_swallow_activity_and_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let mut writer = LogWriter::new(&path, "execution".into(), 500);
        let (tx, mut rx) = mpsc::unbounded_channel();
        writer.set_log_sender(tx);
        // The writer initialized normally; now simulate an unavailable destination.
        std::fs::create_dir(&path).unwrap();
        let result = writer
            .write(
                LogKind::SessionInfo,
                LogStream::Heartbeat,
                json!({"type": "codex_turn_heartbeat"}),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(rx.recv().await.unwrap().stream, LogStream::Heartbeat);
    }

    #[tokio::test]
    async fn restart_after_partial_record_keeps_subsequent_events_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.jsonl");
        let mut writer = LogWriter::new(&path, "execution".into(), 4096);
        writer
            .write(
                LogKind::Assistant,
                LogStream::Main,
                json!({"text": "before"}),
            )
            .await
            .unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"schema_version\":")
            .unwrap();
        let mut writer = LogWriter::new(&path, "execution".into(), 4096);
        writer
            .write(
                LogKind::Assistant,
                LogStream::Main,
                json!({"text": "after"}),
            )
            .await
            .unwrap();
        let entries = LogReader::read(&path, 0, 10).await.unwrap().entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].sequence, 1);
        assert_eq!(entries[1].payload["text"], "after");
    }

    #[tokio::test]
    async fn interrupted_compression_preserves_plain_segment_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        let mut writer = LogWriter::new(&path, "execution".into(), 1024);
        writer
            .write(
                LogKind::Assistant,
                LogStream::Main,
                json!({"text": "retained context"}),
            )
            .await
            .unwrap();
        let segments = log_storage::segment_dir(&path);
        std::fs::create_dir(&segments).unwrap();
        std::fs::rename(&path, segments.join("00000000000000000000.jsonl")).unwrap();
        std::fs::write(
            segments.join("00000000000000000000.jsonl.gz.tmp"),
            "partial gzip",
        )
        .unwrap();
        assert_eq!(
            LogReader::read(&path, 0, 10).await.unwrap().entries.len(),
            1
        );
        LogWriter::compact(&path).await.unwrap();
        let mut writer = LogWriter::new(&path, "execution".into(), 1024);
        assert_eq!(writer.sequence(), 1);
        writer
            .write(
                LogKind::Assistant,
                LogStream::Main,
                json!({"text": "continued"}),
            )
            .await
            .unwrap();
        assert_eq!(
            LogReader::read(&path, 0, 10).await.unwrap().entries.len(),
            2
        );
    }
}
