use chrono::Local;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast;

/// A single log line
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct LogLine {
    pub service: String,
    pub timestamp: String,
    pub stream: String, // "stdout" or "stderr"
    pub message: String,
}

/// Per-service ring buffer: retains the last RING_BUFFER_CAPACITY lines in memory.
pub const RING_BUFFER_CAPACITY: usize = 10_000;
pub type LogBuffer = Arc<Mutex<HashMap<String, VecDeque<LogLine>>>>;

/// Start collecting logs from a child process.
/// Lines are broadcast to all subscribers and, if `buffer` is provided,
/// appended to the in-memory ring buffer.
pub fn spawn_log_collector(
    service_name: String,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    log_tx: broadcast::Sender<LogLine>,
    buffer: Option<LogBuffer>,
) {
    // Spawn stdout reader
    let name = service_name.clone();
    let tx = log_tx.clone();
    let buf = buffer.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let log_line = LogLine {
                service: name.clone(),
                timestamp: Local::now().format("%H:%M:%S").to_string(),
                stream: "stdout".to_string(),
                message: line,
            };
            push_to_buffer(&buf, &log_line);
            let _ = tx.send(log_line);
        }
    });

    // Spawn stderr reader
    let name = service_name;
    let tx = log_tx;
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let log_line = LogLine {
                service: name.clone(),
                timestamp: Local::now().format("%H:%M:%S").to_string(),
                stream: "stderr".to_string(),
                message: line,
            };
            push_to_buffer(&buffer, &log_line);
            let _ = tx.send(log_line);
        }
    });
}

fn push_to_buffer(buffer: &Option<LogBuffer>, line: &LogLine) {
    if let Some(buf) = buffer {
        if let Ok(mut map) = buf.lock() {
            let deque = map.entry(line.service.clone()).or_default();
            deque.push_back(line.clone());
            if deque.len() > RING_BUFFER_CAPACITY {
                deque.pop_front();
            }
        }
    }
}

/// Read the last `tail` lines from the ring buffer for a service.
pub fn read_from_buffer(buffer: &LogBuffer, service: &str, tail: usize) -> Vec<LogLine> {
    if tail == 0 {
        return vec![];
    }
    let map = match buffer.lock() {
        Ok(m) => m,
        Err(_) => return vec![],
    };
    match map.get(service) {
        None => vec![],
        Some(deque) => {
            let skip = deque.len().saturating_sub(tail);
            deque.iter().skip(skip).cloned().collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_buffer() -> LogBuffer {
        Arc::new(Mutex::new(HashMap::new()))
    }

    fn make_line(svc: &str, msg: &str) -> LogLine {
        LogLine {
            service: svc.to_string(),
            timestamp: "12:00:00".to_string(),
            stream: "stdout".to_string(),
            message: msg.to_string(),
        }
    }

    #[test]
    fn test_push_and_read() {
        let buf = make_buffer();
        for i in 0..5 {
            push_to_buffer(&Some(buf.clone()), &make_line("api", &format!("line{}", i)));
        }
        let lines = read_from_buffer(&buf, "api", 3);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].message, "line2");
        assert_eq!(lines[2].message, "line4");
    }

    #[test]
    fn test_read_empty() {
        let buf = make_buffer();
        assert!(read_from_buffer(&buf, "api", 10).is_empty());
    }

    #[test]
    fn test_ring_buffer_cap() {
        let buf = make_buffer();
        for i in 0..RING_BUFFER_CAPACITY + 10 {
            push_to_buffer(&Some(buf.clone()), &make_line("api", &format!("line{}", i)));
        }
        let lines = read_from_buffer(&buf, "api", RING_BUFFER_CAPACITY + 10);
        assert_eq!(lines.len(), RING_BUFFER_CAPACITY);
        assert_eq!(lines[0].message, "line10");
    }

    #[test]
    fn test_log_line_serialization() {
        let line = make_line("api", "hello");
        let json = serde_json::to_string(&line).unwrap();
        let parsed: LogLine = serde_json::from_str(&json).unwrap();
        assert_eq!(line, parsed);
    }
}
