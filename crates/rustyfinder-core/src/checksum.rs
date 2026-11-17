//! File checksums: MD5, SHA-1, SHA-256, SHA-512.

use std::io::Read;

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Algorithm {
    Md5,
    Sha1,
    #[default]
    Sha256,
    Sha512,
}

impl Algorithm {
    pub fn name(self) -> &'static str {
        match self {
            Algorithm::Md5 => "MD5",
            Algorithm::Sha1 => "SHA-1",
            Algorithm::Sha256 => "SHA-256",
            Algorithm::Sha512 => "SHA-512",
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read everything from `reader` and return its digest as lowercase hex.
/// `progress` gets the bytes read so far; `keep_going` returning false
/// stops early with `Ok(None)`.
pub fn digest(
    reader: &mut dyn Read,
    algorithm: Algorithm,
    keep_going: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64),
) -> std::io::Result<Option<String>> {
    enum Hasher {
        Md5(Md5),
        Sha1(Sha1),
        Sha256(Sha256),
        Sha512(Sha512),
    }
    let mut hasher = match algorithm {
        Algorithm::Md5 => Hasher::Md5(Md5::new()),
        Algorithm::Sha1 => Hasher::Sha1(Sha1::new()),
        Algorithm::Sha256 => Hasher::Sha256(Sha256::new()),
        Algorithm::Sha512 => Hasher::Sha512(Sha512::new()),
    };
    let mut buffer = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        if !keep_going() {
            return Ok(None);
        }
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        let chunk = &buffer[..n];
        match &mut hasher {
            Hasher::Md5(h) => h.update(chunk),
            Hasher::Sha1(h) => h.update(chunk),
            Hasher::Sha256(h) => h.update(chunk),
            Hasher::Sha512(h) => h.update(chunk),
        }
        total += n as u64;
        progress(total);
    }
    Ok(Some(match hasher {
        Hasher::Md5(h) => hex(&h.finalize()),
        Hasher::Sha1(h) => hex(&h.finalize()),
        Hasher::Sha256(h) => hex(&h.finalize()),
        Hasher::Sha512(h) => hex(&h.finalize()),
    }))
}

/// Does `digest` equal what the user pasted, ignoring case and any
/// whitespace in the pasted text?
pub fn matches(digest: &str, pasted: &str) -> bool {
    let cleaned: String = pasted.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    !cleaned.is_empty() && cleaned == digest.to_lowercase()
}

/// One CSV field: quoted when it holds a comma, a quote or a line break.
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// `name,algorithm,digest` per row, for pasting into a spreadsheet.
pub fn csv(rows: &[(String, Algorithm, String)]) -> String {
    rows.iter()
        .map(|(name, algorithm, digest)| format!("{},{},{}", csv_field(name), algorithm.name(), digest))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(data: &[u8], algorithm: Algorithm) -> String {
        digest(&mut &data[..], algorithm, &|| true, &mut |_| {}).unwrap().unwrap()
    }

    #[test]
    fn tl06_known_digests() {
        assert_eq!(
            of(b"hello\n", Algorithm::Sha256),
            "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
        );
        assert_eq!(of(b"hello\n", Algorithm::Md5), "b1946ac92492d2347c6235b4d2611184");
        assert_eq!(of(b"hello\n", Algorithm::Sha1), "f572d396fae9206628714fb2ce00f72e94f2258f");
        assert_eq!(
            of(b"", Algorithm::Sha512),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
    }

    #[test]
    fn progress_counts_bytes_and_stop_gives_nothing() {
        let data = vec![7u8; 200_000];
        let mut seen = 0;
        digest(&mut &data[..], Algorithm::Sha256, &|| true, &mut |n| seen = n).unwrap();
        assert_eq!(seen, 200_000);
        assert_eq!(digest(&mut &data[..], Algorithm::Sha256, &|| false, &mut |_| {}).unwrap(), None);
    }

    #[test]
    fn pasted_text_matches_whatever_its_case_and_spacing() {
        let d = "5891b5b522d5df08";
        assert!(matches(d, " 5891B5B5 22D5DF08\n"));
        assert!(!matches(d, "5891b5b522d5df09"));
        assert!(!matches(d, "   "));
    }

    #[test]
    fn tl07_rows_copy_as_csv() {
        let rows = vec![
            ("a.txt".to_string(), Algorithm::Sha256, "aa".to_string()),
            ("we,ird \"name\".txt".to_string(), Algorithm::Md5, "bb".to_string()),
        ];
        assert_eq!(csv(&rows), "a.txt,SHA-256,aa\n\"we,ird \"\"name\"\".txt\",MD5,bb");
    }
}
