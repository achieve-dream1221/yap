use std::str::FromStr;

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
    T::try_from(val).map_err(|e| serde::de::Error::custom(format!("Invalid value: {val} ({e:?})")))
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

pub fn serialize_duration_as_ms<S>(
    duration: &std::time::Duration,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    // lmao at how `as_millis()` returns a u128 but `from_` takes a u64
    // like i get you're likely not making a new one with that length
    // but still
    let millis = duration.as_millis();
    serializer.serialize_u64(millis as u64)
}

pub fn deserialize_duration_from_ms<'de, D>(
    deserializer: D,
) -> Result<std::time::Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let millis = u64::deserialize(deserializer)?;
    Ok(std::time::Duration::from_millis(millis))
}
