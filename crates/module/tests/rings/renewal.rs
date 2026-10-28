// lispm-remote: a node that outlives its delegation.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A node on a short delegation talks, renews the delegation before its
//! signing horizon, and talks again past the horizon of the one it started
//! on, which could send nothing there.  Its replay records go on, kept in
//! a store or in memory, so the peer takes its frames: their sequences
//! did not start over.

use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use lispm_remote_module::Account;
use lispm_remote_module::Event;
use lispm_remote_module::Lifetime;

use crate::support::Peer;
use crate::support::connect;

/// Rings stops signing 25 s before a delegation expires: one of 50 s stops
/// 25 s after it starts, and the node renews it 5 s before.
const LIFETIME: Lifetime = Lifetime {
    ttl: Duration::from_secs(50),
    lead: Duration::from_secs(5),
};
/// When the first delegation stops signing at the latest, from just after
/// its node started.
const HORIZON: Duration = Duration::from_secs(25);

fn renews_and_still_talks(store: Option<&Path>) {
    let mut a = Peer::start_lasting(&Account::generate(), store, LIFETIME);
    let horizon = Instant::now() + HORIZON;
    let mut b = Peer::start();
    connect(&mut a, &mut b);
    a.send(&b.did(), b"before".to_vec());
    assert_eq!(b.frame_from(&a.did()), b"before");

    let renewal = a.next(|event| match event {
        Event::Renewed => Some(Ok(())),
        Event::RenewalFailed { error } => Some(Err(error.clone())),
        _ => None,
    });
    assert_eq!(renewal, Ok(()));
    std::thread::sleep(horizon.saturating_duration_since(Instant::now()));
    // The renewed node is connected to no one; it joins again, as Emacs's
    // does on `renewed`.
    connect(&mut a, &mut b);
    a.send(&b.did(), b"after the horizon".to_vec());
    assert_eq!(b.frame_from(&a.did()), b"after the horizon");
    b.send(&a.did(), b"to a".to_vec());
    assert_eq!(a.frame_from(&b.did()), b"to a");

    a.node.stop();
    b.node.stop();
}

#[test]
fn a_node_renews_over_its_store_and_still_talks() {
    let store = std::env::temp_dir().join(format!("lispm-remote-renewal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&store);
    renews_and_still_talks(Some(&store));
    std::fs::remove_dir_all(&store).unwrap();
}

#[test]
fn a_node_renews_over_its_memory_and_still_talks() {
    renews_and_still_talks(None);
}
