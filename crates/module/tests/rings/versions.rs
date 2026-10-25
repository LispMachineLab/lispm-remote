// lispm-remote: what a node of another Rings version says.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! This build's Rings handshake carries no version: a seed of another Rings
//! version only fails to read the offer it is sent.  A seed with a wire
//! version (RingsNetwork/rings#1106) refuses an offer from before wire
//! versioning by name instead.  LispM Remote tells both failures by their
//! words (`lispm-remote-rings-mismatch-p` in lispm-remote.el,
//! `ringsMismatch` in web/src/version.ts), and names this end's Rings
//! version from `nodeInfo`.  These tests hold Rings to all three.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
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
use rings_rpc::jsonrpc::Client;
use rings_rpc::jsonrpc::RpcError;
use rings_rpc::protos::rings_node::AnswerOfferRequest;
use rings_rpc::protos::rings_node::NodeDidRequest;
use serde_json::json;

use crate::support::Peer;

/// The words LispM Remote takes for a node that cannot read a Rings
/// message of this one's version.
const MISMATCH: &str = "Codec deserialization error";

/// Why LispM Remote 0.1.0 could not join through node.rings.rs once it ran
/// Rings 0.36.0, word for word as the module reported it.
const SEEN_FROM_NODE_RINGS_RS: &str = "Internal rpc services error: Server error: Connect remote rpc server failed: Server returned rpc error Server error: Answer offer info failed: Codec deserialization error...";

/// Why this build cannot join through a seed with a wire version, as the
/// module reports it.  The seed says "ours" of itself, and "the peer's" of
/// this node.
const REFUSED_BEFORE_VERSIONING: &str = "Internal rpc services error: Server error: Connect remote rpc server failed: Server returned rpc error Server error: Incompatible wire version: ours is 1, the peer's is from before wire versioning...";

/// The DID the stand-in seed answers `nodeDid` with.
const SEED_DID: &str = "0x9b94cddd19023c1ed5fed0d54eb5ed27eab78fd3";

/// A seed on this computer that answers `nodeDid` as a seed with a wire
/// version does, and every other call with the JSON-RPC error `error`;
/// its URL.  Rings's node joins only seeds at public addresses, so this
/// build's JSON-RPC client stands for it, and the node's own conversions
/// word what the client heard (`join_failure`).
fn stand_in_seed(error: serde_json::Value) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            answer(stream.unwrap(), &error);
        }
    });
    url
}

/// Answers the one JSON-RPC request on `stream`.
fn answer(mut stream: TcpStream, error: &serde_json::Value) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            }
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let reply = match request["method"].as_str() {
        Some("nodeDid") => json!({
            "jsonrpc": "2.0", "id": request["id"],
            "result": { "did": SEED_DID, "wire_version": 1 },
        }),
        _ => json!({ "jsonrpc": "2.0", "id": request["id"], "error": error }),
    }
    .to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    )
    .unwrap();
}

/// What came of joining through `seed`, whose `answerOffer` fails: the
/// JSON-RPC code and data this build's client heard, and the error the
/// module reports, as `connectPeerViaHttp` and the node's request word it.
fn join_failure(seed: &str) -> (i64, Option<serde_json::Value>, String) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let client = Client::new(seed).unwrap();
        // The join gets past nodeDid: a reply that states a wire version
        // still reads.
        let did = client.node_did(&NodeDidRequest {}).await.unwrap().did;
        assert_eq!(did, SEED_DID);
        let error = client
            .answer_offer(&AnswerOfferRequest {
                offer: "an offer".to_owned(),
            })
            .await
            .unwrap_err();
        let RpcError::JsonClientError(heard) = &error else {
            panic!("the seed's error did not arrive: {error}");
        };
        let (code, data) = (heard.code.code(), heard.data.clone());
        let failed = rings_node::error::Error::from(error);
        let reported = rings_node::error::Error::InternalRpcError(failed.into()).to_string();
        (code, data, reported)
    })
}

#[test]
fn the_stand_in_seed_words_a_failure_as_node_rings_rs_did() {
    let (code, _, reported) = join_failure(&stand_in_seed(json!({
        "code": 208,
        "message": "Answer offer info failed: Codec deserialization error.",
    })));
    assert_eq!(code, 208);
    assert_eq!(reported, SEEN_FROM_NODE_RINGS_RS);
}

#[test]
fn a_seed_with_a_wire_version_refuses_this_node_in_words() {
    // As RingsNetwork/rings#1106 answers an offer from before wire
    // versioning: its code, its versions, its words.
    let (code, data, reported) = join_failure(&stand_in_seed(json!({
        "code": 210,
        "message": "Incompatible wire version: ours is 1, the peer's is from before wire versioning.",
        "data": { "ours": 1, "theirs": null },
    })));
    // This build's client hears the code and the versions, and the node
    // passes on only the words.
    assert_eq!(code, 210);
    assert_eq!(data, Some(json!({ "ours": 1, "theirs": null })));
    assert_eq!(reported, REFUSED_BEFORE_VERSIONING);
}

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
