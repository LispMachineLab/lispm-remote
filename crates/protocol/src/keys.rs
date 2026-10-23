// lispm-remote: the keys of a pairing.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The algebra is X25519's: the Montgomery curve `v² = u³ + 486662u² + u`
//! over `F_p`, `p = 2²⁵⁵ − 19`, whose points carry the group used for
//! Diffie–Hellman.  A [`StaticSecret`] `a` is a scalar; X25519 clamps it
//! (clears its three low bits, making it a multiple of the cofactor 8, and
//! sets bit 254), so every 32-byte string is a valid secret.  Its
//! [`PublicKey`] is the u-coordinate of `a·B` for the base point `B`, and
//! the shared secret of `a` and `b·B` is `a·(b·B) = b·(a·B)`.  Noise
//! performs every such product; this module only names the carriers:
//!
//! ```text
//!   StaticSecret = {0,1}²⁵⁶          (clamped on use)
//!   PublicKey    = u(a·B) ∈ F_p      (32 bytes, little-endian)
//!   Psk          = {0,1}²⁵⁶          (one-time, from the offer)
//!   Fingerprint  = BLAKE2s(label ‖ h)[0..4], h the handshake hash after
//!                  the first message, equal at both ends
//! ```

use core::fmt;

use blake2::Blake2s256;
use blake2::Digest;
use snow::params::DHChoice;
use snow::resolvers::CryptoResolver;
use snow::resolvers::DefaultResolver;
use zeroize::Zeroize;
use zeroize::ZeroizeOnDrop;

use crate::entropy::Entropy;
use crate::entropy::Purpose;
use crate::ids::PeerId;

/// The length of keys and pre-shared keys in bytes.
pub const KEY_LEN: usize = 32;

/// An X25519 public key.
///
/// Law: `Copy` is sound because a public key is a published value;
/// duplicating it confers no authority.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PublicKey([u8; KEY_LEN]);

impl PublicKey {
    /// The key whose encoding is `bytes`.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// The key's encoding.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    pub(crate) fn from_slice(bytes: &[u8]) -> Option<Self> {
        <[u8; KEY_LEN]>::try_from(bytes).ok().map(Self)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PublicKey(")?;
        for byte in self.0.iter().take(4) {
            write!(f, "{byte:02x}")?;
        }
        f.write_str("..)")
    }
}

/// An X25519 secret key, kept by the side it belongs to.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct StaticSecret([u8; KEY_LEN]);

impl StaticSecret {
    /// A new secret derived from `entropy`.
    #[must_use]
    pub fn generate(entropy: Entropy) -> Self {
        Self(*entropy.derive(Purpose::Static).bytes())
    }

    /// The secret whose encoding is `bytes`, as the shell stored it.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// The secret's encoding, for the shell to store and for nothing else.
    #[must_use]
    pub const fn expose(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    fn public_key(&self) -> Result<PublicKey, KeyError> {
        let mut dh = DefaultResolver
            .resolve_dh(&DHChoice::Curve25519)
            .ok_or(KeyError::Unsupported)?;
        dh.set(&self.0);
        PublicKey::from_slice(dh.pubkey()).ok_or(KeyError::Unsupported)
    }
}

impl fmt::Debug for StaticSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StaticSecret(..)")
    }
}

/// A one-time pre-shared key: the secret half of a pairing offer.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Psk([u8; KEY_LEN]);

impl Psk {
    pub(crate) fn generate(entropy: &Entropy) -> Self {
        Self(*entropy.derive(Purpose::Psk).bytes())
    }

    pub(crate) const fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for Psk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Psk(..)")
    }
}

/// Four bytes both ends of a pairing show, for the owner to compare.
///
/// Law: `Copy` is sound because a fingerprint is a published digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 4]);

impl Fingerprint {
    const LABEL: &'static [u8] = b"lispm-remote/1 fingerprint";

    /// The fingerprint of the handshake hash `hash`.
    pub(crate) fn of_handshake(hash: &[u8]) -> Self {
        let digest = Blake2s256::new()
            .chain_update(Self::LABEL)
            .chain_update(hash)
            .finalize();
        let mut bytes = [0_u8; 4];
        for (out, byte) in bytes.iter_mut().zip(digest.iter()) {
            *out = *byte;
        }
        Self(bytes)
    }

    /// The fingerprint's bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.0;
        write!(f, "{a:02X}{b:02X}-{c:02X}{d:02X}")
    }
}

/// Law: `s.parse::<Fingerprint>() = Ok(f)` for `s = f.to_string()`; case
/// is ignored, so a fingerprint typed in lower case reads the same.
impl core::str::FromStr for Fingerprint {
    type Err = KeyError;

    fn from_str(text: &str) -> Result<Self, KeyError> {
        let (high, low) = text.split_once('-').ok_or(KeyError::Fingerprint)?;
        let half = |part: &str| {
            if part.len() == 4 && part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                u16::from_str_radix(part, 16).map_err(|_| KeyError::Fingerprint)
            } else {
                Err(KeyError::Fingerprint)
            }
        };
        let [a, b] = half(high)?.to_be_bytes();
        let [c, d] = half(low)?.to_be_bytes();
        Ok(Self([a, b, c, d]))
    }
}

/// Who one end is: its transport address and its static key pair.
///
/// Invariant: `public` is the public key of `secret`; [`Identity::new`]
/// is the only constructor.
#[derive(Debug)]
pub struct Identity {
    peer: PeerId,
    secret: StaticSecret,
    public: PublicKey,
}

impl Identity {
    /// The identity of the end addressed as `peer` that holds `secret`.
    ///
    /// # Errors
    ///
    /// [`KeyError::Unsupported`] if X25519 is missing from the build.
    pub fn new(peer: PeerId, secret: StaticSecret) -> Result<Self, KeyError> {
        let public = secret.public_key()?;
        Ok(Self {
            peer,
            secret,
            public,
        })
    }

    /// The transport address.
    #[must_use]
    pub const fn peer(&self) -> &PeerId {
        &self.peer
    }

    /// The static public key.
    #[must_use]
    pub const fn public_key(&self) -> &PublicKey {
        &self.public
    }

    pub(crate) const fn secret(&self) -> &StaticSecret {
        &self.secret
    }
}

/// Why a key could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The build has no X25519.
    #[error("X25519 is not available in this build")]
    Unsupported,
    /// Text is not a fingerprint: two groups of four hexadecimal digits.
    #[error("a fingerprint is two groups of four hexadecimal digits, as 4F2A-91C0")]
    Fingerprint,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::ENTROPY_LEN;

    #[test]
    fn a_secret_has_one_public_key() {
        let one = StaticSecret::from_bytes([5; KEY_LEN]).public_key().unwrap();
        let two = StaticSecret::from_bytes([5; KEY_LEN]).public_key().unwrap();
        let other = StaticSecret::from_bytes([6; KEY_LEN]).public_key().unwrap();
        assert_eq!(one, two);
        assert_ne!(one, other);
    }

    #[test]
    fn the_rfc_7748_base_point_multiple_is_reproduced() {
        // RFC 7748, section 6.1: Alice's private and public keys.
        let secret: [u8; KEY_LEN] = [
            0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2,
            0x66, 0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5,
            0x1d, 0xb9, 0x2c, 0x2a,
        ];
        let public: [u8; KEY_LEN] = [
            0x85, 0x20, 0xf0, 0x09, 0x89, 0x30, 0xa7, 0x54, 0x74, 0x8b, 0x7d, 0xdc, 0xb4, 0x3e,
            0xf7, 0x5a, 0x0d, 0xbf, 0x3a, 0x0d, 0x26, 0x38, 0x1a, 0xf4, 0xeb, 0xa4, 0xa9, 0x8e,
            0xaa, 0x9b, 0x4e, 0x6a,
        ];
        let derived = StaticSecret::from_bytes(secret).public_key().unwrap();
        assert_eq!(derived, PublicKey::from_bytes(public));
    }

    #[test]
    fn generated_secrets_follow_their_entropy() {
        let a = StaticSecret::generate(Entropy::new([1; ENTROPY_LEN]));
        let b = StaticSecret::generate(Entropy::new([1; ENTROPY_LEN]));
        let c = StaticSecret::generate(Entropy::new([2; ENTROPY_LEN]));
        assert_eq!(a.expose(), b.expose());
        assert_ne!(a.expose(), c.expose());
    }

    #[test]
    fn a_fingerprint_reads_as_two_groups_of_hex() {
        let text = Fingerprint([0x4f, 0x2a, 0x91, 0xc0]).to_string();
        assert_eq!(text, "4F2A-91C0");
    }

    #[test]
    fn a_fingerprint_reads_back_from_its_text_in_either_case() {
        let fingerprint = Fingerprint([0x4f, 0x2a, 0x91, 0xc0]);
        assert_eq!("4F2A-91C0".parse(), Ok(fingerprint));
        assert_eq!("4f2a-91c0".parse(), Ok(fingerprint));
        for bad in ["4F2A91C0", "4F2A-91C", "4F2A-91CG", "+F2A-91C0", ""] {
            assert_eq!(
                bad.parse::<Fingerprint>(),
                Err(KeyError::Fingerprint),
                "{bad}"
            );
        }
    }

    #[test]
    fn secrets_do_not_print() {
        assert_eq!(
            format!("{:?}", StaticSecret::from_bytes([1; KEY_LEN])),
            "StaticSecret(..)"
        );
        assert_eq!(format!("{:?}", Psk::from_bytes([1; KEY_LEN])), "Psk(..)");
    }
}
