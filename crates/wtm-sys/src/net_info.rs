//! What Task Manager lists under a network adapter's graph: its IP
//! addresses and DNS suffix, and for Wi-Fi the network name, signal
//! quality and radio standard. Keyed by the interface LUID that
//! [`crate::system::net_adapters`] reports.

use std::net::{Ipv4Addr, Ipv6Addr};

use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, HANDLE};
use windows::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceGuidToLuid, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
    GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces,
    WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, wlan_interface_state_connected,
    wlan_intf_opcode_current_connection,
};
use windows::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Addresses {
    pub luid: u64,
    pub ipv4: Vec<Ipv4Addr>,
    pub ipv6: Vec<Ipv6Addr>,
    pub dns_suffix: String,
    /// Hardware (MAC) address, such as "00-1A-2B-3C-4D-5E"; empty when the
    /// adapter has none.
    #[serde(default)]
    pub mac: String,
}

/// A hardware address as Windows writes it ("00-1A-2B-3C-4D-5E"); empty
/// when it has no bytes or only zeros.
pub fn mac_text(bytes: &[u8]) -> String {
    if bytes.iter().all(|b| *b == 0) {
        return String::new();
    }
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join("-")
}

/// Largest adapter list accepted.
const MAX_LIST_BYTES: u32 = 4 * 1024 * 1024;
/// Most adapters, and addresses per adapter, read.
const MAX_ITEMS: usize = 64;

fn wide(p: windows::core::PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: a non-null string the API returned, NUL-terminated and
    // alive with the list that holds it.
    String::from_utf16_lossy(unsafe { p.as_wide() })
}

/// The unicast addresses and DNS suffix of every adapter.
pub fn addresses() -> crate::Result<Vec<Addresses>> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    for _ in 0..3 {
        if size > MAX_LIST_BYTES {
            break;
        }
        // u64 elements keep the entries aligned.
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let first = buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `buf` is writable for `size` bytes.
        let status = unsafe {
            GetAdaptersAddresses(u32::from(AF_UNSPEC.0), flags, None, Some(first), &mut size)
        };
        if status == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if status != 0 {
            return Err(
                windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(status))
                    .into(),
            );
        }
        let mut out = Vec::new();
        let mut entry = first.cast_const();
        while !entry.is_null() && out.len() < MAX_ITEMS {
            // SAFETY: the API linked its entries inside `buf`, alive here.
            let a = unsafe { &*entry };
            let mut item = Addresses {
                // SAFETY: plain integer view of the LUID union.
                luid: unsafe { a.Luid.Value },
                dns_suffix: wide(a.DnsSuffix),
                mac: mac_text(
                    &a.PhysicalAddress
                        [..(a.PhysicalAddressLength as usize).min(a.PhysicalAddress.len())],
                ),
                ..Default::default()
            };
            let mut unicast = a.FirstUnicastAddress.cast_const();
            let mut seen = 0;
            while !unicast.is_null() && seen < MAX_ITEMS {
                seen += 1;
                // SAFETY: as above.
                let u = unsafe { &*unicast };
                let sockaddr = u.Address.lpSockaddr;
                let len = usize::try_from(u.Address.iSockaddrLength).unwrap_or(0);
                if !sockaddr.is_null() {
                    // SAFETY: the address is `len` bytes long; its family
                    // says which structure it is.
                    let family = unsafe { (*sockaddr).sa_family };
                    if family == AF_INET && len >= size_of::<SOCKADDR_IN>() {
                        // SAFETY: an AF_INET address of full size.
                        let v4 = unsafe { &*sockaddr.cast::<SOCKADDR_IN>() };
                        // SAFETY: plain integer view of the address union.
                        let raw = unsafe { v4.sin_addr.S_un.S_addr };
                        item.ipv4.push(Ipv4Addr::from(u32::from_be(raw)));
                    } else if family == AF_INET6 && len >= size_of::<SOCKADDR_IN6>() {
                        // SAFETY: an AF_INET6 address of full size.
                        let v6 = unsafe { &*sockaddr.cast::<SOCKADDR_IN6>() };
                        // SAFETY: plain byte view of the address union.
                        item.ipv6
                            .push(Ipv6Addr::from(unsafe { v6.sin6_addr.u.Byte }));
                    }
                }
                unicast = u.Next.cast_const();
            }
            out.push(item);
            entry = a.Next.cast_const();
        }
        return Ok(out);
    }
    Ok(Vec::new())
}

/// The network a Wi-Fi adapter is connected to.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WifiLink {
    pub luid: u64,
    /// Network name; empty when Windows withholds it (location access).
    pub ssid: String,
    /// Signal quality, 0..=100.
    pub signal: u32,
    /// The radio standard, such as "802.11ax".
    pub standard: Option<&'static str>,
}

fn standard(phy: i32) -> Option<&'static str> {
    Some(match phy {
        4 => "802.11a",
        5 => "802.11b",
        6 => "802.11g",
        7 => "802.11n",
        8 => "802.11ac",
        9 => "802.11ad",
        10 => "802.11ax",
        11 => "802.11be",
        _ => return None,
    })
}

/// Connected Wi-Fi adapters; empty without the WLAN service.
pub fn wifi_links() -> Vec<WifiLink> {
    let mut version = 0u32;
    let mut handle = HANDLE::default();
    // SAFETY: both out-pointers reference live locals.
    if unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
    // SAFETY: valid handle; `list` receives memory freed below.
    if unsafe { WlanEnumInterfaces(handle, None, &mut list) } == 0 && !list.is_null() {
        // SAFETY: the list holds dwNumberOfItems entries.
        let interfaces = unsafe {
            std::slice::from_raw_parts(
                (*list).InterfaceInfo.as_ptr(),
                ((*list).dwNumberOfItems as usize).min(MAX_ITEMS),
            )
        };
        for i in interfaces
            .iter()
            .filter(|i| i.isState == wlan_interface_state_connected)
        {
            let mut luid = NET_LUID_LH::default();
            // SAFETY: both pointers reference live values.
            if unsafe { ConvertInterfaceGuidToLuid(&i.InterfaceGuid, &mut luid) }.is_err() {
                continue;
            }
            let mut size = 0u32;
            let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
            // SAFETY: valid handle and GUID; `data` receives memory freed below.
            let status = unsafe {
                WlanQueryInterface(
                    handle,
                    &i.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    None,
                    &mut size,
                    &mut data,
                    None,
                )
            };
            if status != 0 || data.is_null() {
                continue;
            }
            if size as usize >= size_of::<WLAN_CONNECTION_ATTRIBUTES>() {
                // SAFETY: the API returned a full WLAN_CONNECTION_ATTRIBUTES.
                let a = unsafe {
                    &(*data.cast::<WLAN_CONNECTION_ATTRIBUTES>()).wlanAssociationAttributes
                };
                let len = (a.dot11Ssid.uSSIDLength as usize).min(a.dot11Ssid.ucSSID.len());
                out.push(WifiLink {
                    // SAFETY: plain integer view of the LUID union.
                    luid: unsafe { luid.Value },
                    ssid: String::from_utf8_lossy(&a.dot11Ssid.ucSSID[..len]).into_owned(),
                    signal: a.wlanSignalQuality.min(100),
                    standard: standard(a.dot11PhyType.0),
                });
            }
            // SAFETY: memory the API allocated for us, freed once.
            unsafe { WlanFreeMemory(data) };
        }
        // SAFETY: as above.
        unsafe { WlanFreeMemory(list.cast()) };
    }
    // SAFETY: we opened the handle above and close it once.
    unsafe { WlanCloseHandle(handle, None) };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_addresses_read_like_windows_writes_them() {
        assert_eq!(
            mac_text(&[0, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]),
            "00-1A-2B-3C-4D-5E"
        );
        assert_eq!(mac_text(&[0; 6]), "");
        assert_eq!(mac_text(&[]), "");
    }

    #[test]
    fn names_radio_standards() {
        assert_eq!(standard(10), Some("802.11ax"));
        assert_eq!(standard(11), Some("802.11be"));
        assert_eq!(standard(0), None);
    }

    #[test]
    fn lists_the_loopback_address() {
        let all = addresses().unwrap();
        assert!(all.len() <= MAX_ITEMS);
        assert!(
            all.iter().any(|a| a.ipv4.contains(&Ipv4Addr::LOCALHOST)),
            "{all:?}"
        );
        // Machines without Wi-Fi report none; that is fine.
        for link in wifi_links() {
            assert!(link.signal <= 100);
        }
    }
}
