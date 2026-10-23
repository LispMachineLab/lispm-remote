// lispm-remote: the node, as Lisp sees it.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The module's Lisp functions, all named `lispm-remote-core-…`.  A node
//! lives in a `user-ptr`; Lisp starts work on it, the module writes one
//! byte to the node's pipe process when events wait, and the process
//! filter collects them with `lispm-remote-core-events`:
//!
//! ```text
//!   (frame FROM BYTES)      a peer's frame, FROM its DID, BYTES unibyte
//!   (sent ID)               a send left
//!   (reply ID JSON)         a request answered
//!   (failed ID MESSAGE)     a send or request failed
//!   (lost COUNT)            frames dropped while Emacs did not collect
//! ```
//!
//! Conversion between Lisp and Rust happens here and nowhere else; errors
//! become Lisp errors here.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use emacs::Env;
use emacs::IntoLisp;
use emacs::Result;
use emacs::Value;
use emacs::defun;

use crate::node::Account;
use crate::node::Event;
use crate::node::NAMESPACE;
use crate::node::Node;
use crate::node::Wake;

/// Wakes Emacs by writing to the pipe `open-channel` gave.
struct Pipe(Mutex<Box<dyn Write + Send>>);

impl Wake for Pipe {
    fn wake(&self) {
        if let Ok(mut writer) = self.0.lock() {
            // Emacs reads the pipe whenever it waits for input; a failed
            // write means it is gone, and then nobody waits.
            let _ = writer.write_all(b".");
        }
    }
}

/// A node, or nothing once stopped.
struct Handle(Option<Node>);

impl Handle {
    fn node(&self) -> Result<&Node> {
        self.0
            .as_ref()
            .ok_or_else(|| emacs::Error::msg("the node is stopped"))
    }
}

/// Start a Rings node for ACCOUNT, a key from
/// `lispm-remote-core-generate-account', with ICE-SERVERS, a list such
/// as "stun://stun.l.google.com:19302".  PROCESS is a pipe process; the
/// module writes to it when events wait.  STORE, when non-nil, is the
/// directory the node keeps its replay records in across restarts.
#[defun(user_ptr)]
fn start(
    env: &Env,
    account: String,
    ice_servers: String,
    process: Value,
    store: Option<String>,
) -> Result<Handle> {
    let account = Account::from_hex(&account)?;
    let writer = env.open_channel(process)?;
    let pipe = Pipe(Mutex::new(Box::new(writer)));
    let store = store.map(PathBuf::from);
    Ok(Handle(Some(Node::start(
        &account,
        &ice_servers,
        store.as_deref(),
        pipe,
    )?)))
}

/// Stop NODE.  After this, the module no longer writes to its process.
#[defun]
fn stop(handle: &mut Handle) -> Result<()> {
    if let Some(node) = handle.0.take() {
        node.stop();
    }
    Ok(())
}

/// Whether NODE is running.
#[defun]
fn live_p(handle: &Handle) -> Result<bool> {
    Ok(handle.0.is_some())
}

/// The DID of NODE.
#[defun]
fn did(handle: &Handle) -> Result<String> {
    Ok(handle.node()?.did().to_owned())
}

/// Start sending FRAME, a unibyte string, from NODE to the peer TO, a DID.
/// Return the id its `sent' or `failed' event carries.
#[defun]
fn send(handle: &Handle, to: String, frame: Vec<u8>) -> Result<i64> {
    let id = handle.node()?.send(&to, frame)?;
    Ok(i64::try_from(id)?)
}

/// Start the Rings JSON-RPC request METHOD on NODE with PARAMS, a JSON
/// object as a string.  Return the id its `reply' or `failed' event carries.
#[defun]
fn request(handle: &Handle, method: String, params: String) -> Result<i64> {
    let id = handle.node()?.request(&method, &params)?;
    Ok(i64::try_from(id)?)
}

/// The events of NODE since the last call, oldest first.
#[defun]
fn events<'e>(env: &'e Env, handle: &Handle) -> Result<Value<'e>> {
    let events = handle
        .node()?
        .events()
        .into_iter()
        .map(|event| event_value(env, event))
        .collect::<Result<Vec<_>>>()?;
    env.list(&events)
}

fn event_value(env: &Env, event: Event) -> Result<Value<'_>> {
    match event {
        Event::Frame { from, payload } => env.list((env.intern("frame")?, from, payload)),
        Event::Sent { id } => env.list((env.intern("sent")?, i64::try_from(id)?)),
        Event::Reply { id, result } => env.list((env.intern("reply")?, i64::try_from(id)?, result)),
        Event::Failed { id, error } => env.list((env.intern("failed")?, i64::try_from(id)?, error)),
        Event::Lost { frames } => env.list((env.intern("lost")?, i64::try_from(frames)?)),
    }
}

/// A fresh Rings account key, as hexadecimal text, for the caller to keep
/// secret and store.
#[defun]
fn generate_account() -> Result<String> {
    Ok(Account::generate().to_hex())
}

/// The DID of the Rings account key ACCOUNT.
#[defun]
fn account_did(account: String) -> Result<String> {
    Ok(Account::from_hex(&account)?.did())
}

/// The namespace the module's frames travel under.
#[defun]
fn namespace(env: &Env) -> Result<Value<'_>> {
    NAMESPACE.into_lisp(env)
}
