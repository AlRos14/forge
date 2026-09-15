use crate::{
    log_schema::{LogEntry, LogKind},
    log_storage,
};
use std::{collections::VecDeque, path::Path};

pub struct LogReadResult {
    pub entries: Vec<LogEntry>,
    pub has_more: bool,
    pub next_sequence: Option<u64>,
}

pub struct LogReader;

impl LogReader {
    /// Read one page from the logical log, across plain and compressed segments.
    pub async fn read(
        path: &Path,
        from_sequence: u64,
        limit: usize,
    ) -> std::io::Result<LogReadResult> {
        log_storage::blocking(path, move |path| {
            let mut entries = Vec::new();
            let mut has_more = false;
            log_storage::visit(path, |entry| {
                if entry.sequence < from_sequence {
                    return true;
                }
                if entries.len() >= limit {
                    has_more = true;
                    return false;
                }
                entries.push(entry);
                true
            })?;
            let next_sequence = entries
                .last()
                .map(|e| e.sequence + 1)
                .or(Some(from_sequence));
            Ok(LogReadResult {
                entries,
                has_more,
                next_sequence,
            })
        })
        .await
    }

    /// Fold the full log without retaining raw events in memory. Used by context
    /// and review consumers so segment boundaries never remove agent context.
    pub async fn fold<T: Send + 'static>(
        path: &Path,
        mut state: T,
        mut apply: impl FnMut(&mut T, LogEntry) + Send + 'static,
    ) -> std::io::Result<T> {
        log_storage::blocking(path, move |path| {
            log_storage::visit(path, |entry| {
                apply(&mut state, entry);
                true
            })?;
            Ok(state)
        })
        .await
    }

    /// Read newest segments first. Bound turn-context expansion to 1,000
    /// entries so a long-running turn cannot materialize the full log in the UI.
    pub async fn tail(path: &Path, n: usize) -> std::io::Result<LogReadResult> {
        log_storage::blocking(path, move |path| {
            let parts = log_storage::parts(path)?;
            let requested = n.min(1_000);
            let bound = 1_000;
            let mut recent = VecDeque::new();
            let mut seen = 0;
            let mut remaining = parts.len();
            for part in parts.iter().rev() {
                remaining -= 1;
                let mut previous = VecDeque::new();
                log_storage::visit_part(part, &mut |entry| {
                    seen += 1;
                    previous.push_back(entry);
                    if previous.len() > bound {
                        previous.pop_front();
                    }
                    true
                })?;
                previous.append(&mut recent);
                while previous.len() > bound {
                    previous.pop_front();
                }
                recent = previous;
                let tail_start = recent.len().saturating_sub(requested);
                let boundary = recent
                    .iter()
                    .take(tail_start)
                    .rposition(is_tail_context_boundary);
                if boundary.is_some() || recent.len() >= bound || remaining == 0 {
                    let start = boundary.unwrap_or(tail_start);
                    let entries: Vec<_> = recent.into_iter().skip(start).collect();
                    let has_more = remaining > 0 || seen > entries.len();
                    let next_sequence = entries.last().map(|e| e.sequence + 1);
                    return Ok(LogReadResult {
                        entries,
                        has_more,
                        next_sequence,
                    });
                }
            }
            Ok(LogReadResult {
                entries: Vec::new(),
                has_more: false,
                next_sequence: None,
            })
        })
        .await
    }
}

fn is_tail_context_boundary(entry: &LogEntry) -> bool {
    entry.kind == LogKind::User
        || (entry.kind == LogKind::SessionInfo
            && entry
                .payload
                .get("method")
                .and_then(serde_json::Value::as_str)
                == Some("thread/started"))
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
