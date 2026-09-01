use std::process::Stdio;

use serde_json::{json, Value};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    time::{sleep, timeout, Duration, Instant},
};

use crate::{Result, ServiceError};

pub async fn refresh_codex_usage(config_json: &str) -> Result<Value> {
    let config = serde_json::from_str::<executors::CodexConfig>(config_json).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid Codex agent configuration: {error}"))
    })?;
    cli_adapters::codex::query_account_usage(&config)
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to query Codex usage: {error}"))
        })
}

pub async fn refresh_cursor_usage() -> Result<Value> {
    let mut command = Command::new("script");
    command
        .args([
            "-qec",
            "stty cols 100 rows 40; exec cursor-agent",
            "/dev/null",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("NO_COLOR", "1");
    command.kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        ServiceError::invalid_operation(format!("failed to start Cursor usage PTY: {error}"))
    })?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ServiceError::invalid_operation("Cursor usage PTY has no stdin"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ServiceError::invalid_operation("Cursor usage PTY has no stdout"))?;
    let mut output = Vec::new();
    // Cursor's slash-command palette needs one Enter to select `/usage` and
    // another to execute it. Wait for Cursor's actual terminal states instead
    // of guessing how long startup and the asynchronous quota fetch will take.
    read_until(&mut stdout, &mut output, Duration::from_secs(10), |text| {
        text.contains("Ready")
    })
    .await
    .map_err(|error| {
        ServiceError::invalid_operation(format!("Cursor did not become ready: {error}"))
    })?;
    stdin.write_all(b"/usage\r").await.map_err(|error| {
        ServiceError::invalid_operation(format!("failed to query Cursor usage: {error}"))
    })?;
    sleep(Duration::from_millis(500)).await;
    stdin.write_all(b"\r").await.map_err(|error| {
        ServiceError::invalid_operation(format!("failed to open Cursor usage: {error}"))
    })?;
    read_until(&mut stdout, &mut output, Duration::from_secs(20), |text| {
        text.contains("Monthly plan and on-demand usage") && text.contains("View in dashboard")
    })
    .await
    .map_err(|error| {
        ServiceError::invalid_operation(format!("Cursor /usage did not load: {error}"))
    })?;
    stdin.write_all(b"\x1b").await.map_err(|error| {
        ServiceError::invalid_operation(format!("failed to close Cursor usage: {error}"))
    })?;
    sleep(Duration::from_millis(250)).await;
    stdin.write_all(b"/quit\r").await.map_err(|error| {
        ServiceError::invalid_operation(format!("failed to stop Cursor usage probe: {error}"))
    })?;
    sleep(Duration::from_millis(250)).await;
    stdin.write_all(b"\r").await.map_err(|error| {
        ServiceError::invalid_operation(format!("failed to stop Cursor usage probe: {error}"))
    })?;
    drop(stdin);
    let _ = timeout(Duration::from_secs(3), stdout.read_to_end(&mut output)).await;
    timeout(Duration::from_secs(5), child.wait())
        .await
        .map_err(|_| ServiceError::invalid_operation("Cursor /usage timed out"))?
        .map_err(|error| {
            ServiceError::invalid_operation(format!("Cursor /usage failed: {error}"))
        })?;
    parse_cursor_usage(&String::from_utf8_lossy(&output))
}

async fn read_until<R, F>(
    reader: &mut R,
    output: &mut Vec<u8>,
    duration: Duration,
    predicate: F,
) -> std::result::Result<(), &'static str>
where
    R: AsyncRead + Unpin,
    F: Fn(&str) -> bool,
{
    let deadline = Instant::now() + duration;
    let mut chunk = [0_u8; 4096];
    loop {
        if predicate(&strip_terminal_codes(&String::from_utf8_lossy(output))) {
            return Ok(());
        }
        let read = timeout(
            deadline.saturating_duration_since(Instant::now()),
            reader.read(&mut chunk),
        )
        .await
        .map_err(|_| "timed out")?
        .map_err(|_| "terminal output could not be read")?;
        if read == 0 {
            return Err("process exited early");
        }
        output.extend_from_slice(&chunk[..read]);
    }
}

fn parse_cursor_usage(output: &str) -> Result<Value> {
    let text = strip_terminal_codes(output);
    let lines = text.lines().map(str::trim).collect::<Vec<_>>();
    let usage_start = lines
        .iter()
        .rposition(|line| line.contains("Monthly plan and on-demand usage"))
        .map(|index| index.saturating_sub(1))
        .ok_or_else(|| {
            ServiceError::invalid_operation("Cursor /usage returned no usage summary")
        })?;
    let usage = lines[usage_start..]
        .iter()
        .copied()
        .take_while(|line| !line.contains("View in dashboard"))
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("usage")
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
    let header = usage.iter().find(|line| line.starts_with("Usage"));
    let plan = header
        .and_then(|line| line.split('•').nth(1))
        .and_then(|tail| {
            tail.split_once("Resets")
                .map(|(plan, _)| plan.trim().to_owned())
        });
    let resets_at = header.and_then(|line| {
        line.split_once("Resets")
            .map(|(_, reset)| reset.trim().to_owned())
    });
    let percentage = |category: &str| {
        usage
            .iter()
            .find(|line| line.starts_with(category))
            .and_then(|line| line.split_whitespace().find(|part| part.ends_with('%')))
            .and_then(|part| part.trim_end_matches('%').parse::<u8>().ok())
    };
    let on_demand_enabled = usage
        .iter()
        .find(|line| line.starts_with("On-Demand"))
        .map(|line| !line.to_ascii_lowercase().contains("disabled"));
    Ok(json!({
        "plan": plan,
        "resets_at": resets_at,
        "categories": {
            "included": percentage("Included"),
            "auto": percentage("Auto"),
            "api": percentage("API"),
        },
        "on_demand_enabled": on_demand_enabled,
        "pools": usage,
        "raw_kind": "cursor_interactive_usage"
    }))
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

    #[test]
    fn parses_the_final_cursor_usage_panel() {
        let output = "Tip: Use /plan to plan execution\n\
            Usage • Pro Resets Sep 30\n\
            Monthly plan and on-demand usage\n\
            Category Current Usage\n\
            Included 11% used\n\
            Auto 11% used\n\
            API 0% used\n\
            On-Demand Disabled\n\
            View in dashboard: cursor.com/dashboard\n";
        let parsed = parse_cursor_usage(output).unwrap();
        let pools = parsed["pools"].as_array().unwrap();

        assert!(pools.iter().any(|line| line == "Usage • Pro Resets Sep 30"));
        assert!(pools.iter().any(|line| line == "Included 11% used"));
        assert!(!pools
            .iter()
            .any(|line| line.as_str().is_some_and(|line| line.contains("Tip:"))));
        assert_eq!(parsed["plan"], "Pro");
        assert_eq!(parsed["resets_at"], "Sep 30");
        assert_eq!(parsed["categories"]["included"], 11);
        assert_eq!(parsed["categories"]["auto"], 11);
        assert_eq!(parsed["categories"]["api"], 0);
        assert_eq!(parsed["on_demand_enabled"], false);
    }
}
