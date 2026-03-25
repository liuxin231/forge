use super::protocol::{Request, Response};
use crate::log::collector::LogLine;
use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Connection timeout for supervisor client
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Read timeout for supervisor responses
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
        let json = serde_json::to_string(&request)? + "\n";
        self.writer.write_all(json.as_bytes()).await?;

        let mut line = String::new();
        let n = tokio::time::timeout(
            READ_TIMEOUT,
            self.reader.read_line(&mut line),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Supervisor response timed out after {}s", READ_TIMEOUT.as_secs()))?
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
