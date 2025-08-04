use std::num::ParseIntError;

use serialport::UsbPortInfo;

#[derive(Debug, Clone, PartialEq)]
pub struct DeserializedUsb {
    /// Vendor ID
    pub vid: u16,
    /// Product ID
    pub pid: u16,
    /// Serial number (arbitrary string)
    pub serial_number: Option<String>,
}

impl PartialEq<UsbPortInfo> for DeserializedUsb {
    fn eq(&self, other: &UsbPortInfo) -> bool {
        self.vid == other.vid
            && self.pid == other.pid
            && match (&self.serial_number, &other.serial_number) {
                // no serial number was specified by the user, so we'll match any that matched vid and pid
                (None, _) => true,
                // compare serial numbers!
                (Some(serial_num), Some(other_serial_num)) => serial_num == other_serial_num,
                // if we're checking for a specific serial number but the checked device doesn't have one.
                (Some(_), None) => false,
            }
    }
}

impl From<UsbPortInfo> for DeserializedUsb {
    fn from(value: UsbPortInfo) -> Self {
        let UsbPortInfo {
            vid,
            pid,
            serial_number,
            ..
        } = value;
        Self {
            vid,
            pid,
            serial_number,
        }
    }
}
impl From<DeserializedUsb> for UsbPortInfo {
    fn from(value: DeserializedUsb) -> Self {
        let DeserializedUsb {
            vid,
            pid,
            serial_number,
        } = value;
        Self {
            vid,
            pid,
            serial_number,
            manufacturer: None,
            product: None,
        }
    }
}

impl std::fmt::Display for DeserializedUsb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.serial_number {
            Some(serial) => write!(f, "{:04X}:{:04X}:{serial}", self.vid, self.pid),
            None => write!(f, "{:04X}:{:04X}", self.vid, self.pid),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeserializeUsbError {
    #[error("missing USB VID")]
    MissingVid,
    #[error("missing USB PID")]
    MissingPid,
    #[error("invalid USB VID")]
    ParseVid(#[source] ParseIntError),
    #[error("invalid USB PID")]
    ParsePid(#[source] ParseIntError),
    #[error("empty USB serial number")]
    EmptySerial,
}

impl std::str::FromStr for DeserializedUsb {
    type Err = DeserializeUsbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(3, ':');
        let vid = parts.next().ok_or(Self::Err::MissingVid)?;
        let pid = parts.next().ok_or(Self::Err::MissingPid)?;
        let serial = parts.next();

        let vid = u16::from_str_radix(vid, 16).map_err(Self::Err::ParseVid)?;
        let pid = u16::from_str_radix(pid, 16).map_err(Self::Err::ParsePid)?;
        let serial_number = match serial {
            Some(s) => {
                let trim = s.trim();
                if trim.is_empty() {
                    return Err(Self::Err::EmptySerial);
                }
                Some(trim.to_string())
            }
            None => None,
        };

        Ok(DeserializedUsb {
            vid,
            pid,
            serial_number,
        })
    }
}

impl serde::Serialize for DeserializedUsb {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for DeserializedUsb {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = DeserializedUsb;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string in the format VID:PID or VID:PID:SERIAL")
            }

            fn visit_str<E>(self, value: &str) -> Result<DeserializedUsb, E>
            where
                E: serde::de::Error,
            {
                value.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}
