// lispm-remote: what a node of another Rings version says.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A seed of another Rings version, and what LispM Remote hears of it.
//!
//! Since Rings 0.37 (RingsNetwork/rings#1104) this node reads the seed's
//! wire version from its `nodeDid` before it offers.  It refuses a seed of
//! another wire version, or one from before wire versioning, with the
//! JSON-RPC error 210: the words "Incompatible wire version: ours is n,
//! the peer's is m" and the data `{"ours": n, "theirs": m}`, both from
//! this node's side.  The module passes words and data on to Lisp
//! (`Event::failed`); the page's node passes on only the words.  A seed
//! that cannot read the offer all the same fails with a codec error, as
//! before 0.37.  LispM Remote tells which side is older from these
//! (`lispm-remote--rings-older` in lispm-remote.el, `ringsOlder` in
//! web/src/version.ts), and names this end's Rings version from
//! `nodeInfo`.  These tests hold Rings to all of it.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use lispm_remote_module::Event;
use rings_core::delegation::DelegationBuilder;
use rings_core::dht::Did;
use rings_core::dht::delivery::NextHop;
use rings_core::ecc::SecretKey;
use rings_core::message::Encoder;
use rings_core::message::MessagePayload;
use rings_core::message::MessageSigner;
use rings_core::message::PeerWireVersion;
use rings_core::message::WIRE_VERSION;
use rings_node::error::Error;
use rings_node::native::config::DEFAULT_NETWORK_ID;
use rings_rpc::jsonrpc::Client;
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

/// The DID the stand-in seed answers `nodeDid` with.
const SEED_DID: &str = "0x9b94cddd19023c1ed5fed0d54eb5ed27eab78fd3";

/// A seed on this computer, and how many offers it was sent.
struct StandIn {
    url: String,
    offers: Arc<AtomicUsize>,
}

/// A seed that states the wire version `wire_version` in its `nodeDid`,
/// none for a seed from before wire versioning, and answers every offer
/// with the JSON-RPC error `error`.
fn stand_in_seed(wire_version: Option<u16>, error: serde_json::Value) -> StandIn {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let offers = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&offers);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            answer(stream.unwrap(), wire_version, &error, &counted);
        }
    });
    StandIn { url, offers }
}

/// Answers the one JSON-RPC request on `stream`.
fn answer(
    mut stream: TcpStream,
    wire_version: Option<u16>,
    error: &serde_json::Value,
    offers: &AtomicUsize,
) {
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
    let reply = if request["method"] == "nodeDid" {
        let mut node = json!({ "did": SEED_DID });
        if let Some(version) = wire_version {
            node["wire_version"] = json!(version);
        }
        json!({ "jsonrpc": "2.0", "id": request["id"], "result": node })
    } else {
        offers.fetch_add(1, Ordering::SeqCst);
        json!({ "jsonrpc": "2.0", "id": request["id"], "error": error })
    }
    .to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    )
    .unwrap();
}

/// What the module reports when joining through the seed at `url` fails:
/// the words and the JSON-RPC data of its `Event::Failed`.
///
/// Rings's node joins only seeds at public addresses, so this goes the
/// way of its `connectPeerViaHttp` with Rings's own parts: the seed's
/// `nodeDid`, the check of its wire version (`admit_endpoint_wire_version`,
/// crate-private, is these two public calls), the offer; then the node's
/// conversions, as its request hands the error to the module.
fn join_failure(url: &str) -> (String, Option<serde_json::Value>) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let failed = runtime.block_on(async {
        let client = Client::new(url).unwrap();
        let seed = client.node_did(&NodeDidRequest {}).await.unwrap();
        assert_eq!(seed.did, SEED_DID);
        PeerWireVersion::reported(seed.wire_version)
            .admit()
            .map_err(Error::IncompatibleWireVersion)?;
        let refused = client
            .answer_offer(&AnswerOfferRequest {
                offer: "an offer".to_owned(),
            })
            .await
            .unwrap_err();
        Err::<(), _>(Error::from(refused))
    });
    let failed = failed.unwrap_err();
    let Event::Failed { error, data, .. } =
        Event::failed(1, &Error::InternalRpcError(failed.into()))
    else {
        panic!("a failure is an Event::Failed");
    };
    (error, data.map(|data| serde_json::from_str(&data).unwrap()))
}

/// The words this node refuses a seed with, whose wire version is `theirs`.
fn refused(theirs: &str) -> String {
    format!(
        "Internal rpc services error: Server error: Incompatible wire version: ours is {WIRE_VERSION}, the peer's is {theirs}.."
    )
}

#[test]
fn the_stand_in_seed_words_a_failure_as_node_rings_rs_did() {
    let seed = stand_in_seed(
        Some(WIRE_VERSION),
        json!({ "code": 208, "message": "Answer offer info failed: Codec deserialization error." }),
    );
    assert_eq!(
        join_failure(&seed.url),
        (SEEN_FROM_NODE_RINGS_RS.to_owned(), None)
    );
    assert_eq!(seed.offers.load(Ordering::SeqCst), 1);
}

#[test]
fn a_seed_of_a_later_wire_version_is_refused_before_the_offer() {
    let later = WIRE_VERSION + 1;
    let seed = stand_in_seed(Some(later), json!({}));
    let (error, data) = join_failure(&seed.url);
    assert_eq!(error, refused(&later.to_string()));
    assert_eq!(data, Some(json!({ "ours": WIRE_VERSION, "theirs": later })));
    assert_eq!(seed.offers.load(Ordering::SeqCst), 0);
}

#[test]
fn a_seed_from_before_wire_versioning_is_refused_before_the_offer() {
    // A seed of Rings 0.36 or older: its nodeDid states no wire version.
    let seed = stand_in_seed(None, json!({}));
    let (error, data) = join_failure(&seed.url);
    assert_eq!(error, refused("from before wire versioning"));
    assert_eq!(data, Some(json!({ "ours": WIRE_VERSION, "theirs": null })));
    assert_eq!(seed.offers.load(Ordering::SeqCst), 0);
}

#[test]
fn a_seed_that_refuses_the_offer_is_named_from_this_side() {
    // A seed that states this wire version and refuses the offer all the
    // same, in its own words: this node states the refusal as its own.
    let later = WIRE_VERSION + 1;
    let seed = stand_in_seed(
        Some(WIRE_VERSION),
        json!({
            "code": 210,
            "message": format!("Incompatible wire version: ours is {later}, the peer's is {WIRE_VERSION}."),
            "data": { "ours": later, "theirs": WIRE_VERSION },
        }),
    );
    let (error, data) = join_failure(&seed.url);
    assert_eq!(error, refused(&later.to_string()));
    assert_eq!(data, Some(json!({ "ours": WIRE_VERSION, "theirs": later })));
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
    // The version Cargo resolved, whether Cargo.toml takes a tag or a rev.
    let lock = repository_file("Cargo.lock");
    let built = between(&lock, "\nname = \"rings-node\"\n", "version = \"", "\"");
    // A build from a checkout appends the commit: 0.36.0-99feaaf.
    assert!(
        running == built || running.starts_with(&format!("{built}-")),
        "nodeInfo says {running}; Cargo.lock has Rings {built}"
    );
    // The page takes the same Rings: the tag "v0.36.0" or the same rev.
    let manifest = repository_file("Cargo.toml");
    let line = between(&manifest, "\nrings-node =", "{", "}");
    let pin = if line.contains("tag = \"") {
        between(line, "tag", "\"", "\"")
    } else {
        between(line, "rev", "\"", "\"")
    };
    let page = repository_file("web/package.json");
    assert_eq!(between(&page, "@ringsnetwork/rings-node", "#", "\""), pin);
}
