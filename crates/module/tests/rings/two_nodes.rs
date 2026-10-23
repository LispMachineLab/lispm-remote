// lispm-remote: two Rings nodes in one process.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Two nodes connect directly and exchange frames under LispM Remote's
//! namespace.

use lispm_remote_module::Event;

use crate::support::Peer;
use crate::support::connect;

#[test]
fn two_nodes_connect_and_exchange_frames_both_ways() {
    let mut a = Peer::start();
    let mut b = Peer::start();
    connect(&mut a, &mut b);

    a.send(&b.did(), b"\x00\x01to b".to_vec());
    assert_eq!(b.frame_from(&a.did()), b"\x00\x01to b");
    b.send(&a.did(), b"to a".to_vec());
    assert_eq!(a.frame_from(&b.did()), b"to a");

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
