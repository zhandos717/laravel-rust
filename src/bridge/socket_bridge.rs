use anyhow::Result;
use crate::bridge::connection_pool::{ConnectionPool, ConnectionPoolConfig};
use crate::bridge::retry::{RetryConfig, retry_with_backoff};
use crate::bridge::PhpResponse;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug)]
pub struct SocketBridgeConfig {
    pub socket_path: String,
}

// SocketBridgeConfig теперь используется только как структура для хранения пути к сокету

#[derive(Serialize, Deserialize, Debug)]
pub struct PhpRequest {
    pub id: Option<String>,
    pub command: String,
    pub data: Option<HashMap<String, serde_json::Value>>,
}

pub struct SocketBridge {
    config: SocketBridgeConfig,
    connection_pool: Arc<ConnectionPool>,
    cleanup_on_drop: Arc<AsyncMutex<()>>,
}

impl SocketBridge {
    #[allow(dead_code)]
    pub fn new() -> Result<Arc<Self>> {
        // Load environment variables
        dotenvy::dotenv().ok();

        // Get socket path from environment variables, using default path as fallback
        let socket_path = std::env::var("SOCKET_PATH").unwrap_or_else(|_| "/tmp/rust_php_bridge.sock".to_string());

        let config = SocketBridgeConfig { socket_path };

        // Create connection pool with configuration from environment
        let pool_config = ConnectionPoolConfig::from_env();
        let connection_pool = Arc::new(ConnectionPool::new(pool_config));

        // Initialize the pool with minimum connections
        let bridge = Arc::new(Self {
            config,
            connection_pool,
            cleanup_on_drop: Arc::new(AsyncMutex::new(())),
        });

        // Initialize the pool with minimum connections in a background task
        // This ensures connections are pre-established but doesn't block the creation
        let bridge_clone = bridge.clone();
        tokio::spawn(async move {
            let retry_config = crate::bridge::retry::RetryConfig::from_env();
            if let Err(e) = retry_with_backoff(
                &retry_config,
                "initialize_connection_pool",
                || async {
                    bridge_clone.connection_pool.initialize().await
                }
            ).await {
                eprintln!("Failed to initialize connection pool after all retry attempts: {}", e);
                // Still continue even if initialization failed, as connections can be created on-demand
            }
        });

        Ok(bridge)
    }

    #[allow(dead_code)]
    pub fn new_with_config(app_config: &crate::config::AppConfig) -> Result<Arc<Self>> {
        let config = SocketBridgeConfig {
            socket_path: app_config.connection.socket_path.clone()
        };

        // Create connection pool with configuration from app config
        let pool_config = ConnectionPool::create_config_from_app_config(app_config);
        let connection_pool = Arc::new(ConnectionPool::new(pool_config));

        // Initialize the pool with minimum connections
        let bridge = Arc::new(Self {
            config,
            connection_pool,
            cleanup_on_drop: Arc::new(AsyncMutex::new(())),
        });

        // Initialize the pool with minimum connections in a background task
        // This ensures connections are pre-established but doesn't block the creation
        let bridge_clone = bridge.clone();
        let retry_config = crate::bridge::retry::RetryConfig {
            max_attempts: app_config.retry.max_attempts,
            base_delay: app_config.retry.base_delay,
            max_delay: app_config.retry.max_delay,
        };
        
        // Spawn initialization task to ensure connections are ready before the server starts handling requests
        tokio::spawn(async move {
            if let Err(e) = retry_with_backoff(
                &retry_config,
                "initialize_connection_pool",
                || async {
                    bridge_clone.connection_pool.initialize().await
                }
            ).await {
                eprintln!("Failed to initialize connection pool after all retry attempts: {}", e);
                // Still continue even if initialization failed, as connections can be created on-demand
            }
        });

        Ok(bridge)
    }
    
    
    #[allow(dead_code)]
    pub async fn send_http_request(
        &self,
        http_request_data: serde_json::Value,
    ) -> Result<PhpResponse> {
        self.connection_pool.send_http_request(http_request_data).await
    }
}

impl SocketBridge {
    #[allow(dead_code)]
    pub async fn cleanup(&self) {
        self.connection_pool.close_all().await;
    }
}

impl Drop for SocketBridge {
    fn drop(&mut self) {
        // Remove socket file when dropping
        if Path::new(&self.config.socket_path).exists() {
            let _ = std::fs::remove_file(&self.config.socket_path);
        }
        println!("⚠️ SocketBridge уничтожается, файл сокета удален");
    }
}
