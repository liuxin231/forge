use serde::{Deserialize, Serialize};

use crate::log::collector::LogLine;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Up(Vec<String>),
    Down(Vec<String>),
    Restart(Vec<String>),
    Status(Vec<String>),
    Logs {
        services: Vec<String>,
        tail: usize,
        follow: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Response {
    Ok,
    Services(Vec<ServiceStatus>),
    LogLines(Vec<LogLine>),
    LogStream, // Indicates client should switch to streaming mode
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceStatus {
    pub name: String,
    pub port: Option<u16>,
    pub status: ProcessStatus,
    pub health: HealthStatus,
    pub pid: Option<u32>,
    pub restarts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProcessStatus {
    Running,
    Stopped,
    Errored,
    Starting,
}

impl std::fmt::Display for ProcessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessStatus::Running => write!(f, "running"),
            ProcessStatus::Stopped => write!(f, "stopped"),
            ProcessStatus::Errored => write!(f, "errored"),
            ProcessStatus::Starting => write!(f, "starting"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
    None,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Unhealthy => write!(f, "unhealthy"),
            HealthStatus::Unknown => write!(f, "unknown"),
            HealthStatus::None => write!(f, "-"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization_roundtrip() {
        let requests = vec![
            Request::Up(vec!["api".to_string(), "web".to_string()]),
            Request::Down(vec![]),
            Request::Restart(vec!["api".to_string()]),
            Request::Status(vec![]),
            Request::Logs {
                services: vec!["api".to_string()],
                tail: 100,
                follow: true,
            },
        ];
        for req in requests {
            let json = serde_json::to_string(&req).unwrap();
            let parsed: Request = serde_json::from_str(&json).unwrap();
            // Just verify it doesn't panic — Request doesn't implement PartialEq
            let _ = format!("{:?}", parsed);
        }
    }

    #[test]
    fn test_response_serialization_roundtrip() {
        let responses = vec![
            Response::Ok,
            Response::Error("test error".to_string()),
            Response::LogLines(vec![]),
            Response::LogStream,
            Response::Services(vec![ServiceStatus {
                name: "api".to_string(),
                port: Some(8080),
                status: ProcessStatus::Running,
                health: HealthStatus::Healthy,
                pid: Some(12345),
                restarts: 0,
            }]),
        ];
        for resp in &responses {
            let json = serde_json::to_string(resp).unwrap();
            let parsed: Response = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, resp);
        }
    }

    #[test]
    fn test_process_status_display() {
        assert_eq!(ProcessStatus::Running.to_string(), "running");
        assert_eq!(ProcessStatus::Stopped.to_string(), "stopped");
        assert_eq!(ProcessStatus::Errored.to_string(), "errored");
        assert_eq!(ProcessStatus::Starting.to_string(), "starting");
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(HealthStatus::Healthy.to_string(), "healthy");
        assert_eq!(HealthStatus::Unhealthy.to_string(), "unhealthy");
        assert_eq!(HealthStatus::Unknown.to_string(), "unknown");
        assert_eq!(HealthStatus::None.to_string(), "-");
    }

    #[test]
    fn test_service_status_equality() {
        let s1 = ServiceStatus {
            name: "api".to_string(),
            port: Some(8080),
            status: ProcessStatus::Running,
            health: HealthStatus::Healthy,
            pid: Some(1234),
            restarts: 0,
        };
        let s2 = s1.clone();
        assert_eq!(s1, s2);
    }
}
