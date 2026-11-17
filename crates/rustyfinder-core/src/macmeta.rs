//! Facts about local items that only macOS keeps: the date an item was
//! added to its folder, its Finder tags, and owner / group names.
//!
//! Everything here fails soft: an item without the attribute (or a file
//! system that has none) just gets `None` or no tags.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Mutex;

fn c_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

#[cfg(target_os = "macos")]
mod sys {
    use super::*;

    pub const ATTR_BIT_MAP_COUNT: u16 = 5;
    pub const ATTR_CMN_ADDEDTIME: u32 = 0x1000_0000;
    pub const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
    pub const FSOPT_NOFOLLOW: u32 = 0x1;
    pub const XATTR_NOFOLLOW: c_int = 0x1;

    #[repr(C)]
    pub struct AttrList {
        pub bitmapcount: u16,
        pub reserved: u16,
        pub commonattr: u32,
        pub volattr: u32,
        pub dirattr: u32,
        pub fileattr: u32,
        pub forkattr: u32,
    }

    extern "C" {
        pub fn getattrlist(
            path: *const c_char,
            list: *mut AttrList,
            buf: *mut c_void,
            size: usize,
            options: u32,
        ) -> c_int;
    }
}

/// When the item was added to its folder, in ms since 1970. macOS records
/// this for items on APFS and HFS+; the link itself is asked, not its target.
#[cfg(target_os = "macos")]
pub fn added_ms(path: &Path) -> Option<i64> {
    let c = c_path(path)?;
    let mut list = sys::AttrList {
        bitmapcount: sys::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: sys::ATTR_CMN_RETURNED_ATTRS | sys::ATTR_CMN_ADDEDTIME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    // u32 length, 5 × u32 returned-attribute bitmap, then a timespec.
    let mut buf = [0u8; 4 + 20 + 16];
    let rc =
        unsafe { sys::getattrlist(c.as_ptr(), &mut list, buf.as_mut_ptr().cast(), buf.len(), sys::FSOPT_NOFOLLOW) };
    if rc != 0 {
        return None;
    }
    let returned = u32::from_ne_bytes(buf[4..8].try_into().ok()?);
    if returned & sys::ATTR_CMN_ADDEDTIME == 0 {
        return None;
    }
    let secs = i64::from_ne_bytes(buf[24..32].try_into().ok()?);
    let nanos = i64::from_ne_bytes(buf[32..40].try_into().ok()?);
    Some(secs * 1000 + nanos / 1_000_000)
}

#[cfg(not(target_os = "macos"))]
pub fn added_ms(_path: &Path) -> Option<i64> {
    None
}

/// Finder tag names of the item itself (a link's own tags, not its
/// target's), in the order Finder keeps them.
#[cfg(target_os = "macos")]
pub fn tags(path: &Path) -> Vec<String> {
    const NAME: &[u8] = b"com.apple.metadata:_kMDItemUserTags\0";
    let Some(c) = c_path(path) else { return Vec::new() };
    let mut buf = vec![0u8; 512];
    loop {
        let n = unsafe {
            libc::getxattr(c.as_ptr(), NAME.as_ptr().cast(), buf.as_mut_ptr().cast(), buf.len(), 0, sys::XATTR_NOFOLLOW)
        };
        if n >= 0 {
            buf.truncate(n as usize);
            return parse_tags(&buf);
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::ERANGE) || buf.len() > 1 << 20 {
            return Vec::new();
        }
        buf.resize(buf.len() * 4, 0);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn tags(_path: &Path) -> Vec<String> {
    Vec::new()
}

/// Tag names from the binary property list Finder stores: an array of
/// strings, each "Name" or "Name\n<colour number>".
pub fn parse_tags(data: &[u8]) -> Vec<String> {
    bplist_strings(data)
        .unwrap_or_default()
        .into_iter()
        .map(|s| match s.rsplit_once('\n') {
            Some((name, colour)) if colour.chars().all(|c| c.is_ascii_digit()) => name.to_string(),
            _ => s,
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// The strings of a binary plist whose top object is an array of strings.
fn bplist_strings(d: &[u8]) -> Option<Vec<String>> {
    if d.len() < 40 || &d[..8] != b"bplist00" {
        return None;
    }
    let t = &d[d.len() - 32..];
    let off_size = t[6] as usize;
    let ref_size = t[7] as usize;
    let count = be(&t[8..16])?;
    let top = be(&t[16..24])?;
    let table = be(&t[24..32])?;
    let offset = |i: usize| -> Option<usize> {
        let at = table.checked_add(i.checked_mul(off_size)?)?;
        be(d.get(at..at + off_size)?)
    };
    // Marker byte plus a length that may spill into a following int object.
    let header = |at: usize| -> Option<(u8, usize, usize)> {
        let m = *d.get(at)?;
        let low = (m & 0x0f) as usize;
        if low != 0x0f {
            return Some((m >> 4, low, at + 1));
        }
        let int = *d.get(at + 1)?;
        if int >> 4 != 1 {
            return None;
        }
        let n = 1usize << (int & 0x0f);
        Some((m >> 4, be(d.get(at + 2..at + 2 + n)?)?, at + 2 + n))
    };
    if top >= count {
        return None;
    }
    let (kind, len, body) = header(offset(top)?)?;
    if kind != 0xa {
        return None;
    }
    let mut out = Vec::with_capacity(len);
    for k in 0..len {
        let r = body + k * ref_size;
        let obj = be(d.get(r..r + ref_size)?)?;
        if obj >= count {
            return None;
        }
        let (kind, n, at) = header(offset(obj)?)?;
        match kind {
            0x5 => out.push(String::from_utf8_lossy(d.get(at..at + n)?).into_owned()),
            0x6 => {
                let units: Vec<u16> =
                    d.get(at..at + 2 * n)?.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                out.push(String::from_utf16_lossy(&units));
            }
            _ => {}
        }
    }
    Some(out)
}

fn be(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || bytes.len() > 8 {
        return None;
    }
    Some(bytes.iter().fold(0u64, |acc, b| acc << 8 | *b as u64) as usize)
}

static USERS: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);
static GROUPS: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

/// Account name for a user id, or the number when there is none.
pub fn user_name(uid: u32) -> String {
    cached(&USERS, uid, |id| unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut out = std::ptr::null_mut();
        let mut buf = vec![0 as c_char; 4096];
        if libc::getpwuid_r(id, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) == 0 && !out.is_null() {
            Some(CStr::from_ptr(pw.pw_name).to_string_lossy().into_owned())
        } else {
            None
        }
    })
}

/// Group name for a group id, or the number when there is none.
pub fn group_name(gid: u32) -> String {
    cached(&GROUPS, gid, |id| unsafe {
        let mut gr: libc::group = std::mem::zeroed();
        let mut out = std::ptr::null_mut();
        let mut buf = vec![0 as c_char; 4096];
        if libc::getgrgid_r(id, &mut gr, buf.as_mut_ptr(), buf.len(), &mut out) == 0 && !out.is_null() {
            Some(CStr::from_ptr(gr.gr_name).to_string_lossy().into_owned())
        } else {
            None
        }
    })
}

fn cached(cache: &Mutex<Option<HashMap<u32, String>>>, id: u32, look_up: impl Fn(u32) -> Option<String>) -> String {
    let mut guard = cache.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.entry(id).or_insert_with(|| look_up(id).unwrap_or_else(|| id.to_string())).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `plutil -convert binary1` of ["Red\n6", "Work", "Café\n0"].
    fn sample() -> Vec<u8> {
        let mut d = b"bplist00".to_vec();
        let mut offsets = vec![];
        offsets.push(d.len());
        d.extend([0xa3, 1, 2, 3]);
        offsets.push(d.len());
        d.push(0x55);
        d.extend(b"Red\n6");
        offsets.push(d.len());
        d.push(0x54);
        d.extend(b"Work");
        offsets.push(d.len());
        let cafe: Vec<u16> = "Café\n0".encode_utf16().collect();
        d.push(0x60 | cafe.len() as u8);
        for u in cafe {
            d.extend(u.to_be_bytes());
        }
        let table = d.len();
        d.extend(offsets.iter().map(|o| *o as u8));
        d.extend([0, 0, 0, 0, 0, 0, 1, 1]);
        d.extend(4u64.to_be_bytes());
        d.extend(0u64.to_be_bytes());
        d.extend((table as u64).to_be_bytes());
        d
    }

    #[test]
    fn reads_tag_names() {
        assert_eq!(parse_tags(&sample()), vec!["Red", "Work", "Café"]);
    }

    #[test]
    fn junk_gives_no_tags() {
        assert!(parse_tags(b"not a plist at all, really not").is_empty());
        let mut cut = sample();
        cut.truncate(30);
        assert!(parse_tags(&cut).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tags_and_added_date_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.txt");
        std::fs::write(&file, "x").unwrap();
        let ok = std::process::Command::new("xattr")
            .args(["-wx", "com.apple.metadata:_kMDItemUserTags"])
            .arg(sample().iter().map(|b| format!("{b:02x}")).collect::<String>())
            .arg(&file)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        assert_eq!(tags(&file), vec!["Red", "Work", "Café"]);
        let added = added_ms(&file).expect("APFS keeps the added date");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        assert!((now - added).abs() < 60_000, "added {added} vs now {now}");
    }

    #[test]
    fn names_fall_back_to_numbers() {
        assert_eq!(user_name(0), "root");
        assert_eq!(user_name(4_000_000_000), "4000000000");
    }
}
