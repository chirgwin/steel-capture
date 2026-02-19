use crate::types::{CaptureFrame, CompactFrame};
use crossbeam_channel::Receiver;
use log::{error, info, warn};
use sha1_smol::Sha1;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Combined HTTP + WebSocket server.
///
/// - `GET /` or `GET /visualization.html` → serves the viz page
/// - WebSocket upgrade → streams CaptureFrame JSON at throttled rate
///
/// Single port, no separate HTTP server needed.
pub struct WsServer {
    frame_rx: Receiver<CaptureFrame>,
    addr: String,
    target_fps: u32,
    viz_path: PathBuf,
}

struct WsClient {
    stream: TcpStream,
    alive: bool,
}

impl WsClient {
    fn new(stream: TcpStream) -> Self {
        let _ = stream.set_nonblocking(true);
        let _ = stream.set_nodelay(true);
        Self {
            stream,
            alive: true,
        }
    }

    fn send_text(&mut self, text: &str) -> bool {
        let payload = text.as_bytes();
        let len = payload.len();
        let mut frame = Vec::with_capacity(10 + len);
        frame.push(0x81); // FIN + text opcode
        if len < 126 {
            frame.push(len as u8);
        } else if len < 65536 {
            frame.push(126);
            frame.push((len >> 8) as u8);
            frame.push((len & 0xFF) as u8);
        } else {
            frame.push(127);
            for i in (0..8).rev() {
                frame.push(((len >> (i * 8)) & 0xFF) as u8);
            }
        }
        frame.extend_from_slice(payload);
        match self.stream.write_all(&frame) {
            Ok(()) => true,
            Err(_) => {
                self.alive = false;
                false
            }
        }
    }
}

type ClientList = Arc<Mutex<Vec<WsClient>>>;

/// Parsed HTTP request — enough to decide WS vs HTTP.
struct HttpRequest {
    path: String,
    is_upgrade: bool,
    ws_key: Option<String>,
}

fn parse_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut path = String::from("/");
    let mut is_upgrade = false;
    let mut ws_key = None;
    let mut first = true;

    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            break;
        }
        if first {
            // Parse "GET /path HTTP/1.1"
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                path = parts[1].to_string();
            }
            first = false;
        }
        let lower = trimmed.to_lowercase();
        if lower.starts_with("upgrade:") && lower.contains("websocket") {
            is_upgrade = true;
        }
        if lower.starts_with("sec-websocket-key:") {
            ws_key = Some(trimmed[18..].trim().to_string());
        }
    }
    Ok(HttpRequest {
        path,
        is_upgrade,
        ws_key,
    })
}

fn ws_handshake(stream: &mut TcpStream, key: &str) -> Result<(), String> {
    let magic = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = Sha1::new();
    hasher.update(format!("{}{}", key, magic).as_bytes());
    let hash = hasher.digest().bytes();
    let accept = base64_encode(&hash);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\
         \r\n",
        accept
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|e| e.to_string())
}

fn serve_html(stream: &mut TcpStream, content: &[u8]) {
    serve_static(stream, content, "text/html; charset=utf-8");
}

fn serve_static(stream: &mut TcpStream, content: &[u8], content_type: &str) {
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         Cache-Control: no-cache\r\n\
         \r\n",
        content_type,
        content.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(content);
}

fn content_type_for(path: &str) -> &'static str {
    if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".otf") {
        "font/otf"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

fn serve_404(stream: &mut TcpStream) {
    let body = b"<h1>404</h1><p>Open <a href=\"/\">/</a> for visualization</p>";
    let header = format!(
        "HTTP/1.1 404 Not Found\r\n\
         Content-Type: text/html\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < data.len() {
            data[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < data.len() {
            data[i + 2] as u32
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if i + 1 < data.len() {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if i + 2 < data.len() {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        i += 3;
    }
    result
}

impl WsServer {
    pub fn new(
        frame_rx: Receiver<CaptureFrame>,
        addr: String,
        target_fps: u32,
        viz_path: PathBuf,
    ) -> Self {
        Self {
            frame_rx,
            addr,
            target_fps,
            viz_path,
        }
    }

    pub fn run(self) {
        let clients: ClientList = Arc::new(Mutex::new(Vec::new()));

        // Pre-load the visualization HTML
        let viz_html = match fs::read(&self.viz_path) {
            Ok(data) => {
                info!(
                    "Loaded visualization: {} ({} bytes)",
                    self.viz_path.display(),
                    data.len()
                );
                Arc::new(data)
            }
            Err(e) => {
                warn!(
                    "Could not load {}: {} — HTTP serving disabled",
                    self.viz_path.display(),
                    e
                );
                Arc::new(Vec::new())
            }
        };

        // Base directory for serving static assets (siblings of visualization.html)
        let base_dir: Arc<PathBuf> = Arc::new(
            self.viz_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf(),
        );

        // Spawn acceptor thread
        let accept_clients = clients.clone();
        let addr = self.addr.clone();
        let html = viz_html.clone();
        let static_dir = base_dir.clone();
        thread::Builder::new()
            .name("ws-accept".into())
            .spawn(move || {
                let listener = match TcpListener::bind(&addr) {
                    Ok(l) => l,
                    Err(e) => {
                        error!("Server failed to bind {}: {}", addr, e);
                        return;
                    }
                };
                info!("Server listening on http://{}", addr);
                info!("  Open http://{} in your browser", addr);

                for stream in listener.incoming() {
                    match stream {
                        Ok(mut stream) => {
                            let html2 = html.clone();
                            let cl = accept_clients.clone();
                            let sdir = static_dir.clone();
                            // Handle each connection in a short-lived thread
                            // (HTTP connections close immediately; WS connections
                            //  get moved to the client list)
                            thread::spawn(move || {
                                match parse_request(&mut stream) {
                                    Ok(req) if req.is_upgrade => {
                                        if let Some(key) = req.ws_key {
                                            match ws_handshake(&mut stream, &key) {
                                                Ok(()) => {
                                                    info!("WebSocket client connected");
                                                    cl.lock().unwrap().push(WsClient::new(stream));
                                                }
                                                Err(e) => warn!("WS handshake failed: {}", e),
                                            }
                                        }
                                    }
                                    Ok(req) => {
                                        // Serve HTTP
                                        match req.path.as_str() {
                                            "/" | "/visualization.html" | "/index.html" => {
                                                if html2.is_empty() {
                                                    serve_404(&mut stream);
                                                } else {
                                                    serve_html(&mut stream, &html2);
                                                }
                                            }
                                            path => {
                                                // Serve static files from the viz directory
                                                // Sanitize: strip leading /, reject path traversal
                                                let clean = path.trim_start_matches('/');
                                                if clean.contains("..") || clean.contains('\\') {
                                                    serve_404(&mut stream);
                                                } else {
                                                    let file_path = sdir.join(clean);
                                                    match fs::read(&file_path) {
                                                        Ok(data) => {
                                                            let ct = content_type_for(clean);
                                                            serve_static(&mut stream, &data, ct);
                                                        }
                                                        Err(_) => serve_404(&mut stream),
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => warn!("Request parse error: {}", e),
                                }
                            });
                        }
                        Err(e) => warn!("TCP accept error: {}", e),
                    }
                }
            })
            .unwrap();

        // Broadcast loop — accumulate attacks across throttled frames
        let frame_interval = Duration::from_micros(1_000_000 / self.target_fps as u64);
        let mut last_send = Instant::now();
        let mut pending_attacks = [false; 10]; // OR-accumulate attacks between sends

        for frame in self.frame_rx.iter() {
            // Latch any attacks from this frame
            for (i, pending) in pending_attacks.iter_mut().enumerate() {
                if frame.attacks[i] {
                    *pending = true;
                }
            }

            let now = Instant::now();
            if now.duration_since(last_send) < frame_interval {
                continue; // Skip broadcast but attacks are latched
            }
            last_send = now;

            // Merge latched attacks into the frame we're about to send
            let mut send_frame = frame.clone();
            for (i, &pending) in pending_attacks.iter().enumerate() {
                if pending {
                    send_frame.attacks[i] = true;
                }
            }
            pending_attacks = [false; 10]; // Clear after broadcast

            let compact = CompactFrame::from(&send_frame);
            let json = match serde_json::to_string(&compact) {
                Ok(j) => j,
                Err(e) => {
                    warn!("JSON serialize error: {}", e);
                    continue;
                }
            };

            let mut cl = clients.lock().unwrap();
            for client in cl.iter_mut() {
                client.send_text(&json);
            }
            cl.retain(|c| c.alive);
        }
    }
}
