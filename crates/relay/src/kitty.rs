// lispm-remote: Kitty graphics commands.
//
// Copyright (C) 2026  Ryan.K
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The part of the Kitty graphics protocol LispM uses, read from an APC
//! payload:
//!
//! ```text
//!   payload = 'G' ‖ key=value (',' key=value)* [';' base64]
//!   value   = integer | one character
//! ```
//!
//! Keys this relay acts on: `a` action, `q` quiet, `f` format, `t`
//! medium, `m` more chunks, `i` image, `p` placement, `c` `r` cells,
//! `x` `y` `w` `h` crop, `X` `Y` offset, `z` layer, `C` cursor stays,
//! `d` what to delete.  Others are ignored, as Kitty ignores unknown keys.

/// What a command asks for.
///
/// Law: `Copy` is sound because an action is a tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Store an image (`a=t`).
    Transmit,
    /// Store an image and place it (`a=T`).
    TransmitAndPut,
    /// Place a stored image (`a=p`).
    Put,
    /// Delete placements or images (`a=d`).
    Delete,
    /// Ask whether graphics work (`a=q`).
    Query,
    /// Anything else, which is ignored.
    Other(u8),
}

/// One graphics command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// What it asks for.
    pub action: Action,
    /// 0 answers everything, 1 only errors, 2 nothing.
    pub quiet: u32,
    /// The data's format: 100 is PNG.
    pub format: u32,
    /// Where the data is: `d` in the payload.
    pub medium: u8,
    /// Whether more chunks of the data follow.
    pub more: bool,
    /// The image's number.
    pub image: u32,
    /// The placement's number.
    pub placement: u32,
    /// The columns to cover, 0 for as many as the image needs.
    pub cols: u16,
    /// The rows to cover, 0 for as many as the image needs.
    pub rows: u16,
    /// The crop's left edge.
    pub x: u32,
    /// The crop's top edge.
    pub y: u32,
    /// The crop's width, 0 for the rest of the image.
    pub w: u32,
    /// The crop's height, 0 for the rest of the image.
    pub h: u32,
    /// Pixels right of the cell's edge.
    pub offset_x: u16,
    /// Pixels below the cell's edge.
    pub offset_y: u16,
    /// The layer.
    pub z: i32,
    /// Whether the cursor stays where it is.
    pub cursor_stays: bool,
    /// What to delete (`a` all, `i` by image, upper case also frees data).
    pub delete: u8,
    /// Whether the command named any key besides `m` and `q`: a chunk
    /// after the first names none.
    pub keyed: bool,
    /// The payload, still base64.
    pub data: Vec<u8>,
}

impl Command {
    /// The command an APC payload holds, or `None` when it is not a
    /// graphics command.
    #[must_use]
    pub fn parse(apc: &[u8]) -> Option<Self> {
        let body = apc.strip_prefix(b"G")?;
        let (keys, data) = match body.iter().position(|&byte| byte == b';') {
            Some(at) => (body.get(..at)?, body.get(at + 1..)?),
            None => (body, &[][..]),
        };
        let mut command = Self::empty(data.to_vec());
        for pair in keys
            .split(|&byte| byte == b',')
            .filter(|pair| !pair.is_empty())
        {
            let [key, b'=', value @ ..] = pair else {
                return None;
            };
            command.set(*key, Value::parse(value)?);
        }
        Some(command)
    }

    const fn empty(data: Vec<u8>) -> Self {
        Self {
            action: Action::Transmit,
            quiet: 0,
            format: 32,
            medium: b'd',
            more: false,
            image: 0,
            placement: 0,
            cols: 0,
            rows: 0,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            offset_x: 0,
            offset_y: 0,
            z: 0,
            cursor_stays: false,
            delete: b'a',
            keyed: false,
            data,
        }
    }

    fn set(&mut self, key: u8, value: Value) {
        if !matches!(key, b'm' | b'q') {
            self.keyed = true;
        }
        match (key, value) {
            (b'a', Value::Char(action)) => self.action = Action::of(action),
            (b'q', Value::Int(n)) => self.quiet = clamp(n),
            (b'f', Value::Int(n)) => self.format = clamp(n),
            (b't', Value::Char(medium)) => self.medium = medium,
            (b'm', Value::Int(n)) => self.more = n != 0,
            (b'i', Value::Int(n)) => self.image = clamp(n),
            (b'p', Value::Int(n)) => self.placement = clamp(n),
            (b'c', Value::Int(n)) => self.cols = clamp(n),
            (b'r', Value::Int(n)) => self.rows = clamp(n),
            (b'x', Value::Int(n)) => self.x = clamp(n),
            (b'y', Value::Int(n)) => self.y = clamp(n),
            (b'w', Value::Int(n)) => self.w = clamp(n),
            (b'h', Value::Int(n)) => self.h = clamp(n),
            (b'X', Value::Int(n)) => self.offset_x = clamp(n),
            (b'Y', Value::Int(n)) => self.offset_y = clamp(n),
            (b'z', Value::Int(n)) => self.z = clamp(n),
            (b'C', Value::Int(n)) => self.cursor_stays = n != 0,
            (b'd', Value::Char(what)) => self.delete = what,
            _ => {}
        }
    }

    /// Whether this command is a later chunk of the transmission open
    /// for `image`.
    #[must_use]
    pub const fn continues(&self, image: u32) -> bool {
        !self.keyed
            || (matches!(self.action, Action::Transmit | Action::TransmitAndPut)
                && self.image == image)
    }
}

impl Action {
    const fn of(byte: u8) -> Self {
        match byte {
            b't' => Self::Transmit,
            b'T' => Self::TransmitAndPut,
            b'p' => Self::Put,
            b'd' => Self::Delete,
            b'q' => Self::Query,
            other => Self::Other(other),
        }
    }
}

/// A key's value.
enum Value {
    Int(i64),
    Char(u8),
}

impl Value {
    fn parse(text: &[u8]) -> Option<Self> {
        let digits = text.strip_prefix(b"-").unwrap_or(text);
        if !digits.is_empty() && digits.len() <= 18 && digits.iter().all(u8::is_ascii_digit) {
            let magnitude = digits
                .iter()
                .fold(0_i64, |n, digit| n * 10 + i64::from(digit - b'0'));
            Some(Self::Int(if digits.len() == text.len() {
                magnitude
            } else {
                -magnitude
            }))
        } else if let [one] = text {
            Some(Self::Char(*one))
        } else {
            None
        }
    }
}

/// `n` in the target type, saturating at its bounds.
fn clamp<T: TryFrom<i64> + Bounded>(n: i64) -> T {
    T::try_from(n).unwrap_or(if n < 0 { T::MIN } else { T::MAX })
}

/// A number type's bounds.
trait Bounded {
    const MIN: Self;
    const MAX: Self;
}

macro_rules! bounded {
    ($($t:ty),+) => {$(
        impl Bounded for $t {
            const MIN: Self = <$t>::MIN;
            const MAX: Self = <$t>::MAX;
        }
    )+};
}

bounded!(u16, u32, i32);

/// The pixel size of a PNG, from its header.
#[must_use]
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !bytes.starts_with(SIGNATURE) || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let word = |at: usize| -> Option<u32> {
        let quad: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(quad))
    };
    Some((word(16)?, word(20)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placement_like_the_desktops_parses() {
        let command =
            Command::parse(b"Gq=2,a=p,i=7,p=3,c=4,r=2,z=-1,X=5,C=1,x=1,y=2,w=30,h=40").unwrap();
        assert_eq!(command.action, Action::Put);
        assert_eq!((command.image, command.placement), (7, 3));
        assert_eq!((command.cols, command.rows), (4, 2));
        assert_eq!((command.z, command.offset_x, command.quiet), (-1, 5, 2));
        assert_eq!((command.x, command.y, command.w, command.h), (1, 2, 30, 40));
        assert!(command.cursor_stays);
    }

    #[test]
    fn a_transmission_carries_its_data() {
        let command = Command::parse(b"Ga=t,q=2,f=100,i=9,m=1;iVBORw0K").unwrap();
        assert_eq!(command.action, Action::Transmit);
        assert_eq!(command.format, 100);
        assert!(command.more);
        assert_eq!(command.data, b"iVBORw0K".to_vec());
    }

    #[test]
    fn a_later_chunk_continues_its_transmission() {
        let bare = Command::parse(b"Gm=0;AAAA").unwrap();
        assert!(bare.continues(9));
        let keyed = Command::parse(b"Ga=t,i=9,m=0;AAAA").unwrap();
        assert!(keyed.continues(9));
        assert!(!keyed.continues(8));
    }

    #[test]
    fn other_apcs_and_bad_keys_are_not_commands() {
        assert_eq!(Command::parse(b"Xa=t"), None);
        assert_eq!(Command::parse(b"Ga=too"), None);
        assert_eq!(Command::parse(b"Gnokey"), None);
    }

    #[test]
    fn numbers_saturate_at_their_fields() {
        let command = Command::parse(b"Gc=99999,z=-99999999999").unwrap();
        assert_eq!(command.cols, u16::MAX);
        assert_eq!(command.z, i32::MIN);
    }

    #[test]
    fn a_png_header_gives_its_size() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&16_u32.to_be_bytes());
        png.extend_from_slice(&32_u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((16, 32)));
        assert_eq!(png_size(b"GIF89a"), None);
    }
}
