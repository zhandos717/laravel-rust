#![warn(unused_imports)]
#![warn(unused_variables)]
#![warn(unused_mut)]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::os::unix::net::UnixStream;

mod bridge;
mod server;
use server::HttpServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Устанавливаем обработчик сигналов для корректного завершения
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
        println!("Получен сигнал завершения, останавливаем сервисы...");
    })
    .expect("Ошибка при установке обработчика сигналов");

    println!("🚀 Запускаем Laravel Rust Bridge...");

    // Запускаем PHP worker в отдельном процессе
    let php_process_result = start_php_worker();
    match &php_process_result {
        Ok(_) => println!("✅ PHP worker запущен"),
        Err(e) => eprintln!("❌ Ошибка запуска PHP worker: {}", e),
    }

    // Проверяем, что сокет создан и готов к использованию
    let socket_path = std::env::var("SOCKET_PATH").unwrap_or_else(|_| "/tmp/rust_php_bridge.sock".to_string());
    let mut attempts = 0;
    let max_attempts = 20; // 10 seconds max wait
    
    println!("⏳ Ожидаем готовности PHP worker и сокета...");
    while attempts < max_attempts {
        if std::path::Path::new(&socket_path).exists() {
            // Проверяем, можно ли подключиться к сокету
            match std::os::unix::net::UnixStream::connect(&socket_path) {
                Ok(_) => {
                    println!("✅ Сокет PHP worker готов к использованию");
                    break;
                }
                Err(_) => {
                    // Сокет существует, но не готов к подключению, ждем
                    thread::sleep(Duration::from_millis(500));
                    attempts += 1;
                }
            }
        } else {
            thread::sleep(Duration::from_millis(500));
            attempts += 1;
        }
    }
    
    if attempts >= max_attempts {
        eprintln!("⚠️ PHP worker не готов к подключению в течение 10 секунд");
    }

    // Создаем и запускаем Rust HTTP сервер
    let socket_bridge = match crate::bridge::socket_bridge::SocketBridge::new() {
        Ok(bridge) => bridge,
        Err(e) => {
            eprintln!("Ошибка инициализации SocketBridge: {}", e);
            return Err(e.into());
        }
    };

    let server = match HttpServer::new(socket_bridge.clone()).await {
        Ok(server) => server,
        Err(e) => {
            eprintln!("Ошибка инициализации HTTP сервера: {}", e);
            return Err(e.into());
        }
    };
    println!("✅ Rust HTTP сервер готов к работе");

    // Запускаем HTTP сервер
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.start().await {
            eprintln!("Ошибка в HTTP сервере: {}", e);
            std::process::exit(1);
        }
    });

    // Ждем сигнал завершения
    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }

    // Завершаем PHP процесс
    if let Ok(mut proc) = php_process_result {
        println!("🛑 Останавливаем PHP worker...");
        let _ = proc.kill();
        let _ = proc.wait();
    }

    // Завершаем сервер
    println!("🛑 Останавливаем Rust HTTP сервер...");

    // Ждем завершения сервера
    let _ = server_handle.await;

    // Очищаем соединения в SocketBridge
    socket_bridge.cleanup().await;

    Ok(())
}

fn start_php_worker() -> Result<std::process::Child, Box<dyn std::error::Error + Send + Sync + 'static>> {
    // Получаем путь к PHP из переменной окружения или используем стандартный
    let php_path = std::env::var("PHP_PATH").unwrap_or_else(|_| "php".to_string());
    
    // Получаем путь к Laravel проекту
    let laravel_path = std::env::var("LARAVEL_PATH").unwrap_or_else(|_| {
        // Если LARAVEL_PATH не задан, используем родительскую директорию от текущей (rust-runtime)
        let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        current_dir.parent().unwrap_or(&current_dir).to_string_lossy().to_string()
    });
    
    let artisan_path = std::path::Path::new(&laravel_path).join("artisan");

    if !artisan_path.exists() {
        return Err(format!("Файл artisan не найден по пути: {:?}", artisan_path).into());
    }

    // Запускаем PHP artisan worker:serve
    let mut cmd = Command::new(&php_path);
    cmd.arg(&artisan_path).arg("worker:serve").current_dir(&laravel_path); // Устанавливаем директорию в корень Laravel проекта

    let child = cmd
        .spawn()
        .map_err(|e| format!("Ошибка при запуске PHP worker: {}", e))?;

    Ok(child)
}
