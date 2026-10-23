// lispm-remote: one connected client.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A session: one client, served by one thread.  The thread alternates
//! between taking what the client says (waiting a moment at most) and
//! bringing the client up to date: the window list, the surfaces, their
//! screens, the notices.  It never blocks the surfaces' readers; a slow
//! client only slows its own thread.
//!
//! The client is reached through a [`Transport`]: a WebSocket ([`Socket`])
//! or a LispM Remote channel (`crate::channel`).

use std::collections::BTreeMap;
use std::io;
use std::io::Read;
use std::io::Write;
use std::time::Instant;

use lispm_remote_display::ClientMessage;
use lispm_remote_display::RESOURCE_CHUNK;
use lispm_remote_display::ResourceId;
use lispm_remote_display::ServerMessage;
use lispm_remote_display::SurfaceId;
use tungstenite::Message;
use tungstenite::WebSocket;

use crate::follow::Follower;
use crate::hub::Hub;
use crate::hub::Request;

/// Why a session ended.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The connection failed.
    #[error("the connection failed: {0}")]
    Connection(#[from] tungstenite::Error),
    /// The client left.
    #[error("the client left")]
    Closed,
}

/// What a session reaches its client through.
pub trait Transport {
    /// The next message from the client, waiting a moment at most;
    /// `Ok(None)` when none came.
    ///
    /// # Errors
    ///
    /// [`SessionError`] when the client is gone.
    fn receive(&mut self) -> Result<Option<Vec<u8>>, SessionError>;

    /// Sends `message` to the client.
    ///
    /// # Errors
    ///
    /// [`SessionError`] when the client is gone.
    fn send(&mut self, message: Vec<u8>) -> Result<(), SessionError>;
}

/// A WebSocket whose stream times reads out.
pub struct Socket<S>(pub WebSocket<S>);

fn timed_out(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(error)
        if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
}

impl<S: Read + Write> Transport for Socket<S> {
    fn receive(&mut self) -> Result<Option<Vec<u8>>, SessionError> {
        match self.0.read() {
            Ok(Message::Binary(bytes)) => Ok(Some(bytes.to_vec())),
            Ok(Message::Close(_))
            | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                Err(SessionError::Closed)
            }
            Ok(_) => Ok(None),
            Err(error) if timed_out(&error) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn send(&mut self, message: Vec<u8>) -> Result<(), SessionError> {
        self.0.send(Message::Binary(message.into()))?;
        Ok(())
    }
}

/// What a session knows of its client.
struct Session {
    followers: BTreeMap<SurfaceId, Follower>,
    changes: u64,
    notices: u64,
    started: Instant,
}

/// Serves the client on `transport` until it leaves.
///
/// # Errors
///
/// [`SessionError::Connection`] when the connection failed; a client
/// that leaves ends the session without an error.
pub fn serve(hub: &Hub, transport: &mut impl Transport) -> Result<(), SessionError> {
    hub.client_joined();
    let mut session = Session {
        followers: BTreeMap::new(),
        changes: 0,
        notices: hub.notices.since(u64::MAX).0,
        started: Instant::now(),
    };
    let result = match session.run(hub, transport) {
        Err(SessionError::Closed) => Ok(()),
        other => other,
    };
    hub.client_left();
    result
}

impl Session {
    fn run(&mut self, hub: &Hub, transport: &mut impl Transport) -> Result<(), SessionError> {
        loop {
            if let Some(bytes) = transport.receive()? {
                if let Ok(message) = ClientMessage::decode(&bytes) {
                    self.receive(hub, transport, message)?;
                }
            }
            self.catch_up(hub, transport)?;
        }
    }

    fn receive(
        &mut self,
        hub: &Hub,
        transport: &mut impl Transport,
        message: ClientMessage,
    ) -> Result<(), SessionError> {
        match message {
            ClientMessage::Open { window, size, cell } => {
                hub.request(Request::Open { window, size, cell })
            }
            ClientMessage::Close { surface } => hub.request(Request::Close { surface }),
            ClientMessage::Resize {
                surface,
                size,
                cell,
            } => {
                if let Some(surface) = hub.surface(surface) {
                    // A resize the pseudo-terminal refuses leaves the old size.
                    let _ = surface.resize(size, cell);
                }
            }
            ClientMessage::Input { surface, input } => {
                if let Some(surface) = hub.surface(surface) {
                    let _ = surface.input(&input);
                }
            }
            ClientMessage::Ack { surface, seq } => {
                if let Some(follower) = self.followers.get_mut(&surface) {
                    follower.ack(seq);
                }
            }
            ClientMessage::Resync { surface } => {
                if let Some(follower) = self.followers.get_mut(&surface) {
                    follower.resync();
                }
            }
            ClientMessage::Want { id } => send_resource(hub, transport, id)?,
            ClientMessage::Hello => {}
            ClientMessage::Bye => return Err(SessionError::Closed),
        }
        Ok(())
    }

    /// Sends what changed since the client was last told.
    fn catch_up(&mut self, hub: &Hub, transport: &mut impl Transport) -> Result<(), SessionError> {
        let changes = hub.changes();
        if changes != self.changes {
            self.changes = changes;
            send(transport, &ServerMessage::Windows(hub.windows()))?;
            self.announce(hub, transport)?;
        }
        let (seen, notices) = hub.notices.since(self.notices);
        self.notices = seen;
        for notice in &notices {
            send(transport, notice)?;
        }
        for surface in hub.all_surfaces() {
            let Some(follower) = self.followers.get_mut(&surface.id()) else {
                continue;
            };
            let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            if follower.due(now, surface.version()) {
                let (version, screen) = surface.snapshot();
                for frame in follower.frames(now, version, &screen) {
                    send(transport, &frame)?;
                }
            }
        }
        Ok(())
    }

    /// Tells the client of surfaces opened and closed since it last knew.
    fn announce(&mut self, hub: &Hub, transport: &mut impl Transport) -> Result<(), SessionError> {
        let surfaces = hub.all_surfaces();
        let gone: Vec<SurfaceId> = self
            .followers
            .keys()
            .filter(|id| !surfaces.iter().any(|surface| surface.id() == **id))
            .copied()
            .collect();
        for surface in gone {
            self.followers.remove(&surface);
            send(transport, &ServerMessage::Closed { surface })?;
        }
        for surface in surfaces {
            if self.followers.contains_key(&surface.id()) {
                continue;
            }
            let (_, screen) = surface.snapshot();
            send(
                transport,
                &ServerMessage::Opened {
                    surface: surface.id(),
                    window: surface.window().to_owned(),
                    size: screen.size(),
                },
            )?;
            self.followers
                .insert(surface.id(), Follower::new(surface.id()));
        }
        Ok(())
    }
}

fn send(transport: &mut impl Transport, message: &ServerMessage) -> Result<(), SessionError> {
    transport.send(message.encode())
}

/// Sends the resource `id` in chunks, or says it is unknown.
fn send_resource(
    hub: &Hub,
    transport: &mut impl Transport,
    id: ResourceId,
) -> Result<(), SessionError> {
    let Some(bytes) = hub.pool.get(&id) else {
        return send(transport, &ServerMessage::Unknown { id });
    };
    let Ok(total) = u32::try_from(bytes.len()) else {
        return send(transport, &ServerMessage::Unknown { id });
    };
    for (offset, chunk) in (0..)
        .step_by(RESOURCE_CHUNK)
        .zip(bytes.chunks(RESOURCE_CHUNK))
    {
        let Ok(offset) = u32::try_from(offset) else {
            break;
        };
        send(
            transport,
            &ServerMessage::Resource {
                id,
                total,
                offset,
                bytes: chunk.to_vec(),
            },
        )?;
    }
    Ok(())
}
