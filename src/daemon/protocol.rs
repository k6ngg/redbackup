//! Protocolo JSON simples usado no IPC local.
//!
//! Cada mensagem é uma linha JSON. O socket vive em
//! `<repositório>/metadata/redbackup.sock` e tem permissões `0600`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub command: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

impl Request {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args: serde_json::Value::Null,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub status: String,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl Response {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            status: "ok".to_string(),
            message: message.into(),
            data: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: "error".to_string(),
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }
}
