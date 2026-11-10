use crate::{Error, Result};
use alloc::string::String;

const DIGITS: &[u8; 16] = b"0123456789abcdef";

pub fn encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    text
}

pub fn decode_array<const N: usize>(text: &str) -> Result<[u8; N]> {
    let bytes = text.as_bytes();
    if bytes.len() != N * 2 {
        return Err(Error::InvalidHex);
    }
    let mut result = [0_u8; N];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        result[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(result)
}

fn nibble(digit: u8) -> Result<u8> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => Err(Error::InvalidHex),
    }
}

/// Fixed-size bytes on the wire: hex text in human-readable formats (JSON), raw bytes in compact
/// ones (the binary form a hardware signer receives).
pub(crate) fn serialize<S: serde::Serializer, const N: usize>(
    bytes: &[u8; N],
    serializer: S,
) -> core::result::Result<S::Ok, S::Error> {
    if serializer.is_human_readable() {
        serializer.serialize_str(&encode(bytes))
    } else {
        serializer.serialize_bytes(bytes)
    }
}

pub(crate) fn deserialize<'de, D: serde::Deserializer<'de>, const N: usize>(
    deserializer: D,
) -> core::result::Result<[u8; N], D::Error> {
    use serde::{Deserialize, de::Error as _};
    if deserializer.is_human_readable() {
        let text = <alloc::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        return decode_array(&text).map_err(D::Error::custom);
    }
    struct Bytes<const N: usize>;
    impl<const N: usize> serde::de::Visitor<'_> for Bytes<N> {
        type Value = [u8; N];

        fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(formatter, "{N} bytes")
        }

        fn visit_bytes<E: serde::de::Error>(
            self,
            bytes: &[u8],
        ) -> core::result::Result<[u8; N], E> {
            bytes
                .try_into()
                .map_err(|_| E::invalid_length(bytes.len(), &self))
        }
    }
    deserializer.deserialize_bytes(Bytes::<N>)
}
