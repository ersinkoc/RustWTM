//! Windows power-plan frequency policy. These settings affect shared cores,
//! not individual PIDs. Policy class numbers are not vendor P/E identifiers.
use crate::Result;
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{ERROR_INVALID_PARAMETER, HLOCAL, LocalFree, WIN32_ERROR};
use windows::Win32::System::Power::{
    PowerGetActiveScheme, PowerReadACValueIndex, PowerReadDCValueIndex, PowerSetActiveScheme,
    PowerWriteACValueIndex, PowerWriteDCValueIndex,
};
use windows::core::GUID;

const PROCESSOR: GUID = GUID::from_u128(0x54533251_82be_4824_96c1_47b60b740d00);
const FREQUENCY: [GUID; 2] = [
    GUID::from_u128(0x75b0ae3f_bce0_45a7_8c89_c9611c25e100),
    GUID::from_u128(0x75b0ae3f_bce0_45a7_8c89_c9611c25e101),
];

/// The battery as Windows reports it; each value `None` when unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryStatus {
    /// Charge, 0..=100.
    pub percent: Option<u8>,
    /// Plugged in.
    pub on_ac: Option<bool>,
    pub charging: bool,
    /// Time left on battery, in seconds.
    pub seconds_left: Option<u32>,
    pub saver: bool,
}

/// What `SYSTEM_POWER_STATUS` says, or `None` without a system battery
/// (desktops) or when Windows does not know.
pub fn decode_battery(
    ac_line: u8,
    flag: u8,
    percent: u8,
    saver: u8,
    seconds: u32,
) -> Option<BatteryStatus> {
    // 128: no system battery; 255: status unknown.
    if flag & 128 != 0 || flag == 255 {
        return None;
    }
    Some(BatteryStatus {
        percent: (percent <= 100).then_some(percent),
        on_ac: match ac_line {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        charging: flag & 8 != 0,
        seconds_left: (seconds != u32::MAX).then_some(seconds),
        saver: saver & 1 != 0,
    })
}

/// The system battery, if the PC has one.
pub fn battery() -> Option<BatteryStatus> {
    let mut s = windows::Win32::System::Power::SYSTEM_POWER_STATUS::default();
    // SAFETY: `s` is a writable SYSTEM_POWER_STATUS.
    unsafe { windows::Win32::System::Power::GetSystemPowerStatus(&mut s) }.ok()?;
    decode_battery(
        s.ACLineStatus,
        s.BatteryFlag,
        s.BatteryLifePercent,
        s.SystemStatusFlag,
        s.BatteryLifeTime,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerSource {
    Ac,
    Dc,
}
impl PowerSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ac => "Plugged in",
            Self::Dc => "Battery",
        }
    }
}

fn setting(class: u8) -> Result<&'static GUID> {
    FREQUENCY.get(usize::from(class)).ok_or_else(|| {
        windows::core::Error::from_hresult(ERROR_INVALID_PARAMETER.to_hresult()).into()
    })
}

pub fn active_scheme() -> Result<String> {
    let mut pointer = std::ptr::null_mut();
    // SAFETY: writable out-pointer receives a LocalAlloc-owned GUID.
    unsafe { PowerGetActiveScheme(None, &mut pointer) }.ok()?;
    if pointer.is_null() {
        return Err(
            windows::core::Error::from_hresult(ERROR_INVALID_PARAMETER.to_hresult()).into(),
        );
    }
    // SAFETY: the successful call returned a readable GUID; copying precedes free.
    let guid = unsafe { *pointer };
    // SAFETY: returned allocation must be released with LocalFree.
    unsafe { LocalFree(Some(HLOCAL(pointer.cast()))) };
    Ok(format!("{guid:?}"))
}

pub fn read_frequency(plan: &str, class: u8, source: PowerSource) -> Result<u32> {
    let plan = GUID::try_from(plan)?;
    let setting = setting(class)?;
    let mut value = 0;
    // SAFETY: GUIDs and out-pointer are live for this synchronous call.
    unsafe {
        match source {
            PowerSource::Ac => PowerReadACValueIndex(
                None,
                Some(&plan),
                Some(&PROCESSOR),
                Some(setting),
                &mut value,
            )
            .ok()?,
            PowerSource::Dc => WIN32_ERROR(PowerReadDCValueIndex(
                None,
                Some(&plan),
                Some(&PROCESSOR),
                Some(setting),
                &mut value,
            ))
            .ok()?,
        }
    };
    Ok(value)
}

pub fn write_frequency(plan: &str, class: u8, source: PowerSource, value: u32) -> Result<()> {
    let plan = GUID::try_from(plan)?;
    let setting = setting(class)?;
    // SAFETY: valid GUID references; Windows enforces policy write permissions.
    unsafe {
        WIN32_ERROR(match source {
            PowerSource::Ac => {
                PowerWriteACValueIndex(None, &plan, Some(&PROCESSOR), Some(setting), value).0
            }
            PowerSource::Dc => {
                PowerWriteDCValueIndex(None, &plan, Some(&PROCESSOR), Some(setting), value)
            }
        })
        .ok()?
    };
    Ok(())
}

/// The built-in High performance power plan.
pub const HIGH_PERFORMANCE: &str = "8C5E7FDA-E8BF-4A96-9A85-A6E23A8C635C";

/// Makes `plan` (a GUID) the active power plan.
pub fn set_active_scheme(plan: &str) -> Result<()> {
    refresh_scheme(plan)
}

pub fn refresh_scheme(plan: &str) -> Result<()> {
    let plan = GUID::try_from(plan)?;
    // SAFETY: the scheme GUID reference is valid for the synchronous call.
    unsafe { PowerSetActiveScheme(None, Some(&plan)) }.ok()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_status_reads_windows_sentinels() {
        assert_eq!(decode_battery(1, 128, 255, 0, u32::MAX), None, "no battery");
        assert_eq!(decode_battery(255, 255, 255, 0, u32::MAX), None, "unknown");
        let b = decode_battery(0, 1, 84, 1, 9000).unwrap();
        assert_eq!(b.percent, Some(84));
        assert_eq!(b.on_ac, Some(false));
        assert!(!b.charging && b.saver);
        assert_eq!(b.seconds_left, Some(9000));
        let charging = decode_battery(1, 8, 255, 0, u32::MAX).unwrap();
        assert!(charging.charging);
        assert_eq!((charging.percent, charging.seconds_left), (None, None));
    }
    #[test]
    fn reads_current_plan_without_changing_it() {
        let plan = active_scheme().unwrap();
        assert_eq!(plan.len(), 36);
        let _ = read_frequency(&plan, 0, PowerSource::Ac);
        assert!(read_frequency(&plan, 2, PowerSource::Ac).is_err());
        assert_eq!(active_scheme().unwrap(), plan);
    }
}
