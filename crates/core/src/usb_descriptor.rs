//! 通用 USB 描述符的只读解析，不依赖 HID 类，也不访问存储介质。

#[derive(Debug, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub vendor: u16,
    pub product: u16,
    pub usb_version: u16,
    pub class: u8,
    pub configurations: u8,
}

impl DeviceIdentity {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 18 || data[0] != 18 || data[1] != 1 {
            return None;
        }
        Some(Self {
            vendor: u16::from_le_bytes([data[8], data[9]]),
            product: u16::from_le_bytes([data[10], data[11]]),
            usb_version: u16::from_le_bytes([data[2], data[3]]),
            class: data[4],
            configurations: data[17],
        })
    }
}

/// 配置内的 TLV 描述符；遇到截断、零长度或不足两个字节的条目即终止报错。
pub struct Descriptors<'a> {
    remaining: &'a [u8],
}

impl<'a> Descriptors<'a> {
    pub fn configuration(data: &'a [u8]) -> Option<Self> {
        if data.len() < 9 || data[0] != 9 || data[1] != 2 {
            return None;
        }
        let length = u16::from_le_bytes([data[2], data[3]]) as usize;
        if length < 9 || length > data.len() {
            return None;
        }
        Some(Self {
            remaining: &data[..length],
        })
    }
}

impl<'a> Iterator for Descriptors<'a> {
    type Item = Result<&'a [u8], &'static str>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        let length = self.remaining[0] as usize;
        if length < 2 || length > self.remaining.len() {
            self.remaining = &[];
            return Some(Err("truncated or invalid USB descriptor"));
        }
        let (entry, rest) = self.remaining.split_at(length);
        self.remaining = rest;
        Some(Ok(entry))
    }
}

pub fn string(data: &[u8]) -> Option<String> {
    if data.len() < 2
        || data[1] != 3
        || data[0] as usize != data.len()
        || !data.len().is_multiple_of(2)
    {
        return None;
    }
    let units: Vec<_> = data[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

pub fn class_name(class: u8) -> &'static str {
    match class {
        0 => "per-interface",
        3 => "HID",
        8 => "Mass Storage",
        9 => "Hub",
        0xff => "vendor-specific",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mass_storage_is_identified_from_interface_not_device_class() {
        let device = [
            18, 1, 0, 2, 0, 0, 0, 64, 0x34, 0x12, 0x78, 0x56, 0, 1, 1, 2, 3, 1,
        ];
        let identity = DeviceIdentity::parse(&device).unwrap();
        assert_eq!(
            (identity.vendor, identity.product, identity.class),
            (0x1234, 0x5678, 0)
        );
        let config = [9, 2, 18, 0, 1, 1, 0, 0x80, 50, 9, 4, 0, 0, 2, 8, 6, 0x50, 0];
        let descriptors: Vec<_> = Descriptors::configuration(&config).unwrap().collect();
        assert_eq!(descriptors[1].as_ref().unwrap()[5], 8);
        assert_eq!(class_name(8), "Mass Storage");
    }

    #[test]
    fn malformed_descriptors_stop_without_looping_or_reading_past_end() {
        assert!(DeviceIdentity::parse(&[18, 1]).is_none());
        assert!(Descriptors::configuration(&[9, 2, 20, 0, 1, 1, 0, 0x80, 50]).is_none());
        for invalid in [0, 1, 10] {
            let config = [9, 2, 11, 0, 1, 1, 0, 0x80, 50, invalid, 4];
            let mut iter = Descriptors::configuration(&config).unwrap();
            assert!(iter.next().unwrap().is_ok());
            assert!(iter.next().unwrap().is_err());
            assert!(iter.next().is_none());
        }
    }

    #[test]
    fn string_lengths_and_utf16_are_checked() {
        assert_eq!(string(&[6, 3, b'U', 0, b'S', 0]), Some("US".into()));
        assert!(string(&[4, 3, 0]).is_none());
        assert_eq!(string(&[4, 3, 0, 0xd8]), Some("�".into()));
    }
}
