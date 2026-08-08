use crate::log_schema::{LogEntry, LogKind, LogStream};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

/// Default cap on bytes written to one execution's JSONL log.
pub const DEFAULT_MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;

pub struct LogWriter {
    path: PathBuf,
    execution_id: String,
    sequence: u64,
    bytes_written: u64,
    max_output_bytes: u64,
    truncated: bool,
    log_sender: Option<mpsc::UnboundedSender<LogEntry>>,
}

impl LogWriter {
    pub fn new(path: impl Into<PathBuf>, execution_id: String, max_output_bytes: u64) -> Self {
        let path = path.into();
        let sequence = next_sequence_for_existing_log(&path);
        Self {
            path,
            execution_id,
            sequence,
            bytes_written: 0,
            max_output_bytes,
            truncated: false,
            log_sender: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set_log_sender(&mut self, sender: mpsc::UnboundedSender<LogEntry>) {
        self.log_sender = Some(sender);
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub async fn write(
        &mut self,
        kind: LogKind,
        stream: LogStream,
        payload: serde_json::Value,
    ) -> std::io::Result<()> {
        if self.truncated {
            return Ok(());
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

        let mut line = serde_json::to_string(&entry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');

        let line_bytes = line.len() as u64;

        if self.bytes_written + line_bytes > self.max_output_bytes {
            // Write a final truncation marker
            let truncation_entry = LogEntry {
                truncated: true,
                ..entry
            };
            let mut truncation_line = serde_json::to_string(&truncation_entry)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            truncation_line.push('\n');

            self.append_to_file(truncation_line.as_bytes()).await?;
            self.truncated = true;
            self.sequence += 1;
            if let Some(sender) = &self.log_sender {
                let _ = sender.send(truncation_entry);
            }
            return Ok(());
        }

        self.append_to_file(line.as_bytes()).await?;
        self.bytes_written += line_bytes;
        self.sequence += 1;
        if let Some(sender) = &self.log_sender {
            let _ = sender.send(entry);
        }

        Ok(())
    }

    async fn append_to_file(&self, data: &[u8]) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(data).await?;
        file.flush().await?;
        Ok(())
    }
}

fn next_sequence_for_existing_log(path: &Path) -> u64 {
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<LogEntry>(&line).ok())
        .map(|entry| entry.sequence)
        .max()
        .map_or(0, |sequence| sequence.saturating_add(1))
}
