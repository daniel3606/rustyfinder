//! Archive formats the system's `bsdtar` cannot read on its own: 7z and
//! RAR with passwords, lone `.xz`, `.lzma` and `.zst` files and `.tar.zst`
//! (macOS ships libarchive without zstd), and 7z archives made with a
//! password. Everything is unpacked into a cache folder the same way
//! `archive::unpack` does, with the same rule: no entry is ever written
//! outside the folder, through a symlink, or over another entry's link.

use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use crate::archive::{ArchiveError, CompressOptions};
use crate::paths;

/// `parts` joined under `root`, with every folder on the way created.
/// `None` if any folder on the way is not a real folder (a symlink an
/// earlier entry made), so nothing is ever written through a link.
pub(crate) fn safe_dest(root: &Path, parts: &[&str]) -> Option<PathBuf> {
    let (last, folders) = parts.split_last()?;
    let mut here = root.to_path_buf();
    for part in folders {
        here.push(part);
        match fs::symlink_metadata(&here) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => return None,
            Err(_) => fs::create_dir(&here).ok()?,
        }
    }
    here.push(last);
    // An item already there (a link or a folder from another entry) is
    // left alone.
    if fs::symlink_metadata(&here).is_ok_and(|m| !m.is_dir() || m.file_type().is_symlink()) {
        return None;
    }
    Some(here)
}

fn set_mtime(path: &Path, ms: Option<i64>) {
    if let Some(ms) = ms {
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64);
        if let Ok(file) = File::options().write(true).open(path) {
            let _ = file.set_modified(when);
        }
    }
}

// ----------------------------------------------------------------- 7z

/// What 7z's attributes say: the Unix mode in the high half when the
/// 0x8000 flag is set (as p7zip and 7-Zip on Unix write it).
fn unix_mode(attributes: u32) -> Option<u32> {
    (attributes & 0x8000 != 0).then_some(attributes >> 16).filter(|m| *m != 0)
}

fn nt_ms(time: sevenz_rust2::NtTime) -> Option<i64> {
    let when: std::time::SystemTime = time.into();
    when.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

/// Unpack a 7z archive into `dir`. Entries that climb out are listed in
/// the result. A missing or wrong password is reported as such.
pub(crate) fn unpack_7z(file: &str, password: Option<&str>, dir: &Path) -> Result<Vec<String>, ArchiveError> {
    use sevenz_rust2::{ArchiveReader, Error, Password};
    let given = password.filter(|p| !p.is_empty());
    let secret = given.map(Password::from).unwrap_or_else(Password::empty);
    let password_problem =
        || if given.is_some() { ArchiveError::WrongPassword } else { ArchiveError::PasswordRequired };
    let mut reader = match ArchiveReader::open(file, secret) {
        Ok(reader) => reader,
        Err(Error::PasswordRequired | Error::MaybeBadPassword(_)) => return Err(password_problem()),
        // A header that cannot be read with a password given: the password.
        Err(_) if given.is_some() => return Err(ArchiveError::WrongPassword),
        Err(Error::BadSignature(_)) => return Err(ArchiveError::Unrecognized),
        Err(e) => return Err(ArchiveError::Failed(e.to_string())),
    };
    let mut blocked = Vec::new();
    let mut links: Vec<(PathBuf, String)> = Vec::new();
    let result = reader.for_each_entries(|entry, data| {
        let Some(parts) = crate::archive::safe_components(entry.name()) else {
            blocked.push(entry.name().to_string());
            return Ok(true);
        };
        if parts.is_empty() || entry.is_anti_item() {
            return Ok(true);
        }
        let Some(dest) = safe_dest(dir, &parts) else {
            blocked.push(entry.name().to_string());
            return Ok(true);
        };
        let mode = entry.has_windows_attributes.then(|| unix_mode(entry.windows_attributes())).flatten();
        if entry.is_directory() {
            fs::create_dir_all(&dest).map_err(|e| Error::Io(e, "create folder".into()))?;
            return Ok(true);
        }
        if mode.is_some_and(|m| m & 0o170000 == 0o120000) {
            // A link's content is where it points; made last, after every
            // file, so no file is ever written through it.
            let mut target = String::new();
            data.take(4096).read_to_string(&mut target).map_err(|e| Error::Io(e, "read link".into()))?;
            links.push((dest, target));
            return Ok(true);
        }
        let mut out = File::create(&dest).map_err(|e| Error::Io(e, "create file".into()))?;
        std::io::copy(data, &mut out).map_err(|e| Error::Io(e, "write file".into()))?;
        drop(out);
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(mode & 0o7777));
        }
        if entry.has_last_modified_date {
            set_mtime(&dest, nt_ms(entry.last_modified_date()));
        }
        Ok(true)
    });
    match result {
        Ok(()) => {}
        Err(Error::PasswordRequired | Error::MaybeBadPassword(_)) => return Err(password_problem()),
        // Encrypted data read with the wrong key fails to decode or to
        // match its checksum.
        Err(_) if given.is_some() => return Err(ArchiveError::WrongPassword),
        Err(e) => return Err(ArchiveError::Failed(e.to_string())),
    }
    for (dest, target) in links {
        let _ = std::os::unix::fs::symlink(target, dest);
    }
    Ok(blocked)
}

/// Write a 7z archive of `sources` at `partial`, encrypted with
/// `password` (AES-256, names included). `Ok(false)` if stopped.
pub(crate) fn compress_7z(
    sources: &[String],
    partial: &str,
    options: &CompressOptions,
    password: &str,
    keep_going: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, &str),
) -> Result<bool, String> {
    use sevenz_rust2::encoder_options::AesEncoderOptions;
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, EncoderMethod, Password};
    let out = File::create(partial).map_err(|e| e.to_string())?;
    let mut writer = ArchiveWriter::new(out).map_err(|e| e.to_string())?;
    writer.set_content_methods(vec![
        AesEncoderOptions::new(Password::from(password)).into(),
        EncoderMethod::LZMA2.into(),
    ]);
    writer.set_encrypt_header(true);

    // Every item with its name inside the archive, parents first.
    let mut items: Vec<(PathBuf, String)> = Vec::new();
    fn walk(path: &Path, name: String, options: &CompressOptions, items: &mut Vec<(PathBuf, String)>) {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if options.exclude_metadata && (file_name == ".DS_Store" || file_name.starts_with("._")) {
            return;
        }
        items.push((path.to_path_buf(), name.clone()));
        let is_dir = fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
        if is_dir {
            let mut children: Vec<_> = fs::read_dir(path).into_iter().flatten().flatten().map(|e| e.path()).collect();
            children.sort();
            for child in children {
                let child_name = child.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                walk(&child, format!("{name}/{child_name}"), options, items);
            }
        }
    }
    for source in sources {
        let path = Path::new(source);
        if fs::symlink_metadata(path).is_err() {
            return Err(format!("“{}” can’t be read.", paths::file_name(source)));
        }
        walk(path, paths::file_name(source).to_string(), options, &mut items);
    }

    for (count, (path, name)) in items.iter().enumerate() {
        if !keep_going() {
            return Ok(false);
        }
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        let mut entry = ArchiveEntry::from_path(path, name.clone());
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        entry.has_windows_attributes = true;
        // Unix mode in the high half, as 7-Zip on Unix writes it.
        entry.windows_attributes = 0x8000 | (mode << 16) | if meta.is_dir() { 0x10 } else { 0 };
        let pushed = if meta.file_type().is_symlink() {
            let target = fs::read_link(path).map_err(|e| e.to_string())?;
            entry.is_directory = false;
            entry.has_stream = true;
            writer.push_archive_entry(entry, Some(target.to_string_lossy().as_bytes()))
        } else if meta.is_dir() {
            writer.push_archive_entry::<&[u8]>(entry, None)
        } else {
            let file = File::open(path).map_err(|e| e.to_string())?;
            writer.push_archive_entry(entry, Some(file))
        };
        pushed.map_err(|e| e.to_string())?;
        progress(count as u64 + 1, name);
    }
    let mut file = writer.finish().map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    Ok(true)
}

// ---------------------------------------------------------------- RAR

/// Unpack a RAR archive (v4 or v5) into `dir` with the RAR authors' own
/// decoder.
pub(crate) fn unpack_rar(file: &str, password: Option<&str>, dir: &Path) -> Result<Vec<String>, ArchiveError> {
    use unrar::error::Code;
    let given = password.filter(|p| !p.is_empty());
    let password_problem =
        || if given.is_some() { ArchiveError::WrongPassword } else { ArchiveError::PasswordRequired };
    let map = |code: Code, message: String| match code {
        Code::MissingPassword => password_problem(),
        Code::BadPassword => ArchiveError::WrongPassword,
        // A wrong password shows as damaged data (RAR 4 has no check value).
        Code::BadData if given.is_some() => ArchiveError::WrongPassword,
        Code::UnknownFormat | Code::BadArchive => ArchiveError::Unrecognized,
        _ => ArchiveError::Failed(message),
    };
    let archive = match given {
        Some(pw) => unrar::Archive::with_password(file, pw),
        None => unrar::Archive::new(file),
    };
    let mut open = archive.open_for_processing().map_err(|e| map(e.code, e.to_string()))?;
    let mut blocked = Vec::new();
    loop {
        let Some(header) = open.read_header().map_err(|e| map(e.code, e.to_string()))? else { break };
        let entry = header.entry();
        let name = entry.filename.to_string_lossy().replace('\\', "/");
        let dest = crate::archive::safe_components(&name)
            .filter(|parts| !parts.is_empty())
            .and_then(|parts| safe_dest(dir, &parts));
        let is_dir = entry.is_directory();
        let ms = dos_ms(entry.file_time);
        open = match dest {
            None => {
                blocked.push(name);
                header.skip().map_err(|e| map(e.code, e.to_string()))?
            }
            Some(dest) if is_dir => {
                let _ = fs::create_dir_all(&dest);
                header.skip().map_err(|e| map(e.code, e.to_string()))?
            }
            Some(dest) => {
                let next = header.extract_to(&dest).map_err(|e| map(e.code, e.to_string()))?;
                if fs::symlink_metadata(&dest).is_ok_and(|m| m.is_file()) {
                    set_mtime(&dest, ms);
                }
                next
            }
        };
    }
    Ok(blocked)
}

/// RAR keeps MS-DOS times: date in the high half, time in the low.
fn dos_ms(stamp: u32) -> Option<i64> {
    crate::archive::dos_time_ms(u64::from(stamp >> 16), u64::from(stamp & 0xFFFF))
}

// ------------------------------------------------------- single files

/// Decompress a lone `.xz`, `.lzma` or `.zst` file (`ext`, lower case,
/// with the dot) from `file` into `out`.
pub(crate) fn decompress_single(file: &str, ext: &str, out: &mut dyn Write) -> std::io::Result<()> {
    let mut input = BufReader::new(File::open(file)?);
    let bad = |e: lzma_rs::error::Error| std::io::Error::other(e.to_string());
    match ext {
        ".xz" | ".txz" => lzma_rs::xz_decompress(&mut input, &mut &mut *out).map_err(bad),
        ".lzma" => lzma_rs::lzma_decompress(&mut input, &mut &mut *out).map_err(bad),
        ".zst" | ".tzst" => {
            let mut decoder = ruzstd::decoding::StreamingDecoder::new(input).map_err(std::io::Error::other)?;
            std::io::copy(&mut decoder, out).map(|_| ())
        }
        _ => Err(std::io::Error::other("not a compressed file")),
    }
}
