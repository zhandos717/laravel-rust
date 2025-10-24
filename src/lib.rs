mod bridge;

pub use bridge::worker_manager::WorkerManager;

// Основной модуль для интеграции с Laravel
pub mod laravel_integration {
    pub struct LaravelRustServer {}

    impl LaravelRustServer {
        pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
            Ok(Self {})
        }
    }

    // Функции для вызова из PHP/Laravel
    pub extern "C" fn init_server() -> *mut LaravelRustServer {
        match LaravelRustServer::new() {
            Ok(server) => {
                let boxed_server = Box::new(server);
                Box::into_raw(boxed_server)
            }
            Err(_) => std::ptr::null_mut(),
        }
    }

    pub extern "C" fn destroy_server(server: *mut LaravelRustServer) {
        if !server.is_null() {
            unsafe {
                let _ = Box::from_raw(server);
            }
        }
    }
}
