//! Small icons of executables, as RGBA pixels.

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, HBITMAP,
    HGDIOBJ, ReleaseDC,
};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::PCWSTR;

/// An icon image: `size` x `size` RGBA pixels, not premultiplied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    pub size: u32,
    pub rgba: Vec<u8>,
}

/// The small (16 px) icon of the executable at `path`; `None` when the file
/// has no icon or cannot be read.
pub fn small_icon(path: &str) -> Option<IconImage> {
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let mut icon = HICON::default();
    // SAFETY: `wide` is NUL-terminated; one small icon is written to `icon`.
    let count = unsafe { ExtractIconExW(PCWSTR(wide.as_ptr()), 0, None, Some(&mut icon), 1) };
    if count == 0 || icon.is_invalid() {
        return None;
    }
    let image = pixels(icon);
    // SAFETY: the icon was created for us by ExtractIconExW.
    unsafe {
        let _ = DestroyIcon(icon);
    }
    image
}

/// Reads the colour (and, for icons without alpha, the mask) bitmap.
fn pixels(icon: HICON) -> Option<IconImage> {
    let mut info = ICONINFO::default();
    // SAFETY: `icon` is valid; GetIconInfo creates two bitmaps we delete below.
    unsafe { GetIconInfo(icon, &mut info).ok()? };
    let result = read_bitmaps(info.hbmColor, info.hbmMask);
    // SAFETY: both bitmaps belong to us now.
    unsafe {
        let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
        let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
    }
    result
}

fn read_bitmaps(color: HBITMAP, mask: HBITMAP) -> Option<IconImage> {
    if color.is_invalid() {
        return None;
    }
    let size = 16u32;
    let read = |bitmap: HBITMAP| -> Option<Vec<u8>> {
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (size * size * 4) as usize];
        // SAFETY: a screen DC for the conversion, released below; `buf`
        // holds `size` rows of 32-bit pixels as described by `header`.
        unsafe {
            let dc = GetDC(None);
            let lines = GetDIBits(
                dc,
                bitmap,
                0,
                size,
                Some(buf.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, dc);
            (lines == size as i32).then_some(buf)
        }
    };
    let mut bgra = read(color)?;
    // Old icons have no alpha channel: take transparency from the mask.
    if bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
        let mask = read(mask)?;
        for (p, m) in bgra
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(mask.as_chunks::<4>().0)
        {
            p[3] = if m[0] == 0 { 255 } else { 0 };
        }
    }
    for p in bgra.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Some(IconImage { size, rgba: bgra })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_icon_of_a_system_program() {
        let icon = small_icon(r"C:\Windows\System32\notepad.exe").expect("notepad has an icon");
        assert_eq!(icon.size, 16);
        assert_eq!(icon.rgba.len(), 16 * 16 * 4);
        assert!(
            icon.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
            "not fully transparent"
        );
    }

    #[test]
    fn missing_files_and_iconless_files_have_none() {
        assert!(small_icon(r"C:\definitely\missing.exe").is_none());
        assert!(small_icon(r"C:\Windows\System32\drivers\etc\hosts").is_none());
    }
}
