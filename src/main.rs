//! # Laravel Rust Bridge
//!
//! Этот модуль реализует мост между Laravel (PHP) и Rust, позволяя
//! обрабатывать HTTP-запросы через Rust-сервер, который взаимодействует
//! с PHP-приложением Laravel через Unix-сокет.

use anyhow::Result;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

mod bridge;
mod server;
mod errors;
mod config;
use server::HttpServer;
use config::AppConfig;

// Константы для конфигурации (для обратной совместимости)
const DEFAULT_SOCKET_PATH: &str = "/tmp/rust_php_bridge.sock";
const SOCKET_WAIT_MAX_ATTEMPTS: usize = 20;
const SOCKET_WAIT_INTERVAL_MS: u64 = 500;
const SHUTDOWN_CHECK_INTERVAL_MS: u64 = 100;

#[tokio::main]
async fn main() -> Result<()> {
    // Инициализируем систему логирования
    init_logging()?;

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

    // Загружаем конфигурацию приложения
    let config = match AppConfig::from_env() {
        Ok(config) => {
            if let Err(validation_err) = config.validate() {
                eprintln!("❌ Ошибка валидации конфигурации: {}", validation_err);
                return Err(validation_err);
            }
            config
        }
        Err(e) => {
            eprintln!("❌ Ошибка загрузки конфигурации: {}", e);
            return Err(e);
        }
    };

    // Проверяем, что сокет создан и готов к использованию
    let _ = wait_for_php_worker(&config.connection.socket_path);

    // Создаем и запускаем Rust HTTP сервер
    let socket_bridge = match crate::bridge::socket_bridge::SocketBridge::new_with_config(&config) {
        Ok(bridge) => bridge,
        Err(e) => {
            eprintln!("Ошибка инициализации SocketBridge: {}", e);
            return Err(e.into());
        }
    };
    println!("✅ Rust HTTP сервер готов к работе");

    let server = match HttpServer::new_with_config(socket_bridge.clone(), &config).await {
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
        thread::sleep(config.connection.shutdown_check_interval);
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

/// Инициализация системы логирования с поддержкой записи в файл
///
/// Настраивает логирование в файл и в консоль с возможностью фильтрации
/// по уровням и сохранения в директорию, указанную в переменных окружения.
///
/// # Returns
///
/// * `Ok(())` - если логирование успешно инициализировано
/// * `Err` - если произошла ошибка при настройке логирования
fn init_logging() -> Result<()> {
    use std::fs;
    use tracing_subscriber::fmt;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
    use tracing_subscriber::Layer;
    use tracing_subscriber::util::SubscriberInitExt;

    // Загружаем переменные окружения
    dotenvy::dotenv().ok();

    // Получаем уровень логирования из переменной окружения
    let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

    // Получаем директорию для логов из переменной окружения
    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "./logs".to_string());

    // Создаем директорию для логов, если она не существует
    fs::create_dir_all(&log_dir)?;

    // Создаем файл для логов
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open(Path::new(&log_dir).join("server.log"))?;

    // Настройка фильтрации по уровню логирования
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or(EnvFilter::new(&format!("laravel-rust-server={},hyper=info", log_level)));

    // Настройка форматирования логов в файл
    let file_layer = fmt::layer()
        .with_writer(log_file)
        .with_ansi(false) // Отключаем цвета в файле
        .with_target(true)
        .with_line_number(true)
        .with_filter(env_filter.clone()); // Клонируем фильтр для использования в нескольких слоях

    // Настройка консольного вывода
    let stdout_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(true)
        .with_line_number(true)
        .with_filter(env_filter); // Используем оригинальный фильтр

    // Инициализируем глобальный subscriber с обеими записями
    tracing_subscriber::registry()
        .with(file_layer)
        .with(stdout_layer)
        .init();

    Ok(())
}

/// Ожидание готовности PHP worker
///
/// Проверяет существование Unix-сокета и возможность подключения к нему
/// в течение определенного времени.
///
/// # Arguments
///
/// * `socket_path` - путь к Unix-сокету, который использует PHP worker
///
/// # Returns
///
/// * `Ok())` - если сокет готов к использованию
/// * `Err` - если сокет не готов в течение отведенного времени
fn wait_for_php_worker(socket_path: &str) -> Result<()> {
    let mut attempts = 0;
    
    // Для обратной совместимости используем конфигурацию по умолчанию
    let max_attempts = std::env::var("SOCKET_WAIT_MAX_ATTEMPTS")
        .unwrap_or_else(|_| "10".to_string())  // Reduced from 20 to 10
        .parse()
        .unwrap_or(10);
    let interval = std::env::var("SOCKET_WAIT_INTERVAL_MS")
        .unwrap_or_else(|_| "250".to_string())  // Reduced from 50 to 250ms
        .parse()
        .unwrap_or(250);

    println!("⏳ Ожидаем готовности PHP worker и сокета...");
    while attempts < max_attempts {
        if std::path::Path::new(socket_path).exists() {
            // Проверяем, можно ли подключиться к сокету
            match std::os::unix::net::UnixStream::connect(socket_path) {
                Ok(_) => {
                    println!("✅ Сокет PHP worker готов к использованию");
                    return Ok(());
                }
                Err(_) => {
                    // Сокет существует, но не готов к подключению, ждем
                    thread::sleep(Duration::from_millis(interval));
                    attempts += 1;
                }
            }
        } else {
            thread::sleep(Duration::from_millis(interval));
            attempts += 1;
        }
    }

    eprintln!("⚠️ PHP worker не готов к подключению в течение {} секунд", (max_attempts * interval) / 1000);
    Err(anyhow::anyhow!("PHP worker не готов к подключению"))
}

/// Запуск PHP worker процесса
///
/// Запускает PHP процесс с Laravel artisan командой, которая создает
/// сервер для обработки запросов из Rust.
///
/// # Returns
///
/// * `Ok(Child)` - дескриптор дочернего процесса PHP worker
/// * `Err` - ошибка запуска процесса
fn start_php_worker() -> Result<std::process::Child> {
    // Получаем путь к PHP из переменной окружения или используем стандартный
    let php_path = std::env::var("PHP_PATH").unwrap_or_else(|_| "php".to_string());

    // Получаем путь к Laravel проекту
    let laravel_path = std::env::var("LARAVEL_PATH").unwrap_or_else(|_| {
        // Если LARAVEL_PATH не задан, используем родительскую директорию от текущей (rust-runtime)
        let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        current_dir
            .parent()
            .unwrap_or(&current_dir)
            .to_string_lossy()
            .to_string()
    });

    let artisan_path = std::path::Path::new(&laravel_path).join("artisan");

    if !artisan_path.exists() {
        return Err(anyhow::anyhow!("Файл artisan не найден по пути: {:?}", artisan_path));
    }

    // Получаем команду запуска из переменной окружения
    let startup_command = std::env::var("STARTUP_COMMAND").unwrap_or_else(|_| "laravel-rust:serve".to_string());

    // Запускаем PHP artisan с командой из переменной окружения
    let mut cmd = Command::new(&php_path);
    cmd.arg(&artisan_path).arg(&startup_command).current_dir(&laravel_path); // Устанавливаем директорию в корень Laravel проекта

    let child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Ошибка при запуске PHP worker: {}", e))?;

    Ok(child)
}
