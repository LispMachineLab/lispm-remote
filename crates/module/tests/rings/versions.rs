// lispm-remote: what a node of another Rings version says.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rings's handshake carries no version: a seed of another Rings version
//! only fails to read the offer it is sent.  LispM Remote tells that
//! failure by its words (`lispm-remote--rings-mismatch-p` in
//! lispm-remote.el, `ringsMismatch` in web/src/version.ts), and names this
//! end's Rings version from `nodeInfo`.  These tests hold Rings to both.

use std::path::PathBuf;
use std::str::FromStr;

use rings_core::delegation::DelegationBuilder;
use rings_core::dht::Did;
use rings_core::dht::delivery::NextHop;
use rings_core::ecc::SecretKey;
use rings_core::message::Encoder;
use rings_core::message::MessagePayload;
use rings_core::message::MessageSigner;
use rings_node::native::config::DEFAULT_NETWORK_ID;

use crate::support::Peer;

/// The words LispM Remote takes for a node that cannot read a Rings
/// message of this one's version.
const MISMATCH: &str = "Codec deserialization error";

/// An offer to `seed` from a node of another Rings version, signed now.
///
/// node.rings.rs on Rings 0.36 still verified the offers of LispM Remote
/// 0.1.0, on Rings 0.34.2, but could not read the message they carried.
/// So this one is signed as an offer is, around a message of a kind this
/// version does not have.  A recorded offer would not do: its signature
/// expires, and a seed checks that before it reads the message.
fn offer_of_another_version(seed: &str) -> String {
    let secret = SecretKey::random();
    let account = Did::from(secret.address());
    let builder = DelegationBuilder::new(account.to_string(), "secp256k1".to_owned());
    let signature = secret.sign(&builder.unsigned_proof()).unwrap();
    let session = builder
        .set_delegator_signature(signature.to_vec())
        .build()
        .unwrap();
    let seed = Did::from_str(seed).unwrap();
    let unknown_kind = u32::MAX;
    MessagePayload::new_send_with_sequence(
        unknown_kind,
        MessageSigner::new(&session, DEFAULT_NETWORK_ID),
        NextHop::toward(seed),
        seed,
        1,
        None,
    )
    .unwrap()
    .encode()
    .unwrap()
    .to_string()
}

/// The repository's file at `path`.
fn repository_file(path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(path)).unwrap()
}

/// The text between the first `start` after `after` and the next `end`.
fn between<'t>(text: &'t str, after: &str, start: &str, end: &str) -> &'t str {
    let rest = &text[text.find(after).unwrap()..];
    let rest = &rest[rest.find(start).unwrap() + start.len()..];
    &rest[..rest.find(end).unwrap()]
}

#[test]
fn a_seed_of_another_rings_version_cannot_read_the_offer() {
    // This node stands for the seed: it runs this build's Rings, and the
    // offer comes from a node of another version.
    let mut seed = Peer::start();
    let offer = offer_of_another_version(&seed.did());
    let error = seed
        .request("answerOffer", serde_json::json!({ "offer": offer }))
        .unwrap_err();
    assert!(error.contains("Answer offer info failed"), "{error}");
    assert!(error.contains(MISMATCH), "{error}");
    seed.node.stop();
}

#[test]
fn a_node_says_which_rings_it_runs_the_one_the_page_runs() {
    let mut node = Peer::start();
    let info = node.ask("nodeInfo", serde_json::json!({}));
    let running = info["version"].as_str().unwrap().to_owned();
    node.node.stop();
    let manifest = repository_file("Cargo.toml");
    let built = between(&manifest, "\nrings-node =", "tag = \"v", "\"");
    // A build from a checkout appends the commit: 0.36.0-99feaaf.
    assert!(
        running == built || running.starts_with(&format!("{built}-")),
        "nodeInfo says {running}; Cargo.toml takes Rings {built}"
    );
    let page = repository_file("web/package.json");
    assert_eq!(
        between(&page, "@ringsnetwork/rings-node", "#v", "\""),
        built
    );
}
