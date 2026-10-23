// lispm-remote: one connected client.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A session: one client on one connection, served by one thread.  The
//! thread alternates between reading what the client says (with a short
//! timeout) and bringing the client up to date: the window list, the
//! surfaces, their screens, the notices.  It never blocks the surfaces'
//! readers; a slow client only slows its own thread.

use std::collections::BTreeMap;
use std::io;
use std::io::Read;
use std::io::Write;

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
    /// The connection failed or closed.
    #[error("the connection ended: {0}")]
    Connection(#[from] tungstenite::Error),
}

/// What a session knows of its client.
struct Session {
    followers: BTreeMap<SurfaceId, Follower>,
    changes: u64,
    notices: u64,
}

/// Serves the client on `socket` until it leaves.  `socket`'s stream
/// should time reads out, so the session keeps the client up to date
/// while it is silent.
pub fn serve<S: Read + Write>(hub: &Hub, socket: &mut WebSocket<S>) -> Result<(), SessionError> {
    hub.client_joined();
    let mut session = Session {
        followers: BTreeMap::new(),
        changes: 0,
        notices: hub.notices.since(u64::MAX).0,
    };
    let result = session.run(hub, socket);
    hub.client_left();
    result
}

fn timed_out(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(error)
        if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
}

impl Session {
    fn run<S: Read + Write>(
        &mut self,
        hub: &Hub,
        socket: &mut WebSocket<S>,
    ) -> Result<(), SessionError> {
        loop {
            match socket.read() {
                Ok(Message::Binary(bytes)) => {
                    if let Ok(message) = ClientMessage::decode(&bytes) {
                        self.receive(hub, socket, message)?;
                    }
                }
                Ok(Message::Close(_)) => return Ok(()),
                Ok(_) => {}
                Err(error) if timed_out(&error) => {}
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            }
            self.catch_up(hub, socket)?;
        }
    }

    fn receive<S: Read + Write>(
        &mut self,
        hub: &Hub,
        socket: &mut WebSocket<S>,
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
            ClientMessage::Want { id } => send_resource(hub, socket, id)?,
        }
        Ok(())
    }

    /// Sends what changed since the client was last told.
    fn catch_up<S: Read + Write>(
        &mut self,
        hub: &Hub,
        socket: &mut WebSocket<S>,
    ) -> Result<(), SessionError> {
        let changes = hub.changes();
        if changes != self.changes {
            self.changes = changes;
            send(socket, &ServerMessage::Windows(hub.windows()))?;
            self.announce(hub, socket)?;
        }
        let (seen, notices) = hub.notices.since(self.notices);
        self.notices = seen;
        for notice in &notices {
            send(socket, notice)?;
        }
        for surface in hub.all_surfaces() {
            let Some(follower) = self.followers.get_mut(&surface.id()) else {
                continue;
            };
            if follower.behind(surface.version()) && !follower.waiting() {
                let (version, screen) = surface.snapshot();
                for frame in follower.frames(version, &screen) {
                    send(socket, &frame)?;
                }
            }
        }
        Ok(())
    }

    /// Tells the client of surfaces opened and closed since it last knew.
    fn announce<S: Read + Write>(
        &mut self,
        hub: &Hub,
        socket: &mut WebSocket<S>,
    ) -> Result<(), SessionError> {
        let surfaces = hub.all_surfaces();
        let gone: Vec<SurfaceId> = self
            .followers
            .keys()
            .filter(|id| !surfaces.iter().any(|surface| surface.id() == **id))
            .copied()
            .collect();
        for surface in gone {
            self.followers.remove(&surface);
            send(socket, &ServerMessage::Closed { surface })?;
        }
        for surface in surfaces {
            if self.followers.contains_key(&surface.id()) {
                continue;
            }
            let (_, screen) = surface.snapshot();
            send(
                socket,
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

fn send<S: Read + Write>(
    socket: &mut WebSocket<S>,
    message: &ServerMessage,
) -> Result<(), SessionError> {
    socket.send(Message::Binary(message.encode().into()))?;
    Ok(())
}

/// Sends the resource `id` in chunks, or says it is unknown.
fn send_resource<S: Read + Write>(
    hub: &Hub,
    socket: &mut WebSocket<S>,
    id: ResourceId,
) -> Result<(), SessionError> {
    let Some(bytes) = hub.pool.get(&id) else {
        return send(socket, &ServerMessage::Unknown { id });
    };
    let Ok(total) = u32::try_from(bytes.len()) else {
        return send(socket, &ServerMessage::Unknown { id });
    };
    for (offset, chunk) in (0..)
        .step_by(RESOURCE_CHUNK)
        .zip(bytes.chunks(RESOURCE_CHUNK))
    {
        let Ok(offset) = u32::try_from(offset) else {
            break;
        };
        send(
            socket,
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
