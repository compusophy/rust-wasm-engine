// Hand-rolled HTTP server replacing the Python dev server. Serves static
// files (index.html, game.wasm, models/*) and a single /sync endpoint for
// multiplayer heartbeat + broadcast. No crates.
//
// Protocol:
//   GET /sync?id=ID&name=N&class=C&x=X&y=Y&z=Z&yaw=Y&speed=S
//     Upserts the caller's state and returns a plain-text body of all *other*
//     connected players, one per line:
//         id|name|class|x|y|z|yaw|speed|anim
//     Players that haven't heartbeat in 5s are evicted.
//   GET /health -> "ok"
//   GET /<file> -> static file relative to CWD (must be project root).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Player {
    name: String,
    class_: u8,
    x: f32, y: f32, z: f32,
    yaw: f32,
    speed: f32,
    anim: String,
    last_seen: Instant,
}

type Players = Arc<Mutex<HashMap<String, Player>>>;

fn main() {
    let players: Players = Arc::new(Mutex::new(HashMap::new()));
    // Railway / Fly / Heroku / Render all inject PORT at runtime. Fall back to
    // :8000 for local dev.
    let port: u16 = std::env::var("PORT").ok()
        .and_then(|s| s.parse().ok()).unwrap_or(8000);
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| {
        eprintln!("bind {addr}: {e}");
        std::process::exit(1);
    });
    println!("server on http://0.0.0.0:{port}  (ctrl-c to stop)");

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let p = players.clone();
        thread::spawn(move || {
            if let Err(e) = handle(stream, p) {
                let _ = e;  // silent drop — client may have disconnected
            }
        });
    }
}

fn handle(mut stream: TcpStream, players: Players) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    // Read request headers (bounded).
    let mut req = Vec::with_capacity(1024);
    let mut buf = [0u8; 1024];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 { break; }
        req.extend_from_slice(&buf[..n]);
        // Headers are \r\n\r\n terminated; if we see the blank line, stop.
        if req.windows(4).any(|w| w == b"\r\n\r\n") { break; }
        if req.len() > 16 * 1024 { break; }
    }
    let req_str = match std::str::from_utf8(&req) { Ok(s) => s, Err(_) => return Ok(()) };
    let mut lines = req_str.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let full_path = parts.next().unwrap_or("");

    let (path, query) = match full_path.find('?') {
        Some(i) => (&full_path[..i], &full_path[i+1..]),
        None => (full_path, ""),
    };

    if method != "GET" && method != "HEAD" {
        return respond_text(&mut stream, 405, "method not allowed");
    }

    if path == "/sync" {
        return handle_sync(&mut stream, query, &players);
    }
    if path == "/health" {
        return respond_text(&mut stream, 200, "ok");
    }

    // Static files. "/" -> index.html, everything else is a relative path.
    let rel = if path == "/" { "index.html".to_string() } else {
        path.trim_start_matches('/').to_string()
    };
    serve_file(&mut stream, &rel)
}

fn handle_sync(stream: &mut TcpStream, query: &str, players: &Players) -> std::io::Result<()> {
    let params = parse_query(query);
    let id = params.get("id").cloned().unwrap_or_default();

    if !id.is_empty() {
        let name  = params.get("name").cloned().unwrap_or_default();
        let class_: u8 = params.get("class").and_then(|s| s.parse().ok()).unwrap_or(0);
        let x:  f32 = parse_f(&params, "x");
        let y:  f32 = parse_f(&params, "y");
        let z:  f32 = parse_f(&params, "z");
        let yaw:   f32 = parse_f(&params, "yaw");
        let speed: f32 = parse_f(&params, "speed");
        let anim = params.get("anim").cloned().unwrap_or_else(|| "idle".to_string());

        let mut map = players.lock().unwrap();
        map.insert(id.clone(), Player {
            name, class_, x, y, z, yaw, speed, anim, last_seen: Instant::now(),
        });
        // Evict anyone silent for >5s.
        let now = Instant::now();
        map.retain(|_, p| now.duration_since(p.last_seen) < Duration::from_secs(5));
    }

    let mut out = String::new();
    {
        let map = players.lock().unwrap();
        for (pid, p) in map.iter() {
            if pid == &id { continue; }
            out.push_str(&format!(
                "{}|{}|{}|{:.3}|{:.3}|{:.3}|{:.4}|{:.3}|{}\n",
                escape_pipe(pid), escape_pipe(&p.name), p.class_,
                p.x, p.y, p.z, p.yaw, p.speed, escape_pipe(&p.anim),
            ));
        }
    }
    respond_text(stream, 200, &out)
}

fn parse_f(m: &HashMap<String, String>, k: &str) -> f32 {
    m.get(k).and_then(|s| s.parse().ok()).unwrap_or(0.0)
}

// Pipes are our separator — if a name or id contains one we swap it for a
// visually close lookalike so a malicious client can't forge fields.
fn escape_pipe(s: &str) -> String { s.replace('|', "/") }

fn parse_query(q: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for part in q.split('&') {
        if part.is_empty() { continue; }
        if let Some(eq) = part.find('=') {
            let k = url_decode(&part[..eq]);
            let v = url_decode(&part[eq+1..]);
            m.insert(k, v);
        } else {
            m.insert(url_decode(part), String::new());
        }
    }
    m
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            out.push(b' ');
            i += 1;
        } else if b == b'%' && i + 2 < bytes.len() {
            let hi = hex_val(bytes[i+1]);
            let lo = hex_val(bytes[i+2]);
            match (hi, lo) {
                (Some(h), Some(l)) => { out.push((h << 4) | l); i += 3; }
                _ => { out.push(b); i += 1; }
            }
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn mime_for(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".html") { "text/html; charset=utf-8" }
    else if lower.ends_with(".js") { "application/javascript; charset=utf-8" }
    else if lower.ends_with(".wasm") { "application/wasm" }
    else if lower.ends_with(".glb") { "model/gltf-binary" }
    else if lower.ends_with(".gltf") { "model/gltf+json" }
    else if lower.ends_with(".png") { "image/png" }
    else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") { "image/jpeg" }
    else if lower.ends_with(".css") { "text/css; charset=utf-8" }
    else if lower.ends_with(".json") { "application/json; charset=utf-8" }
    else { "application/octet-stream" }
}

fn serve_file(stream: &mut TcpStream, rel: &str) -> std::io::Result<()> {
    // Block directory traversal.
    if rel.contains("..") || rel.starts_with('/') || rel.starts_with('\\') {
        return respond_text(stream, 400, "bad path");
    }
    let p = Path::new(rel);
    match std::fs::read(p) {
        Ok(bytes) => {
            let mime = mime_for(rel);
            let header = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: {mime}\r\n\
                 Content-Length: {}\r\n\
                 Cache-Control: no-store\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Connection: close\r\n\r\n",
                bytes.len());
            stream.write_all(header.as_bytes())?;
            stream.write_all(&bytes)?;
            Ok(())
        }
        Err(_) => respond_text(stream, 404, "not found"),
    }
}

fn respond_text(stream: &mut TcpStream, code: u16, body: &str) -> std::io::Result<()> {
    let status = match code {
        200 => "OK", 400 => "Bad Request", 404 => "Not Found",
        405 => "Method Not Allowed", _ => "OK",
    };
    let header = format!(
        "HTTP/1.1 {code} {status}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\r\n",
        body.len());
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    Ok(())
}
