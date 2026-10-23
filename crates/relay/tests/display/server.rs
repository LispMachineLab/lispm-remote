// lispm-remote: the relay's server, over a real socket.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A client on a real WebSocket: it is refused without the token or from
//! another site's page, it asks Emacs for a window, and what is written
//! on the surface's tty reaches it as frames of the display protocol.

use std::fs::File;
use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use lispm_remote_display::CellPixels;
use lispm_remote_display::ClientMessage;
use lispm_remote_display::Screen;
use lispm_remote_display::Seq;
use lispm_remote_display::ServerMessage;
use lispm_remote_display::Size;
use lispm_remote_display::apply;
use lispm_remote_relay::Hub;
use lispm_remote_relay::Request;
use lispm_remote_relay::Server;
use lispm_remote_relay::Wake;
use tungstenite::Message;
use tungstenite::WebSocket;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;

struct Count(Arc<AtomicUsize>);

impl Wake for Count {
    fn wake(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

fn server() -> (Arc<Hub>, Server) {
    let hub = Arc::new(Hub::new(Count(Arc::new(AtomicUsize::new(0)))));
    let web = std::env::temp_dir();
    let server = Server::start(Arc::clone(&hub), "127.0.0.1:0", web).unwrap();
    (hub, server)
}

fn connect(
    server: &Server,
    token: &str,
    origin: Option<&str>,
) -> Result<WebSocket<TcpStream>, tungstenite::Error> {
    let address = server.address();
    let mut request = format!("ws://{address}/ws?t={token}")
        .into_client_request()
        .unwrap();
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("origin", HeaderValue::from_str(origin).unwrap());
    }
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    tungstenite::client(request, stream)
        .map(|(socket, _)| socket)
        .map_err(|error| match error {
            tungstenite::HandshakeError::Failure(error) => error,
            tungstenite::HandshakeError::Interrupted(_) => tungstenite::Error::ConnectionClosed,
        })
}

fn receive(socket: &mut WebSocket<TcpStream>) -> ServerMessage {
    loop {
        if let Message::Binary(bytes) = socket.read().unwrap() {
            return ServerMessage::decode(&bytes).unwrap();
        }
    }
}

fn eventually<T>(mut probe: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_client_without_the_token_or_from_elsewhere_is_refused() {
    let (_hub, server) = server();
    assert!(connect(&server, "wrong", None).is_err());
    let token = server.token().to_owned();
    assert!(connect(&server, &token, Some("http://evil.example")).is_err());
    let own = format!("http://{}", server.address());
    assert!(connect(&server, &token, Some(&own)).is_ok());
}

#[test]
fn what_emacs_writes_reaches_the_client_as_frames() {
    let (hub, server) = server();
    let token = server.token().to_owned();
    let mut socket = connect(&server, &token, None).unwrap();
    assert_eq!(receive(&mut socket), ServerMessage::Windows(Vec::new()));

    // The client asks for a window; Emacs is told.
    let size = Size::new(20, 4).unwrap();
    let cell = CellPixels {
        width: 8,
        height: 16,
    };
    let open = ClientMessage::Open {
        window: "*scratch*".to_owned(),
        size,
        cell,
    };
    socket.send(Message::Binary(open.encode().into())).unwrap();
    let request = eventually(|| {
        hub.requests()
            .into_iter()
            .find(|request| matches!(request, Request::Open { .. }))
    });
    assert_eq!(
        request,
        Request::Open {
            window: "*scratch*".to_owned(),
            size,
            cell
        }
    );

    // Emacs opens a surface and draws on its tty.
    let surface = hub.open("*scratch*".to_owned(), size, cell).unwrap();
    let mut tty = File::options()
        .read(true)
        .write(true)
        .open(surface.tty())
        .unwrap();
    tty.write_all(b"\x1b[?1049h\x1b[H\x1b[2Jhello\x1b[2;1Hworld")
        .unwrap();

    let mut copy: Option<Screen> = None;
    let mut seq = Seq::ZERO;
    let text = eventually(|| {
        match receive(&mut socket) {
            ServerMessage::Opened {
                surface: opened,
                window,
                ..
            } => {
                assert_eq!((opened, window.as_str()), (surface.id(), "*scratch*"));
            }
            ServerMessage::Frame {
                base,
                seq: next,
                update,
                ..
            } => {
                let base_screen = if base == Seq::ZERO {
                    Screen::blank(update.size)
                } else {
                    copy.clone().unwrap()
                };
                assert!(base == Seq::ZERO || base == seq);
                copy = Some(apply(&base_screen, &update).unwrap());
                seq = next;
                let ack = ClientMessage::Ack {
                    surface: surface.id(),
                    seq,
                };
                socket.send(Message::Binary(ack.encode().into())).unwrap();
            }
            _ => {}
        }
        copy.as_ref()
            .map(Screen::text)
            .filter(|text| text.contains("world"))
    });
    assert!(text.starts_with("hello"), "{text:?}");

    // Typing on the client reaches the tty as an xterm's input.
    let key = ClientMessage::Input {
        first: Seq::new(1),
        inputs: vec![(
            surface.id(),
            lispm_remote_display::Input::Text("x".to_owned()),
        )],
    };
    socket.send(Message::Binary(key.encode().into())).unwrap();
    hub.close(surface.id());
}
