use std::process::Stdio;

use serde_json::{json, Value};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    time::{timeout, Duration},
};

use crate::{Result, ServiceError};

pub async fn refresh_cursor_usage() -> Result<Value> {
    let mut child = Command::new("script")
        .args(["-qec", "cursor-agent", "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1")
        .spawn()
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to start Cursor usage PTY: {error}"))
        })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ServiceError::invalid_operation("Cursor usage PTY has no stdin"))?;
    stdin.write_all(b"/usage\r/quit\r").await.map_err(|error| {
        ServiceError::invalid_operation(format!("failed to query Cursor usage: {error}"))
    })?;
    drop(stdin);
    let output = timeout(Duration::from_secs(12), child.wait_with_output())
        .await
        .map_err(|_| ServiceError::invalid_operation("Cursor /usage timed out"))?
        .map_err(|error| {
            ServiceError::invalid_operation(format!("Cursor /usage failed: {error}"))
        })?;
    let text = strip_terminal_codes(&String::from_utf8_lossy(&output.stdout));
    let usage = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("plan")
                || lower.contains("included")
                || lower.contains("auto")
                || lower.contains("api")
                || lower.contains("on-demand")
                || lower.contains("reset")
        })
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if usage.is_empty() {
        return Err(ServiceError::invalid_operation(
            "Cursor /usage returned no recognizable quota pools",
        ));
    }
    Ok(json!({ "pools": usage, "raw_kind": "cursor_interactive_usage" }))
}

fn strip_terminal_codes(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else if ch != '\r' {
            output.push(ch);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_sequences() {
        assert_eq!(
            strip_terminal_codes("\u{1b}[32mIncluded 42%\u{1b}[0m\r\n"),
            "Included 42%\n"
        );
    }
}
