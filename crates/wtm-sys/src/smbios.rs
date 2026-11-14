//! Memory modules from the firmware's SMBIOS table (type 17, "Memory
//! Device"): the speed, slots used and form factor Task Manager shows under
//! the memory graph. Reading the table needs no administrator rights.

use windows::Win32::System::SystemInformation::{FIRMWARE_TABLE_PROVIDER, GetSystemFirmwareTable};

use crate::Result;

/// One memory slot. An empty slot has a `size` of 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemoryModule {
    /// Installed size in bytes; 0 for an empty slot.
    pub size: u64,
    /// SMBIOS form factor code (9 DIMM, 13 SODIMM, ...).
    pub form_factor: u8,
    /// SMBIOS memory type code (26 DDR4, 34 DDR5, ...).
    pub memory_type: u8,
    /// Rated and configured speed in MT/s.
    pub speed: Option<u32>,
    pub configured_speed: Option<u32>,
    /// The slot's label on the board, such as "DIMM A1".
    pub locator: String,
    /// The bank or channel, such as "P0 CHANNEL A": boards that name the
    /// slots of each channel alike tell them apart by it.
    pub bank: String,
    pub manufacturer: String,
    pub part_number: String,
}

impl MemoryModule {
    pub fn form_factor_name(&self) -> Option<&'static str> {
        Some(match self.form_factor {
            0x03 => "SIMM",
            0x05 => "Chip",
            0x06 => "DIP",
            0x08 => "Proprietary card",
            0x09 => "DIMM",
            0x0A => "TSOP",
            0x0B => "Row of chips",
            0x0C => "RIMM",
            0x0D => "SODIMM",
            0x0E => "SRIMM",
            0x0F => "FB-DIMM",
            0x10 => "Die",
            0x11 => "CAMM",
            _ => return None,
        })
    }

    pub fn type_name(&self) -> Option<&'static str> {
        Some(match self.memory_type {
            0x12 => "DDR",
            0x13 => "DDR2",
            0x18 => "DDR3",
            0x1A => "DDR4",
            0x1B => "LPDDR",
            0x1C => "LPDDR2",
            0x1D => "LPDDR3",
            0x1E => "LPDDR4",
            0x1F => "Logical non-volatile",
            0x20 => "HBM",
            0x21 => "HBM2",
            0x22 => "DDR5",
            0x23 => "LPDDR5",
            0x24 => "HBM3",
            _ => return None,
        })
    }
}

/// 'RSMB': the raw SMBIOS provider.
const RSMB: FIRMWARE_TABLE_PROVIDER = FIRMWARE_TABLE_PROVIDER(u32::from_be_bytes(*b"RSMB"));
/// Largest table accepted; real ones are a few kilobytes.
const MAX_TABLE: u32 = 1024 * 1024;
/// Most slots reported.
const MAX_MODULES: usize = 64;

/// The memory slots of this PC, empty ones included.
pub fn memory_modules() -> Result<Vec<MemoryModule>> {
    // SAFETY: no buffer: asks for the size only.
    let size = unsafe { GetSystemFirmwareTable(RSMB, 0, None) };
    if size == 0 || size > MAX_TABLE {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` is writable for its whole length.
    let written = unsafe { GetSystemFirmwareTable(RSMB, 0, Some(&mut buf)) };
    if written == 0 || written as usize > buf.len() {
        return Ok(Vec::new());
    }
    Ok(decode(&buf[..written as usize]))
}

/// Decodes a `RawSMBIOSData` block: an 8-byte header, then the table.
fn decode(raw: &[u8]) -> Vec<MemoryModule> {
    let Some(len) = raw.get(4..8) else {
        return Vec::new();
    };
    let len = u32::from_le_bytes(len.try_into().unwrap_or_default()) as usize;
    let table = &raw[8..raw.len().min(8usize.saturating_add(len))];
    let mut modules = Vec::new();
    let mut at = 0usize;
    while let (Some(&kind), Some(&length)) = (table.get(at), table.get(at + 1)) {
        let length = usize::from(length);
        if length < 4 || at + length > table.len() {
            break;
        }
        let formatted = &table[at..at + length];
        // The strings follow, ended by an empty one (two NULs).
        let strings_start = at + length;
        let Some(strings_len) = table[strings_start..].windows(2).position(|w| w == [0, 0]) else {
            break;
        };
        let strings = &table[strings_start..strings_start + strings_len];
        if kind == 17 && modules.len() < MAX_MODULES {
            modules.push(memory_device(formatted, strings));
        }
        if kind == 127 {
            break; // end of table
        }
        at = strings_start + strings_len + 2;
    }
    modules
}

/// The `n`th (1-based) string of a structure; 0 means none.
fn string(strings: &[u8], n: u8) -> String {
    if n == 0 {
        return String::new();
    }
    strings
        .split(|b| *b == 0)
        .nth(usize::from(n) - 1)
        .map(|s| String::from_utf8_lossy(s).trim().to_owned())
        .unwrap_or_default()
}

fn memory_device(f: &[u8], strings: &[u8]) -> MemoryModule {
    let byte = |at: usize| f.get(at).copied();
    let word = |at: usize| Some(u16::from_le_bytes(f.get(at..at + 2)?.try_into().ok()?));
    let dword = |at: usize| Some(u32::from_le_bytes(f.get(at..at + 4)?.try_into().ok()?));
    let size = match word(0x0C) {
        None | Some(0) | Some(0xFFFF) => 0,
        // The extended size, in MiB.
        Some(0x7FFF) => u64::from(dword(0x1C).unwrap_or(0) & 0x7FFF_FFFF) << 20,
        // Bit 15 set: KiB, else MiB.
        Some(s) if s & 0x8000 != 0 => u64::from(s & 0x7FFF) << 10,
        Some(s) => u64::from(s) << 20,
    };
    let speed = |at: usize, extended: usize| match word(at) {
        None | Some(0) => None,
        Some(0xFFFF) => dword(extended).filter(|s| *s > 0).map(|s| s & 0x7FFF_FFFF),
        Some(s) => Some(u32::from(s)),
    };
    MemoryModule {
        size,
        form_factor: byte(0x0E).unwrap_or(0),
        memory_type: byte(0x12).unwrap_or(0),
        speed: speed(0x15, 0x54),
        configured_speed: speed(0x20, 0x58),
        locator: string(strings, byte(0x10).unwrap_or(0)),
        bank: string(strings, byte(0x11).unwrap_or(0)),
        manufacturer: string(strings, byte(0x17).unwrap_or(0)),
        part_number: string(strings, byte(0x1A).unwrap_or(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A type 17 structure of SMBIOS 3.3 length with its strings.
    fn device(size: u16, ext_size: u32, speed: u16, configured: u16, strings: &[&str]) -> Vec<u8> {
        let mut f = vec![0u8; 0x5C];
        f[0] = 17;
        f[1] = 0x5C;
        f[0x0C..0x0E].copy_from_slice(&size.to_le_bytes());
        f[0x0E] = 0x0D; // SODIMM
        f[0x10] = 1; // locator
        f[0x11] = 4; // bank
        f[0x12] = 0x22; // DDR5
        f[0x15..0x17].copy_from_slice(&speed.to_le_bytes());
        f[0x17] = 2; // manufacturer
        f[0x1A] = 3; // part number
        f[0x1C..0x20].copy_from_slice(&ext_size.to_le_bytes());
        f[0x20..0x22].copy_from_slice(&configured.to_le_bytes());
        f[0x54..0x58].copy_from_slice(&6400u32.to_le_bytes());
        for s in strings {
            f.extend_from_slice(s.as_bytes());
            f.push(0);
        }
        if strings.is_empty() {
            f.push(0);
        }
        f.push(0);
        f
    }

    fn raw(structures: &[Vec<u8>]) -> Vec<u8> {
        let table: Vec<u8> = structures.concat();
        let mut raw = vec![0, 3, 3, 0];
        raw.extend_from_slice(&(table.len() as u32).to_le_bytes());
        raw.extend(table);
        raw
    }

    #[test]
    fn decodes_installed_and_empty_slots() {
        let other = {
            let mut s = vec![4u8, 6, 0, 0, 1, 0];
            s.extend_from_slice(b"CPU\0\0");
            s
        };
        let end = vec![127u8, 4, 0, 0, 0, 0];
        let modules = decode(&raw(&[
            other,
            device(
                16384,
                0,
                5600,
                5200,
                &["DIMM A", "Samsung", "M425R2GA3BB0", "P0 CHANNEL A"],
            ),
            device(0x7FFF, 64 * 1024, 0xFFFF, 0, &["DIMM B", "Micron", "X"]),
            device(0, 0, 0, 0, &[]),
            end,
            device(8192, 0, 4800, 4800, &["after end", "", ""]),
        ]));
        assert_eq!(modules.len(), 3, "{modules:?}");
        let a = &modules[0];
        assert_eq!(a.size, 16 << 30);
        assert_eq!((a.speed, a.configured_speed), (Some(5600), Some(5200)));
        assert_eq!(a.locator, "DIMM A");
        assert_eq!(a.bank, "P0 CHANNEL A");
        assert_eq!(a.manufacturer, "Samsung");
        assert_eq!(a.part_number, "M425R2GA3BB0");
        assert_eq!(a.form_factor_name(), Some("SODIMM"));
        assert_eq!(a.type_name(), Some("DDR5"));
        let b = &modules[1];
        assert_eq!(b.size, 64 << 30, "extended size");
        assert_eq!(b.speed, Some(6400), "extended speed");
        assert_eq!(b.configured_speed, None);
        assert_eq!(b.bank, "", "a missing string reads as none");
        let empty = &modules[2];
        assert_eq!(empty.size, 0);
        assert_eq!(empty.locator, "");
    }

    #[test]
    fn malformed_tables_end_without_reading_out_of_bounds() {
        assert!(decode(&[]).is_empty());
        assert!(decode(&[0, 3, 3, 0, 255, 255, 255, 255]).is_empty());
        let mut d = device(4096, 0, 3200, 3200, &["A"]);
        // No string terminator: the structure is dropped.
        d.truncate(d.len() - 2);
        assert!(decode(&raw(&[d])).is_empty());
        // A short formatted area reads missing fields as unknown.
        let short = vec![
            17u8, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x04, 9, 0, 0, 0,
        ];
        let m = decode(&raw(&[short]));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].size, 1 << 30);
        assert_eq!(m[0].form_factor, 9);
        assert_eq!(m[0].speed, None);
        // A KiB-sized module.
        assert_eq!(
            memory_device(
                &{
                    let mut f = vec![0u8; 0x10];
                    f[0x0C..0x0E].copy_from_slice(&(0x8000u16 | 512).to_le_bytes());
                    f
                },
                &[]
            )
            .size,
            512 << 10
        );
    }

    #[test]
    fn reads_this_machines_table() {
        // Virtual machines may report no slots; real ones report some.
        let modules = memory_modules().unwrap();
        assert!(modules.len() <= MAX_MODULES);
    }
}
