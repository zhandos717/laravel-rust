# Laravel Rust Bridge Server

High-performance HTTP server written in Rust that acts as a bridge between incoming HTTP requests and Laravel applications via Unix socket communication.

## Architecture Overview

The system consists of three main components:

1. **Rust HTTP Server**: Listens on port 8080 (configurable), receives HTTP requests, and forwards them to Laravel via Unix socket
2. **Unix Socket Bridge**: Provides efficient communication between Rust and PHP processes
3. **Laravel Socket Handler**: PHP script that handles requests from Rust and processes them through Laravel's framework

```
HTTP Client → Rust HTTP Server → Unix Socket → Laravel Socket Handler → Laravel Application → Response Flow Reverses
```

## Features

- **High Performance**: Uses async/await with Tokio runtime for maximum throughput
- **Unix Socket Communication**: Zero-copy communication between Rust and PHP
- **Configurable**: Environment variables for all settings
- **Logging**: Comprehensive tracing with different log levels
- **Error Handling**: Robust error handling and recovery mechanisms
- **Hot Reload**: Automatic restart of Laravel processes if they crash

## Prerequisites

- Rust (1.70+)
- PHP (7.4+)
- Laravel application

## Installation

1. Clone the repository
2. Install Rust dependencies:
   ```bash
   cd rust-runtime
   cargo build
   ```

3. Configure environment variables in `.env`:
   ```env
   HTTP_PORT=8080
   HTTP_HOST=127.0.0.1
   SOCKET_PATH=/tmp/rust_php_bridge.sock
   PHP_PATH=/usr/bin/php
   LARAVEL_PATH=/path/to/your/laravel/app
   LOG_LEVEL=info
   ```

## Usage

### Starting the Servers

1. First, start the Laravel socket handler:
   ```bash
   php rust-runtime/php_socket_handler.php
   ```

2. Then start the Rust HTTP server:
   ```bash
   cd rust-runtime
   cargo run
   ```

### Making Requests

Once both servers are running, you can make HTTP requests to the Rust server:

```bash
curl -X GET http://localhost:8080/api/users
curl -X POST http://localhost:8080/api/users -d '{"name": "John", "email": "john@example.com"}'
```

## Configuration

All configuration is handled through environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `HTTP_PORT` | 8080 | Port for the Rust HTTP server |
| `HTTP_HOST` | 127.0.0.1 | Host for the Rust HTTP server |
| `SOCKET_PATH` | /tmp/rust_php_bridge.sock | Path to Unix socket file |
| `PHP_PATH` | php | Path to PHP executable |
| `LARAVEL_PATH` | Current directory | Path to Laravel application |
| `LOG_LEVEL` | info | Logging level (trace, debug, info, warn, error) |
| `LOG_DIR` | ./logs | Directory for log files |
| `STARTUP_COMMAND` | laravel-rust:serve | Laravel Artisan command to start the PHP worker |
| `SOCKET_POOL_MIN` | 2 | Minimum number of connections in the pool |
| `SOCKET_POOL_MAX` | 10 | Maximum number of connections in the pool |
| `SOCKET_CONNECTION_TIMEOUT` | 5 | Connection timeout in seconds |
| `SOCKET_HEALTH_CHECK_INTERVAL` | 30 | Health check interval in seconds |

## Performance Optimizations

- **Async I/O**: Non-blocking operations for maximum throughput
- **Connection Pooling**: Reuse connections where possible
- **Buffering**: Efficient data buffering to minimize system calls
- **Zero-copy**: Unix sockets provide zero-copy data transfer between processes

## Error Handling

The system handles various error scenarios:

- Socket connection failures
- Laravel process crashes
- Invalid request formats
- Timeout handling
- Resource cleanup

## Security Considerations

- Unix sockets are used for IPC (more secure than TCP)
- Input validation on both Rust and PHP sides
- Proper error message sanitization
- File permission restrictions on socket files

## Development

To run tests:
```bash
cargo test
```

To build for production:
```bash
cargo build --release
```

## Troubleshooting

### Common Issues

1. **Socket file already exists**:
   - Solution: Remove the socket file manually or restart the system

2. **Permission denied for socket file**:
   - Solution: Check file permissions and user access rights

3. **Laravel not responding**:
   - Solution: Verify that the Laravel socket handler is running

### Debugging

Enable debug logging by setting `LOG_LEVEL=debug` in your environment.

## Future Enhancements

- TLS/SSL support
- Request/response compression
- Advanced caching mechanisms
- Metrics and monitoring endpoints
- Connection pooling improvements
- Request rate limiting

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Submit a pull request

## License

MIT License - see the LICENSE file for details.