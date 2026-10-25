// lispm-remote: the device's machine, as plain values.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The machine of a device, stepped with values only: [`Machine`] takes the
//! moment and the randomness as arguments, and returns each effect as a
//! [`Shape`], a type and named fields, which `lib.rs` turns into a plain
//! JavaScript object.  Nothing here touches JavaScript, so it is tested
//! natively.
//!
//! ```text
//!   shape : Effect → Shape
//!   Transmit { to, frame }       ↦ transmit           to: text, frame: bytes
//!   ShowFingerprint(f)           ↦ show-fingerprint   fingerprint: text
//!   Paired(p)                    ↦ paired             responder: text, key: hex
//!   Unpaired                     ↦ unpaired
//!   Opened(c)                    ↦ opened             channel: hex
//!   Delivered(p)                 ↦ delivered          payload: bytes
//!   Closed(r)                    ↦ closed             reason: kebab(r)
//!   Failed(Refused(r))           ↦ failed             reason: refused, detail: kebab(r)
//!   Failed(Handshake(e))         ↦ failed             reason: handshake, detail: e
//!   Failed(OtherVersion(v))      ↦ failed             reason: other-version, detail: v, ours: VERSION
//!   Failed(f)                    ↦ failed             reason: kebab(f)
//!   Discarded(d)                 ↦ discarded          why: d, as Rust prints it
//! ```
//!
//! The names are the module's for Lisp (`machines.rs`), so the page and
//! LispM's tests speak of the same effects.

use core::fmt::Debug;
use core::time::Duration;

use lispm_remote_protocol::DeviceName;
use lispm_remote_protocol::Entropy;
use lispm_remote_protocol::IdError;
use lispm_remote_protocol::Identity;
use lispm_remote_protocol::Initiator;
use lispm_remote_protocol::KEY_LEN;
use lispm_remote_protocol::KeyError;
use lispm_remote_protocol::Millis;
use lispm_remote_protocol::Offer;
use lispm_remote_protocol::OfferError;
use lispm_remote_protocol::Pairing;
use lispm_remote_protocol::Payload;
use lispm_remote_protocol::PeerId;
use lispm_remote_protocol::PublicKey;
use lispm_remote_protocol::StaticSecret;
use lispm_remote_protocol::VERSION;
use lispm_remote_protocol::WireError;
use lispm_remote_protocol::device::CommandError;
use lispm_remote_protocol::device::Effect;
use lispm_remote_protocol::device::Failure;
use lispm_remote_protocol::device::Input;

/// One field of an effect.
#[derive(Debug, PartialEq, Eq)]
pub enum Field {
    /// Text: a DID, a key or channel in hexadecimal, a name.
    Text(String),
    /// Bytes: a frame or a payload.
    Bytes(Vec<u8>),
}

/// An effect as the page sees it.
#[derive(Debug, PartialEq, Eq)]
pub struct Shape {
    /// What the effect is, as `transmit` or `show-fingerprint`.
    pub kind: &'static str,
    /// Its fields, by name.
    pub fields: Vec<(&'static str, Field)>,
}

impl Shape {
    fn new(kind: &'static str, fields: Vec<(&'static str, Field)>) -> Self {
        Self { kind, fields }
    }

    /// The text field `name`, if the effect has one.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        self.fields.iter().find_map(|(field, value)| match value {
            Field::Text(text) if *field == name => Some(text.as_str()),
            Field::Text(_) | Field::Bytes(_) => None,
        })
    }

    /// The bytes field `name`, if the effect has one.
    #[must_use]
    pub fn bytes(&self, name: &str) -> Option<&[u8]> {
        self.fields.iter().find_map(|(field, value)| match value {
            Field::Bytes(bytes) if *field == name => Some(bytes.as_slice()),
            Field::Text(_) | Field::Bytes(_) => None,
        })
    }
}

/// Why a call did not step the machine.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A DID, a device name or a payload is out of bounds.
    #[error("{0}")]
    Id(#[from] IdError),
    /// A key is not 32 bytes in hexadecimal.
    #[error("{0} is {KEY_LEN} bytes in hexadecimal")]
    Hex(&'static str),
    /// The machine's own key could not be made.
    #[error("{0}")]
    Key(#[from] KeyError),
    /// The QR code holds no offer.
    #[error("the code is not a pairing offer: {0}")]
    Offer(#[from] OfferError),
    /// A payload is too large.
    #[error("{0}")]
    Wire(#[from] WireError),
    /// The machine refused the command.
    #[error("{0}")]
    Command(#[from] CommandError),
    /// The machine is stepping already.
    #[error("the initiator is stepping")]
    Stepping,
    /// The shell has no random source.
    #[error("there is no random source")]
    Random,
}

/// `⌊x⌋` for `0 ≤ x < 2⁶⁴`, by `Duration`'s checked conversion; `None`
/// for a negative, infinite or undefined `x`.
#[must_use]
pub fn whole(x: f64) -> Option<u64> {
    Duration::try_from_secs_f64(x)
        .ok()
        .map(|span| span.as_secs())
}

/// The moment `Date.now()` reports, in milliseconds since the epoch; the
/// epoch itself for a clock that reports nothing sensible.
#[must_use]
pub fn moment(millis: f64) -> Millis {
    Millis::new(whole(millis).unwrap_or(0))
}

/// The name of a fieldless variant in kebab case, so
/// `CloseReason::ByPeer` is `by-peer` and `Refusal::NotPaired` `not-paired`.
#[must_use]
pub fn kebab(value: impl Debug) -> String {
    let name = format!("{value:?}");
    let mut kebab = String::with_capacity(name.len() + 4);
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                kebab.push('-');
            }
            kebab.push(character.to_ascii_lowercase());
        } else {
            kebab.push(character);
        }
    }
    kebab
}

/// The effect `effect` as the page sees it.
#[must_use]
pub fn shape(effect: Effect) -> Shape {
    let text = |name, value: String| (name, Field::Text(value));
    match effect {
        Effect::Transmit { to, frame } => Shape::new(
            "transmit",
            vec![text("to", to.to_string()), ("frame", Field::Bytes(frame))],
        ),
        Effect::ShowFingerprint(fingerprint) => Shape::new(
            "show-fingerprint",
            vec![text("fingerprint", fingerprint.to_string())],
        ),
        Effect::Paired(pairing) => Shape::new(
            "paired",
            vec![
                text("responder", pairing.responder().to_string()),
                text("key", hex::encode(pairing.key().as_bytes())),
            ],
        ),
        Effect::Unpaired => Shape::new("unpaired", Vec::new()),
        Effect::Opened(channel) => Shape::new(
            "opened",
            vec![text("channel", hex::encode(channel.as_bytes()))],
        ),
        Effect::Delivered(payload) => Shape::new(
            "delivered",
            vec![("payload", Field::Bytes(payload.into_bytes()))],
        ),
        Effect::Closed(reason) => Shape::new("closed", vec![text("reason", kebab(reason))]),
        Effect::Failed(Failure::Refused(refusal)) => Shape::new(
            "failed",
            vec![
                text("reason", "refused".to_owned()),
                text("detail", kebab(refusal)),
            ],
        ),
        Effect::Failed(Failure::Handshake(error)) => Shape::new(
            "failed",
            vec![
                text("reason", "handshake".to_owned()),
                text("detail", error.to_string()),
            ],
        ),
        Effect::Failed(Failure::OtherVersion(version)) => Shape::new(
            "failed",
            vec![
                text("reason", "other-version".to_owned()),
                text("detail", version.to_string()),
                text("ours", VERSION.to_string()),
            ],
        ),
        Effect::Failed(failure) => Shape::new("failed", vec![text("reason", kebab(failure))]),
        Effect::Discarded(why) => Shape::new("discarded", vec![text("why", format!("{why:?}"))]),
    }
}

fn key_bytes(text: &str, what: &'static str) -> Result<[u8; KEY_LEN], Error> {
    let decoded = hex::decode(text).map_err(|_| Error::Hex(what))?;
    <[u8; KEY_LEN]>::try_from(decoded.as_slice()).map_err(|_| Error::Hex(what))
}

/// The pairing with `responder`, keyed `key` in hexadecimal, as the page
/// stored it from a `paired` effect.
///
/// # Errors
///
/// [`Error::Id`] for an empty or overlong DID, [`Error::Hex`] for a key
/// that is not 32 bytes in hexadecimal.
pub fn pairing(responder: &str, key: &str) -> Result<Pairing, Error> {
    Ok(Pairing::new(
        PeerId::new(responder.as_bytes())?,
        PublicKey::from_bytes(key_bytes(key, "a key")?),
    ))
}

/// A fresh static secret from `entropy`, in hexadecimal, for the page to
/// keep secret and store.
#[must_use]
pub fn generate_static(entropy: Entropy) -> String {
    hex::encode(StaticSecret::generate(entropy).expose())
}

/// What the page asks of the machine.  The randomness and the moment come
/// with the step, from the shell.
#[derive(Debug)]
pub enum Command<'a> {
    /// Pair with the offer `token`, calling the device `name`.
    Pair {
        /// The token after the `#` of the QR code's URL.
        token: &'a str,
        /// What the owner will see the device called.
        name: &'a str,
    },
    /// Open a session.
    Connect,
    /// A frame arrived from the DID `from`.
    Receive {
        /// The sender, as Rings authenticated it.
        from: &'a str,
        /// The frame.
        frame: &'a [u8],
    },
    /// Send a payload on the open channel.
    Send(&'a [u8]),
    /// Close the channel, or stop opening it.
    Close,
    /// Time passed.
    Tick,
}

/// A device's machine, absent only while it steps.
pub struct Machine(Option<Initiator>);

impl Machine {
    /// The machine of the device holding `secret` (hexadecimal) at the DID
    /// `did`, paired as `pairing` says.
    ///
    /// # Errors
    ///
    /// [`Error::Hex`] for a secret that is not 32 bytes in hexadecimal,
    /// [`Error::Id`] for a DID out of bounds.
    pub fn new(secret: &str, did: &str, pairing: Option<Pairing>) -> Result<Self, Error> {
        let secret = StaticSecret::from_bytes(key_bytes(secret, "a static secret")?);
        let identity = Identity::new(PeerId::new(did.as_bytes())?, secret)?;
        Ok(Self(Some(Initiator::new(identity, pairing))))
    }

    /// Whether a channel is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.0.as_ref().is_some_and(Initiator::is_open)
    }

    /// Steps the machine with `command` at `now`; `entropy` is drawn only
    /// for the commands that need it.
    ///
    /// # Errors
    ///
    /// An [`Error`] when the command is malformed or the machine refuses it.
    ///
    /// Post: on error, the machine is the one given.
    pub fn step(
        &mut self,
        now: Millis,
        command: Command<'_>,
        entropy: impl FnOnce() -> Result<Entropy, Error>,
    ) -> Result<Vec<Shape>, Error> {
        let input = match command {
            Command::Pair { token, name } => Input::Pair {
                offer: Offer::from_token(token)?,
                name: DeviceName::new(name)?,
                entropy: entropy()?,
            },
            Command::Connect => Input::Connect {
                entropy: entropy()?,
            },
            Command::Receive { from, frame } => Input::Receive {
                from: PeerId::new(from.as_bytes())?,
                frame: frame.to_vec(),
            },
            Command::Send(payload) => Input::Send(Payload::new(payload.to_vec())?),
            Command::Close => Input::Close,
            Command::Tick => Input::Tick,
        };
        let state = self.0.take().ok_or(Error::Stepping)?;
        let (state, outcome) = state.step(now, input);
        self.0 = Some(state);
        Ok(outcome?.into_iter().map(shape).collect())
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use lispm_remote_protocol::ENTROPY_LEN;
    use lispm_remote_protocol::Fingerprint;
    use lispm_remote_protocol::Refusal;
    use lispm_remote_protocol::Responder;
    use lispm_remote_protocol::device::CloseReason;
    use lispm_remote_protocol::lispm;

    use super::*;

    const LISPM: &str = "0x1111111111111111111111111111111111111111";
    const PHONE: &str = "0x2222222222222222222222222222222222222222";
    const START: Millis = Millis::new(1_000_000);

    fn entropy(seed: u8) -> impl FnOnce() -> Result<Entropy, Error> {
        move || Ok(Entropy::new([seed; ENTROPY_LEN]))
    }

    fn secret(seed: u8) -> String {
        generate_static(Entropy::new([seed; ENTROPY_LEN]))
    }

    /// LispM's machine, stepped as the module steps it.
    struct Lispm(Option<Responder>);

    impl Lispm {
        fn new() -> Self {
            let secret =
                StaticSecret::from_bytes(key_bytes(&secret(1), "a static secret").unwrap());
            let identity = Identity::new(PeerId::new(LISPM.as_bytes()).unwrap(), secret).unwrap();
            Self(Some(Responder::new(identity, Vec::new())))
        }

        fn step(&mut self, now: Millis, input: lispm::Input) -> Vec<lispm::Effect> {
            let (state, outcome) = self.0.take().unwrap().step(now, input);
            self.0 = Some(state);
            outcome.unwrap()
        }

        fn offer(&mut self) -> String {
            let effects = self.step(
                START,
                lispm::Input::Offer {
                    expires: START.after(Duration::from_secs(120)),
                    entropy: Entropy::new([3; ENTROPY_LEN]),
                },
            );
            effects
                .into_iter()
                .find_map(|effect| match effect {
                    lispm::Effect::ShowOffer(token) => Some(token.expose().to_owned()),
                    _ => None,
                })
                .unwrap()
        }

        /// Feeds LispM the frames among the phone's `shapes`.
        fn hear(&mut self, now: Millis, shapes: &[Shape]) -> Vec<lispm::Effect> {
            shapes
                .iter()
                .filter(|shape| shape.kind == "transmit")
                .flat_map(|shape| {
                    assert_eq!(shape.text("to"), Some(LISPM));
                    self.step(
                        now,
                        lispm::Input::Receive {
                            from: PeerId::new(PHONE.as_bytes()).unwrap(),
                            frame: shape.bytes("frame").unwrap().to_vec(),
                            entropy: Entropy::new([4; ENTROPY_LEN]),
                        },
                    )
                })
                .collect()
        }
    }

    /// Feeds the phone the frames among LispM's `effects`.
    fn hear(phone: &mut Machine, now: Millis, effects: Vec<lispm::Effect>) -> Vec<Shape> {
        effects
            .into_iter()
            .filter_map(|effect| match effect {
                lispm::Effect::Transmit { to, frame } => {
                    assert_eq!(to.to_string(), PHONE);
                    Some(frame)
                }
                _ => None,
            })
            .flat_map(|frame| {
                phone
                    .step(
                        now,
                        Command::Receive {
                            from: LISPM,
                            frame: &frame,
                        },
                        entropy(9),
                    )
                    .unwrap()
            })
            .collect()
    }

    fn kinds(shapes: &[Shape]) -> Vec<&'static str> {
        shapes.iter().map(|shape| shape.kind).collect()
    }

    /// Pairs a fresh phone with `lispm`; returns the phone and what it
    /// stored.
    fn paired(lispm: &mut Lispm) -> (Machine, Pairing) {
        let token = lispm.offer();
        let mut phone = Machine::new(&secret(2), PHONE, None).unwrap();
        let claim = phone
            .step(
                START,
                Command::Pair {
                    token: &token,
                    name: "Phone",
                },
                entropy(5),
            )
            .unwrap();
        assert_eq!(kinds(&claim), ["transmit", "show-fingerprint"]);
        let shown: Fingerprint = claim[1].text("fingerprint").unwrap().parse().unwrap();
        let asked = lispm.hear(START, &claim);
        assert!(asked.iter().any(|effect| matches!(
            effect,
            lispm::Effect::AskOwner { fingerprint, .. } if *fingerprint == shown
        )));
        let accept = lispm.step(START, lispm::Input::Approve { fingerprint: shown });
        let done = hear(&mut phone, START, accept);
        assert_eq!(kinds(&done), ["paired", "opened"]);
        assert_eq!(done[0].text("responder"), Some(LISPM));
        let stored = pairing(
            done[0].text("responder").unwrap(),
            done[0].text("key").unwrap(),
        )
        .unwrap();
        assert!(phone.is_open());
        (phone, stored)
    }

    #[test]
    fn a_phone_pairs_and_its_payloads_reach_lispm() {
        let mut lispm = Lispm::new();
        let (mut phone, _) = paired(&mut lispm);
        let sent = phone
            .step(START, Command::Send(br#"{"type":"state?"}"#), entropy(6))
            .unwrap();
        assert_eq!(kinds(&sent), ["transmit"]);
        let delivered = lispm.hear(START, &sent);
        assert!(delivered.iter().any(|effect| matches!(
            effect,
            lispm::Effect::Delivered { payload, .. } if payload.as_bytes() == br#"{"type":"state?"}"#
        )));
    }

    #[test]
    fn a_reloaded_phone_opens_a_session_from_its_stored_pairing() {
        let mut lispm = Lispm::new();
        let (_, stored) = paired(&mut lispm);
        let mut phone = Machine::new(&secret(2), PHONE, Some(stored)).unwrap();
        assert!(!phone.is_open());
        let open = phone.step(START, Command::Connect, entropy(7)).unwrap();
        let answer = lispm.hear(START, &open);
        let opened = hear(&mut phone, START, answer);
        assert_eq!(kinds(&opened), ["opened"]);
        assert!(phone.is_open());
        let closed = phone.step(START, Command::Close, entropy(8)).unwrap();
        assert_eq!(kinds(&closed), ["transmit", "closed"]);
        assert_eq!(closed[1].text("reason"), Some("by-us"));
    }

    #[test]
    fn a_waiting_phone_sends_its_frame_again_and_then_gives_up() {
        let mut lispm = Lispm::new();
        let (_, stored) = paired(&mut lispm);
        let mut phone = Machine::new(&secret(2), PHONE, Some(stored)).unwrap();
        let first = phone.step(START, Command::Connect, entropy(7)).unwrap();
        let again = phone
            .step(
                START.after(Duration::from_secs(2)),
                Command::Tick,
                entropy(0),
            )
            .unwrap();
        assert_eq!(kinds(&again), ["transmit"]);
        assert_eq!(again[0].bytes("frame"), first[0].bytes("frame"));
        let late = phone
            .step(
                START.after(Duration::from_secs(31)),
                Command::Tick,
                entropy(0),
            )
            .unwrap();
        assert_eq!(kinds(&late), ["failed"]);
        assert_eq!(late[0].text("reason"), Some("timed-out"));
    }

    #[test]
    fn a_forgotten_phone_learns_it_is_no_longer_paired() {
        let mut lispm = Lispm::new();
        let (_, stored) = paired(&mut lispm);
        lispm.step(
            START,
            lispm::Input::Forget {
                peer: PeerId::new(PHONE.as_bytes()).unwrap(),
            },
        );
        let mut phone = Machine::new(&secret(2), PHONE, Some(stored)).unwrap();
        let open = phone.step(START, Command::Connect, entropy(7)).unwrap();
        let refused = hear(&mut phone, START, lispm.hear(START, &open));
        assert_eq!(kinds(&refused), ["unpaired", "failed"]);
        assert_eq!(refused[1].text("reason"), Some("refused"));
        assert_eq!(refused[1].text("detail"), Some("not-paired"));
    }

    #[test]
    fn a_phone_hears_which_version_lispm_speaks() {
        let mut lispm = Lispm::new();
        let (_, stored) = paired(&mut lispm);
        let mut phone = Machine::new(&secret(2), PHONE, Some(stored)).unwrap();
        let open = phone.step(START, Command::Connect, entropy(7)).unwrap();
        // LispM of a later version answers in its own.
        let later = VERSION + 1;
        let answer = lispm
            .hear(START, &open)
            .into_iter()
            .map(|effect| match effect {
                lispm::Effect::Transmit { to, mut frame } => {
                    frame[0] = later;
                    lispm::Effect::Transmit { to, frame }
                }
                other => other,
            })
            .collect();
        let failed = hear(&mut phone, START, answer);
        assert_eq!(kinds(&failed), ["failed"]);
        assert_eq!(failed[0].text("reason"), Some("other-version"));
        assert_eq!(failed[0].text("detail"), Some(later.to_string().as_str()));
        assert_eq!(failed[0].text("ours"), Some(VERSION.to_string().as_str()));
    }

    #[test]
    fn a_refused_command_leaves_the_machine_as_it_was() {
        let mut phone = Machine::new(&secret(2), PHONE, None).unwrap();
        assert!(matches!(
            phone.step(START, Command::Connect, entropy(7)),
            Err(Error::Command(CommandError::NotPaired))
        ));
        assert!(matches!(
            phone.step(START, Command::Send(b"x"), entropy(7)),
            Err(Error::Command(CommandError::NotOpen))
        ));
        assert!(matches!(
            phone.step(
                START,
                Command::Pair {
                    token: "not a token",
                    name: "Phone"
                },
                entropy(7)
            ),
            Err(Error::Offer(_))
        ));
        assert!(
            phone
                .step(START, Command::Tick, entropy(7))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_expired_offer_is_refused_before_anything_is_sent() {
        let mut lispm = Lispm::new();
        let token = lispm.offer();
        let mut phone = Machine::new(&secret(2), PHONE, None).unwrap();
        let late = START.after(Duration::from_secs(121));
        assert!(matches!(
            phone.step(
                late,
                Command::Pair {
                    token: &token,
                    name: "Phone"
                },
                entropy(5)
            ),
            Err(Error::Command(CommandError::OfferExpired))
        ));
    }

    #[test]
    fn stored_values_are_checked() {
        assert!(matches!(
            Machine::new("abcd", PHONE, None),
            Err(Error::Hex("a static secret"))
        ));
        assert!(matches!(
            Machine::new(&secret(2), "", None),
            Err(Error::Id(_))
        ));
        assert!(matches!(pairing(LISPM, "zz"), Err(Error::Hex("a key"))));
        assert!(matches!(
            pairing("", &"00".repeat(KEY_LEN)),
            Err(Error::Id(_))
        ));
        assert!(pairing(LISPM, &"00".repeat(KEY_LEN)).is_ok());
    }

    #[test]
    fn static_secrets_are_hex_and_follow_their_entropy() {
        assert_eq!(secret(1), secret(1));
        assert_ne!(secret(1), secret(2));
        assert_eq!(secret(1).len(), 2 * KEY_LEN);
    }

    #[test]
    fn variants_are_named_in_kebab_case() {
        assert_eq!(kebab(CloseReason::ByPeer), "by-peer");
        assert_eq!(kebab(Refusal::OfferUnavailable), "offer-unavailable");
        assert_eq!(kebab(Failure::TimedOut), "timed-out");
    }

    #[test]
    fn the_clock_is_read_as_whole_milliseconds() {
        assert_eq!(whole(1_761_234_567_890.0), Some(1_761_234_567_890));
        assert_eq!(whole(2.75), Some(2));
        assert_eq!(whole(0.0), Some(0));
        assert_eq!(whole(-1.0), None);
        assert_eq!(whole(f64::NAN), None);
        assert_eq!(whole(f64::INFINITY), None);
        assert_eq!(moment(-5.0), Millis::new(0));
        assert_eq!(moment(1_500.0), Millis::new(1_500));
    }
}
