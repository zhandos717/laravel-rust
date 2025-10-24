use crate::bridge::PhpResponse;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex as AsyncMutex;

#[derive(Debug)]
pub struct SocketBridgeConfig {
    pub socket_path: String,
}

impl SocketBridgeConfig {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        dotenvy::dotenv().ok();

        // Проверяем, задан ли путь к сокету в переменных окружения
        let socket_path = std::env::var("SOCKET_PATH").unwrap_or_else(|_| {
            // Создаем временный файл для сокета
            let temp_file = NamedTempFile::new().expect("Не удалось создать временный файл для сокета");
            let path = temp_file.path().to_str().unwrap().to_string();
            temp_file.keep().unwrap(); // Сохраняем файл, чтобы он не был удален
            path
        });

        Ok(SocketBridgeConfig { socket_path })
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PhpRequest {
    pub id: Option<String>,
    pub command: String,
    pub data: Option<HashMap<String, serde_json::Value>>,
}

pub struct SocketBridge {
    config: SocketBridgeConfig,
    connection_pool: Arc<AsyncMutex<Vec<UnixStream>>>,
}

impl SocketBridge {
    pub fn new() -> Result<Arc<Self>, Box<dyn std::error::Error + Send + Sync>> {
        let config = SocketBridgeConfig::from_env()?;

        let bridge = Arc::new(Self {
            config,
            connection_pool: Arc::new(AsyncMutex::new(Vec::new())),
        });

        Ok(bridge)
    }

    async fn get_connection(&self) -> Result<UnixStream, Box<dyn std::error::Error + Send + Sync>> {
        // Попробуем получить соединение из пула
        let mut pool = self.connection_pool.lock().await;
        if let Some(stream) = pool.pop() {
            // Проверим, что соединение все еще валидно
            if stream.peer_addr().is_ok() {
                drop(pool); // освобождаем мьютекс перед возвратом
                return Ok(stream);
            }
        }
        drop(pool); // освобождаем мьютекс перед подключением

        // Создаем новое соединение
        let stream = UnixStream::connect(&self.config.socket_path).await
            .map_err(|e| format!("Failed to connect to socket '{}': {}", self.config.socket_path, e))?;
        Ok(stream)
    }

    async fn return_connection(&self, stream: UnixStream) {
        // Проверяем, что соединение все еще валидно
        if stream.peer_addr().is_ok() {
            let mut pool = self.connection_pool.lock().await;
            // Ограничиваем размер пула, чтобы избежать утечки памяти
            if pool.len() < 10 {
                pool.push(stream);
            }
        }
    }

    pub async fn send_command(
        &self,
        command: &str,
        data: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<PhpResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Убедимся, что сокет существует перед подключением
        if !Path::new(&self.config.socket_path).exists() {
            return Err("Socket file does not exist. Laravel socket server may not be running.".into());
        }

        let request = PhpRequest {
            id: None,
            command: command.to_string(),
            data,
        };

        // Сериализуем запрос в JSON
        let request_json = serde_json::to_string(&request)?;

        // Получаем соединение из пула или создаем новое
        let mut stream = self.get_connection().await?;

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
        let php_response: PhpResponse = serde_json::from_str(&response_str)
            .unwrap_or_else(|_| PhpResponse::new_error(None, format!("Ошибка парсинга ответа: {}", response_str)));

        // Возвращаем соединение в пул
        self.return_connection(stream).await;

        Ok(php_response)
    }

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

        // Получаем соединение из пула или создаем новое
        let mut stream = self.get_connection().await?;

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

        // Возвращаем соединение в пул
        self.return_connection(stream).await;

        Ok(php_response)
    }

    pub fn get_socket_path(&self) -> &str {
        &self.config.socket_path
    }

    // Убираем функцию start_server, так как сервер сокета создается в Laravel Worker
    // Rust-сервер теперь только отправляет запросы в Laravel Worker через сокет
}

// Убираем функции handle_client и process_php_request, так как они не используются
// Вместо этого Laravel Worker сам обрабатывает соединения

// Добавим метод для очистки пула соединений
impl SocketBridge {
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
