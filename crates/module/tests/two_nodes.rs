// lispm-remote: two Rings nodes in one process.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Two nodes connect over WebRTC on this machine, the offer and answer
//! passed by hand as a QR code or a seed would pass them, and exchange
//! frames under LispM Remote's namespace.

// The helpers are test code too: clippy's test allowances reach them
// only inside a `cfg(test)` item.
#[cfg(test)]
mod scenario {
    use std::sync::Arc;
    use std::sync::Condvar;
    use std::sync::Mutex;
    use std::time::Duration;
    use std::time::Instant;

    use lispm_remote_module::Account;
    use lispm_remote_module::Event;
    use lispm_remote_module::Node;
    use lispm_remote_module::Wake;

    const ICE: &str = "stun://stun.l.google.com:19302";
    const PATIENCE: Duration = Duration::from_secs(30);

    /// A waker the test waits on instead of sleeping.
    #[derive(Default)]
    struct Bell {
        rings: Mutex<u64>,
        changed: Condvar,
    }

    #[derive(Clone, Default)]
    struct Shared(Arc<Bell>);

    impl Wake for Shared {
        fn wake(&self) {
            *self.0.rings.lock().unwrap() += 1;
            self.0.changed.notify_all();
        }
    }

    /// A node and the events it produced that the test has not used yet.
    struct Peer {
        node: Node,
        bell: Shared,
        seen: Vec<Event>,
    }

    impl Peer {
        fn start() -> Self {
            let bell = Shared::default();
            let node = Node::start(&Account::generate(), ICE, bell.clone()).unwrap();
            Self {
                node,
                bell,
                seen: Vec::new(),
            }
        }

        /// The first event `pick` accepts, waiting for the node's wakes.
        fn next<T>(&mut self, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
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
        fn ask(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
            let id = self.node.request(method, &params.to_string()).unwrap();
            let reply = self.next(|event| match event {
                Event::Reply { id: got, result } if *got == id => Some(Ok(result.clone())),
                Event::Failed { id: got, error } if *got == id => Some(Err(error.clone())),
                _ => None,
            });
            serde_json::from_str(&reply.unwrap()).unwrap()
        }

        fn connected_to(&mut self, other: &str) -> bool {
            let peers = self.ask("listPeers", serde_json::json!({}));
            peers["peers"].as_array().is_some_and(|peers| {
                peers
                    .iter()
                    .any(|peer| peer["did"] == other && peer["state"] == "Connected")
            })
        }
    }

    fn connect(a: &mut Peer, b: &mut Peer) {
        let offer = a.ask("createOffer", serde_json::json!({ "did": b.node.did() }));
        let answer = b.ask(
            "answerOffer",
            serde_json::json!({ "offer": offer["offer"] }),
        );
        a.ask(
            "acceptAnswer",
            serde_json::json!({ "answer": answer["answer"] }),
        );
        let deadline = Instant::now() + PATIENCE;
        let other = b.node.did().to_owned();
        while !a.connected_to(&other) {
            assert!(Instant::now() < deadline, "the nodes did not connect");
            // listPeers has no event to wait for; ask again shortly.
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn frame_from(peer: &mut Peer, from: &str) -> Vec<u8> {
        peer.next(|event| match event {
            Event::Frame {
                from: sender,
                payload,
            } if sender == from => Some(payload.clone()),
            _ => None,
        })
    }

    #[test]
    fn two_nodes_connect_and_exchange_frames_both_ways() {
        let mut a = Peer::start();
        let mut b = Peer::start();
        connect(&mut a, &mut b);

        let to_b = b.node.did().to_owned();
        let id = a.node.send(&to_b, b"\x00\x01to b".to_vec()).unwrap();
        a.next(|event| matches!(event, Event::Sent { id: got } if *got == id).then_some(()));
        let from_a = a.node.did().to_owned();
        assert_eq!(frame_from(&mut b, &from_a), b"\x00\x01to b");

        let to_a = a.node.did().to_owned();
        b.node.send(&to_a, b"to a".to_vec()).unwrap();
        assert_eq!(frame_from(&mut a, &to_b), b"to a");

        a.node.stop();
        b.node.stop();
    }

    #[test]
    fn a_send_to_an_unknown_did_fails_as_an_event() {
        let mut a = Peer::start();
        let id = a
            .node
            .send("0x0000000000000000000000000000000000000001", b"x".to_vec())
            .unwrap();
        let failed = a.next(|event| match event {
            Event::Failed { id: got, .. } if *got == id => Some(true),
            Event::Sent { id: got } if *got == id => Some(false),
            _ => None,
        });
        assert!(failed, "a node alone cannot reach anyone");
        assert!(a.node.send("not a did", Vec::new()).is_err());
    }
}
