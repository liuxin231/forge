use super::protocol::{Request, Response};
use crate::log::collector::LogLine;
use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Connection timeout for supervisor client
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Default read timeout for quick supervisor responses (status, down, logs)
const READ_TIMEOUT_DEFAULT: std::time::Duration = std::time::Duration::from_secs(30);
/// Extended read timeout for operations that involve health checks (up, restart)
pub const READ_TIMEOUT_LONG: std::time::Duration = std::time::Duration::from_secs(300);

pub struct SupervisorClient {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl SupervisorClient {
    pub async fn connect(port: u16) -> Result<Self> {
        let stream = tokio::time::timeout(
            CONNECT_TIMEOUT,
            TcpStream::connect(format!("127.0.0.1:{}", port)),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Connection to supervisor timed out after {}s", CONNECT_TIMEOUT.as_secs()))?
        .context("Failed to connect to supervisor")?;

        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }

    pub async fn send(&mut self, request: Request) -> Result<Response> {
        let timeout = match &request {
            Request::Up(_) | Request::Restart(_) => READ_TIMEOUT_LONG,
            _ => READ_TIMEOUT_DEFAULT,
        };

        self.write_request(&request).await?;
        self.read_response(timeout).await
    }

    /// Write a request without waiting for the response.
    pub async fn write_request(&mut self, request: &Request) -> Result<()> {
        let json = serde_json::to_string(request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;
        Ok(())
    }

    /// Read a response with the given timeout.
    pub async fn read_response(&mut self, timeout: std::time::Duration) -> Result<Response> {
        let mut line = String::new();
        let n = tokio::time::timeout(
            timeout,
            self.reader.read_line(&mut line),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Supervisor response timed out after {}s", timeout.as_secs()))?
        .context("Failed to read supervisor response")?;

        if n == 0 {
            anyhow::bail!("Supervisor closed connection without responding");
        }

        let response: Response =
            serde_json::from_str(line.trim()).context("Failed to parse supervisor response")?;
        Ok(response)
    }

    pub async fn stream_logs(&mut self, json: bool, follow_label: Option<String>) -> Result<()> {
        loop {
            let mut line = String::new();
            tokio::select! {
                result = self.reader.read_line(&mut line) => {
                    match result? {
                        0 => break, // Connection closed
                        _ => {}
                    }
                }
                _ = tokio::signal::ctrl_c() => break,
            }

            match serde_json::from_str::<LogLine>(line.trim()) {
                Ok(log_line) if log_line.stream == "_follow_start_" => {
                    if let Some(ref label) = follow_label {
                        use colored::Colorize;
                        eprintln!("{}", format!("--- following {} (Ctrl+C to stop) ---", label).dimmed());
                    }
                }
                Ok(log_line) => {
                    if json {
                        println!("{}", serde_json::to_string(&log_line)?);
                    } else {
                        print_log_line(&log_line);
                    }
                }
                Err(e) => {
                    tracing::debug!("Failed to parse log line: {}", e);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::collector::LogLine;

    // ── Timeout constants ─────────────────────────────────────────────────────

    #[test]
    fn test_long_timeout_greater_than_default() {
        assert!(
            READ_TIMEOUT_LONG > READ_TIMEOUT_DEFAULT,
            "Up/Restart must use a longer timeout than regular commands"
        );
    }

    #[test]
    fn test_default_timeout_at_least_five_seconds() {
        assert!(READ_TIMEOUT_DEFAULT.as_secs() >= 5);
    }

    #[test]
    fn test_long_timeout_at_least_sixty_seconds() {
        // health checks during `up` can take a while
        assert!(READ_TIMEOUT_LONG.as_secs() >= 60);
    }

    // ── print_log_line ────────────────────────────────────────────────────────

    #[test]
    fn test_print_log_line_empty_fields_no_panic() {
        let line = LogLine {
            service: String::new(),
            stream: "stdout".to_string(),
            message: String::new(),
            timestamp: String::new(),
        };
        print_log_line(&line); // must not panic
    }

    #[test]
    fn test_print_log_line_with_timestamp_no_panic() {
        let line = LogLine {
            service: "api".to_string(),
            stream: "stdout".to_string(),
            message: "server started".to_string(),
            timestamp: "10:30:00".to_string(),
        };
        print_log_line(&line);
    }

    #[test]
    fn test_print_log_line_without_timestamp_no_panic() {
        let line = LogLine {
            service: "api".to_string(),
            stream: "stderr".to_string(),
            message: "warning: disk low".to_string(),
            timestamp: String::new(),
        };
        print_log_line(&line);
    }

    #[test]
    fn test_print_log_line_all_six_color_branches_no_panic() {
        // The color array has 6 entries. Exercise every hash bucket (0–5) to ensure
        // no branch is accidentally unreachable or panics.
        let color_count = 6usize;
        let mut hit = vec![false; color_count];
        for i in 0u32..=255 {
            let name = format!("{}", i);
            let idx = name.bytes().fold(0usize, |acc, b| acc.wrapping_add(b as usize)) % color_count;
            if !hit[idx] {
                hit[idx] = true;
                let line = LogLine {
                    service: name,
                    stream: "stdout".to_string(),
                    message: "test".to_string(),
                    timestamp: String::new(),
                };
                print_log_line(&line);
            }
            if hit.iter().all(|&x| x) {
                break;
            }
        }
        assert!(hit.iter().all(|&x| x), "not all 6 color branches were exercised");
    }
}

fn print_log_line(line: &LogLine) {
    use colored::Colorize;

    let colors = ["blue", "green", "yellow", "cyan", "magenta", "red"];
    let hash = line
        .service
        .bytes()
        .fold(0usize, |acc, b| acc.wrapping_add(b as usize));
    let color = colors[hash % colors.len()];

    let prefix = format!("[{}]", line.service);
    let colored_prefix = match color {
        "blue" => prefix.blue(),
        "green" => prefix.green(),
        "yellow" => prefix.yellow(),
        "cyan" => prefix.cyan(),
        "magenta" => prefix.magenta(),
        "red" => prefix.red(),
        _ => prefix.normal(),
    };

    if line.timestamp.is_empty() {
        println!("{} {}", colored_prefix, line.message);
    } else {
        use colored::Colorize;
        println!("{} {} {}", colored_prefix, line.timestamp.dimmed(), line.message);
    }
}
