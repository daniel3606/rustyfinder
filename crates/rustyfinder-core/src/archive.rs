//! Archives: open one as a read-only folder, and build new ones.
//!
//! - A plain zip is read in place: its directory at the end of the file
//!   gives the listing at once, and entries are inflated on demand.
//! - Everything else (encrypted zips, tar and its compressed forms, 7z,
//!   rar, cpio, iso, xar, lha, cab) is unpacked once into a cache folder by
//!   the system's `bsdtar` (libarchive) and shown from there.
//! - New archives are written by `bsdtar` too.
//!
//! Either way the result is a `Vfs`, so browsing, searching and copying
//! out (which is what Extract does) are the ordinary code paths. Entry
//! paths that try to leave the archive (`..`) never enter the tree.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};

use crate::entry::{Entry, EntryKind};
use crate::error::{VfsError, VfsResult};
use crate::local::LocalFs;
use crate::paths;
use crate::vfs::Vfs;

const TOOL: &str = "/usr/bin/bsdtar";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    /// Not an archive, a format nothing here reads, or a damaged file.
    Unrecognized,
    PasswordRequired,
    WrongPassword,
    Failed(String),
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveError::Unrecognized => write!(f, "Unrecognized archive format."),
            ArchiveError::PasswordRequired => write!(f, "The archive needs a password."),
            ArchiveError::WrongPassword => write!(f, "The password is not correct."),
            ArchiveError::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// An archive opened for reading.
pub struct Opened {
    pub vfs: Arc<dyn Vfs>,
    /// Entries left out because their paths point outside the archive.
    pub blocked: Vec<String>,
}

/// Extensions of archives that hold several items.
const CONTAINERS: [&str; 23] = [
    ".zip",
    ".tar",
    ".tar.gz",
    ".tgz",
    ".tar.bz2",
    ".tbz",
    ".tbz2",
    ".tar.xz",
    ".txz",
    ".tar.lzma",
    ".tar.zst",
    ".tzst",
    ".7z",
    ".rar",
    ".cpio",
    ".iso",
    ".xar",
    ".pkg",
    ".lha",
    ".lzh",
    ".cab",
    ".jar",
    ".tar.z",
];
/// One compressed file, no container.
const SINGLES: [&str; 5] = [".gz", ".bz2", ".xz", ".lzma", ".zst"];

/// Containers compressed with zstd, which the system's bsdtar cannot read.
const ZSTD_TARS: [&str; 2] = [".tar.zst", ".tzst"];

const SEVEN_ZIP_MAGIC: [u8; 6] = *b"7z\xBC\xAF\x27\x1C";

/// Does the name look like an archive RustyFinder can open as a folder?
pub fn is_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    CONTAINERS.iter().chain(SINGLES.iter()).any(|ext| lower.len() > ext.len() && lower.ends_with(ext))
}

/// The name without its archive extension: `a.tar.gz` gives `a`.
pub fn stem(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut all: Vec<&str> = CONTAINERS.iter().chain(SINGLES.iter()).copied().collect();
    // Longest first, so ".tar.gz" wins over ".gz".
    all.sort_by_key(|ext| std::cmp::Reverse(ext.len()));
    for ext in all {
        if lower.len() > ext.len() && lower.ends_with(ext) {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

/// Open the archive at local path `file`. `cache_dir` is where formats
/// that need unpacking are unpacked; the unpacked copy is deleted when the
/// returned file system is dropped.
pub fn open(file: &str, password: Option<&str>, cache_dir: &str) -> Result<Opened, ArchiveError> {
    let lower = file.to_lowercase();
    let mut magic = [0u8; 8];
    let got = File::open(file).and_then(|mut f| f.read(&mut magic)).map_err(|e| ArchiveError::Failed(e.to_string()))?;
    let looks_like_zip = got >= 4 && magic[..2] == *b"PK";
    // RAR: the RAR authors' own decoder reads every version, encrypted or not.
    if got >= 6 && magic[..6] == *b"Rar!\x1a\x07" {
        return unpack_with(cache_dir, |dir| crate::archive_formats::unpack_rar(file, password, dir));
    }
    // 7z: read here so a password can be used; anything this decoder does
    // not know goes on to bsdtar.
    if got >= 6 && magic[..6] == SEVEN_ZIP_MAGIC {
        match unpack_with(cache_dir, |dir| crate::archive_formats::unpack_7z(file, password, dir)) {
            Err(ArchiveError::Failed(_)) => {}
            done => return done,
        }
    }
    if let Some(ext) = ZSTD_TARS.iter().find(|ext| lower.ends_with(*ext)) {
        return unpack_zstd_tar(file, ext, password, cache_dir);
    }
    if looks_like_zip {
        if let Some(zip) = ZipVfs::open(file) {
            if zip.readable_in_place() {
                let blocked = zip.blocked.clone();
                return Ok(Opened { vfs: Arc::new(zip), blocked });
            }
        }
        return unpack(file, password, cache_dir);
    }
    let single = SINGLES.iter().find(|ext| lower.ends_with(*ext) && !CONTAINERS.iter().any(|c| lower.ends_with(c)));
    if let Some(ext) = single {
        return unpack_single(file, ext, cache_dir);
    }
    unpack(file, password, cache_dir)
}

// ------------------------------------------------------------------ zip

/// One file or folder from a zip's central directory.
#[derive(Debug, Clone)]
struct ZipItem {
    raw_name: Vec<u8>,
    encrypted: bool,
    method: u16,
    compressed: u64,
    size: u64,
    /// Where the item's local header starts.
    offset: u64,
    modified_ms: Option<i64>,
    mode: Option<u32>,
    is_dir: bool,
    is_symlink: bool,
}

fn u16le(b: &[u8], at: usize) -> u64 {
    u16::from_le_bytes([b[at], b[at + 1]]) as u64
}

fn u32le(b: &[u8], at: usize) -> u64 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as u64
}

fn u64le(b: &[u8], at: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(raw)
}

/// MS-DOS date and time (local time, 2-second steps) as ms since the epoch.
pub(crate) fn dos_time_ms(date: u64, time: u64) -> Option<i64> {
    if date == 0 {
        return None;
    }
    // SAFETY: `tm` is plain data, fully initialised before mktime reads it.
    let seconds = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = ((date >> 9) & 0x7f) as i32 + 80;
        tm.tm_mon = ((date >> 5) & 0x0f) as i32 - 1;
        tm.tm_mday = (date & 0x1f) as i32;
        tm.tm_hour = ((time >> 11) & 0x1f) as i32;
        tm.tm_min = ((time >> 5) & 0x3f) as i32;
        tm.tm_sec = ((time & 0x1f) * 2) as i32;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm)
    };
    (seconds >= 0).then_some(seconds * 1000)
}

/// The directory of a zip file must stay under this to be read (a zip
/// with ten million entries is about this big).
const MAX_DIRECTORY: u64 = 1 << 30;

/// Read the central directory. `None` if the file is not a usable zip.
fn read_zip_directory(file: &mut File) -> Option<Vec<ZipItem>> {
    let len = file.metadata().ok()?.len();
    // The end record is the last 22 bytes plus a comment of up to 64 KB.
    let tail_len = len.min(22 + 65535 + 20);
    file.seek(SeekFrom::Start(len - tail_len)).ok()?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail).ok()?;
    let end = (0..=tail.len().checked_sub(22)?).rev().find(|i| tail[*i..*i + 4] == *b"PK\x05\x06")?;
    let mut total = u16le(&tail, end + 10);
    let mut size = u32le(&tail, end + 12);
    let mut offset = u32le(&tail, end + 16);
    if total == 0xFFFF || size == 0xFFFF_FFFF || offset == 0xFFFF_FFFF {
        // Zip64: a locator just before the end record points at the real one.
        let locator = end.checked_sub(20)?;
        if tail[locator..locator + 4] != *b"PK\x06\x07" {
            return None;
        }
        let at = u64le(&tail, locator + 8);
        file.seek(SeekFrom::Start(at)).ok()?;
        let mut record = [0u8; 56];
        file.read_exact(&mut record).ok()?;
        if record[..4] != *b"PK\x06\x06" {
            return None;
        }
        total = u64le(&record, 32);
        size = u64le(&record, 40);
        offset = u64le(&record, 48);
    }
    if size > MAX_DIRECTORY || offset.checked_add(size)? > len {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut dir = vec![0u8; size as usize];
    file.read_exact(&mut dir).ok()?;

    let mut items = Vec::with_capacity(total.min(1 << 20) as usize);
    let mut at = 0usize;
    while at + 46 <= dir.len() && dir[at..at + 4] == *b"PK\x01\x02" {
        let made_by = u16le(&dir, at + 4);
        let flags = u16le(&dir, at + 8);
        let method = u16le(&dir, at + 10) as u16;
        let (time, date) = (u16le(&dir, at + 12), u16le(&dir, at + 14));
        let mut compressed = u32le(&dir, at + 20);
        let mut size = u32le(&dir, at + 24);
        let (name_len, extra_len, comment_len) =
            (u16le(&dir, at + 28) as usize, u16le(&dir, at + 30) as usize, u16le(&dir, at + 32) as usize);
        let external = u32le(&dir, at + 38);
        let mut offset = u32le(&dir, at + 42);
        let name_at = at + 46;
        let extra_at = name_at + name_len;
        let next = extra_at + extra_len + comment_len;
        if next > dir.len() {
            return None;
        }
        let raw_name = dir[name_at..extra_at].to_vec();
        let mut modified_ms = dos_time_ms(date, time);

        // Extra fields: 64-bit sizes, and an exact modification time.
        let extra = &dir[extra_at..extra_at + extra_len];
        let mut e = 0usize;
        while e + 4 <= extra.len() {
            let (id, field_len) = (u16le(extra, e), u16le(extra, e + 2) as usize);
            let data = extra.get(e + 4..e + 4 + field_len)?;
            match id {
                0x0001 => {
                    // Present only for the fields that overflowed, in order.
                    let mut d = 0usize;
                    for value in [&mut size, &mut compressed, &mut offset] {
                        if *value == 0xFFFF_FFFF && d + 8 <= data.len() {
                            *value = u64le(data, d);
                            d += 8;
                        }
                    }
                }
                0x5455 if data.len() >= 5 && data[0] & 1 == 1 => {
                    modified_ms = Some(u32le(data, 1) as i64 * 1000);
                }
                _ => {}
            }
            e += 4 + field_len;
        }

        // Unix permissions and file type ride in the high half of the
        // external attributes when the archive was made on Unix.
        let unix = (made_by >> 8) == 3;
        let st_mode = (external >> 16) as u32;
        let is_symlink = unix && st_mode & 0o170000 == 0o120000;
        let is_dir = raw_name.ends_with(b"/") || (unix && st_mode & 0o170000 == 0o040000);
        items.push(ZipItem {
            raw_name,
            encrypted: flags & 1 == 1,
            method,
            compressed,
            size,
            offset,
            modified_ms,
            mode: (unix && st_mode != 0).then_some(st_mode & 0o7777),
            is_dir,
            is_symlink,
        });
        at = next;
    }
    Some(items)
}

/// Code page 437, the encoding zip names have when nothing says otherwise.
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

fn cp437(bytes: &[u8]) -> String {
    let high: Vec<char> = CP437_HIGH.chars().collect();
    bytes.iter().map(|b| if *b < 0x80 { *b as char } else { high[(*b - 0x80) as usize] }).collect()
}

/// Entry names as text. Names that are valid UTF-8 are taken as such
/// (flagged or not). For the rest the encoding is guessed from all of them
/// together: Japanese, Cyrillic and so on, else code page 437.
fn decode_names(raw: &[&[u8]]) -> Vec<String> {
    let legacy: Vec<&[u8]> = raw.iter().copied().filter(|n| std::str::from_utf8(n).is_err()).collect();
    let encoding = (!legacy.is_empty()).then(|| {
        let mut detector = EncodingDetector::new(Iso2022JpDetection::Allow);
        for name in &legacy {
            detector.feed(name, false);
            detector.feed(b"\n", false);
        }
        detector.feed(b"", true);
        detector.guess(None, Utf8Detection::Deny)
    });
    raw.iter()
        .map(|name| match (std::str::from_utf8(name), encoding) {
            (Ok(text), _) => text.to_string(),
            // The detector's answer for "some Western text" is Windows-1252,
            // which zip tools never wrote: those names are CP437.
            (Err(_), Some(e)) if e != encoding_rs::WINDOWS_1252 => e.decode(name).0.into_owned(),
            (Err(_), _) => cp437(name),
        })
        .collect()
}

/// Split an entry path into safe components. `None` if it tries to climb
/// out with `..`. A leading `/` is simply dropped.
pub(crate) fn safe_components(path: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains('\0') => return None,
            p => parts.push(p),
        }
    }
    Some(parts)
}

/// A zip read in place.
struct ZipVfs {
    file: String,
    items: Vec<ZipItem>,
    /// Folder path ("" for the top) to the entries in it.
    dirs: HashMap<String, Vec<Entry>>,
    /// File path to its index in `items`.
    files: HashMap<String, usize>,
    blocked: Vec<String>,
}

impl ZipVfs {
    fn open(file: &str) -> Option<ZipVfs> {
        let mut handle = File::open(file).ok()?;
        let items = read_zip_directory(&mut handle)?;
        let raw: Vec<&[u8]> = items.iter().map(|i| i.raw_name.as_slice()).collect();
        let names = decode_names(&raw);

        let mut vfs = ZipVfs {
            file: file.to_string(),
            items: Vec::new(),
            dirs: HashMap::from([(String::new(), Vec::new())]),
            files: HashMap::new(),
            blocked: Vec::new(),
        };
        // Path to entry; a later entry with the same path replaces an earlier one.
        let mut entries: HashMap<String, Entry> = HashMap::new();
        for (index, (item, name)) in items.iter().zip(&names).enumerate() {
            let Some(parts) = safe_components(name) else {
                vfs.blocked.push(name.clone());
                continue;
            };
            let Some(last) = parts.last() else { continue };
            let path = parts.join("/");
            let kind = if item.is_dir {
                EntryKind::Directory
            } else if item.is_symlink {
                EntryKind::Symlink
            } else {
                EntryKind::File
            };
            let entry = Entry {
                name: last.to_string(),
                kind,
                is_dir: item.is_dir,
                size: if item.is_dir { 0 } else { item.size },
                modified_ms: item.modified_ms,
                created_ms: None,
                is_hidden: last.starts_with('.'),
                mode: item.mode,
                link_target: None,
                is_broken_link: false,
                ext: Default::default(),
            };
            if !item.is_dir {
                vfs.files.insert(path.clone(), index);
            }
            entries.insert(path, entry);
        }
        vfs.items = items;

        // Symlinks keep their target as the entry's content.
        let links: Vec<String> =
            entries.iter().filter(|(_, e)| e.kind == EntryKind::Symlink).map(|(p, _)| p.clone()).collect();
        for path in links {
            let mut target = String::new();
            let read = vfs.reader(&path).and_then(|r| r.take(4096).read_to_string(&mut target).ok());
            if let (Some(_), Some(entry)) = (read, entries.get_mut(&path)) {
                entry.link_target = Some(target);
            }
        }

        // Folders that are implied by a path but have no entry of their own.
        let mut paths_sorted: Vec<String> = entries.keys().cloned().collect();
        paths_sorted.sort();
        for path in paths_sorted {
            let mut parent = String::new();
            for part in path.split('/') {
                let here = if parent.is_empty() { part.to_string() } else { format!("{parent}/{part}") };
                let entry = entries.get(&here).cloned().unwrap_or_else(|| Entry {
                    name: part.to_string(),
                    kind: EntryKind::Directory,
                    is_dir: true,
                    size: 0,
                    modified_ms: None,
                    created_ms: None,
                    is_hidden: part.starts_with('.'),
                    mode: None,
                    link_target: None,
                    is_broken_link: false,
                    ext: Default::default(),
                });
                let siblings = vfs.dirs.entry(parent.clone()).or_default();
                if !siblings.iter().any(|e| e.name == entry.name) {
                    if entry.is_dir {
                        vfs.dirs.entry(here.clone()).or_default();
                    }
                    vfs.dirs.entry(parent.clone()).or_default().push(entry);
                }
                parent = here;
            }
        }
        Some(vfs)
    }

    /// True if every entry is stored or deflated and none is encrypted.
    fn readable_in_place(&self) -> bool {
        self.items.iter().all(|i| !i.encrypted && (i.is_dir || matches!(i.method, 0 | 8)))
    }

    /// A reader over one entry's uncompressed bytes.
    fn reader(&self, path: &str) -> Option<Box<dyn Read + Send>> {
        let item = &self.items[*self.files.get(path)?];
        let mut file = File::open(&self.file).ok()?;
        file.seek(SeekFrom::Start(item.offset)).ok()?;
        let mut header = [0u8; 30];
        file.read_exact(&mut header).ok()?;
        if header[..4] != *b"PK\x03\x04" {
            return None;
        }
        let skip = u16le(&header, 26) + u16le(&header, 28);
        file.seek(SeekFrom::Current(skip as i64)).ok()?;
        let data = file.take(item.compressed);
        match item.method {
            0 => Some(Box::new(data)),
            8 => Some(Box::new(flate2::read::DeflateDecoder::new(BufReader::new(data)))),
            _ => None,
        }
    }

    fn key(path: &str) -> String {
        path.trim_matches('/').to_string()
    }
}

/// The top folder of an archive, as an entry.
fn root_entry() -> Entry {
    Entry {
        name: String::new(),
        kind: EntryKind::Directory,
        is_dir: true,
        size: 0,
        modified_ms: None,
        created_ms: None,
        is_hidden: false,
        mode: None,
        link_target: None,
        is_broken_link: false,
        ext: Default::default(),
    }
}

impl Vfs for ZipVfs {
    fn id(&self) -> String {
        format!("archive:{}", self.file)
    }

    fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
        self.dirs.get(&Self::key(path)).cloned().ok_or_else(|| VfsError::NotFound(path.to_string()))
    }

    fn stat(&self, path: &str) -> VfsResult<Entry> {
        let key = Self::key(path);
        if key.is_empty() {
            return Ok(root_entry());
        }
        let (parent, name) = match key.rfind('/') {
            Some(i) => (&key[..i], &key[i + 1..]),
            None => ("", key.as_str()),
        };
        self.dirs
            .get(parent)
            .and_then(|entries| entries.iter().find(|e| e.name == name))
            .cloned()
            .ok_or_else(|| VfsError::NotFound(path.to_string()))
    }

    fn open_read(&self, path: &str) -> VfsResult<Box<dyn Read + Send>> {
        self.reader(&Self::key(path)).ok_or_else(|| VfsError::Io {
            path: path.to_string(),
            message: "The item can’t be read from the archive.".into(),
        })
    }

    fn read_only(&self, _path: &str) -> bool {
        true
    }
}

// ------------------------------------------------------------- unpacked

/// An archive unpacked into a cache folder, shown read-only. The folder is
/// deleted when this is dropped.
struct UnpackedVfs {
    root: String,
    local: LocalFs,
}

impl UnpackedVfs {
    fn real(&self, path: &str) -> VfsResult<String> {
        if path.split('/').any(|part| part == "..") {
            return Err(VfsError::NotFound(path.to_string()));
        }
        let rel = path.trim_matches('/');
        Ok(if rel.is_empty() { self.root.clone() } else { format!("{}/{rel}", self.root) })
    }
}

impl Drop for UnpackedVfs {
    fn drop(&mut self) {
        // Only ever a folder this module made.
        if paths::file_name(&self.root).starts_with(CACHE_PREFIX) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

impl Vfs for UnpackedVfs {
    fn id(&self) -> String {
        format!("archive:{}", self.root)
    }

    fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
        self.local.list(&self.real(path)?)
    }

    fn stat(&self, path: &str) -> VfsResult<Entry> {
        if path.trim_matches('/').is_empty() {
            return Ok(root_entry());
        }
        self.local.stat(&self.real(path)?)
    }

    fn open_read(&self, path: &str) -> VfsResult<Box<dyn Read + Send>> {
        self.local.open_read(&self.real(path)?)
    }

    fn read_only(&self, _path: &str) -> bool {
        true
    }
}

const CACHE_PREFIX: &str = "rustyfinder-archive-";

/// A fresh, empty folder under `cache_dir`.
fn new_cache(cache_dir: &str) -> Result<String, ArchiveError> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = format!("{}/{CACHE_PREFIX}{}-{n}", cache_dir.trim_end_matches('/'), std::process::id());
    std::fs::create_dir_all(&dir).map_err(|e| ArchiveError::Failed(e.to_string()))?;
    Ok(dir)
}

/// Unpack with bsdtar. It refuses `..`, strips leading slashes and never
/// writes through a symlink; what it refuses is reported on stderr.
fn unpack(file: &str, password: Option<&str>, cache_dir: &str) -> Result<Opened, ArchiveError> {
    let dir = new_cache(cache_dir)?;
    // Without a passphrase argument bsdtar would ask on the terminal. A
    // stand-in that cannot be right makes an encrypted archive fail cleanly.
    let passphrase = password.filter(|p| !p.is_empty()).unwrap_or("\u{1}no password given\u{1}");
    let output = Command::new(TOOL)
        .args(["-x", "-f", file, "-C", &dir, "--passphrase", passphrase])
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output();
    let fail = |error: ArchiveError| {
        let _ = std::fs::remove_dir_all(&dir);
        Err(error)
    };
    let output = match output {
        Ok(output) => output,
        Err(e) => return fail(ArchiveError::Failed(format!("The archive tool could not be started: {e}"))),
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.to_lowercase().contains("passphrase") {
        return fail(if password.is_some_and(|p| !p.is_empty()) {
            ArchiveError::WrongPassword
        } else {
            ArchiveError::PasswordRequired
        });
    }
    if stderr.contains("Unrecognized archive format") {
        return fail(ArchiveError::Unrecognized);
    }
    let mut blocked = Vec::new();
    let mut other = Vec::new();
    for line in stderr.lines() {
        if line.contains("Path contains '..'") || line.contains("Cannot extract through symlink") {
            blocked.push(line.split(": ").next().unwrap_or(line).to_string());
        } else if !line.starts_with("bsdtar: Removing leading") && !line.contains("Error exit delayed") {
            other.push(line.to_string());
        }
    }
    let empty = std::fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(true);
    if !output.status.success() && empty && blocked.is_empty() {
        return fail(match other.last() {
            Some(line) => ArchiveError::Failed(line.trim_start_matches("bsdtar: ").to_string()),
            None => ArchiveError::Unrecognized,
        });
    }
    Ok(Opened { vfs: Arc::new(UnpackedVfs { root: dir, local: LocalFs }), blocked })
}

/// A lone compressed file (`notes.txt.gz`): the archive holds one item,
/// named without the extension.
/// Unpack into a fresh cache folder with `unpack`, which returns the
/// entries it refused. The folder is removed again if that fails.
fn unpack_with(
    cache_dir: &str,
    unpack: impl FnOnce(&std::path::Path) -> Result<Vec<String>, ArchiveError>,
) -> Result<Opened, ArchiveError> {
    let dir = new_cache(cache_dir)?;
    match unpack(std::path::Path::new(&dir)) {
        Ok(blocked) => Ok(Opened { vfs: Arc::new(UnpackedVfs { root: dir, local: LocalFs }), blocked }),
        Err(error) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(error)
        }
    }
}

/// A tar compressed with zstd: decompressed to a temporary tar here, then
/// unpacked by bsdtar like any tar.
fn unpack_zstd_tar(file: &str, ext: &str, password: Option<&str>, cache_dir: &str) -> Result<Opened, ArchiveError> {
    let scratch = new_cache(cache_dir)?;
    let tar = format!("{scratch}/archive.tar");
    let decoded = File::create(&tar).and_then(|mut out| {
        let ext = if *ext == *".tzst" { ".tzst" } else { ".zst" };
        crate::archive_formats::decompress_single(file, ext, &mut out)
    });
    let result = match decoded {
        Ok(()) => unpack(&tar, password, cache_dir),
        Err(_) => Err(ArchiveError::Unrecognized),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

fn unpack_single(file: &str, ext: &str, cache_dir: &str) -> Result<Opened, ArchiveError> {
    let dir = new_cache(cache_dir)?;
    let name = paths::file_name(file);
    let inner = &name[..name.len() - ext.len()];
    let target = format!("{dir}/{}", if inner.is_empty() { "data" } else { inner });
    let result = (|| -> std::io::Result<()> {
        let mut out = File::create(&target)?;
        if ext == ".gz" {
            let mut decoder = flate2::read::MultiGzDecoder::new(BufReader::new(File::open(file)?));
            std::io::copy(&mut decoder, &mut out)?;
        } else if ext != ".bz2" {
            crate::archive_formats::decompress_single(file, ext, &mut out)?;
        } else {
            let mut child = Command::new("/usr/bin/bzip2")
                .args(["-d", "-c", file])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;
            if let Some(mut stdout) = child.stdout.take() {
                std::io::copy(&mut stdout, &mut out)?;
            }
            if !child.wait()?.success() {
                return Err(std::io::Error::other("not a bzip2 file"));
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(ArchiveError::Unrecognized);
    }
    Ok(Opened { vfs: Arc::new(UnpackedVfs { root: dir, local: LocalFs }), blocked: Vec::new() })
}

/// What Extract copies out of an opened archive: the paths to copy, and
/// the name to give a single source. One item at the top is extracted as
/// itself; several go into a folder named after the archive.
pub fn extract_plan(vfs: &dyn Vfs, archive_name: &str) -> VfsResult<(Vec<String>, Option<String>)> {
    let top = vfs.list("/")?;
    Ok(match top.as_slice() {
        [only] => (vec![format!("/{}", only.name)], None),
        _ => (vec!["/".to_string()], Some(stem(archive_name))),
    })
}

// ------------------------------------------------------------- compress

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Zip,
    TarGz,
    TarBz2,
    TarXz,
    SevenZip,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::TarGz => "tar.gz",
            Format::TarBz2 => "tar.bz2",
            Format::TarXz => "tar.xz",
            Format::SevenZip => "7z",
        }
    }

    /// Zip (AES-256) and 7z archives can be given a password.
    pub fn takes_password(self) -> bool {
        matches!(self, Format::Zip | Format::SevenZip)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    Store,
    Fast,
    #[default]
    Normal,
    Best,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressOptions {
    pub format: Format,
    pub level: Level,
    pub password: Option<String>,
    /// Leave out `.DS_Store` and the `._` companions that carry Finder
    /// metadata.
    pub exclude_metadata: bool,
}

impl Default for CompressOptions {
    fn default() -> Self {
        Self { format: Format::Zip, level: Level::Normal, password: None, exclude_metadata: true }
    }
}

/// Name for a new archive, without extension: one item gives its own name
/// (a file's extension dropped, so `scan 2.pdf` gives `scan 2`); several
/// give "Archive".
pub fn default_name(items: &[(String, bool)]) -> String {
    match items {
        [(name, is_dir)] => {
            if *is_dir {
                return name.clone();
            }
            match name.rfind('.') {
                Some(i) if i > 0 => name[..i].to_string(),
                _ => name.clone(),
            }
        }
        _ => "Archive".to_string(),
    }
}

/// `base.ext`, or `base 2.ext`, `base 3.ext`… if that name is taken in `dir`.
pub fn free_name(dir: &str, base: &str, ext: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let first = format!("{base}.{ext}");
    if !taken(&paths::join(dir, &first)) {
        return first;
    }
    (2u32..)
        .map(|n| format!("{base} {n}.{ext}"))
        .find(|name| !taken(&paths::join(dir, name)))
        .expect("an unused name exists")
}

/// Build an archive at local path `target` from local `sources`. Returns
/// `Ok(false)` if stopped (nothing is left behind). `progress` gets the
/// number of items written so far and the one being written.
pub fn compress(
    sources: &[String],
    target: &str,
    options: &CompressOptions,
    keep_going: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, &str),
) -> Result<bool, String> {
    let partial = format!("{target}.rustyfinder-partial");
    // bsdtar cannot encrypt a 7z archive; the 7z writer here can.
    if let (Format::SevenZip, Some(password)) = (options.format, options.password.as_deref().filter(|p| !p.is_empty()))
    {
        let done = crate::archive_formats::compress_7z(sources, &partial, options, password, keep_going, progress);
        return match done {
            Ok(true) => std::fs::rename(&partial, target).map(|_| true).map_err(|e| e.to_string()),
            other => {
                let _ = std::fs::remove_file(&partial);
                other
            }
        };
    }
    let mut command = Command::new(TOOL);
    command.args(["-c", "-v", "-f", &partial]);
    let mut settings: Vec<String> = Vec::new();
    let prefix = match options.format {
        Format::Zip => {
            command.args(["--format", "zip"]);
            "zip:"
        }
        Format::TarGz => {
            command.arg("-z");
            ""
        }
        Format::TarBz2 => {
            command.arg("-j");
            ""
        }
        Format::TarXz => {
            command.arg("-J");
            ""
        }
        Format::SevenZip => {
            command.args(["--format", "7zip"]);
            "7zip:"
        }
    };
    match (options.level, options.format) {
        (Level::Normal, _) => {}
        (Level::Store, Format::Zip) => settings.push("zip:compression=store".into()),
        (Level::Store, Format::SevenZip) => settings.push("7zip:compression=store".into()),
        (Level::Store | Level::Fast, _) => settings.push(format!("{prefix}compression-level=1")),
        (Level::Best, _) => settings.push(format!("{prefix}compression-level=9")),
    }
    let password = options.password.as_deref().filter(|p| !p.is_empty() && options.format.takes_password());
    if let Some(password) = password {
        settings.push("zip:encryption=aes256".into());
        command.args(["--passphrase", password]);
    }
    if !settings.is_empty() {
        command.args(["--options", &settings.join(",")]);
    }
    if options.exclude_metadata {
        command.args(["--no-mac-metadata", "--exclude", ".DS_Store"]);
    }
    for source in sources {
        let dir = paths::parent(source).unwrap_or_else(|| "/".to_string());
        let name = paths::file_name(source);
        // A leading "-" would read as an option and "@" as "include this
        // archive's entries": "./" keeps both as plain names.
        let safe = if name.starts_with(['-', '@']) { format!("./{name}") } else { name.to_string() };
        command.args(["-C", &dir, &safe]);
    }
    // Names are stored as UTF-8 (and flagged so in a zip).
    command.env("LC_ALL", "en_US.UTF-8").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("The archive tool could not be started: {e}"))?;

    // "-v" names each item on stderr as it goes in.
    let (tx, rx) = mpsc::channel::<String>();
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        }
    });
    let mut written = 0u64;
    let mut errors: Vec<String> = Vec::new();
    let stopped = loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(line) => match line.strip_prefix("a ") {
                Some(name) => {
                    written += 1;
                    progress(written, name);
                }
                None => errors.push(line),
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
        }
        if !keep_going() {
            let _ = child.kill();
            break true;
        }
    };
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = reader.join();
    if stopped {
        let _ = std::fs::remove_file(&partial);
        return Ok(false);
    }
    if !status.success() {
        let _ = std::fs::remove_file(&partial);
        let reason = errors.last().map(|l| l.trim_start_matches("bsdtar: ").to_string());
        return Err(reason.unwrap_or_else(|| "The archive could not be created.".into()));
    }
    std::fs::rename(&partial, target).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn tool(args: &[&str], cwd: &std::path::Path) {
        let (program, rest) = args.split_first().unwrap();
        let status = Command::new(program).args(rest).current_dir(cwd).stderr(Stdio::null()).status().unwrap();
        assert!(status.success(), "{args:?} failed");
    }

    /// A folder with `src/docs/readme.txt`, `src/a b.txt`, a symlink and a
    /// `.DS_Store`.
    fn sources() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(src.join("docs")).unwrap();
        fs::write(src.join("docs/readme.txt"), "read me").unwrap();
        fs::write(src.join("a b.txt"), "top").unwrap();
        fs::write(src.join(".DS_Store"), "junk").unwrap();
        std::os::unix::fs::symlink("docs/readme.txt", src.join("link")).unwrap();
        fs::create_dir(dir.path().join("cache")).unwrap();
        dir
    }

    fn path(dir: &tempfile::TempDir, rel: &str) -> String {
        dir.path().join(rel).to_str().unwrap().to_string()
    }

    fn names(vfs: &dyn Vfs, dir: &str) -> Vec<String> {
        let mut names: Vec<String> = vfs.list(dir).unwrap().into_iter().map(|e| e.name).collect();
        names.sort();
        names
    }

    fn text(vfs: &dyn Vfs, file: &str) -> String {
        let mut out = String::new();
        vfs.open_read(file).unwrap().read_to_string(&mut out).unwrap();
        out
    }

    /// Write a zip of stored (uncompressed) entries with exactly these
    /// name bytes, as a tool on another platform would.
    fn stored_zip(target: &str, entries: &[(&[u8], &[u8])]) {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let offset = out.len() as u32;
            let mut header = Vec::new();
            header.extend_from_slice(&[20, 0, 0, 0, 0, 0]); // version, flags, method (stored)
            header.extend_from_slice(&[0, 0, 0x21, 0x58]); // time, date (2024-01-01)
            header.extend_from_slice(&0u32.to_le_bytes()); // crc (not checked here)
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&header);
            out.extend_from_slice(name);
            out.extend_from_slice(data);
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&[20, 0]); // made by: MS-DOS
            central.extend_from_slice(&header);
            central.extend_from_slice(&[0; 10]); // comment len, disk, attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name);
        }
        let start = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        fs::File::create(target).unwrap().write_all(&out).unwrap();
    }

    #[test]
    fn names_and_stems() {
        assert!(is_archive("a.zip") && is_archive("B.TAR.GZ") && is_archive("x.7z") && is_archive("notes.txt.gz"));
        assert!(!is_archive("a.txt") && !is_archive(".zip") && !is_archive("zip"));
        assert_eq!(stem("a.tar.gz"), "a");
        assert_eq!(stem("Backup 2024.ZIP"), "Backup 2024");
        assert_eq!(stem("notes.txt.gz"), "notes.txt");
    }

    #[test]
    fn a_zip_is_browsed_in_place() {
        // AR-01, AR-02: folders, contents and the archived dates.
        let dir = sources();
        tool(&["/usr/bin/zip", "-q", "-r", "-y", "../a.zip", "docs", "a b.txt", "link"], &dir.path().join("src"));
        let opened = open(&path(&dir, "a.zip"), None, &path(&dir, "cache")).unwrap();
        let vfs = &*opened.vfs;
        assert_eq!(names(vfs, "/"), ["a b.txt", "docs", "link"]);
        assert_eq!(names(vfs, "/docs"), ["readme.txt"]);
        assert_eq!(text(vfs, "/docs/readme.txt"), "read me");
        assert_eq!(text(vfs, "/a b.txt"), "top");
        let entry = vfs.stat("/docs/readme.txt").unwrap();
        assert_eq!(entry.size, 7);
        let on_disk = fs::metadata(dir.path().join("src/docs/readme.txt")).unwrap().modified().unwrap();
        let on_disk_ms = on_disk.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        assert!((entry.modified_ms.unwrap() - on_disk_ms).abs() < 2000, "date kept");
        assert_eq!(entry.mode.unwrap() & 0o777, 0o644);
        let link = vfs.stat("/link").unwrap();
        assert_eq!(link.kind, EntryKind::Symlink);
        assert_eq!(link.link_target.as_deref(), Some("docs/readme.txt"));
        assert!(vfs.stat("/docs").unwrap().is_dir && vfs.stat("/").unwrap().is_dir);
        assert!(vfs.list("/nope").is_err() && vfs.read_only("/"));
        // Nothing was unpacked for a plain zip.
        assert_eq!(fs::read_dir(dir.path().join("cache")).unwrap().count(), 0);
    }

    #[test]
    fn folders_without_their_own_entry_still_appear() {
        let dir = sources();
        stored_zip(&path(&dir, "bare.zip"), &[(b"one/two/deep.txt", b"deep"), (b"top.txt", b"t")]);
        let opened = open(&path(&dir, "bare.zip"), None, &path(&dir, "cache")).unwrap();
        assert_eq!(names(&*opened.vfs, "/"), ["one", "top.txt"]);
        assert_eq!(names(&*opened.vfs, "/one"), ["two"]);
        assert_eq!(text(&*opened.vfs, "/one/two/deep.txt"), "deep");
    }

    #[test]
    fn entries_that_climb_out_are_left_out_and_reported() {
        // AR-04, zip read in place.
        let dir = sources();
        stored_zip(&path(&dir, "evil.zip"), &[(b"ok.txt", b"fine"), (b"../evil.txt", b"bad"), (b"/abs.txt", b"abs")]);
        let opened = open(&path(&dir, "evil.zip"), None, &path(&dir, "cache")).unwrap();
        assert_eq!(opened.blocked, ["../evil.txt"]);
        // A leading slash is dropped: the item stays inside.
        assert_eq!(names(&*opened.vfs, "/"), ["abs.txt", "ok.txt"]);
    }

    #[test]
    fn an_unpacked_archive_cannot_write_outside_its_cache() {
        // AR-04, tar: bsdtar refuses, and says which entries.
        let dir = sources();
        let script = "import tarfile, io\nt = tarfile.open('evil.tar', 'w')\nfor n, d in [('ok.txt', b'fine'), ('../evil.txt', b'bad')]:\n    i = tarfile.TarInfo(n); i.size = len(d); t.addfile(i, io.BytesIO(d))\nt.close()\n";
        tool(&["/usr/bin/python3", "-c", script], dir.path());
        let opened = open(&path(&dir, "evil.tar"), None, &path(&dir, "cache")).unwrap();
        assert_eq!(opened.blocked, ["../evil.txt"]);
        assert_eq!(names(&*opened.vfs, "/"), ["ok.txt"]);
        assert!(!dir.path().join("evil.txt").exists());
        assert!(opened.vfs.list("/../..").is_err());
    }

    #[test]
    fn legacy_japanese_names_are_readable() {
        // AR-09: CP932 bytes, no UTF-8 flag.
        let dir = sources();
        let (first, _, _) = encoding_rs::SHIFT_JIS.encode("日本語のファイル.txt");
        let (second, _, _) = encoding_rs::SHIFT_JIS.encode("資料/議事録.txt");
        stored_zip(&path(&dir, "jp.zip"), &[(&first, b"a"), (&second, b"b")]);
        let opened = open(&path(&dir, "jp.zip"), None, &path(&dir, "cache")).unwrap();
        assert_eq!(names(&*opened.vfs, "/"), ["日本語のファイル.txt", "資料"]);
        assert_eq!(text(&*opened.vfs, "/資料/議事録.txt"), "b");
    }

    #[test]
    fn western_names_without_a_flag_are_cp437() {
        let dir = sources();
        stored_zip(&path(&dir, "dos.zip"), &[(b"caf\x82.txt", b"x")]);
        let opened = open(&path(&dir, "dos.zip"), None, &path(&dir, "cache")).unwrap();
        assert_eq!(names(&*opened.vfs, "/"), ["café.txt"]);
    }

    #[test]
    fn an_encrypted_zip_asks_for_its_password() {
        // AR-08
        let dir = sources();
        tool(&["/usr/bin/zip", "-q", "-r", "-P", "pw", "../enc.zip", "docs"], &dir.path().join("src"));
        let (file, cache) = (path(&dir, "enc.zip"), path(&dir, "cache"));
        assert_eq!(open(&file, None, &cache).err(), Some(ArchiveError::PasswordRequired));
        assert_eq!(open(&file, Some("nope"), &cache).err(), Some(ArchiveError::WrongPassword));
        // A failed attempt leaves nothing behind.
        assert_eq!(fs::read_dir(dir.path().join("cache")).unwrap().count(), 0);
        let opened = open(&file, Some("pw"), &cache).unwrap();
        assert_eq!(text(&*opened.vfs, "/docs/readme.txt"), "read me");
    }

    #[test]
    fn tar_gz_and_7z_are_unpacked_once_and_cleaned_up_afterwards() {
        let dir = sources();
        tool(&[TOOL, "-czf", "../t.tar.gz", "docs", "a b.txt", "link"], &dir.path().join("src"));
        tool(&[TOOL, "-cf", "../t.7z", "--format", "7zip", "docs", "a b.txt"], &dir.path().join("src"));
        let cache = path(&dir, "cache");
        {
            let opened = open(&path(&dir, "t.tar.gz"), None, &cache).unwrap();
            assert_eq!(names(&*opened.vfs, "/"), ["a b.txt", "docs", "link"]);
            assert_eq!(text(&*opened.vfs, "/docs/readme.txt"), "read me");
            assert_eq!(opened.vfs.stat("/link").unwrap().link_target.as_deref(), Some("docs/readme.txt"));
            let seven = open(&path(&dir, "t.7z"), None, &cache).unwrap();
            assert_eq!(names(&*seven.vfs, "/docs"), ["readme.txt"]);
            assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);
        }
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0, "caches are removed when closed");
    }

    #[test]
    fn a_lone_gz_holds_one_file() {
        let dir = sources();
        tool(&["/bin/sh", "-c", "gzip -c 'src/a b.txt' > notes.txt.gz"], dir.path());
        let opened = open(&path(&dir, "notes.txt.gz"), None, &path(&dir, "cache")).unwrap();
        assert_eq!(names(&*opened.vfs, "/"), ["notes.txt"]);
        assert_eq!(text(&*opened.vfs, "/notes.txt"), "top");
    }

    #[test]
    fn junk_is_not_an_archive() {
        let dir = sources();
        for name in ["junk.7z", "junk.zip", "junk.gz"] {
            fs::write(dir.path().join(name), "not an archive at all").unwrap();
            assert_eq!(open(&path(&dir, name), None, &path(&dir, "cache")).err(), Some(ArchiveError::Unrecognized));
        }
        assert_eq!(fs::read_dir(dir.path().join("cache")).unwrap().count(), 0);
    }

    #[test]
    fn extract_plan_follows_the_top_level() {
        // AR-03: several items go into a folder named after the archive.
        let dir = sources();
        stored_zip(&path(&dir, "many.zip"), &[(b"1.txt", b"1"), (b"2.txt", b"2"), (b"3.txt", b"3")]);
        stored_zip(&path(&dir, "one.zip"), &[(b"only/inner.txt", b"1")]);
        let cache = path(&dir, "cache");
        let many = open(&path(&dir, "many.zip"), None, &cache).unwrap();
        assert_eq!(extract_plan(&*many.vfs, "many.zip").unwrap(), (vec!["/".to_string()], Some("many".to_string())));
        let one = open(&path(&dir, "one.zip"), None, &cache).unwrap();
        assert_eq!(extract_plan(&*one.vfs, "one.zip").unwrap(), (vec!["/only".to_string()], None));
    }

    #[test]
    fn default_names() {
        // AR-05, AR-07
        let item = |name: &str, dir: bool| (name.to_string(), dir);
        assert_eq!(default_name(&[item("report", true)]), "report");
        assert_eq!(default_name(&[item("notes.txt", false)]), "notes");
        assert_eq!(default_name(&[item("scan 2.pdf", false)]), "scan 2");
        assert_eq!(default_name(&[item("v1.2", true)]), "v1.2");
        assert_eq!(default_name(&[item(".zshrc", false)]), ".zshrc");
        assert_eq!(default_name(&[item("a", false), item("b", false)]), "Archive");
        let taken = |path: &str| path == "/d/Archive.zip" || path == "/d/Archive 2.zip";
        assert_eq!(free_name("/d", "Archive", "zip", &taken), "Archive 3.zip");
        assert_eq!(free_name("/d", "scan 2", "zip", &taken), "scan 2.zip");
    }

    fn pack(dir: &tempfile::TempDir, target: &str, options: &CompressOptions) {
        let sources = [path(dir, "src/docs"), path(dir, "src/a b.txt"), path(dir, "src/.DS_Store")];
        let mut seen = Vec::new();
        let done =
            compress(&sources, &path(dir, target), options, &|| true, &mut |n, name| seen.push((n, name.to_string())));
        assert_eq!(done, Ok(true));
        assert!(seen.iter().any(|(_, name)| name == "docs/readme.txt"), "progress names items: {seen:?}");
        assert!(!dir.path().join(format!("{target}.rustyfinder-partial")).exists());
    }

    #[test]
    fn compress_makes_a_zip_that_reads_back() {
        // AR-05
        let dir = sources();
        pack(&dir, "out.zip", &CompressOptions::default());
        let opened = open(&path(&dir, "out.zip"), None, &path(&dir, "cache")).unwrap();
        // Finder metadata is left out by default.
        assert_eq!(names(&*opened.vfs, "/"), ["a b.txt", "docs"]);
        assert_eq!(text(&*opened.vfs, "/docs/readme.txt"), "read me");

        let keep = CompressOptions { exclude_metadata: false, ..CompressOptions::default() };
        pack(&dir, "all.zip", &keep);
        let opened = open(&path(&dir, "all.zip"), None, &path(&dir, "cache")).unwrap();
        assert!(names(&*opened.vfs, "/").contains(&".DS_Store".to_string()));
    }

    #[test]
    fn compress_with_a_password_needs_it_to_read_back() {
        // AR-06
        let dir = sources();
        let options = CompressOptions { password: Some("pw".into()), ..CompressOptions::default() };
        pack(&dir, "locked.zip", &options);
        let (file, cache) = (path(&dir, "locked.zip"), path(&dir, "cache"));
        assert_eq!(open(&file, None, &cache).err(), Some(ArchiveError::PasswordRequired));
        let opened = open(&file, Some("pw"), &cache).unwrap();
        assert_eq!(text(&*opened.vfs, "/docs/readme.txt"), "read me");
    }

    #[test]
    fn other_formats_and_levels() {
        let dir = sources();
        for (format, level) in [
            (Format::TarGz, Level::Best),
            (Format::TarBz2, Level::Fast),
            (Format::TarXz, Level::Normal),
            (Format::SevenZip, Level::Normal),
            (Format::Zip, Level::Store),
        ] {
            let name = format!("out-{level:?}.{}", format.extension());
            let options = CompressOptions { format, level, ..CompressOptions::default() };
            pack(&dir, &name, &options);
            let opened = open(&path(&dir, &name), None, &path(&dir, "cache")).unwrap();
            assert_eq!(text(&*opened.vfs, "/a b.txt"), "top", "{name}");
        }
    }

    #[test]
    fn names_that_look_like_options_are_just_names() {
        let dir = sources();
        fs::write(dir.path().join("src/-rf"), "dash").unwrap();
        fs::write(dir.path().join("src/@list"), "at").unwrap();
        let sources = [path(&dir, "src/-rf"), path(&dir, "src/@list")];
        let done = compress(&sources, &path(&dir, "odd.zip"), &CompressOptions::default(), &|| true, &mut |_, _| {});
        assert_eq!(done, Ok(true));
        let opened = open(&path(&dir, "odd.zip"), None, &path(&dir, "cache")).unwrap();
        assert_eq!(names(&*opened.vfs, "/"), ["-rf", "@list"]);
    }

    #[test]
    fn lone_xz_lzma_and_zst_files_hold_one_item() {
        // §1: single .xz, .lzma and .zst files.
        let dir = sources();
        let data = b"compressed once\n";
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut &data[..], &mut xz).unwrap();
        fs::write(dir.path().join("notes.txt.xz"), &xz).unwrap();
        let mut lzma = Vec::new();
        lzma_rs::lzma_compress(&mut &data[..], &mut lzma).unwrap();
        fs::write(dir.path().join("notes.txt.lzma"), &lzma).unwrap();
        let zst = ruzstd::encoding::compress_to_vec(&data[..], ruzstd::encoding::CompressionLevel::Fastest);
        fs::write(dir.path().join("notes.txt.zst"), &zst).unwrap();
        for name in ["notes.txt.xz", "notes.txt.lzma", "notes.txt.zst"] {
            assert!(is_archive(name), "{name}");
            let opened = open(&path(&dir, name), None, &path(&dir, "cache")).unwrap();
            assert_eq!(names(&*opened.vfs, "/"), ["notes.txt"], "{name}");
            assert_eq!(text(&*opened.vfs, "/notes.txt"), "compressed once\n", "{name}");
        }
        // Not really compressed: unrecognized, and nothing left behind.
        fs::write(dir.path().join("fake.zst"), "plain text").unwrap();
        assert_eq!(open(&path(&dir, "fake.zst"), None, &path(&dir, "cache")).err(), Some(ArchiveError::Unrecognized));
    }

    #[test]
    fn a_tar_zst_is_browsed() {
        let dir = sources();
        tool(&[TOOL, "-cf", "../t.tar", "docs", "a b.txt"], &dir.path().join("src"));
        let tar = fs::read(dir.path().join("t.tar")).unwrap();
        let zst = ruzstd::encoding::compress_to_vec(&tar[..], ruzstd::encoding::CompressionLevel::Fastest);
        fs::write(dir.path().join("t.tar.zst"), zst).unwrap();
        let cache = path(&dir, "cache");
        {
            let opened = open(&path(&dir, "t.tar.zst"), None, &cache).unwrap();
            assert_eq!(names(&*opened.vfs, "/"), ["a b.txt", "docs"]);
            assert_eq!(text(&*opened.vfs, "/docs/readme.txt"), "read me");
            // The temporary tar is gone; only the unpacked copy stays.
            assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
        }
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
        assert_eq!(stem("t.tar.zst"), "t");
    }

    #[test]
    fn a_7z_with_a_password_is_made_and_read_back() {
        // §2 and §4: 7z passwords, in Compress with Options and when browsing.
        let dir = sources();
        let options =
            CompressOptions { format: Format::SevenZip, password: Some("pw".into()), ..CompressOptions::default() };
        assert!(Format::SevenZip.takes_password() && !Format::TarGz.takes_password());
        let sources =
            [path(&dir, "src/docs"), path(&dir, "src/a b.txt"), path(&dir, "src/link"), path(&dir, "src/.DS_Store")];
        let mut seen = Vec::new();
        let done = compress(&sources, &path(&dir, "locked.7z"), &options, &|| true, &mut |n, name| {
            seen.push((n, name.to_string()))
        });
        assert_eq!(done, Ok(true));
        assert!(seen.iter().any(|(_, name)| name == "docs/readme.txt"), "{seen:?}");
        assert!(!dir.path().join("locked.7z.rustyfinder-partial").exists());
        // The names are encrypted too: nothing can be listed without it.
        let raw = fs::read(dir.path().join("locked.7z")).unwrap();
        assert!(!raw.windows(10).any(|w| w == b"readme.txt"));
        let (file, cache) = (path(&dir, "locked.7z"), path(&dir, "cache"));
        assert_eq!(open(&file, None, &cache).err(), Some(ArchiveError::PasswordRequired));
        assert_eq!(open(&file, Some("nope"), &cache).err(), Some(ArchiveError::WrongPassword));
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0, "failed attempts leave nothing");
        let opened = open(&file, Some("pw"), &cache).unwrap();
        assert_eq!(names(&*opened.vfs, "/"), ["a b.txt", "docs", "link"], "metadata left out");
        assert_eq!(text(&*opened.vfs, "/docs/readme.txt"), "read me");
        assert_eq!(opened.vfs.stat("/link").unwrap().link_target.as_deref(), Some("docs/readme.txt"));
        let original = fs::metadata(dir.path().join("src/a b.txt")).unwrap().modified().unwrap();
        let unpacked = opened.vfs.stat("/a b.txt").unwrap().modified_ms.unwrap();
        let original_ms = original.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        assert!((unpacked - original_ms).abs() < 2000, "dates kept");
        // A stopped one leaves nothing.
        let stopped = compress(&sources, &path(&dir, "never.7z"), &options, &|| false, &mut |_, _| {});
        assert_eq!(stopped, Ok(false));
        assert!(!dir.path().join("never.7z").exists() && !dir.path().join("never.7z.rustyfinder-partial").exists());
    }

    #[test]
    fn a_7z_cannot_write_through_its_own_links() {
        // An archive with a link "out" -> "/tmp" and then "out/x": the
        // file must not land in /tmp.
        let dir = sources();
        let outside = tempfile::tempdir().unwrap();
        let evil = dir.path().join("evil");
        fs::create_dir(&evil).unwrap();
        std::os::unix::fs::symlink(outside.path(), evil.join("out")).unwrap();
        // Write "out" (the link) and then a file named "out/x" by hand.
        use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
        let mut writer = ArchiveWriter::new(fs::File::create(dir.path().join("evil.7z")).unwrap()).unwrap();
        let mut link = ArchiveEntry::new_file("out");
        link.has_windows_attributes = true;
        link.windows_attributes = 0x8000 | (0o120777 << 16);
        writer.push_archive_entry(link, Some(outside.path().to_str().unwrap().as_bytes())).unwrap();
        writer.push_archive_entry(ArchiveEntry::new_file("out/x"), Some(&b"bad"[..])).unwrap();
        writer.push_archive_entry(ArchiveEntry::new_file("../up"), Some(&b"bad"[..])).unwrap();
        writer.finish().unwrap();
        let opened = open(&path(&dir, "evil.7z"), None, &path(&dir, "cache")).unwrap();
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0, "nothing written through the link");
        assert_eq!(opened.blocked, ["../up"]);
        assert_eq!(names(&*opened.vfs, "/"), ["out"]);
    }

    #[test]
    fn rar_archives_ask_for_their_password() {
        // §2: RAR passwords, for the contents and for the names too.
        let testdata = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata");
        let dir = sources();
        let cache = path(&dir, "cache");
        let crypted = format!("{testdata}/crypted.rar");
        assert_eq!(open(&crypted, None, &cache).err(), Some(ArchiveError::PasswordRequired));
        assert_eq!(open(&crypted, Some("nope"), &cache).err(), Some(ArchiveError::WrongPassword));
        let opened = open(&crypted, Some("unrar"), &cache).unwrap();
        let listed = names(&*opened.vfs, "/");
        assert_eq!(listed.len(), 1);
        assert_eq!(text(&*opened.vfs, &format!("/{}", listed[0])), "target\nCargo.lock\n");

        let hidden = format!("{testdata}/comment-hpw-password.rar");
        assert_eq!(open(&hidden, None, &cache).err(), Some(ArchiveError::PasswordRequired));
        assert_eq!(open(&hidden, Some("wrong"), &cache).err(), Some(ArchiveError::WrongPassword));
        let opened = open(&hidden, Some("password"), &cache).unwrap();
        assert_eq!(names(&*opened.vfs, "/").len(), 1);
        drop(opened);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1, "only the first archive is still open");
    }

    #[test]
    fn a_stopped_compress_leaves_nothing() {
        let dir = sources();
        let sources = [path(&dir, "src/docs")];
        let done = compress(&sources, &path(&dir, "never.zip"), &CompressOptions::default(), &|| false, &mut |_, _| {});
        assert_eq!(done, Ok(false));
        assert!(!dir.path().join("never.zip").exists());
        assert!(!dir.path().join("never.zip.rustyfinder-partial").exists());
    }

    #[test]
    fn a_missing_source_is_an_error_and_leaves_nothing() {
        let dir = sources();
        let sources = [path(&dir, "src/missing.txt")];
        let done = compress(&sources, &path(&dir, "bad.zip"), &CompressOptions::default(), &|| true, &mut |_, _| {});
        assert!(done.is_err());
        assert!(!dir.path().join("bad.zip").exists());
    }
}
