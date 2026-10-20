//! CPU Set topology. Efficiency classes are relative Windows performance tiers,
//! not vendor model identifiers. All processor indices remain group-relative.
use crate::Result;
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_DATA, HANDLE};
use windows::Win32::System::SystemInformation::{
    GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicalCpu {
    pub id: u32,
    pub group: u16,
    pub index: u8,
    pub core: u8,
    pub efficiency_class: u8,
    pub parked: bool,
    pub reserved_elsewhere: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    pub cpus: Vec<LogicalCpu>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreSelection {
    Performance,
    Efficient,
    LowPower,
    All,
}

impl CoreSelection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Performance => "P-cores",
            Self::Efficient => "E-cores",
            Self::LowPower => "LP-E cores",
            Self::All => "All available cores",
        }
    }
}

impl Topology {
    pub fn classes(&self) -> Vec<u8> {
        let mut classes: Vec<_> = self.cpus.iter().map(|c| c.efficiency_class).collect();
        classes.sort_unstable();
        classes.dedup();
        classes
    }
    pub fn hybrid(&self) -> bool {
        self.classes().len() > 1
    }
    pub fn groups(&self) -> Vec<u16> {
        let mut groups: Vec<_> = self.cpus.iter().map(|c| c.group).collect();
        groups.sort_unstable();
        groups.dedup();
        groups
    }
    pub fn tier(&self, class: u8) -> String {
        let classes = self.classes();
        if classes.len() < 2 {
            return "Uniform".into();
        }
        if classes.len() > 3 {
            return format!("Class {class}");
        }
        if Some(&class) == classes.last() {
            "P".into()
        } else if classes.len() == 3 && Some(&class) == classes.first() {
            "LP-E".into()
        } else {
            "E".into()
        }
    }
    pub fn mask(
        &self,
        selection: CoreSelection,
        group: u16,
        allowed: u64,
    ) -> std::result::Result<u64, String> {
        let classes = self.classes();
        let class = match selection {
            CoreSelection::All => None,
            _ if classes.len() < 2 => {
                return Err("Windows reports one CPU class; P/E selection is unavailable".into());
            }
            CoreSelection::Performance => classes.last().copied(),
            CoreSelection::Efficient if classes.len() <= 3 => Some(classes[classes.len() - 2]),
            CoreSelection::LowPower if classes.len() == 3 => classes.first().copied(),
            _ => return Err("This topology has no unambiguous matching core tier".into()),
        };
        let mask = self
            .cpus
            .iter()
            .filter(|c| {
                c.group == group
                    && c.index < 64
                    && !c.reserved_elsewhere
                    && class.is_none_or(|v| v == c.efficiency_class)
            })
            .fold(0u64, |mask, c| mask | (1u64 << c.index))
            & allowed;
        if mask == 0 {
            Err("No matching available processors in the process's processor group".into())
        } else {
            Ok(mask)
        }
    }
}

fn invalid() -> crate::Error {
    windows::core::Error::from_hresult(ERROR_INVALID_DATA.to_hresult()).into()
}

pub fn topology() -> Result<Topology> {
    query(None)
}

pub(crate) fn query(process: Option<HANDLE>) -> Result<Topology> {
    let mut required = 0u32;
    // SAFETY: null buffer requests its required size; the out-pointer is writable.
    let first = unsafe { GetSystemCpuSetInformation(None, 0, &mut required, process, None) };
    if !first.as_bool() {
        let error = windows::core::Error::from_thread();
        if error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
            return Err(error.into());
        }
    }
    for _ in 0..3 {
        if required == 0 || required > 4 * 1024 * 1024 {
            return Err(invalid());
        }
        let capacity = required.div_ceil(8) as usize;
        let mut buffer = vec![0u64; capacity];
        let mut returned = 0;
        // SAFETY: the u64 buffer is aligned for SYSTEM_CPU_SET_INFORMATION and
        // writable for the advertised capacity. The optional handle is live.
        let ok = unsafe {
            GetSystemCpuSetInformation(
                Some(buffer.as_mut_ptr().cast()),
                (capacity * 8) as u32,
                &mut returned,
                process,
                None,
            )
        };
        if !ok.as_bool() {
            let error = windows::core::Error::from_thread();
            if error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() {
                required = returned;
                continue;
            }
            return Err(error.into());
        }
        if returned as usize > capacity * 8 {
            return Err(invalid());
        }
        // SAFETY: the API initialized the returned byte range in this buffer;
        // it remains alive throughout decode, which copies all fields.
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), returned as usize) };
        return decode(bytes).ok_or_else(invalid);
    }
    Err(invalid())
}

fn decode(bytes: &[u8]) -> Option<Topology> {
    let mut cpus = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let header = bytes.get(offset..offset.checked_add(8)?)?;
        let size = u32::from_le_bytes(header[..4].try_into().ok()?) as usize;
        if size < 8 {
            return None;
        }
        let record = bytes.get(offset..offset.checked_add(size)?)?;
        if u32::from_le_bytes(header[4..8].try_into().ok()?) == 0 {
            if size < size_of::<SYSTEM_CPU_SET_INFORMATION>() {
                return None;
            }
            let id = u32::from_le_bytes(record[8..12].try_into().ok()?);
            let group = u16::from_le_bytes(record[12..14].try_into().ok()?);
            if record[14] >= 64
                || cpus
                    .iter()
                    .any(|c: &LogicalCpu| c.id == id || (c.group == group && c.index == record[14]))
            {
                return None;
            }
            cpus.push(LogicalCpu {
                id,
                group,
                index: record[14],
                core: record[15],
                efficiency_class: record[18],
                parked: record[19] & 1 != 0,
                reserved_elsewhere: record[19] & 2 != 0 && record[19] & 4 == 0,
            });
        }
        offset += size;
    }
    cpus.sort_by_key(|c| (c.group, c.index));
    (!cpus.is_empty()).then_some(Topology { cpus })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(id: u32, group: u16, index: u8, class: u8) -> Vec<u8> {
        let mut b = vec![0; size_of::<SYSTEM_CPU_SET_INFORMATION>()];
        let size = b.len() as u32;
        b[..4].copy_from_slice(&size.to_le_bytes());
        b[8..12].copy_from_slice(&id.to_le_bytes());
        b[12..14].copy_from_slice(&group.to_le_bytes());
        b[14] = index;
        b[15] = index;
        b[18] = class;
        b
    }
    #[test]
    fn tiers_masks_and_groups_preserve_real_classes_and_unknown_records() {
        let mut bytes = record(1, 0, 0, 10);
        bytes.extend(record(2, 0, 1, 100));
        bytes.extend(record(3, 1, 0, 200));
        bytes.extend([8, 0, 0, 0, 99, 0, 0, 0]);
        let t = decode(&bytes).unwrap();
        assert_eq!(t.classes(), [10, 100, 200]);
        assert_eq!(t.groups(), [0, 1]);
        assert_eq!(t.tier(10), "LP-E");
        assert_eq!(t.tier(100), "E");
        assert_eq!(t.tier(200), "P");
        assert_eq!(t.mask(CoreSelection::Efficient, 0, u64::MAX).unwrap(), 2);
        assert!(t.mask(CoreSelection::Performance, 0, u64::MAX).is_err());
        assert_eq!(t.mask(CoreSelection::Performance, 1, u64::MAX).unwrap(), 1);
        assert!(t.mask(CoreSelection::Efficient, 0, 1).is_err());
        let mut homogeneous = t.clone();
        for c in &mut homogeneous.cpus {
            c.efficiency_class = 0;
        }
        assert!(!homogeneous.hybrid());
        assert!(
            homogeneous
                .mask(CoreSelection::Performance, 0, u64::MAX)
                .is_err()
        );
        assert_eq!(
            homogeneous.mask(CoreSelection::All, 0, u64::MAX).unwrap(),
            3
        );
        let mut future = record(10, 0, 0, 4);
        future.extend([0; 8]);
        let size = future.len() as u32;
        future[..4].copy_from_slice(&size.to_le_bytes());
        future.extend(record(11, 0, 1, 220));
        let two = decode(&future).unwrap();
        assert_eq!(two.tier(4), "E");
        assert_eq!(two.tier(220), "P");
        assert_eq!(
            two.mask(CoreSelection::Performance, 0, u64::MAX).unwrap(),
            2
        );
        assert!(two.mask(CoreSelection::LowPower, 0, u64::MAX).is_err());
    }
    #[test]
    fn topology_rejects_truncated_duplicate_and_reserved_processors() {
        assert!(decode(&[0; 8]).is_none());
        assert!(decode(&record(1, 0, 0, 0)[..20]).is_none());
        let mut bytes = record(1, 0, 0, 0);
        bytes.extend(record(2, 0, 0, 1));
        assert!(decode(&bytes).is_none());
        let mut bytes = record(1, 0, 63, 0);
        bytes[19] = 2;
        let t = decode(&bytes).unwrap();
        assert!(t.mask(CoreSelection::All, 0, u64::MAX).is_err());
        bytes[19] = 6;
        assert_eq!(
            decode(&bytes)
                .unwrap()
                .mask(CoreSelection::All, 0, u64::MAX)
                .unwrap(),
            1 << 63
        );
    }
    #[test]
    fn discovers_live_topology_without_changing_affinity() {
        let t = topology().unwrap();
        assert!(!t.cpus.is_empty());
        assert_eq!(t.cpus.len() as u32, crate::system::logical_cpu_count());
    }
}
