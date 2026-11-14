//! Windows path helpers.

use std::path::Path;

/// The UNC target of a path on a mapped network drive: `Z:\Photos` → `\\nas\share\Photos`.
/// `None` for local drives and paths that are already UNC. Mappings differ per user and per
/// session, so a source stores both (design.md §2.1(a)).
pub fn unc_target(path: &Path) -> Option<String> {
    let s = path.to_str()?;
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let drive: Vec<u16> = s[..2].encode_utf16().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `drive` is NUL-terminated UTF-16; `buf` has room for `len` UTF-16 units, and the
    // call writes at most that many. This is Rust's equivalent of a JNI call into mpr.dll.
    let rc = unsafe {
        windows_sys::Win32::NetworkManagement::WNet::WNetGetConnectionW(
            drive.as_ptr(),
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if rc != 0 {
        return None; // not a network drive (or not connected)
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    let remote = String::from_utf16_lossy(&buf[..end]);
    Some(format!("{remote}{}", &s[2..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_unc_paths_have_no_target() {
        assert_eq!(unc_target(Path::new(r"C:\Windows")), None);
        assert_eq!(unc_target(Path::new(r"\\server\share\x")), None);
        assert_eq!(unc_target(Path::new("relative")), None);
    }
}
