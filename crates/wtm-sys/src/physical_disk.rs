//! Identity of a physical disk, as Task Manager lists it under the disk
//! graphs: model, bus, SSD or HDD and capacity. Read-only storage queries
//! on a handle opened without read or write access, so no administrator
//! rights are needed and nothing on the disk is touched.

use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery,
    StorageDeviceProperty, StorageDeviceSeekPenaltyProperty, StorageDeviceTemperatureProperty,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PhysicalDisk {
    pub number: u32,
    /// Vendor and product, such as "Samsung SSD 990 PRO 2TB".
    pub model: String,
    /// `STORAGE_BUS_TYPE` code (17 NVMe, 11 SATA, 7 USB, ...).
    pub bus: u32,
    pub removable: bool,
    /// Solid state (no seek penalty); `None` when the drive does not say.
    pub ssd: Option<bool>,
    /// Capacity in bytes.
    pub size: Option<u64>,
}

impl PhysicalDisk {
    pub fn bus_name(&self) -> Option<&'static str> {
        Some(match self.bus {
            1 => "SCSI",
            2 => "ATAPI",
            3 => "ATA",
            4 => "IEEE 1394",
            5 => "SSA",
            6 => "Fibre Channel",
            7 => "USB",
            8 => "RAID",
            9 => "iSCSI",
            10 => "SAS",
            11 => "SATA",
            12 => "SD",
            13 => "MMC",
            14 => "Virtual",
            15 => "File-backed virtual",
            16 => "Storage Spaces",
            17 => "NVMe",
            18 => "SCM",
            19 => "UFS",
            _ => return None,
        })
    }
}

/// A drive's temperature and the limits it reports, in degrees Celsius.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DiskTemperature {
    pub celsius: f32,
    /// Where the drive starts warning about heat.
    pub warning: Option<f32>,
    /// Where the drive may slow down or stop to protect itself.
    pub critical: Option<f32>,
}

/// Room for a device descriptor with its strings.
const DESCRIPTOR_BYTES: usize = 1024;

fn query(handle: HANDLE, code: u32, input: &[u32], out: &mut [u64]) -> Option<usize> {
    let mut returned = 0u32;
    // SAFETY: `handle` is live for the call; both buffers are valid for
    // the lengths passed and the call is METHOD_BUFFERED.
    unsafe {
        DeviceIoControl(
            handle,
            code,
            (!input.is_empty()).then_some(input.as_ptr().cast()),
            size_of_val(input) as u32,
            Some(out.as_mut_ptr().cast()),
            size_of_val(out) as u32,
            Some(&mut returned),
            None,
        )
    }
    .ok()?;
    Some((returned as usize).min(size_of_val(out)))
}

fn bytes(buf: &[u64], len: usize) -> &[u8] {
    // SAFETY: a u64 slice is also readable as bytes; `len` is at most its
    // byte length (clamped in `query`).
    unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), len.min(size_of_val(buf))) }
}

/// `\\.\PhysicalDrive{number}`, opened without read or write access.
fn open(number: u32) -> Result<std::fs::File, String> {
    if number > 4095 {
        return Err("Invalid physical disk number".into());
    }
    std::fs::OpenOptions::new()
        .read(true)
        .access_mode(0)
        .share_mode(7)
        .open(format!(r"\\.\PhysicalDrive{number}"))
        .map_err(|e| e.to_string())
}

/// The temperature of `\\.\PhysicalDrive{number}`, for drives that report
/// one (NVMe and most SATA drives; USB enclosures usually do not). The query
/// may wake a sleeping drive, so callers read it only while its page is open.
pub fn temperature(number: u32) -> Result<DiskTemperature, String> {
    let file = open(number)?;
    let mut buf = [0u64; 64];
    let input = [
        StorageDeviceTemperatureProperty.0 as u32,
        PropertyStandardQuery.0 as u32,
        0,
    ];
    let len = query(
        HANDLE(file.as_raw_handle()),
        IOCTL_STORAGE_QUERY_PROPERTY,
        &input,
        &mut buf,
    )
    .ok_or("The drive does not report its temperature")?;
    decode_temperature(bytes(&buf, len)).ok_or_else(|| "No temperature reading".into())
}

/// Reads a `STORAGE_TEMPERATURE_DATA_DESCRIPTOR`: the first sensor is the
/// drive's own (NVMe "composite") temperature.
fn decode_temperature(d: &[u8]) -> Option<DiskTemperature> {
    let i16_at = |at: usize| -> Option<i16> {
        Some(i16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
    };
    // A plausible reading or limit, in degrees Celsius.
    let degrees = |v: i16| (-40..=150).contains(&v).then_some(f32::from(v));
    if i16_at(12)? == 0 {
        return None;
    }
    // STORAGE_TEMPERATURE_INFO entries start at 24; Temperature is at 2.
    let celsius = i16_at(26).and_then(degrees)?;
    let limit = |at| i16_at(at).filter(|v| *v > 0).and_then(degrees);
    Some(DiskTemperature {
        celsius,
        warning: limit(10),
        critical: limit(8),
    })
}

/// The identity of `\\.\PhysicalDrive{number}`.
pub fn read(number: u32) -> Result<PhysicalDisk, String> {
    let file = open(number)?;
    let handle = HANDLE(file.as_raw_handle());
    let mut disk = PhysicalDisk {
        number,
        ..Default::default()
    };
    let mut buf = [0u64; DESCRIPTOR_BYTES / 8];
    let device = [
        StorageDeviceProperty.0 as u32,
        PropertyStandardQuery.0 as u32,
        0,
    ];
    if let Some(len) = query(handle, IOCTL_STORAGE_QUERY_PROPERTY, &device, &mut buf) {
        decode_descriptor(bytes(&buf, len), &mut disk);
    }
    let mut buf = [0u64; 2];
    let seek = [
        StorageDeviceSeekPenaltyProperty.0 as u32,
        PropertyStandardQuery.0 as u32,
        0,
    ];
    if let Some(len) = query(handle, IOCTL_STORAGE_QUERY_PROPERTY, &seek, &mut buf) {
        // DEVICE_SEEK_PENALTY_DESCRIPTOR: IncursSeekPenalty at 8.
        disk.ssd = bytes(&buf, len).get(8).map(|p| *p == 0);
    }
    let mut buf = [0u64; 8];
    if let Some(len) = query(handle, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut buf) {
        // DISK_GEOMETRY_EX: DiskSize after the 24-byte DISK_GEOMETRY.
        disk.size = bytes(&buf, len)
            .get(24..32)
            .and_then(|b| b.try_into().ok())
            .map(i64::from_le_bytes)
            .and_then(|s| u64::try_from(s).ok())
            .filter(|s| *s > 0);
    }
    Ok(disk)
}

/// A NUL-terminated ASCII string at `offset` of a descriptor; 0 is none.
fn descriptor_string(d: &[u8], offset: u32) -> String {
    let offset = offset as usize;
    if offset == 0 || offset >= d.len() {
        return String::new();
    }
    let s = &d[offset..];
    let end = s.iter().position(|b| *b == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..end]).trim().to_owned()
}

/// Reads a `STORAGE_DEVICE_DESCRIPTOR`.
fn decode_descriptor(d: &[u8], disk: &mut PhysicalDisk) {
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
    };
    disk.removable = d.get(10).is_some_and(|r| *r != 0);
    let vendor = descriptor_string(d, u32_at(12).unwrap_or(0));
    let product = descriptor_string(d, u32_at(16).unwrap_or(0));
    // Some drives repeat the vendor in the product name, or report "ATA"
    // or "NVMe" as their vendor.
    let generic = ["ATA", "NVME", "SCSI", ""].contains(&vendor.to_ascii_uppercase().as_str());
    disk.model = if generic || product.starts_with(&vendor) {
        product
    } else if product.is_empty() {
        vendor
    } else {
        format!("{vendor} {product}")
    };
    disk.bus = u32_at(28).unwrap_or(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(vendor: &str, product: &str, bus: u32, removable: bool) -> Vec<u8> {
        let mut d = vec![0u8; 40];
        d[10] = u8::from(removable);
        let mut put = |at: usize, s: &str| {
            let offset = d.len() as u32;
            d[at..at + 4].copy_from_slice(&offset.to_le_bytes());
            d.extend_from_slice(s.as_bytes());
            d.push(0);
        };
        put(12, vendor);
        put(16, product);
        d[28..32].copy_from_slice(&bus.to_le_bytes());
        d
    }

    #[test]
    fn decodes_the_drive_temperature_and_its_limits() {
        let mut d = vec![0u8; 72];
        d[8..10].copy_from_slice(&85i16.to_le_bytes());
        d[10..12].copy_from_slice(&81i16.to_le_bytes());
        d[12..14].copy_from_slice(&3u16.to_le_bytes());
        d[26..28].copy_from_slice(&46i16.to_le_bytes());
        assert_eq!(
            decode_temperature(&d),
            Some(DiskTemperature {
                celsius: 46.0,
                warning: Some(81.0),
                critical: Some(85.0),
            })
        );
        // No sensors, an implausible reading, or a short buffer: nothing.
        let mut none = d.clone();
        none[12..14].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(decode_temperature(&none), None);
        let mut hot = d.clone();
        hot[26..28].copy_from_slice(&9000i16.to_le_bytes());
        assert_eq!(decode_temperature(&hot), None);
        assert_eq!(decode_temperature(&d[..20]), None);
        // Limits of 0 are "not reported".
        let mut bare = d;
        bare[8..12].fill(0);
        let t = decode_temperature(&bare).unwrap();
        assert_eq!((t.warning, t.critical), (None, None));
    }

    #[test]
    fn decodes_model_bus_and_removable() {
        let mut disk = PhysicalDisk::default();
        decode_descriptor(
            &descriptor("", "Samsung SSD 990 PRO 2TB", 17, false),
            &mut disk,
        );
        assert_eq!(disk.model, "Samsung SSD 990 PRO 2TB");
        assert_eq!(disk.bus_name(), Some("NVMe"));
        assert!(!disk.removable);
        decode_descriptor(&descriptor("SanDisk", "Ultra", 7, true), &mut disk);
        assert_eq!(disk.model, "SanDisk Ultra");
        assert!(disk.removable);
        decode_descriptor(&descriptor("ATA", "WDC WD10EZEX", 11, false), &mut disk);
        assert_eq!(disk.model, "WDC WD10EZEX");
        decode_descriptor(&descriptor("WD", "WD Blue", 11, false), &mut disk);
        assert_eq!(disk.model, "WD Blue");
    }

    #[test]
    fn bad_offsets_read_as_empty() {
        let mut d = descriptor("A", "B", 11, false);
        d[16..20].copy_from_slice(&5000u32.to_le_bytes());
        let mut disk = PhysicalDisk::default();
        decode_descriptor(&d, &mut disk);
        assert_eq!(disk.model, "A");
        decode_descriptor(&[0; 4], &mut disk);
        assert_eq!((disk.model.as_str(), disk.bus), ("", 0));
        assert_eq!(descriptor_string(b"abc", 0), "");
        assert_eq!(descriptor_string(b"abc", 1), "bc", "no terminator");
    }

    #[test]
    fn reads_the_first_disk() {
        let disk = read(0).unwrap();
        assert!(!disk.model.is_empty(), "{disk:?}");
        assert!(disk.size.is_some_and(|s| s > 1 << 30), "{disk:?}");
        assert!(read(5000).is_err());
    }
}
