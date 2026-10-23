// lispm-remote: the development transport, over HTTP and WebSocket.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small server for the display on a local network: it serves the
//! client page from a directory and carries the protocol over a
//! WebSocket at `/ws`.  LispM Remote's real transport is Rings; this one
//! is for development and for a phone on the same network.
//!
//! The page is public; the socket is not.  Opening it needs the token in
//! the URL LispM shows (`#t=…`, which the browser keeps to itself), and
//! a request from another site's page (its `Origin` not this server's
//! own) is refused, so a web page cannot reach the display through the
//! browser that visits it.

use std::fs;
use std::io;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use tungstenite::handshake::server::ErrorResponse;
use tungstenite::handshake::server::Request;
use tungstenite::handshake::server::Response;
use tungstenite::http::StatusCode;

use crate::hub::Hub;
use crate::session;

/// How long a session waits for its client before catching it up.
const TICK: Duration = Duration::from_millis(10);
/// How long a request's head may take to arrive.
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);
/// The longest request head read.
const MAX_HEAD: usize = 8 * 1024;

/// A running server.
pub struct Server {
    address: SocketAddr,
    token: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// Starts serving `hub` on `address` (such as `127.0.0.1:0`), with
    /// the client page from the directory `web`.
    ///
    /// # Errors
    ///
    /// The system's, when the address cannot be bound or no random token
    /// can be had.
    pub fn start(hub: Arc<Hub>, address: &str, web: PathBuf) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let token = token()?;
        let stop = Arc::new(AtomicBool::new(false));
        let accept = Accept {
            hub,
            web,
            token: token.clone(),
            stop: Arc::clone(&stop),
        };
        let thread = std::thread::Builder::new()
            .name("lispm-remote-display".to_owned())
            .spawn(move || accept.run(&listener))?;
        Ok(Self {
            address,
            token,
            stop,
            thread: Some(thread),
        })
    }

    /// The address it listens on.
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    /// The page's URL, token included.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}/#t={}", self.address, self.token)
    }

    /// The token a client must present.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Stops accepting connections.  Sessions end as their clients leave.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 128 random bits as hexadecimal.
fn token() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Whether `a` and `b` are equal, in time that depends only on length.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0_u8, |diff, (x, y)| diff | (x ^ y))
            == 0
}

struct Accept {
    hub: Arc<Hub>,
    web: PathBuf,
    token: String,
    stop: Arc<AtomicBool>,
}

impl Accept {
    fn run(&self, listener: &TcpListener) {
        while !self.stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let hub = Arc::clone(&self.hub);
                    let web = self.web.clone();
                    let token = self.token.clone();
                    let _ = std::thread::Builder::new()
                        .name("lispm-remote-session".to_owned())
                        .spawn(move || connection(&hub, stream, &web, &token));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}

/// The request line's target, read without taking the request.
fn peek_target(stream: &TcpStream) -> Option<String> {
    let started = Instant::now();
    let mut head = vec![0_u8; MAX_HEAD];
    while started.elapsed() < HEAD_TIMEOUT {
        let n = stream.peek(&mut head).ok()?;
        let seen = head.get(..n)?;
        if let Some(end) = seen.windows(2).position(|pair| pair == b"\r\n") {
            let line = std::str::from_utf8(seen.get(..end)?).ok()?;
            let mut words = line.split(' ');
            return match (words.next(), words.next()) {
                (Some("GET"), Some(target)) => Some(target.to_owned()),
                _ => None,
            };
        }
        if n == 0 || n == head.len() {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

fn connection(hub: &Hub, stream: TcpStream, web: &Path, token: &str) {
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(HEAD_TIMEOUT)).is_err()
    {
        return;
    }
    match peek_target(&stream) {
        Some(target) if target == "/ws" || target.starts_with("/ws?") => {
            websocket(hub, stream, token)
        }
        Some(target) => {
            let _ = static_file(stream, web, &target);
        }
        None => {
            let _ = respond(stream, "400 Bad Request", "text/plain", b"bad request");
        }
    }
}

fn websocket(hub: &Hub, stream: TcpStream, token: &str) {
    // tungstenite's handshake callback fixes the refusal's type.
    #[allow(clippy::result_large_err)]
    let check = |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
        if allowed(request, token) {
            Ok(response)
        } else {
            let mut refusal = ErrorResponse::new(Some("forbidden".to_owned()));
            *refusal.status_mut() = StatusCode::FORBIDDEN;
            Err(refusal)
        }
    };
    let Ok(mut socket) = tungstenite::accept_hdr(stream, check) else {
        return;
    };
    if socket.get_mut().set_read_timeout(Some(TICK)).is_err() {
        return;
    }
    let _ = session::serve(hub, &mut socket);
}

/// Whether a WebSocket request carries the token and comes from this
/// server's own page.
fn allowed(request: &Request, token: &str) -> bool {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let presented = request
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .find_map(|pair| pair.strip_prefix("t="));
    let same_origin = match (header("origin"), header("host")) {
        (Some(origin), Some(host)) => origin == format!("http://{host}"),
        (None, _) => true,
        _ => false,
    };
    same_origin && presented.is_some_and(|presented| same_secret(presented, token))
}

/// The content type of a file the page is made of.
fn content_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "html" => Some("text/html; charset=utf-8"),
        "js" => Some("text/javascript; charset=utf-8"),
        "wasm" => Some("application/wasm"),
        "css" => Some("text/css; charset=utf-8"),
        "png" => Some("image/png"),
        "svg" => Some("image/svg+xml"),
        "woff2" => Some("font/woff2"),
        _ => None,
    }
}

/// The file under `web` that `target` names, if it is one of the page's.
fn page_file(web: &Path, target: &str) -> Option<PathBuf> {
    let path = target.split(['?', '#']).next()?;
    let relative = path.strip_prefix('/')?;
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let plain = relative.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    });
    if !plain {
        return None;
    }
    let root = web.canonicalize().ok()?;
    let file = root.join(relative).canonicalize().ok()?;
    (file.starts_with(&root) && file.is_file()).then_some(file)
}

fn static_file(mut stream: TcpStream, web: &Path, target: &str) -> io::Result<()> {
    // Take the request off the connection before answering it.
    let mut head = vec![0_u8; MAX_HEAD];
    let _ = stream.read(&mut head)?;
    let found =
        page_file(web, target).and_then(|file| Some((content_type(&file)?, fs::read(&file).ok()?)));
    match found {
        Some((kind, body)) => respond(stream, "200 OK", kind, &body),
        None => respond(stream, "404 Not Found", "text/plain", b"not found"),
    }
}

fn respond(mut stream: TcpStream, status: &str, kind: &str, body: &[u8]) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'self'; \
         script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; \
         img-src 'self' blob:\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_pages_own_files_are_served() {
        let web = std::env::temp_dir().join(format!("lispm-remote-web-{}", std::process::id()));
        fs::create_dir_all(web.join("pkg")).unwrap();
        fs::write(web.join("index.html"), "<p>").unwrap();
        fs::write(web.join("pkg/client.js"), "").unwrap();
        assert_eq!(
            page_file(&web, "/"),
            Some(web.join("index.html").canonicalize().unwrap())
        );
        assert!(page_file(&web, "/pkg/client.js?v=1").is_some());
        for refused in [
            "/../etc/passwd",
            "/pkg/../../x",
            "/%2e%2e/x",
            "/nothing.html",
            "/pkg",
        ] {
            assert_eq!(page_file(&web, refused), None, "{refused}");
        }
        fs::remove_dir_all(&web).unwrap();
    }

    #[test]
    fn secrets_compare_whole() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "ab"));
    }
}
