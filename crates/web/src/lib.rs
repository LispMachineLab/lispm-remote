// lispm-remote: the device's machine, as the phone's page sees it.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A device's `Initiator` from `lispm-remote-protocol`, for the page at
//! lispm.ai/remote.  Every method of [`DeviceMachine`] (`Initiator` in
//! JavaScript) steps the machine once, with the browser's clock
//! (`Date.now()`) and fresh randomness from `crypto.getRandomValues`, and
//! returns the effects as an array of plain objects for the page to carry
//! out; frames to send are among them, so the network stays with the Rings
//! node.
//!
//! ```text
//!   {type: "transmit", to, frame}            send frame to the DID to
//!   {type: "show-fingerprint", fingerprint}  show it until pairing ends
//!   {type: "paired", responder, key}         store this pairing
//!   {type: "unpaired"}                       delete the stored pairing
//!   {type: "opened", channel}                a channel opened
//!   {type: "delivered", payload}             the channel delivered a payload
//!   {type: "closed", reason}                 the channel closed: by-peer, by-us
//!   {type: "failed", reason, detail?}        pairing or opening ended without one
//!   {type: "discarded", why}                 a frame was discarded
//! ```
//!
//! Keys and channels travel as hexadecimal text, fingerprints as
//! `4F2A-91C0`, frames and payloads as `Uint8Array`s.  A command the
//! machine refuses throws, and leaves the machine as it was.  These are the
//! effects the Emacs module gives Lisp (its `machines.rs`), so the page
//! does what LispM's test phone does.

#![deny(missing_docs)]

pub mod shape;

use js_sys::Array;
use js_sys::Object;
use js_sys::Reflect;
use js_sys::Uint8Array;
use lispm_remote_protocol::ENTROPY_LEN;
use lispm_remote_protocol::Entropy;
use lispm_remote_protocol::Millis;
use wasm_bindgen::JsError;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::shape::Command;
use crate::shape::Error;
use crate::shape::Field;
use crate::shape::Machine;
use crate::shape::Shape;

fn now() -> Millis {
    shape::moment(js_sys::Date::now())
}

fn entropy() -> Result<Entropy, Error> {
    let mut bytes = [0_u8; ENTROPY_LEN];
    getrandom::fill(&mut bytes).map_err(|_| Error::Random)?;
    Ok(Entropy::new(bytes))
}

fn thrown(error: &Error) -> JsValue {
    JsError::new(&error.to_string()).into()
}

fn object(shape: Shape) -> Result<Object, JsValue> {
    let object = Object::new();
    Reflect::set(&object, &"type".into(), &shape.kind.into())?;
    for (name, field) in shape.fields {
        let value: JsValue = match field {
            Field::Text(text) => text.into(),
            Field::Bytes(bytes) => Uint8Array::from(bytes.as_slice()).into(),
        };
        Reflect::set(&object, &name.into(), &value)?;
    }
    Ok(object)
}

fn effects(shapes: Vec<Shape>) -> Result<Array, JsValue> {
    let array = Array::new();
    for shape in shapes {
        array.push(&object(shape)?.into());
    }
    Ok(array)
}

fn text_field(record: &JsValue, name: &str) -> Result<String, JsValue> {
    Reflect::get(record, &name.into())?
        .as_string()
        .ok_or_else(|| JsError::new(&format!("the pairing has no {name}")).into())
}

/// A fresh static secret, in hexadecimal, for the page to keep secret and
/// store.
///
/// # Errors
///
/// Throws when the browser has no random source.
#[wasm_bindgen(js_name = generateStatic)]
pub fn generate_static() -> Result<String, JsValue> {
    Ok(shape::generate_static(
        entropy().map_err(|error| thrown(&error))?,
    ))
}

/// A device's pairing and session machine.
#[wasm_bindgen(js_name = Initiator)]
pub struct DeviceMachine(Machine);

#[wasm_bindgen(js_class = Initiator)]
impl DeviceMachine {
    /// The machine of the device holding the static `secret` (hexadecimal)
    /// at the DID `did`, paired as `pairing` says: `null`, or the
    /// `{responder, key}` of a `paired` effect.
    ///
    /// # Errors
    ///
    /// Throws for a malformed secret, DID or pairing.
    #[wasm_bindgen(constructor)]
    pub fn new(secret: &str, did: &str, pairing: &JsValue) -> Result<Self, JsValue> {
        let pairing = if pairing.is_null() || pairing.is_undefined() {
            None
        } else {
            let responder = text_field(pairing, "responder")?;
            let key = text_field(pairing, "key")?;
            Some(shape::pairing(&responder, &key).map_err(|error| thrown(&error))?)
        };
        Machine::new(secret, did, pairing)
            .map(Self)
            .map_err(|error| thrown(&error))
    }

    fn step(&mut self, command: Command<'_>) -> Result<Array, JsValue> {
        let shapes = self
            .0
            .step(now(), command, entropy)
            .map_err(|error| thrown(&error))?;
        effects(shapes)
    }

    /// Pair with the offer `token`, from the QR code, calling the device
    /// `name`.
    ///
    /// # Errors
    ///
    /// Throws for a malformed or expired offer, or while busy.
    pub fn pair(&mut self, token: &str, name: &str) -> Result<Array, JsValue> {
        self.step(Command::Pair { token, name })
    }

    /// Open a session with the paired LispM.
    ///
    /// # Errors
    ///
    /// Throws unless the device is paired and idle.
    pub fn connect(&mut self) -> Result<Array, JsValue> {
        self.step(Command::Connect)
    }

    /// Take the `frame` that the DID `from` sent, as Rings authenticated
    /// it.
    ///
    /// # Errors
    ///
    /// Throws for a DID out of bounds; frames that do not fit are
    /// `discarded` effects instead.
    pub fn receive(&mut self, from: &str, frame: &[u8]) -> Result<Array, JsValue> {
        self.step(Command::Receive { from, frame })
    }

    /// Send `payload` on the open channel.
    ///
    /// # Errors
    ///
    /// Throws when no channel is open or the payload is too large.
    pub fn send(&mut self, payload: &[u8]) -> Result<Array, JsValue> {
        self.step(Command::Send(payload))
    }

    /// Close the open channel, or stop opening it.
    ///
    /// # Errors
    ///
    /// Throws when there is nothing to close.
    pub fn close(&mut self) -> Result<Array, JsValue> {
        self.step(Command::Close)
    }

    /// Tell the machine that time passed: it sends a waiting frame again,
    /// or gives up on it.
    ///
    /// # Errors
    ///
    /// Throws only while the machine is stepping already.
    pub fn tick(&mut self) -> Result<Array, JsValue> {
        self.step(Command::Tick)
    }

    /// Whether a channel is open.
    #[wasm_bindgen(js_name = openP)]
    #[must_use]
    pub fn open_p(&self) -> bool {
        self.0.is_open()
    }
}
