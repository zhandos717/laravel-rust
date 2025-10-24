use crate::bridge::socket_bridge::SocketBridge;
use crate::bridge::PhpResponse;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct WorkerManager {
    bridge: Arc<SocketBridge>,
    max_workers: usize,
    active_requests: Arc<Mutex<usize>>,
}

impl WorkerManager {
    pub fn new(bridge: Arc<SocketBridge>, max_workers: usize) -> Arc<Self> {
        let manager = Arc::new(Self {
            bridge,
            max_workers,
            active_requests: Arc::new(Mutex::new(0)),
        });

        manager
    }

    pub async fn execute_command(
        &self,
        command: &str,
        data: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<PhpResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Увеличиваем счетчик активных запросов
        {
            let mut active_count = self.active_requests.lock().unwrap();
            *active_count += 1;
        }

        // Вызываем метод send_command у соответствующего моста
        let result = self.bridge.send_command(command, data).await;

        // Уменьшаем счетчик активных запросов
        {
            let mut active_count = self.active_requests.lock().unwrap();
            if *active_count > 0 {
                *active_count -= 1;
            }
        }

        result
    }

    pub fn get_stats(&self) -> HashMap<String, serde_json::Value> {
        let active_requests = *self.active_requests.lock().unwrap();

        let mut stats = HashMap::new();
        stats.insert(
            "active_requests".to_string(),
            serde_json::Value::Number(serde_json::Number::from(active_requests)),
        );
        stats.insert(
            "max_workers".to_string(),
            serde_json::Value::Number(serde_json::Number::from(self.max_workers)),
        );

        stats.insert(
            "bridge_type".to_string(),
            serde_json::Value::String("socket".to_string()),
        );
        stats.insert(
            "socket_path".to_string(),
            serde_json::Value::String(self.bridge.get_socket_path().to_string()),
        );

        stats
    }

    pub async fn restart_all_workers(&self) {
        // При соединении через сокет перезапуск не требуется
        println!("✅ Соединение готово к использованию");
    }
}
