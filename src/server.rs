use anyhow::Result;
use base64;
use hyper::service::{make_service_fn, service_fn};
use hyper::{header, Body, Request, Response, Server, StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};

use crate::bridge::socket_bridge::SocketBridge;

use crate::config::AppConfig;

/// Represents an HTTP request that will be forwarded to Laravel
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HttpRequestPayload {
    pub method: String,
    pub uri: String,
    pub headers: std::collections::HashMap<String, String>,
    pub body: Option<String>,
    pub query_params: std::collections::HashMap<String, String>,
}

/// Represents the response from Laravel
#[derive(Deserialize, Debug)]
pub struct HttpResponsePayload {
    pub status: u16,
    pub headers: std::collections::HashMap<String, String>,
    pub body: String,
}

/// Main HTTP server struct
pub struct HttpServer {
    config: crate::config::ServerConfig,
    socket_bridge: Arc<SocketBridge>,
}

impl HttpServer {
    /// Create a new HTTP server instance
    pub async fn new(
        socket_bridge: Arc<SocketBridge>,
    ) -> Result<Self> {
        dotenvy::dotenv().ok();
        let config = crate::config::ServerConfig::from_env()?;

        Ok(HttpServer { config, socket_bridge })
    }

    /// Create a new HTTP server instance with configuration
    pub async fn new_with_config(
        socket_bridge: Arc<SocketBridge>,
        app_config: &AppConfig,
    ) -> Result<Self> {
        Ok(HttpServer {
            config: app_config.server.clone(),
            socket_bridge
        })
    }

    /// Start the HTTP server
    pub async fn start(&self) -> Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| {
                error!("Failed to parse server address: {}", e);
                Box::new(e)
            })?;

        let socket_bridge = self.socket_bridge.clone();

        info!("🚀 Starting HTTP server on {}:{}", self.config.host, self.config.port);
        info!("🔌 Connecting to Laravel via Unix socket: {}", self.config.socket_path);

        let make_svc = make_service_fn(move |_conn| {
            let socket_bridge = socket_bridge.clone();

            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    let socket_bridge = socket_bridge.clone();
                    handle_request(req, socket_bridge)
                }))
            }
        });

        let server = Server::try_bind(&addr)
            .map_err(|e| {
                error!("Failed to bind to {}: {}", addr, e);
                Box::new(e)
            })?
            .serve(make_svc);

        server.await.map_err(|e| anyhow::Error::from(e))
    }
}

/// Handle incoming HTTP requests and forward them to Laravel
async fn handle_request(req: Request<Body>, socket_bridge: Arc<SocketBridge>) -> Result<Response<Body>, hyper::Error> {
    debug!("Received request: {} {}", req.method(), req.uri());

    // Check if this is a static file request (favicon.ico, assets, etc.)
    let uri_path = req.uri().path();
    if is_static_file_request(uri_path) {
        return handle_static_file_request(uri_path).await;
    }

    // Extract request data
    let method = req.method().clone();
    let uri = req.uri().clone();
    let headers = req.headers().clone();
    let body_bytes = hyper::body::to_bytes(req.into_body()).await
        .map_err(|e| {
            tracing::error!("Failed to read request body: {}", e);
            hyper::Error::from(e)
        })?;

    // Convert headers to HashMap
    let mut header_map = std::collections::HashMap::new();
    for (name, value) in headers.iter() {
        if let Ok(value_str) = value.to_str() {
            header_map.insert(name.as_str().to_string(), value_str.to_string());
        }
    }

    // Parse query parameters
    let query_params = extract_query_params(uri.query());

    // Create request payload for Laravel
    let payload = HttpRequestPayload {
        method: method.to_string(),
        uri: uri.to_string(),
        headers: header_map,
        body: if body_bytes.is_empty() {
            None
        } else {
            String::from_utf8(body_bytes.to_vec()).ok()
        },
        query_params,
    };

    // Send request to Laravel via Unix socket
    match forward_to_laravel(&socket_bridge, payload).await {
        Ok(response) => Ok(response),
        Err(e) => {
            error!("Error forwarding request to Laravel: {}", e);
            // Use the centralized error handler
            Ok(crate::errors::handle_error_response(e))
        }
    }
}

/// Check if the request is for a static file
fn is_static_file_request(uri_path: &str) -> bool {
    // Check if the URI path contains file extensions typical for static files
    let static_extensions = [
        ".ico", ".css", ".js", ".png", ".jpg", ".jpeg", ".gif", ".svg",
        ".woff", ".woff2", ".ttf", ".eot", ".pdf", ".txt", ".json",
        ".xml", ".map", ".webp", ".avif"
    ];
    
    for ext in &static_extensions {
        if uri_path.ends_with(ext) {
            return true;
        }
    }
    
    // Also handle common static file paths
    uri_path == "/favicon.ico" || uri_path.starts_with("/assets/") || uri_path.starts_with("/build/")
}

/// Handle static file requests
async fn handle_static_file_request(uri_path: &str) -> Result<Response<Body>, hyper::Error> {
    // Determine the file path relative to the public directory
    // In Laravel, static files are typically served from the public/ directory
    let file_path = if uri_path == "/favicon.ico" {
        // Special case for favicon.ico
        format!("../public{}", uri_path)
    } else {
        // For other static files, construct the path relative to public directory
        format!("../public{}", uri_path)
    };

    // Read the file
    match tokio::fs::read(&file_path).await {
        Ok(contents) => {
            // Determine the content type based on file extension
            let content_type = get_content_type(&file_path);
            
            let mut response = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CONTENT_LENGTH, contents.len());

            // Add caching headers for static assets
            if uri_path.starts_with("/build/") || uri_path.contains('.') && !uri_path.ends_with(".html") {
                // These are likely versioned assets that can be cached long-term
                response = response.header(header::CACHE_CONTROL, "public, max-age=31536000"); // 1 year
            } else {
                // Other assets might change more frequently
                response = response.header(header::CACHE_CONTROL, "public, max-age=86400"); // 1 day
            }

            Ok(response.body(Body::from(contents)).unwrap_or_else(|_| {
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from("Failed to create response"))
                    .unwrap()
            }))
        }
        Err(_) => {
            // File not found - return 404
            Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("File not found"))
                .unwrap_or_else(|_| {
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from("Failed to create response"))
                        .unwrap()
                }))
        }
    }
}

/// Determine content type based on file extension
fn get_content_type(file_path: &str) -> &'static str {
    let extension = std::path::Path::new(file_path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "application/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" => "text/plain",
        "ico" => "image/vnd.microsoft.icon",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "eot" => "application/vnd.ms-fontobject",
        "pdf" => "application/pdf",
        _ => "application/octet-stream", // Default binary type
    }
}

/// Forward the request to Laravel via Unix socket
async fn forward_to_laravel(
    socket_bridge: &Arc<SocketBridge>,
    payload: HttpRequestPayload,
) -> Result<Response<Body>> {
    // Create a direct HTTP request format that matches what PHP expects
    let http_request_data = serde_json::json!({
        "uri": payload.uri.clone(),
        "method": payload.method.clone(),
        "headers": payload.headers.clone(),
        "parameters": payload.query_params.clone(),
        "content": payload.body.clone(),
        "server": {
            "REQUEST_METHOD": payload.method.clone(),
            "REQUEST_URI": payload.uri.clone(),
            "CONTENT_TYPE": payload.headers.get("content-type").unwrap_or(&"".to_string()).clone(),
            "CONTENT_LENGTH": payload.body.as_ref().map(|b| b.len().to_string()).unwrap_or("0".to_string())
        }
    });

    // Send HTTP request data directly (not as a command)
    let response = socket_bridge.send_http_request(http_request_data).await;

    match response {
        Ok(response) => {
            // Process the response from Laravel
            match response.success {
                true => {
                    if let Some(response_data) = response.data {
                        // Parse Laravel's response - it might be in the format:
                        // {"body": "...", "headers": {...}, "status": 200}
                        let http_response: HttpResponsePayload = parse_laravel_response(response_data).unwrap_or_else(|e| {
                            error!("Failed to parse Laravel response: {}", e);

                            // Fallback for other response formats
                            HttpResponsePayload {
                                status: 200,
                                headers: std::collections::HashMap::new(),
                                body: format!("Error parsing Laravel response: {}", e),
                            }
                        });

                        // Determine content type and handle response body appropriately
                        let content_type = http_response
                            .headers
                            .get("content-type")
                            .or(http_response.headers.get("Content-Type"))
                            .and_then(|ct| ct.split(';').next()) // Extract main content type, ignore parameters like charset
                            .unwrap_or("text/html")
                            .to_lowercase();

                        let response_body = if content_type.contains("application/json") {
                            // For JSON responses, ensure proper formatting and validate JSON
                            match serde_json::from_str::<serde_json::Value>(&http_response.body) {
                                Ok(json_value) => {
                                    // The response is valid JSON, use it as-is
                                    Body::from(
                                        serde_json::to_string(&json_value)
                                            .map_err(|e| anyhow::anyhow!("Failed to serialize JSON response: {}", e))?,
                                    )
                                }
                                Err(_) => {
                                    // The response claims to be JSON but is not valid JSON, return as-is
                                    Body::from(http_response.body)
                                }
                            }
                        } else if content_type.contains("text/") || content_type.contains("application/javascript") {
                            // For text-based responses, return as-is
                            Body::from(http_response.body)
                        } else if content_type.contains("application/octet-stream")
                            || content_type.contains("image/")
                            || content_type.contains("audio/")
                            || content_type.contains("video/")
                        {
                            // For binary responses, we need to handle the body differently
                            // If the body is base64 encoded, we should decode it
                            match base64::Engine::decode(
                                &base64::engine::general_purpose::STANDARD,
                                &http_response.body,
                            ) {
                                Ok(decoded_bytes) => Body::from(decoded_bytes),
                                Err(_) => Body::from(http_response.body), // If not base64, treat as string
                            }
                        } else {
                            // For other content types, return as-is
                            Body::from(http_response.body)
                        };

                        // Build response
                        let mut response_builder = Response::builder()
                            .status(StatusCode::from_u16(http_response.status)
                                .map_err(|_| anyhow::anyhow!("Invalid status code: {}", http_response.status))?);

                        // Add headers
                        for (key, value) in http_response.headers {
                            match hyper::header::HeaderName::from_bytes(key.as_bytes()) {
                                Ok(header_name) => {
                                    // Убираем потенциальные символы новой строки или пробелы в значениях заголовков
                                    let clean_value = value.trim().to_string();
                                    if !clean_value.is_empty() {
                                        response_builder = response_builder.header(header_name, clean_value);
                                    }
                                }
                                Err(_) => {
                                    // If header name is invalid, log and continue
                                    tracing::warn!("Invalid header name: {}", key);
                                }
                            }
                        }

                        Ok(response_builder.body(response_body)?)
                    } else {
                        // When response.data is None, return error response if available
                        if let Some(error_msg) = response.error {
                            Ok(Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .body(Body::from(error_msg))?)
                        } else {
                            // If no data and no error, return a default response
                            Ok(Response::builder()
                                .status(StatusCode::OK)
                                .body(Body::from("Laravel returned empty response"))?)
                        }
                    }
                }
                false => {
                    let error_msg = response
                        .error
                        .unwrap_or_else(|| "Unknown error from Laravel".to_string());
                    Ok(Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from(error_msg))?)
                }
            }
        }
        Err(e) => {
            error!("Failed to connect to Laravel socket: {}", e);
            // Provide more detailed error information
            let error_msg = format!("Service Unavailable - Laravel backend not responding. Error: {}", e);
            Ok(Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body(Body::from(error_msg))?)
        }
    }
}

/// Parse Laravel response format
fn parse_laravel_response(
    response_data: serde_json::Value,
) -> Result<HttpResponsePayload> {
    // Check if response_data has the format: {"body": "...", "headers": {...}, "status": 200}
    if let Some(obj) = response_data.as_object() {
        // Check if it has the expected format with body, headers, and status
        if obj.contains_key("body") && obj.contains_key("headers") && obj.contains_key("status") {
            let body = obj.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();

            let status = obj.get("status").and_then(|v| v.as_u64()).unwrap_or(200) as u16;

            let mut headers = std::collections::HashMap::new();
            if let Some(headers_val) = obj.get("headers").and_then(|v| v.as_object()) {
                for (key, value) in headers_val {
                    // Laravel возвращает заголовки как массивы значений, берем первое значение
                    if let Some(arr) = value.as_array() {
                        if let Some(first_val) = arr.first() {
                            if let Some(str_val) = first_val.as_str() {
                                headers.insert(key.clone(), str_val.to_string());
                            } else {
                                headers.insert(key.clone(), first_val.to_string());
                            }
                        } else {
                            // Если массив пуст, добавляем пустую строку
                            headers.insert(key.clone(), String::new());
                        }
                    } else if let Some(str_val) = value.as_str() {
                        headers.insert(key.clone(), str_val.to_string());
                    } else {
                        // Если значение не массив и не строка, преобразуем в строку
                        headers.insert(key.clone(), value.to_string());
                    }
                }
            }

            return Ok(HttpResponsePayload { status, headers, body });
        }

        // Check if it has a "status" field but different structure (like direct Laravel HTTP response)
        if obj.contains_key("status") {
            let status = obj.get("status").and_then(|v| v.as_u64()).unwrap_or(200) as u16;

            // Try to get body from various possible fields
            let body = if let Some(body_val) = obj.get("body") {
                if let Some(body_str) = body_val.as_str() {
                    body_str.to_string()
                } else {
                    serde_json::to_string(body_val).unwrap_or_else(|_| "{}".to_string())
                }
            } else {
                // If no explicit body, serialize the entire object as fallback
                serde_json::to_string(&response_data).unwrap_or_else(|_| "{}".to_string())
            };

            // Get headers if they exist
            let mut headers = std::collections::HashMap::new();
            if let Some(headers_val) = obj.get("headers").and_then(|v| v.as_object()) {
                for (key, value) in headers_val {
                    // Laravel может возвращать заголовки как массивы значений
                    if let Some(arr) = value.as_array() {
                        if let Some(first_val) = arr.first() {
                            if let Some(str_val) = first_val.as_str() {
                                headers.insert(key.clone(), str_val.to_string());
                            } else {
                                headers.insert(key.clone(), first_val.to_string());
                            }
                        } else {
                            // Если массив пуст, добавляем пустую строку
                            headers.insert(key.clone(), String::new());
                        }
                    } else if let Some(str_val) = value.as_str() {
                        headers.insert(key.clone(), str_val.to_string());
                    } else {
                        headers.insert(key.clone(), value.to_string());
                    }
                }
            }

            return Ok(HttpResponsePayload { status, headers, body });
        }

        // Check if it's a response from Laravel that has "originalContent" or other fields
        // Some Laravel responses might have different field names
        if obj.contains_key("originalContent") {
            // This looks like a Laravel response object
            let body = obj
                .get("originalContent")
                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string()))
                .unwrap_or_else(|| "{}".to_string());

            return Ok(HttpResponsePayload {
                status: 200,
                headers: std::collections::HashMap::new(),
                body,
            });
        }
    }

    // Laravel может возвращать ответы в других форматах, например:
    // 1. Просто строку или JSON как тело ответа
    // 2. Массив с информацией об ответе
    // 3. Объект с другими полями

    // Если это строка, возвращаем как тело с 200 статусом
    if let Some(body_str) = response_data.as_str() {
        return Ok(HttpResponsePayload {
            status: 200,
            headers: std::collections::HashMap::new(),
            body: body_str.to_string(),
        });
    }

    // Если это число, преобразуем в строку
    if response_data.is_number() {
        return Ok(HttpResponsePayload {
            status: 200,
            headers: std::collections::HashMap::new(),
            body: response_data.to_string(),
        });
    }

    // Если это булевое значение
    if response_data.is_boolean() {
        return Ok(HttpResponsePayload {
            status: 200,
            headers: std::collections::HashMap::new(),
            body: response_data.to_string(),
        });
    }

    // Для всех остальных случаев, включая объекты без ожидаемых полей
    // возвращаем сериализованный JSON как тело с 200 статусом
    Ok(HttpResponsePayload {
        status: 200,
        headers: std::collections::HashMap::new(),
        body: serde_json::to_string(&response_data).unwrap_or_else(|_| "{}".to_string()),
    })
}

/// Extract query parameters from URI
fn extract_query_params(query: Option<&str>) -> std::collections::HashMap<String, String> {
    let mut params = std::collections::HashMap::new();

    if let Some(query_str) = query {
        for pair in query_str.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                params.insert(
                    urlencoding::decode(key).unwrap_or_else(|_| key.into()).to_string(),
                    urlencoding::decode(value).unwrap_or_else(|_| value.into()).to_string(),
                );
            } else if !pair.is_empty() {
                params.insert(
                    urlencoding::decode(pair).unwrap_or_else(|_| pair.into()).to_string(),
                    String::new(),
                );
            }
        }
    }

    params
}

/// Create an internal server error response
fn internal_server_error() -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from("Internal Server Error"))
        .unwrap_or_else(|_| {
            // Fallback response in case the builder fails
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Internal Server Error"))
                .unwrap() // This should never panic as we're using valid status and body
        })
}
