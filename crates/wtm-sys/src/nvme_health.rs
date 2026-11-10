//! Fixed-size, read-only NVMe log-page queries. No pass-through or setting writes.
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, NVMeDataTypeLogPage, PropertyStandardQuery, ProtocolTypeNvme,
    StorageAdapterProtocolSpecificProperty, StorageDeviceProtocolSpecificProperty,
};
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub critical: u8,
    pub temperature: Option<f64>,
    pub spare: Option<u8>,
    pub used: u8,
    pub read_units: u128,
    pub written_units: u128,
    pub power_cycles: u128,
    pub hours: u128,
    pub unsafe_shutdowns: u128,
    pub media_errors: u128,
}
pub fn parse(data: &[u8]) -> Result<Health, String> {
    if data.len() != 512 || data.iter().all(|b| *b == 0) || data.iter().all(|b| *b == 255) {
        return Err("NVMe health log is unavailable or invalid".into());
    }
    let count = |offset| {
        u128::from_le_bytes(
            data[offset..offset + 16]
                .try_into()
                .expect("fixed validated log"),
        )
    };
    let kelvin = u16::from_le_bytes([data[1], data[2]]);
    Ok(Health {
        critical: data[0],
        temperature: (200..=1000)
            .contains(&kelvin)
            .then_some(f64::from(kelvin) - 273.15),
        spare: (data[3] <= 100).then_some(data[3]),
        used: data[5],
        read_units: count(32),
        written_units: count(48),
        power_cycles: count(112),
        hours: count(128),
        unsafe_shutdowns: count(144),
        media_errors: count(160),
    })
}
fn log(buffer: &[u32; 140], returned: usize) -> Result<Health, String> {
    if !(48..=560).contains(&returned)
        || buffer[0] != 48
        || buffer[1] != 48
        || buffer[2] != ProtocolTypeNvme.0 as u32
        || buffer[3] != NVMeDataTypeLogPage.0 as u32
    {
        return Err("Invalid NVMe protocol descriptor".into());
    }
    let offset = 8usize
        .checked_add(buffer[6] as usize)
        .ok_or("Invalid log offset")?;
    let length = buffer[7] as usize;
    if buffer[6] < 40
        || length != 512
        || offset.checked_add(length).is_none_or(|end| end > returned)
    {
        return Err("NVMe log lies outside its returned buffer".into());
    }
    // SAFETY: the entire fixed initialized u32 buffer is live; bytes are read only,
    // and offsets above were validated against both its capacity and returned size.
    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), 560) };
    parse(&bytes[offset..offset + length])
}
pub fn read(number: u32) -> Result<Health, String> {
    if number > 4095 {
        return Err("Invalid physical disk number".into());
    }
    let path = format!(r"\\.\PhysicalDrive{number}");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .access_mode(0)
        .share_mode(7)
        .open(path)
        .map_err(|e| e.to_string())?;
    let mut error = String::new();
    for property in [
        StorageDeviceProtocolSpecificProperty,
        StorageAdapterProtocolSpecificProperty,
    ] {
        let mut buffer = [0u32; 140];
        buffer[0] = property.0 as u32;
        buffer[1] = PropertyStandardQuery.0 as u32;
        buffer[2] = ProtocolTypeNvme.0 as u32;
        buffer[3] = NVMeDataTypeLogPage.0 as u32;
        buffer[4] = 2; // Standard SMART/health information log, read-only.
        buffer[6] = 40;
        buffer[7] = 512;
        let pointer = buffer.as_mut_ptr();
        let mut returned = 0;
        // SAFETY: file owns a live handle; both pointers address the same fixed 560-byte
        // initialized METHOD_BUFFERED query. No driver-reported length controls allocation.
        let result = unsafe {
            DeviceIoControl(
                HANDLE(file.as_raw_handle()),
                IOCTL_STORAGE_QUERY_PROPERTY,
                Some(pointer.cast()),
                560,
                Some(pointer.cast()),
                560,
                Some(&mut returned),
                None,
            )
        };
        match result {
            Ok(()) => match log(&buffer, returned as usize) {
                Ok(h) => return Ok(h),
                Err(e) => error = e,
            },
            Err(e) => error = e.to_string(),
        }
    }
    Err(error)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn health_log_decodes_units_and_does_not_invent_temperature() {
        let mut b = [0u8; 512];
        b[1..3].copy_from_slice(&300u16.to_le_bytes());
        b[3] = 99;
        b[5] = 120;
        b[32..48].copy_from_slice(&123u128.to_le_bytes());
        let h = parse(&b).unwrap();
        assert!((h.temperature.unwrap() - 26.85).abs() < 0.001);
        assert_eq!(h.used, 120);
        assert_eq!(h.read_units, 123);
        b[1] = 0;
        b[2] = 0;
        assert_eq!(parse(&b).unwrap().temperature, None);
    }
    #[test]
    fn truncated_oversized_and_out_of_bounds_replies_are_rejected() {
        assert!(parse(&[0; 512]).is_err());
        assert!(parse(&[255; 512]).is_err());
        assert!(parse(&[1; 511]).is_err());
        let mut b = [0u32; 140];
        b[0] = 48;
        b[1] = 48;
        b[2] = 3;
        b[3] = 2;
        b[6] = u32::MAX;
        b[7] = 512;
        assert!(log(&b, 560).is_err());
        assert!(log(&b, 561).is_err());
        assert!(log(&b, 47).is_err());
    }
}
