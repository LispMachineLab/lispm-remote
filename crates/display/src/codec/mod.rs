// lispm-remote: display messages as bytes.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The codec.  A message is
//!
//! ```text
//!   message = version:u8 ‖ tag:u8 ‖ fields
//!   uint    = LEB128                      (u16, u32 and u64 alike, range-checked)
//!   int     = zigzag(LEB128)
//!   string  = len:uint ‖ UTF-8
//!   vec T   = count:uint ‖ T*
//!   row     = cells:uint ‖ run*           run = style ‖ count:uint ‖ content*
//! ```
//!
//! Law: `decode(encode(m)) = Ok(m)` for every message `m`, and `decode`
//! is total: any byte string yields a message or a [`CodecError`], never
//! a panic, and never allocates more than the input could describe.

mod messages;
mod model;
#[cfg(test)]
mod tests;

use crate::screen::SizeError;

/// The protocol version every message starts with.
pub const VERSION: u8 = 1;
/// The longest message: one record of LispM Remote's channel holds it.
pub const MAX_MESSAGE_LEN: usize = 64_000;

/// Why bytes are not a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// The bytes end inside a field.
    #[error("the message is cut short")]
    Truncated,
    /// The message is longer than [`MAX_MESSAGE_LEN`].
    #[error("the message is too long")]
    TooLong,
    /// Another version of the protocol.
    #[error("protocol version {0} is not this one")]
    Version(u8),
    /// A tag names nothing.
    #[error("{what} tag {tag} is unknown")]
    Tag {
        /// What the tag chooses.
        what: &'static str,
        /// The tag.
        tag: u8,
    },
    /// A number does not fit its field.
    #[error("{0} is out of range")]
    Range(&'static str),
    /// Text is not UTF-8, or a character is not a Unicode scalar value.
    #[error("the text is not valid")]
    Text,
    /// A size is not a screen size.
    #[error("{0}")]
    Size(SizeError),
    /// A row's runs do not add up to its length.
    #[error("a row's runs do not add up")]
    Runs,
    /// Bytes are left after the message.
    #[error("{0} bytes follow the message")]
    Trailing(usize),
}

/// Bytes being read, front to back.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub(crate) const fn remaining(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn byte(&mut self) -> Result<u8, CodecError> {
        let (&first, rest) = self.bytes.split_first().ok_or(CodecError::Truncated)?;
        self.bytes = rest;
        Ok(first)
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        if n > self.bytes.len() {
            return Err(CodecError::Truncated);
        }
        let (taken, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(taken)
    }

    /// A LEB128 number.  Post: at most ten bytes are read, and a number
    /// past `u64::MAX` is an error.
    pub(crate) fn varint(&mut self) -> Result<u64, CodecError> {
        let mut value = 0_u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            let bits = u64::from(byte & 0x7f);
            if shift == 63 && bits > 1 {
                return Err(CodecError::Range("a number"));
            }
            value |= bits << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(CodecError::Range("a number"))
    }

    /// A count of items each at least one byte long.  Post: the count is
    /// at most the bytes left, so allocating for it is bounded.
    pub(crate) fn count(&mut self) -> Result<usize, CodecError> {
        let count = usize::try_from(self.varint()?).map_err(|_| CodecError::Range("a count"))?;
        if count > self.remaining() {
            Err(CodecError::Truncated)
        } else {
            Ok(count)
        }
    }
}

pub(crate) fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = u8::try_from(value & 0x7f).unwrap_or(0);
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

/// A value with a byte form.
///
/// Law: `get(put(v)) = Ok(v)`, reading exactly what `put` wrote.
pub(crate) trait Wire: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError>;
}

/// `Wire` for a struct, field by field in the order listed.
macro_rules! wire_struct {
    ($name:ident { $($field:ident),+ $(,)? }) => {
        impl $crate::codec::Wire for $name {
            fn put(&self, out: &mut Vec<u8>) {
                $($crate::codec::Wire::put(&self.$field, out);)+
            }

            fn get(input: &mut $crate::codec::Reader<'_>) -> Result<Self, $crate::codec::CodecError> {
                Ok(Self { $($field: $crate::codec::Wire::get(input)?,)+ })
            }
        }
    };
}

/// `Wire` for an enum of unit variants, one byte each, from one table so
/// that writing and reading are inverse by construction.
macro_rules! wire_tags {
    ($name:ident ($what:literal) { $($variant:ident = $code:literal),+ $(,)? }) => {
        impl $crate::codec::Wire for $name {
            fn put(&self, out: &mut Vec<u8>) {
                out.push(match self {
                    $(Self::$variant => $code,)+
                });
            }

            fn get(input: &mut $crate::codec::Reader<'_>) -> Result<Self, $crate::codec::CodecError> {
                match input.byte()? {
                    $($code => Ok(Self::$variant),)+
                    tag => Err($crate::codec::CodecError::Tag { what: $what, tag }),
                }
            }
        }
    };
}

pub(crate) use wire_struct;
pub(crate) use wire_tags;

/// `Wire` for an unsigned integer, as a range-checked LEB128 number.
macro_rules! wire_uint {
    ($($int:ty),+) => {$(
        impl Wire for $int {
            fn put(&self, out: &mut Vec<u8>) {
                put_varint(out, u64::from(*self));
            }

            fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
                <$int>::try_from(input.varint()?).map_err(|_| CodecError::Range(stringify!($int)))
            }
        }
    )+};
}

wire_uint!(u16, u32);

impl Wire for u64 {
    fn put(&self, out: &mut Vec<u8>) {
        put_varint(out, *self);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        input.varint()
    }
}

impl Wire for u8 {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(*self);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        input.byte()
    }
}

impl Wire for i32 {
    fn put(&self, out: &mut Vec<u8>) {
        let value = i64::from(*self);
        let zigzag = if value >= 0 {
            u64::try_from(value).unwrap_or(0) << 1
        } else {
            (u64::try_from(-(value + 1)).unwrap_or(0) << 1) | 1
        };
        put_varint(out, zigzag);
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        let zigzag = input.varint()?;
        let half = i64::try_from(zigzag >> 1).map_err(|_| CodecError::Range("i32"))?;
        let value = if zigzag & 1 == 0 { half } else { -half - 1 };
        Self::try_from(value).map_err(|_| CodecError::Range("i32"))
    }
}

impl Wire for bool {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        match input.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(CodecError::Tag { what: "bool", tag }),
        }
    }
}

impl Wire for char {
    fn put(&self, out: &mut Vec<u8>) {
        put_varint(out, u64::from(u32::from(*self)));
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        let code = u32::try_from(input.varint()?).map_err(|_| CodecError::Text)?;
        Self::from_u32(code).ok_or(CodecError::Text)
    }
}

impl Wire for String {
    fn put(&self, out: &mut Vec<u8>) {
        put_varint(out, u64::try_from(self.len()).unwrap_or(u64::MAX));
        out.extend_from_slice(self.as_bytes());
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        let len = input.count()?;
        let bytes = input.take(len)?;
        Self::from_utf8(bytes.to_vec()).map_err(|_| CodecError::Text)
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, out: &mut Vec<u8>) {
        put_varint(out, u64::try_from(self.len()).unwrap_or(u64::MAX));
        for item in self {
            item.put(out);
        }
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        let count = input.count()?;
        let mut items = Self::with_capacity(count);
        for _ in 0..count {
            items.push(T::get(input)?);
        }
        Ok(items)
    }
}

impl<T: Wire> Wire for Option<T> {
    fn put(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(value) => {
                out.push(1);
                value.put(out);
            }
        }
    }

    fn get(input: &mut Reader<'_>) -> Result<Self, CodecError> {
        match input.byte()? {
            0 => Ok(None),
            1 => Ok(Some(T::get(input)?)),
            tag => Err(CodecError::Tag {
                what: "option",
                tag,
            }),
        }
    }
}

/// The bytes of `value` behind the version and `tag`.
pub(crate) fn encode_tagged(tag: u8, body: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut out = vec![VERSION, tag];
    body(&mut out);
    out
}

/// Reads a whole message: checks its length and version, gives its tag
/// and the reader to `body`, and refuses bytes left over.
pub(crate) fn decode_tagged<T>(
    bytes: &[u8],
    body: impl FnOnce(u8, &mut Reader<'_>) -> Result<T, CodecError>,
) -> Result<T, CodecError> {
    if bytes.len() > MAX_MESSAGE_LEN {
        return Err(CodecError::TooLong);
    }
    let mut input = Reader::new(bytes);
    let version = input.byte()?;
    if version != VERSION {
        return Err(CodecError::Version(version));
    }
    let tag = input.byte()?;
    let value = body(tag, &mut input)?;
    match input.remaining() {
        0 => Ok(value),
        left => Err(CodecError::Trailing(left)),
    }
}
