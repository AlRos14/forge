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
