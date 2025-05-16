use serialport::UsbPortInfo;

#[derive(Debug, Clone, PartialEq)]
pub struct IgnoreableUsb {
    /// Vendor ID
    pub vid: u16,
    /// Product ID
    pub pid: u16,
    /// Serial number (arbitrary string)
    pub serial: Option<String>,
}

impl PartialEq<UsbPortInfo> for IgnoreableUsb {
    fn eq(&self, other: &UsbPortInfo) -> bool {
        self.vid == other.vid
            && self.pid == other.pid
            && match (&self.serial, &other.serial_number) {
                (None, _) => true,
                (Some(serial), Some(other_serial)) => serial == other_serial,
                // if we're ignoring a serial # but they don't have a serial
                (Some(_), None) => false,
            }
    }
}

impl std::fmt::Display for IgnoreableUsb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.serial {
            Some(serial) => write!(f, "{:04X}:{:04X}:{}", self.vid, self.pid, serial),
            None => write!(f, "{:04X}:{:04X}", self.vid, self.pid),
        }
    }
}

impl std::str::FromStr for IgnoreableUsb {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(3, ':');
        let vid = parts.next().ok_or("Missing VID")?;
        let pid = parts.next().ok_or("Missing PID")?;
        let serial = parts.next();

        let vid = u16::from_str_radix(vid, 16).map_err(|e| format!("VID parse error: {}", e))?;
        let pid = u16::from_str_radix(pid, 16).map_err(|e| format!("PID parse error: {}", e))?;
        let serial = serial.map(|s| s.to_string());

        Ok(IgnoreableUsb { vid, pid, serial })
    }
}

impl serde::Serialize for IgnoreableUsb {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for IgnoreableUsb {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = IgnoreableUsb;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string in the format VID:PID or VID:PID:SERIAL")
            }

            fn visit_str<E>(self, value: &str) -> Result<IgnoreableUsb, E>
            where
                E: serde::de::Error,
            {
                value.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}
