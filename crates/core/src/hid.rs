//! Bounded USB HID report-descriptor parser.
//!
//! This is a Rust port of the C prototype's interpreter with the same safety
//! bounds: no unbounded allocations, no assumptions about descriptor validity,
//! and no interpretation of absolute axes as mouse motion.
//!
//! Supported usages are intentionally limited to what a 2.4 GHz keyboard/mouse
//! receiver presents: Keyboard/Keypad (0x07), Button (0x09), Generic Desktop
//! X/Y/Wheel (0x01) and Consumer (0x0C).

use std::fmt;

/// Maximum number of input fields kept in one descriptor.
pub const MAX_FIELDS: usize = 320;
/// Maximum number of report IDs kept in one descriptor.
pub const MAX_REPORTS: usize = 16;
/// Maximum size in bits of a single report layout.
pub const MAX_REPORT_BITS: u32 = 8192;
/// Bytes needed for the 256-entry HID keyboard usage bitmap.
pub const KEY_BYTES: usize = 32;

/// Bit flags for the kinds of input a report can carry.
pub mod kind {
    pub const KEYBOARD: u8 = 1;
    pub const MOUSE: u8 = 2;
    pub const CONSUMER: u8 = 4;
}

/// Decoded state of one HID input report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decoded {
    /// Union of the `kind` flags present in this report layout.
    pub kinds: u8,
    /// Bitmap of pressed keyboard usages, indexed by usage ID (0x00..=0xFF).
    pub keys: [u8; KEY_BYTES],
    /// Mouse buttons 1..=8, bit 0 is button 1.
    pub buttons: u8,
    /// Relative X motion.
    pub x: i32,
    /// Relative Y motion.
    pub y: i32,
    /// Relative vertical wheel motion.
    pub wheel: i32,
    /// Relative horizontal pan (AC Pan, consumer usage 0x0238).
    pub pan: i32,
    /// First non-zero consumer usage in the report.
    pub consumer: u16,
}

impl Decoded {
    /// Merge keyboard/button/consumer state from another report layout.
    ///
    /// Relative motion is deliberately excluded: it belongs to exactly one
    /// report packet and must never be replayed from a stored layout.
    pub fn merge_state(&mut self, other: &Self) {
        self.kinds |= other.kinds;
        for (dst, src) in self.keys.iter_mut().zip(other.keys.iter()) {
            *dst |= *src;
        }
        self.buttons |= other.buttons;
        if self.consumer == 0 {
            self.consumer = other.consumer;
        }
    }

    /// True when no key, button or consumer usage is active.
    pub fn neutral(&self) -> bool {
        self.buttons == 0 && self.consumer == 0 && self.keys.iter().all(|byte| *byte == 0)
    }

    /// Returns a copy with all relative motion cleared.
    pub fn without_motion(mut self) -> Self {
        self.x = 0;
        self.y = 0;
        self.wheel = 0;
        self.pan = 0;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    TruncatedItem,
    TruncatedLongItem,
    PushOverflow,
    PopUnderflow,
    TooManyUsages,
    TooManyFields,
    TooManyReports,
    InvalidReportId,
    InvalidFieldSize,
    InvalidReportSize,
    BackwardsUsageRange,
    DelimiterUnsupported,
    NoFields,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::TruncatedItem => "truncated HID item",
            Self::TruncatedLongItem => "truncated HID long item",
            Self::PushOverflow => "HID global push stack overflow",
            Self::PopUnderflow => "HID global pop stack underflow",
            Self::TooManyUsages => "too many HID usages",
            Self::TooManyFields => "too many HID input fields",
            Self::TooManyReports => "too many HID report IDs",
            Self::InvalidReportId => "invalid HID report ID",
            Self::InvalidFieldSize => "invalid HID field size/count",
            Self::InvalidReportSize => "HID report layout is too large",
            Self::BackwardsUsageRange => "HID usage maximum is below minimum",
            Self::DelimiterUnsupported => "HID delimiter items are unsupported",
            Self::NoFields => "HID descriptor has no usable input fields",
        };
        f.write_str(text)
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Copy, Debug)]
struct Field {
    offset: u32,
    size: u8,
    usage: u16,
    page: u16,
    report: u8,
    flags: u8,
    logical_min: i32,
}

impl Field {
    fn variable(&self) -> bool {
        self.flags & 0x02 != 0
    }
}

#[derive(Clone, Copy, Debug)]
struct ReportLayout {
    id: u8,
    bits: u32,
    kinds: u8,
}

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u32,
    size: u32,
    count: u32,
    id: u8,
    logical_min: i32,
}

/// Parsed HID report descriptor.
#[derive(Clone, Debug)]
pub struct Parser {
    fields: Vec<Field>,
    reports: Vec<ReportLayout>,
    numbered: bool,
}

impl Parser {
    /// Parse an input report descriptor.
    pub fn parse(descriptor: &[u8]) -> Result<Self, ParseError> {
        let mut parser = Self {
            fields: Vec::new(),
            reports: Vec::new(),
            numbered: false,
        };

        let mut globals = Globals::default();
        let mut stack = [Globals::default(); 8];
        let mut depth = 0usize;

        let mut usages = [0u32; 64];
        let mut usage_count = 0usize;
        let mut usage_min = 0u32;
        let mut usage_max = 0u32;
        let mut range = false;

        let mut pos = 0usize;
        while pos < descriptor.len() {
            let prefix = descriptor[pos];
            pos += 1;

            // Long items are skipped exactly as the C interpreter does.
            if prefix == 0xfe {
                if pos + 2 > descriptor.len() {
                    return Err(ParseError::TruncatedLongItem);
                }
                let len = descriptor[pos] as usize;
                pos += 2;
                if pos + len > descriptor.len() {
                    return Err(ParseError::TruncatedLongItem);
                }
                pos += len;
                continue;
            }

            let len = match prefix & 0x03 {
                0 => 0,
                1 => 1,
                2 => 2,
                _ => 4,
            };
            if pos + len > descriptor.len() {
                return Err(ParseError::TruncatedItem);
            }
            let mut value = 0u32;
            for byte in 0..len {
                value |= (descriptor[pos + byte] as u32) << (byte * 8);
            }
            pos += len;

            let item_type = (prefix >> 2) & 0x03;
            let tag = prefix >> 4;

            match item_type {
                // Global items.
                0x01 => match tag {
                    0x00 => globals.page = value,
                    0x01 => globals.logical_min = sign_extend(value, len * 8),
                    0x07 => globals.size = value,
                    0x08 => {
                        if value == 0 || value > 0xff {
                            return Err(ParseError::InvalidReportId);
                        }
                        globals.id = value as u8;
                        parser.numbered = true;
                    }
                    0x09 => globals.count = value,
                    0x0a => {
                        if depth == stack.len() {
                            return Err(ParseError::PushOverflow);
                        }
                        stack[depth] = globals;
                        depth += 1;
                    }
                    0x0b => {
                        if depth == 0 {
                            return Err(ParseError::PopUnderflow);
                        }
                        depth -= 1;
                        globals = stack[depth];
                    }
                    _ => {}
                },
                // Local items.
                0x02 => match tag {
                    0x00 => {
                        if usage_count == usages.len() {
                            return Err(ParseError::TooManyUsages);
                        }
                        usages[usage_count] = full_usage(globals.page, value, len);
                        usage_count += 1;
                    }
                    0x01 => {
                        usage_min = full_usage(globals.page, value, len);
                        range = true;
                    }
                    0x02 => usage_max = full_usage(globals.page, value, len),
                    0x0a => return Err(ParseError::DelimiterUnsupported),
                    _ => {}
                },
                // Main items.
                0x00 => {
                    if tag == 0x08 {
                        parser.parse_input(
                            value,
                            &globals,
                            &usages[..usage_count],
                            usage_min,
                            usage_max,
                            range,
                        )?;
                    }
                    // Local state is reset after every main item.
                    usage_count = 0;
                    usage_min = 0;
                    usage_max = 0;
                    range = false;
                }
                _ => {}
            }
        }

        if depth != 0 {
            return Err(ParseError::PushOverflow);
        }
        if parser.numbered && parser.reports.iter().any(|report| report.id == 0) {
            return Err(ParseError::InvalidReportId);
        }
        if parser.fields.is_empty() {
            return Err(ParseError::NoFields);
        }
        Ok(parser)
    }

    fn parse_input(
        &mut self,
        flags: u32,
        globals: &Globals,
        usages: &[u32],
        usage_min: u32,
        usage_max: u32,
        range: bool,
    ) -> Result<(), ParseError> {
        if globals.size == 0 || globals.size > 32 || globals.count == 0 || globals.count > 4096 {
            return Err(ParseError::InvalidFieldSize);
        }
        if range && usage_max < usage_min {
            return Err(ParseError::BackwardsUsageRange);
        }

        let report_index = match self
            .reports
            .iter()
            .position(|report| report.id == globals.id)
        {
            Some(index) => index,
            None => {
                if self.reports.len() == MAX_REPORTS {
                    return Err(ParseError::TooManyReports);
                }
                self.reports.push(ReportLayout {
                    id: globals.id,
                    bits: 0,
                    kinds: 0,
                });
                self.reports.len() - 1
            }
        };

        let start_bits = self.reports[report_index].bits;
        let end_bits = u64::from(start_bits) + u64::from(globals.size) * u64::from(globals.count);
        if end_bits > u64::from(MAX_REPORT_BITS) {
            return Err(ParseError::InvalidReportSize);
        }

        let variable = flags & 0x02 != 0;
        let relative = flags & 0x04 != 0;
        let constant = flags & 0x01 != 0;

        let mut report_kinds = 0u8;
        if !constant {
            for field_index in 0..globals.count {
                let usage = if variable {
                    if range {
                        usage_min.saturating_add(field_index.min(usage_max - usage_min))
                    } else if !usages.is_empty() {
                        usages[(field_index as usize).min(usages.len() - 1)]
                    } else {
                        page_usage(globals.page)
                    }
                } else if range {
                    usage_min
                } else if let Some(first) = usages.first() {
                    *first
                } else {
                    page_usage(globals.page)
                };

                let page = (usage >> 16) as u16;
                let id = usage as u16;
                let mut field_kind = classify(page, id, variable);
                // Absolute axes (tablets, touchpads, gamepads) must not become
                // relative mouse motion. Buttons do not need the Relative flag.
                if field_kind == kind::MOUSE && page != 0x09 && !relative {
                    field_kind = 0;
                }
                if field_kind == 0 {
                    continue;
                }
                if self.fields.len() == MAX_FIELDS {
                    return Err(ParseError::TooManyFields);
                }
                self.fields.push(Field {
                    offset: start_bits + field_index * globals.size,
                    size: globals.size as u8,
                    usage: id,
                    page,
                    report: report_index as u8,
                    flags: flags as u8,
                    logical_min: globals.logical_min,
                });
                report_kinds |= field_kind;
            }
        }

        self.reports[report_index].bits = end_bits as u32;
        self.reports[report_index].kinds |= report_kinds;
        Ok(())
    }

    /// Decode one raw input report.
    ///
    /// Returns the decoded state and the index of the matching report layout,
    /// or `None` for an unknown report ID or a truncated packet.
    pub fn decode(&self, data: &[u8]) -> Option<(Decoded, usize)> {
        if data.is_empty() {
            return None;
        }
        let (id, body) = if self.numbered {
            (data[0], &data[1..])
        } else {
            (0u8, data)
        };
        let (report_index, layout) = self
            .reports
            .iter()
            .enumerate()
            .find(|(_, report)| report.id == id)?;
        let required = layout.bits.div_ceil(8) as usize;
        if body.len() < required {
            return None;
        }

        let mut decoded = Decoded {
            kinds: layout.kinds,
            ..Default::default()
        };

        for field in self
            .fields
            .iter()
            .filter(|field| field.report as usize == report_index)
        {
            let raw = read_bits(body, field.offset, field.size);
            let value = if field.logical_min < 0 {
                sign_extend(raw, field.size as usize)
            } else {
                raw as i32
            };

            let usage = if field.variable() {
                if value == 0 {
                    continue;
                }
                field.usage
            } else {
                // A zero array element means "no event".
                if raw == 0 {
                    continue;
                }
                let mapped =
                    i64::from(field.usage) + i64::from(value) - i64::from(field.logical_min);
                if !(0..=i64::from(u16::MAX)).contains(&mapped) {
                    continue;
                }
                mapped as u16
            };

            match field.page {
                0x07 => {
                    let key = usage as usize;
                    if key < KEY_BYTES * 8 {
                        decoded.keys[key / 8] |= 1 << (key % 8);
                    }
                }
                0x09 => {
                    if (1..=8).contains(&usage) {
                        decoded.buttons |= 1 << (usage - 1);
                    }
                }
                0x01 => match usage {
                    0x30 => decoded.x = value,
                    0x31 => decoded.y = value,
                    0x38 => decoded.wheel = value,
                    _ => {}
                },
                0x0c => {
                    if usage == 0x0238 && field.variable() {
                        decoded.pan = value;
                    } else if decoded.consumer == 0 {
                        decoded.consumer = usage;
                    }
                }
                _ => {}
            }
        }

        Some((decoded, report_index))
    }

    /// Number of report layouts found in the descriptor.
    pub fn report_count(&self) -> usize {
        self.reports.len()
    }

    /// Report IDs in descriptor order.
    pub fn report_ids(&self) -> impl Iterator<Item = u8> + '_ {
        self.reports.iter().map(|report| report.id)
    }

    /// Whether the descriptor uses numbered reports.
    pub fn is_numbered(&self) -> bool {
        self.numbered
    }

    /// Kinds present in a report layout.
    pub fn report_kinds(&self, index: usize) -> Option<u8> {
        self.reports.get(index).map(|report| report.kinds)
    }

    /// Bit length of a report layout.
    pub fn report_bits(&self, index: usize) -> Option<u32> {
        self.reports.get(index).map(|report| report.bits)
    }
}

fn sign_extend(value: u32, bits: usize) -> i32 {
    if bits == 0 || bits >= 32 {
        value as i32
    } else if value & (1 << (bits - 1)) != 0 {
        (value | (u32::MAX << bits)) as i32
    } else {
        value as i32
    }
}

fn page_usage(page: u32) -> u32 {
    (page & 0xffff) << 16
}

fn full_usage(page: u32, value: u32, len: usize) -> u32 {
    if len == 4 {
        value
    } else {
        page_usage(page) | (value & 0xffff)
    }
}

fn classify(page: u16, usage: u16, variable: bool) -> u8 {
    let mouse = (page == 0x09 && variable && (1..=8).contains(&usage))
        || (page == 0x01 && variable && matches!(usage, 0x30 | 0x31 | 0x38))
        || (page == 0x0c && variable && usage == 0x0238);

    if page == 0x07 {
        kind::KEYBOARD
    } else if mouse {
        kind::MOUSE
    } else if page == 0x0c {
        kind::CONSUMER
    } else {
        0
    }
}

fn read_bits(data: &[u8], offset: u32, size: u8) -> u32 {
    let mut value = 0u32;
    for bit in 0..u32::from(size) {
        let index = (offset + bit) as usize;
        value |= u32::from((data[index / 8] >> (index % 8)) & 1) << bit;
    }
    value
}

/// HID report map exposed over Bluetooth.
///
/// Report IDs are not included in notification payloads:
/// * 1: boot-format keyboard, 8 modifier bits + 6 key bytes.
/// * 2: 8 buttons, signed 16-bit X/Y, signed 8-bit wheel and AC Pan.
/// * 3: one 16-bit consumer usage.
pub const REPORT_MAP: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x85, 0x01, // Generic Desktop / Keyboard
    0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00, 0x25, 0x01, // modifiers
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, // 8 modifier bits
    0x75, 0x08, 0x95, 0x01, 0x81, 0x01, // reserved byte
    0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x75, 0x01, 0x95, 0x05, // LEDs
    0x91, 0x02, 0x75, 0x03, 0x95, 0x01, 0x91, 0x01, // LED padding
    0x05, 0x07, 0x19, 0x00, 0x29, 0xe7, 0x15, 0x00, 0x26, 0xe7, 0x00, // 6 keys
    0x75, 0x08, 0x95, 0x06, 0x81, 0x00, 0xc0, // end keyboard
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x02, // Mouse
    0x09, 0x01, 0xa1, 0x00, // pointer
    0x05, 0x09, 0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, // 8 buttons
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, // button bits
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, // X/Y
    0x16, 0x01, 0x80, 0x26, 0xff, 0x7f, 0x75, 0x10, 0x95, 0x02, 0x81, 0x06, // relative
    0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, // wheel
    0x05, 0x0c, 0x0a, 0x38, 0x02, 0x81, 0x06, 0xc0, 0xc0, // AC Pan
    0x05, 0x0c, 0x09, 0x01, 0xa1, 0x01, 0x85, 0x03, // Consumer
    0x15, 0x00, 0x26, 0xff, 0x03, 0x19, 0x00, 0x2a, 0xff, 0x03, // 0..=0x3ff
    0x75, 0x10, 0x95, 0x01, 0x81, 0x00, 0xc0,
];

pub const KEYBOARD_REPORT_ID: u8 = 1;
pub const MOUSE_REPORT_ID: u8 = 2;
pub const CONSUMER_REPORT_ID: u8 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    fn init(descriptor: &[u8]) -> Parser {
        Parser::parse(descriptor).expect("descriptor should parse")
    }

    #[test]
    fn keyboard_modifiers_array_and_output_offsets() {
        let descriptor = [
            0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00,
            0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x75, 0x08, 0x95, 0x01, 0x81, 0x01,
            // Output reports must not advance input bit offsets.
            0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x75, 0x01, 0x95, 0x05, 0x91, 0x02, 0x75, 0x03,
            0x95, 0x01, 0x91, 0x01, 0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x15, 0x00, 0x25, 0x65,
            0x75, 0x08, 0x95, 0x06, 0x81, 0x00, 0xc0,
        ];
        let parser = init(&descriptor);
        assert_eq!(parser.report_bits(0), Some(64));

        let report = [0x02, 0x00, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00];
        let (decoded, index) = parser.decode(&report).unwrap();
        assert_eq!(index, 0);
        assert_eq!(decoded.kinds, kind::KEYBOARD);
        assert_eq!(decoded.keys[0], (1 << 4) | (1 << 5));
        assert_ne!(decoded.keys[0xe1 / 8] & (1 << (0xe1 % 8)), 0);

        assert!(parser.decode(&report[..7]).is_none());
        let release = [0u8; 8];
        let (decoded, _) = parser.decode(&release).unwrap();
        assert!(decoded.neutral());
    }

    #[test]
    fn nkro_and_unknown_report_ids() {
        let descriptor = [
            0x05, 0x07, 0x85, 0x09, 0x19, 0x00, 0x29, 0xe7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
            0x96, 0xe8, 0x00, 0x81, 0x02,
        ];
        let parser = init(&descriptor);
        let mut report = [0u8; 30];
        report[0] = 0x09;
        report[1] = 0x10;
        report[29] = 0x80;
        let (decoded, _) = parser.decode(&report).unwrap();
        assert_eq!(decoded.keys[0], 0x10);
        assert_eq!(decoded.keys[28], 0x80);

        report[0] = 0x08;
        assert!(parser.decode(&report).is_none());
    }

    #[test]
    fn packed_12_bit_signed_axes_and_pan() {
        let descriptor = [
            0x05, 0x09, 0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08,
            0x81, 0x02, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x16, 0x00, 0xf8, 0x26, 0xff, 0x07,
            0x75, 0x0c, 0x95, 0x02, 0x81, 0x06, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08,
            0x95, 0x01, 0x81, 0x06, 0x05, 0x0c, 0x0a, 0x38, 0x02, 0x81, 0x06,
        ];
        let parser = init(&descriptor);
        let data = [0x81, 0xfe, 0x0f, 0x40, 0xff, 0x02];
        let (decoded, _) = parser.decode(&data).unwrap();
        assert_eq!(decoded.buttons, 0x81);
        assert_eq!(decoded.x, -2);
        assert_eq!(decoded.y, 1024);
        assert_eq!(decoded.wheel, -1);
        assert_eq!(decoded.pan, 2);
    }

    #[test]
    fn independent_report_layouts_and_consumer_arrays() {
        let descriptor = [
            0x05, 0x0c, 0x85, 0x02, 0x19, 0x00, 0x2a, 0xff, 0x03, 0x15, 0x00, 0x26, 0xff, 0x03,
            0x75, 0x10, 0x95, 0x01, 0x81, 0x00, 0x85, 0x03, 0x09, 0xe9, 0x09, 0xea, 0x15, 0x00,
            0x25, 0x01, 0x75, 0x01, 0x95, 0x02, 0x81, 0x02, 0x95, 0x06, 0x81, 0x01,
        ];
        let parser = init(&descriptor);
        let data = [0x02, 0xcd, 0x00];
        let (decoded, index) = parser.decode(&data).unwrap();
        assert_eq!(index, 0);
        assert_eq!(decoded.consumer, 0xcd);
        assert_eq!(decoded.kinds, kind::CONSUMER);

        let volume = [0x03, 0x02];
        let (decoded, index) = parser.decode(&volume).unwrap();
        assert_eq!(index, 1);
        assert_eq!(decoded.consumer, 0xea);
    }

    #[test]
    fn absolute_axes_are_rejected() {
        let descriptor = [
            0x05, 0x01, 0x09, 0x30, 0x15, 0x00, 0x26, 0xff, 0x7f, 0x75, 0x10, 0x95, 0x01, 0x81,
            0x02,
        ];
        assert_eq!(
            Parser::parse(&descriptor).unwrap_err(),
            ParseError::NoFields
        );
    }

    #[test]
    fn malformed_descriptors_do_not_panic() {
        assert_eq!(
            Parser::parse(&[0x75]).unwrap_err(),
            ParseError::TruncatedItem
        );
        assert_eq!(
            Parser::parse(&[
                0x05, 0x07, 0x19, 0x00, 0x29, 0xff, 0x75, 0x20, 0x96, 0xff, 0xff, 0x81, 0x02
            ])
            .unwrap_err(),
            ParseError::InvalidFieldSize
        );
        assert_eq!(
            Parser::parse(&[0xb4]).unwrap_err(),
            ParseError::PopUnderflow
        );
        assert_eq!(
            Parser::parse(&[0x85, 0x00]).unwrap_err(),
            ParseError::InvalidReportId
        );

        let mut seed = 0x1234_5678u32;
        for run in 0..20_000 {
            let mut bytes = [0u8; 128];
            for byte in &mut bytes {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *byte = seed as u8;
            }
            if let Ok(parser) = Parser::parse(&bytes[..run % bytes.len()]) {
                let _ = parser.decode(&bytes);
            }
        }
    }

    #[test]
    fn report_map_round_trip() {
        let parser = init(REPORT_MAP);
        assert_eq!(parser.report_count(), 3);
        assert_eq!(parser.report_ids().collect::<Vec<_>>(), [1, 2, 3]);

        let keyboard = [0x01, 0x02, 0x00, 0x04, 0, 0, 0, 0, 0];
        let (decoded, index) = parser.decode(&keyboard).unwrap();
        assert_eq!(index, 0);
        assert_eq!(decoded.kinds & kind::KEYBOARD, kind::KEYBOARD);
        assert_eq!(decoded.keys[0] & 0x10, 0x10);

        let mouse = [0x02, 0x01, 0x02, 0x00, 0xff, 0xff, 0x01, 0xfe];
        let (decoded, index) = parser.decode(&mouse).unwrap();
        assert_eq!(index, 1);
        assert_eq!(decoded.buttons, 1);
        assert_eq!(decoded.x, 2);
        assert_eq!(decoded.y, -1);
        assert_eq!(decoded.wheel, 1);
        assert_eq!(decoded.pan, -2);

        let consumer = [0x03, 0x00, 0x01];
        let (decoded, index) = parser.decode(&consumer).unwrap();
        assert_eq!(index, 2);
        assert_eq!(decoded.consumer, 0x0100);
    }
}
