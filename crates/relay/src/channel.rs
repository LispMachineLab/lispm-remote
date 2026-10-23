// lispm-remote: clients on LispM Remote's channels.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Clients that reach the display through a paired device's channel.
//! Emacs holds the channel (lispm-remote.el seals and opens its records),
//! so here a channel client is two queues: payloads Emacs delivers come
//! in through [`deliver`], and messages for the client go out as
//! [`Request::Send`] for Emacs to send.  Each client is served by a
//! session thread, like a WebSocket's.
//!
//! The channel may lose, repeat and reorder payloads; the display's
//! synchronisation (`crate::follow`) is made for that.

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use crate::hub::Hub;
use crate::hub::Request;
use crate::session;
use crate::session::SessionError;
use crate::session::Transport;

/// How long a channel session waits for its client before catching it up.
const TICK: Duration = Duration::from_millis(10);

/// A client on a channel: payloads in from a queue, messages out as
/// requests to Emacs.
struct Channel {
    client: u64,
    inbound: Receiver<Vec<u8>>,
    hub: Arc<Hub>,
}

impl Transport for Channel {
    fn receive(&mut self) -> Result<Option<Vec<u8>>, SessionError> {
        match self.inbound.recv_timeout(TICK) {
            Ok(payload) => Ok(Some(payload)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(SessionError::Closed),
        }
    }

    fn send(&mut self, message: Vec<u8>) -> Result<(), SessionError> {
        self.hub.request(Request::Send {
            client: self.client,
            payload: message,
        });
        Ok(())
    }
}

/// Attaches a client on a channel and starts its session; returns the
/// client's number, which Emacs's requests to send name.
///
/// # Errors
///
/// The system's, when no thread can be had.
pub fn attach(hub: &Arc<Hub>) -> std::io::Result<u64> {
    let (client, inbound) = hub.register_client();
    let mut channel = Channel {
        client,
        inbound,
        hub: Arc::clone(hub),
    };
    let serving = Arc::clone(hub);
    let started = std::thread::Builder::new()
        .name(format!("lispm-remote-channel-{client}"))
        .spawn(move || {
            let _ = session::serve(&serving, &mut channel);
        });
    if let Err(error) = started {
        hub.detach(client);
        return Err(error);
    }
    Ok(client)
}

/// Hands `payload`, which the client on channel `client` sent, to its
/// session.  Returns whether that client is attached.
pub fn deliver(hub: &Hub, client: u64, payload: Vec<u8>) -> bool {
    hub.deliver(client, payload)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use lispm_remote_display::CellPixels;
    use lispm_remote_display::ClientMessage;
    use lispm_remote_display::ServerMessage;
    use lispm_remote_display::Size;

    use super::*;
    use crate::hub::Wake;

    struct Count(Arc<AtomicUsize>);

    impl Wake for Count {
        fn wake(&self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// The requests Emacs would collect within a few seconds, until
    /// `enough` holds of them.
    fn collect(hub: &Hub, enough: impl Fn(&[Request]) -> bool) -> Vec<Request> {
        let started = Instant::now();
        let mut seen = Vec::new();
        while !enough(&seen) {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "timed out with {seen:?}"
            );
            seen.extend(hub.requests());
            std::thread::sleep(Duration::from_millis(10));
        }
        seen
    }

    fn sent(requests: &[Request]) -> Vec<ServerMessage> {
        requests
            .iter()
            .filter_map(|request| match request {
                Request::Send { payload, .. } => ServerMessage::decode(payload).ok(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_channel_client_asks_through_emacs_and_is_answered_through_it() {
        let hub = Arc::new(Hub::new(Count(Arc::new(AtomicUsize::new(0)))));
        let client = attach(&hub).unwrap();
        let seen = collect(&hub, |seen| {
            sent(seen).contains(&ServerMessage::Windows(Vec::new()))
        });
        assert!(seen.contains(&Request::Clients(1)));
        let size = Size::new(10, 3).unwrap();
        let cell = CellPixels {
            width: 8,
            height: 16,
        };
        let open = ClientMessage::Open {
            window: "w".to_owned(),
            size,
            cell,
        };
        assert!(deliver(&hub, client, open.encode()));
        let seen = collect(&hub, |seen| {
            seen.iter()
                .any(|request| matches!(request, Request::Open { .. }))
        });
        assert!(seen.contains(&Request::Open {
            window: "w".to_owned(),
            size,
            cell
        }));
        hub.detach(client);
        collect(&hub, |seen| seen.contains(&Request::Clients(0)));
        assert!(!deliver(&hub, client, Vec::new()));
    }
}
