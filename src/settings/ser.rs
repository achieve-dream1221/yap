use std::str::FromStr;

use bstr::ByteSlice;
use bstr::ByteVec;
use compact_str::ToCompactString;
use serde::Deserialize;
use serde::Serializer;

pub fn serialize_as_u8<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: Copy + Into<u8>,
    S: serde::Serializer,
{
    let val: u8 = (*value).into();
    serializer.serialize_u8(val)
}

pub fn deserialize_from_u8<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: TryFrom<u8>,
    D: serde::Deserializer<'de>,
    <T as TryFrom<u8>>::Error: std::fmt::Debug,
{
    let val = u8::deserialize(deserializer)?;
    T::try_from(val)
        .map_err(|e| serde::de::Error::custom(format!("Invalid value: {} ({:?})", val, e)))
}

pub fn serialize_line_ending<S>(input: &str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let buffer = input.as_bytes().escape_bytes().to_compact_string();
    serializer.serialize_str(&buffer)
}

pub fn deserialize_line_ending<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    match Vec::unescape_bytes(&s).into_string() {
        Ok(result) => Ok(result),
        Err(e) => Err(serde::de::Error::custom(format!(
            "Failed to unescape line ending string: {e}"
        ))),
    }
}

pub fn serialize_as_string<S, T>(input: T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: ToString,
{
    let buffer = input.to_string();
    serializer.serialize_str(&buffer)
}

pub fn deserialize_from_str<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: FromStr,
{
    let s = String::deserialize(deserializer)?;
    let generic: T = s
        .parse()
        .map_err(|_| serde::de::Error::custom(format!("Failed to parse line ending: \"{s}\"")))?;

    Ok(generic)
}
