// lispm-remote: what the integration tests share.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A node the test can wait on, and a way to connect two nodes without a
//! seed: the offer and answer passed by hand.

use std::path::Path;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use lispm_remote_module::Account;
use lispm_remote_module::Event;
use lispm_remote_module::Node;
use lispm_remote_module::Wake;

pub const ICE: &str = "stun://stun.l.google.com:19302";
/// How long a test waits for the network.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// A waker the test waits on instead of sleeping.
#[derive(Default)]
struct Bell {
    rings: Mutex<u64>,
    changed: Condvar,
}

#[derive(Clone, Default)]
pub struct Shared(Arc<Bell>);

impl Wake for Shared {
    fn wake(&self) {
        *self.0.rings.lock().unwrap() += 1;
        self.0.changed.notify_all();
    }
}

/// A node and the events it produced that the test has not used yet.
pub struct Peer {
    pub node: Node,
    bell: Shared,
    seen: Vec<Event>,
}

impl Peer {
    pub fn start() -> Self {
        Self::start_as(&Account::generate(), None)
    }

    /// A node for `account`, its replay records kept in `store`.
    pub fn start_as(account: &Account, store: Option<&Path>) -> Self {
        let bell = Shared::default();
        let node = Node::start(account, ICE, store, bell.clone()).unwrap();
        Self {
            node,
            bell,
            seen: Vec::new(),
        }
    }

    pub fn did(&self) -> String {
        self.node.did().to_owned()
    }

    /// The first event `pick` accepts, waiting for the node's wakes.
    pub fn next<T>(&mut self, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.seen.extend(self.node.events());
            if let Some(index) = self.seen.iter().position(|event| pick(event).is_some()) {
                let event = self.seen.remove(index);
                return pick(&event).unwrap();
            }
            let now = Instant::now();
            assert!(now < deadline, "no such event; saw {:?}", self.seen);
            let rings = self.bell.0.rings.lock().unwrap();
            let _ = self
                .bell
                .0
                .changed
                .wait_timeout(rings, deadline - now)
                .unwrap();
        }
    }

    /// The result of the request `method` with `params`.
    pub fn ask(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        match self.request(method, params) {
            Ok(result) => result,
            Err(error) => panic!("{method} failed: {error}"),
        }
    }

    /// The result of the request `method` with `params`, or why it failed,
    /// as Rings put it.
    pub fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.node.request(method, &params.to_string()).unwrap();
        let reply = self.next(|event| match event {
            Event::Reply { id: got, result } if *got == id => Some(Ok(result.clone())),
            Event::Failed { id: got, error, .. } if *got == id => Some(Err(error.clone())),
            _ => None,
        });
        reply.map(|result| serde_json::from_str(&result).unwrap())
    }

    pub fn connected_to(&mut self, other: &str) -> bool {
        let peers = self.ask("listPeers", serde_json::json!({}));
        peers["peers"].as_array().is_some_and(|peers| {
            peers
                .iter()
                .any(|peer| peer["did"] == other && peer["state"] == "Connected")
        })
    }

    /// Sends `frame` to `to`, again while the network has no route yet.
    pub fn send(&mut self, to: &str, frame: Vec<u8>) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let id = self.node.send(to, frame.clone()).unwrap();
            let failure = self.next(|event| match event {
                Event::Sent { id: got } if *got == id => Some(None),
                Event::Failed { id: got, error, .. } if *got == id => Some(Some(error.clone())),
                _ => None,
            });
            let Some(error) = failure else { return };
            assert!(Instant::now() < deadline, "no route to {to}: {error}");
            // A route appears when the ring stabilizes; no event says so.
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// The next frame from `from`.
    pub fn frame_from(&mut self, from: &str) -> Vec<u8> {
        self.next(|event| match event {
            Event::Frame {
                from: sender,
                payload,
            } if sender == from => Some(payload.clone()),
            _ => None,
        })
    }
}

/// Connects `a` to `b` directly, passing the offer and answer by hand.
pub fn connect(a: &mut Peer, b: &mut Peer) {
    let offer = a.ask("createOffer", serde_json::json!({ "did": b.did() }));
    let answer = b.ask(
        "answerOffer",
        serde_json::json!({ "offer": offer["offer"] }),
    );
    a.ask(
        "acceptAnswer",
        serde_json::json!({ "answer": answer["answer"] }),
    );
    let deadline = Instant::now() + PATIENCE;
    let other = b.did();
    while !a.connected_to(&other) {
        assert!(Instant::now() < deadline, "the nodes did not connect");
        // listPeers has no event to wait for; ask again shortly.
        std::thread::sleep(Duration::from_millis(100));
    }
}
