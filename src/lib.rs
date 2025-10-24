use anyhow::Result;

pub mod bridge;
pub mod config;
pub mod errors;

// Основной модуль для интеграции с Laravel

pub use config::{AppConfig, ServerConfig, LoggingConfig, PhpWorkerConfig, ConnectionConfig, ConnectionPoolConfig, RetryConfig};
