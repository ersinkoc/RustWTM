//! What the display driver reports about a GPU's state: temperature, fan
//! speed and memory clock, as Task Manager shows the temperature under the
//! GPU graphs. Read through the graphics kernel by adapter LUID; no
//! administrator rights are needed and nothing is changed.

use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTER_PERFDATA, D3DKMT_ADAPTER_PERFDATACAPS, D3DKMT_CLOSEADAPTER,
    D3DKMT_OPENADAPTERFROMLUID, D3DKMT_QUERYADAPTERINFO, D3DKMTCloseAdapter,
    D3DKMTOpenAdapterFromLuid, D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERPERFDATA,
    KMTQAITYPE_ADAPTERPERFDATA_CAPS, KMTQUERYADAPTERINFOTYPE,
};
use windows::Win32::Foundation::LUID;

/// A GPU's state; each value is `None` when the driver does not report it.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GpuPerf {
    /// Degrees Celsius.
    pub temperature: Option<f32>,
    /// The highest temperature the GPU is rated for, in degrees Celsius.
    pub temperature_max: Option<f32>,
    /// Fan speed in RPM; 0 when the fans are stopped.
    pub fan_rpm: Option<u32>,
    /// Current memory clock in MHz.
    pub memory_mhz: Option<u32>,
    /// Highest memory clock in MHz.
    pub memory_max_mhz: Option<u32>,
}

/// Reads a LUID as [`crate::gpu`] writes it ("{high:08x}_{low:08x}").
pub fn parse_luid(text: &str) -> Option<(u32, u32)> {
    let (high, low) = text.split_once('_')?;
    Some((
        u32::from_str_radix(high, 16).ok()?,
        u32::from_str_radix(low, 16).ok()?,
    ))
}

/// Drivers report temperatures in tenths of a degree; 0 is "not reported".
fn tenths(v: u32) -> Option<f32> {
    // Above 200 °C is not a reading.
    (v > 0 && v < 2000).then(|| v as f32 / 10.0)
}

fn mhz(hz: u64) -> Option<u32> {
    u32::try_from(hz / 1_000_000).ok().filter(|m| *m > 0)
}

/// What the driver's performance data says.
pub fn decode(
    perf: &D3DKMT_ADAPTER_PERFDATA,
    caps: Option<&D3DKMT_ADAPTER_PERFDATACAPS>,
) -> GpuPerf {
    let has_fan = caps.is_some_and(|c| c.MaxFanRPM > 0);
    GpuPerf {
        temperature: tenths(perf.Temperature),
        temperature_max: caps.and_then(|c| tenths(c.TemperatureMax)),
        // A stopped fan reads 0 RPM, so the fan is known from its maximum.
        fan_rpm: (has_fan || perf.FanRPM > 0).then_some(perf.FanRPM),
        memory_mhz: mhz(perf.MemoryFrequency),
        memory_max_mhz: mhz(perf.MaxMemoryFrequency),
    }
}

/// Fills `out` with adapter information of this type.
fn query<T>(adapter: u32, kind: KMTQUERYADAPTERINFOTYPE, out: &mut T) -> bool {
    let mut q = D3DKMT_QUERYADAPTERINFO {
        hAdapter: adapter,
        Type: kind,
        pPrivateDriverData: (out as *mut T).cast(),
        PrivateDriverDataSize: size_of::<T>() as u32,
    };
    // SAFETY: `q` points at `out`, writable for the size passed.
    unsafe { D3DKMTQueryAdapterInfo(&mut q) }.is_ok()
}

/// The state of the GPU with this LUID, or `None` when its driver does not
/// report one (older drivers, software adapters).
pub fn read(luid: &str) -> Option<GpuPerf> {
    let (high, low) = parse_luid(luid)?;
    let mut open = D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: LUID {
            LowPart: low,
            HighPart: high as i32,
        },
        hAdapter: 0,
    };
    // SAFETY: `open` is a valid, writable request.
    unsafe { D3DKMTOpenAdapterFromLuid(&mut open) }.ok().ok()?;
    let mut perf = D3DKMT_ADAPTER_PERFDATA::default();
    let mut caps = D3DKMT_ADAPTER_PERFDATACAPS::default();
    let read = query(open.hAdapter, KMTQAITYPE_ADAPTERPERFDATA, &mut perf);
    let has_caps = read && query(open.hAdapter, KMTQAITYPE_ADAPTERPERFDATA_CAPS, &mut caps);
    let close = D3DKMT_CLOSEADAPTER {
        hAdapter: open.hAdapter,
    };
    // SAFETY: closes the handle opened above, once.
    let _ = unsafe { D3DKMTCloseAdapter(&close) };
    read.then(|| decode(&perf, has_caps.then_some(&caps)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_what_the_driver_reports() {
        let perf = D3DKMT_ADAPTER_PERFDATA {
            Temperature: 463,
            FanRPM: 0,
            MemoryFrequency: 405_000_000,
            MaxMemoryFrequency: 15_001_000_000,
            ..Default::default()
        };
        let caps = D3DKMT_ADAPTER_PERFDATACAPS {
            MaxFanRPM: 3000,
            TemperatureMax: 880,
            ..Default::default()
        };
        let p = decode(&perf, Some(&caps));
        assert_eq!(p.temperature, Some(46.3));
        assert_eq!(p.temperature_max, Some(88.0));
        assert_eq!(p.fan_rpm, Some(0), "a stopped fan is still a fan");
        assert_eq!(p.memory_mhz, Some(405));
        assert_eq!(p.memory_max_mhz, Some(15_001));
        // No caps and no readings: nothing is claimed.
        let none = decode(&D3DKMT_ADAPTER_PERFDATA::default(), None);
        assert_eq!(none, GpuPerf::default());
        // Out-of-range temperatures are not readings.
        let hot = D3DKMT_ADAPTER_PERFDATA {
            Temperature: 50_000,
            ..Default::default()
        };
        assert_eq!(decode(&hot, None).temperature, None);
    }

    #[test]
    fn parses_luid_text() {
        assert_eq!(parse_luid("00000000_00015365"), Some((0, 0x15365)));
        assert_eq!(parse_luid("ffffffff_00000001"), Some((u32::MAX, 1)));
        assert_eq!(parse_luid("nope"), None);
        assert_eq!(parse_luid("zz_1"), None);
    }
}
