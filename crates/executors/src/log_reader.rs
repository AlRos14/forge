use crate::log_schema::{LogEntry, LogKind};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};

pub struct LogReadResult {
    pub entries: Vec<LogEntry>,
    pub has_more: bool,
    pub next_sequence: Option<u64>,
}

pub struct LogReader;

impl LogReader {
    /// Read log entries starting from `from_sequence`, returning at most `limit` entries.
    pub async fn read(
        path: &Path,
        from_sequence: u64,
        limit: usize,
    ) -> std::io::Result<LogReadResult> {
        let file = tokio::fs::File::open(path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        let mut entries = Vec::new();
        let mut has_more = false;

        while let Some(line) = lines.next_line().await? {
            if line.is_empty() {
                continue;
            }
            let entry: LogEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(_) => continue, // skip malformed lines
            };

            if entry.sequence < from_sequence {
                continue;
            }

            if entries.len() >= limit {
                has_more = true;
                break;
            }

            entries.push(entry);
        }

        let next_sequence = entries
            .last()
            .map(|e| e.sequence + 1)
            .or(Some(from_sequence));

        Ok(LogReadResult {
            entries,
            has_more,
            next_sequence,
        })
    }

    /// Read the last `n` entries from the log file.
    pub async fn tail(path: &Path, n: usize) -> std::io::Result<LogReadResult> {
        let file = tokio::fs::File::open(path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        // Collect all entries, keep only last n
        let mut all_entries = Vec::new();
        while let Some(line) = lines.next_line().await? {
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
                all_entries.push(entry);
            }
        }

        let total = all_entries.len();
        let start = if total > n {
            let tail_start = total - n;
            all_entries[..tail_start]
                .iter()
                .rposition(is_tail_context_boundary)
                .unwrap_or(tail_start)
        } else {
            0
        };

        let entries = all_entries.split_off(start);
        let next_sequence = entries.last().map(|e| e.sequence + 1);

        Ok(LogReadResult {
            entries,
            has_more: start > 0,
            next_sequence,
        })
    }
}

fn is_tail_context_boundary(entry: &LogEntry) -> bool {
    if entry.kind == LogKind::User {
        return true;
    }
    entry.kind == LogKind::SessionInfo
        && entry
            .payload
            .get("method")
            .and_then(serde_json::Value::as_str)
            == Some("thread/started")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_schema::{LogKind, LogStream};
    use crate::LogWriter;
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn log_round_trip_preserves_entries_and_field_names() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("roundtrip.jsonl");
        let mut writer = LogWriter::new(&log_path, "exec-roundtrip".to_string(), 1024 * 1024);

        writer
            .write(
                LogKind::Stdout,
                LogStream::Main,
                serde_json::json!({"line": "stdout chunk"}),
            )
            .await
            .unwrap();
        writer
            .write(
                LogKind::Stderr,
                LogStream::Main,
                serde_json::json!({"line": "stderr chunk"}),
            )
            .await
            .unwrap();
        writer
            .write(
                LogKind::System,
                LogStream::Main,
                serde_json::json!({"status": "completed", "exit_code": 0}),
            )
            .await
            .unwrap();

        let raw = tokio::fs::read_to_string(&log_path).await.unwrap();
        let first_line = raw.lines().next().expect("first jsonl line");
        let object = serde_json::from_str::<serde_json::Value>(first_line).unwrap();
        let keys: BTreeSet<_> = object
            .as_object()
            .expect("json object")
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "schema_version".to_string(),
                "sequence".to_string(),
                "timestamp".to_string(),
                "execution_id".to_string(),
                "kind".to_string(),
                "stream".to_string(),
                "payload".to_string(),
                "truncated".to_string(),
            ])
        );

        let result = LogReader::read(&log_path, 0, 10).await.unwrap();
        assert_eq!(result.entries.len(), 3);
        assert_eq!(result.entries[0].kind, LogKind::Stdout);
        assert_eq!(
            result.entries[0].payload["line"].as_str(),
            Some("stdout chunk")
        );
        assert_eq!(result.entries[1].kind, LogKind::Stderr);
        assert_eq!(
            result.entries[1].payload["line"].as_str(),
            Some("stderr chunk")
        );
        assert_eq!(result.entries[2].kind, LogKind::System);
        assert_eq!(
            result.entries[2].payload["status"].as_str(),
            Some("completed")
        );
        assert_eq!(result.entries[2].execution_id, "exec-roundtrip");
        assert!(!result.has_more);
        assert_eq!(result.next_sequence, Some(3));
    }

    #[tokio::test]
    async fn log_reader_skips_garbage_trailing_line_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("garbage.jsonl");
        let mut writer = LogWriter::new(&log_path, "exec-garbage".to_string(), 1024 * 1024);
        writer
            .write(
                LogKind::Stdout,
                LogStream::Main,
                serde_json::json!({"line": "ok"}),
            )
            .await
            .unwrap();

        let mut contents = tokio::fs::read_to_string(&log_path).await.unwrap();
        contents.push_str("{\"truncated\": true\n");
        tokio::fs::write(&log_path, contents).await.unwrap();

        let result = LogReader::read(&log_path, 0, 10).await.unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].payload["line"].as_str(), Some("ok"));
    }
}
