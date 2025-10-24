use crate::bridge::PhpResponse;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
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
    #[allow(dead_code)]
    connection_pool: Arc<AsyncMutex<Vec<UnixStream>>>,
}

impl SocketBridge {
    #[allow(dead_code)]
    pub fn new() -> Result<Arc<Self>, Box<dyn std::error::Error + Send + Sync>> {
        // Загружаем переменные окружения
        dotenvy::dotenv().ok();

        // Получаем путь к сокету из переменных окружения, используем стандартный путь по умолчанию
        let socket_path = std::env::var("SOCKET_PATH").unwrap_or_else(|_| "/tmp/rust_php_bridge.sock".to_string());

        let config = SocketBridgeConfig { socket_path };

        let bridge = Arc::new(Self {
            config,
            connection_pool: Arc::new(AsyncMutex::new(Vec::new())),
        });

        Ok(bridge)
    }

    
    #[allow(dead_code)]
    pub async fn send_http_request(
        &self,
        http_request_data: serde_json::Value,
    ) -> Result<PhpResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Убедимся, что сокет существует перед подключением
        if !Path::new(&self.config.socket_path).exists() {
            return Err("Socket file does not exist. Laravel socket server may not be running.".into());
        }

        // Сериализуем HTTP-запрос в JSON (PHP worker expects this format directly)
        let request_json = serde_json::to_string(&http_request_data)?;

        // Получаем соединение - сначала пробуем из пула
        let mut pool = self.connection_pool.lock().await;
        let mut stream = if let Some(pooled_stream) = pool.pop() {
            // Проверим, что соединение все еще валидно
            if pooled_stream.peer_addr().is_ok() {
                pooled_stream
            } else {
                // Создаем новое соединение
                UnixStream::connect(&self.config.socket_path).await
                    .map_err(|e| format!("Failed to connect to socket '{}': {}", self.config.socket_path, e))?
            }
        } else {
            // Создаем новое соединение
            UnixStream::connect(&self.config.socket_path).await
                .map_err(|e| format!("Failed to connect to socket '{}': {}", self.config.socket_path, e))?
        };
        drop(pool); // освобождаем мьютекс

        // Отправляем длину сообщения виде 4-байтового префикса (big endian)
        let request_bytes = request_json.as_bytes();
        let len_bytes = (request_bytes.len() as u32).to_be_bytes();
        stream.write_all(&len_bytes).await?;

        // Отправляем JSON-данные
        stream.write_all(request_bytes).await?;
        stream.flush().await?;

        // Читаем ответ
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let response_len = u32::from_be_bytes(len_buf) as usize;

        let mut response_buf = vec![0u8; response_len];
        stream.read_exact(&mut response_buf).await?;

        let response_str = String::from_utf8(response_buf)?;

        // Try to parse as PhpResponse first, then as direct HTTP response
        let php_response: PhpResponse = match serde_json::from_str(&response_str) {
            Ok(parsed) => parsed,
            Err(_) => {
                // If it's not in PhpResponse format, treat it as direct HTTP response
                PhpResponse::new_success(None, Some(serde_json::from_str(&response_str)?))
            }
        };

        // Возвращаем соединение в пул, если валидно и пул не переполнен
        if stream.peer_addr().is_ok() {
            let mut pool = self.connection_pool.lock().await;
            if pool.len() < 10 {
                pool.push(stream);
            }
        }

        Ok(php_response)
    }
}

impl SocketBridge {
    #[allow(dead_code)]
    pub async fn cleanup(&self) {
        let mut pool = self.connection_pool.lock().await;
        pool.clear();
    }
}

impl Drop for SocketBridge {
    fn drop(&mut self) {
        // Удаляем файл сокета при уничтожении
        if Path::new(&self.config.socket_path).exists() {
            let _ = std::fs::remove_file(&self.config.socket_path);
        }
        println!("⚠️ SocketBridge уничтожается, файл сокета удален");
    }
}
