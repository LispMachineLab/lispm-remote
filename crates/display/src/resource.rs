// lispm-remote: resources, named by what they hold.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A resource (an image) is named by the SHA-256 of its bytes.  Two
//! surfaces, two sessions or two devices that show the same icon name it
//! the same way, so a client's pool fills once and serves every later
//! session; and a client checks every resource it receives against its
//! name before keeping it.

use sha2::Digest;
use sha2::Sha256;

/// The most bytes of a resource one message carries.
pub const RESOURCE_CHUNK: usize = 32 * 1024;

/// The SHA-256 of a resource's bytes.
///
/// Law: `Copy` is sound because a hash is a value; equal ids name equal
/// bytes (up to SHA-256 collisions).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId([u8; 32]);

impl ResourceId {
    /// The id of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// The id whose hash is `hash`.
    #[must_use]
    pub const fn from_hash(hash: [u8; 32]) -> Self {
        Self(hash)
    }

    /// The hash.
    #[must_use]
    pub const fn hash(&self) -> &[u8; 32] {
        &self.0
    }

    /// Whether `bytes` are the resource this id names.
    #[must_use]
    pub fn names(&self, bytes: &[u8]) -> bool {
        Self::of(bytes) == *self
    }

    /// The hash as 64 lowercase hexadecimal digits.
    #[must_use]
    pub fn to_hex(&self) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = String::with_capacity(64);
        for byte in self.0 {
            for nibble in [byte >> 4, byte & 0x0f] {
                if let Some(&digit) = DIGITS.get(usize::from(nibble)) {
                    text.push(char::from(digit));
                }
            }
        }
        text
    }

    /// The id written as 64 hexadecimal digits, or `None`.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.as_bytes();
        if digits.len() != 64 {
            return None;
        }
        let mut hash = [0_u8; 32];
        for (byte, pair) in hash.iter_mut().zip(digits.chunks_exact(2)) {
            let [high, low] = pair else { return None };
            *byte = (nibble(*high)? << 4) | nibble(*low)?;
        }
        Some(Self(hash))
    }
}

/// The value of one hexadecimal digit.
const fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_id_of_nothing_is_sha256_of_nothing() {
        assert_eq!(
            ResourceId::of(b"").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn an_id_names_only_its_bytes() {
        let id = ResourceId::of(b"icon");
        assert!(id.names(b"icon"));
        assert!(!id.names(b"icon!"));
    }

    #[test]
    fn bad_hex_is_refused() {
        assert_eq!(ResourceId::from_hex("00"), None);
        assert_eq!(ResourceId::from_hex(&"g".repeat(64)), None);
    }

    proptest! {
        // Law: ∀ h. from_hex(to_hex(h)) = Some(h).
        #[test]
        fn hex_round_trips(hash in any::<[u8; 32]>()) {
            let id = ResourceId::from_hash(hash);
            prop_assert_eq!(ResourceId::from_hex(&id.to_hex()), Some(id));
        }
    }
}
