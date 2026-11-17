//! Parsing a connection URL pasted into the Connect panel's Server field.

/// Protocols the Connect panel knows by URL scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Sftp,
    Ftp,
    Ftps,
    WebDav,
    WebDavs,
    S3,
    Smb,
    Afp,
    Nfs,
    Vnc,
}

/// What a pasted URL fills in. Empty or `None` parts were not in the URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionUrl {
    pub protocol: Protocol,
    pub host: String,
    pub user: String,
    pub password: Option<String>,
    pub port: Option<u16>,
    /// Remote path, "" if the URL had none.
    pub path: String,
}

/// Parse `scheme://[user[:password]@]host[:port][/path]`. Returns `None`
/// for anything that is not a URL with a known scheme: a plain host name
/// typed into the field stays as typed.
pub fn parse(text: &str) -> Option<ConnectionUrl> {
    let text = text.trim();
    let (scheme, rest) = text.split_once("://")?;
    let protocol = match scheme.to_ascii_lowercase().as_str() {
        "sftp" | "ssh" => Protocol::Sftp,
        "ftp" => Protocol::Ftp,
        "ftps" | "ftpes" => Protocol::Ftps,
        "http" | "dav" | "webdav" => Protocol::WebDav,
        "https" | "davs" | "webdavs" => Protocol::WebDavs,
        "s3" => Protocol::S3,
        "smb" | "cifs" => Protocol::Smb,
        "afp" => Protocol::Afp,
        "nfs" => Protocol::Nfs,
        "vnc" => Protocol::Vnc,
        _ => return None,
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    // The last "@" separates credentials from the host: a password may
    // itself contain "@".
    let (credentials, host_port) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    let (user, password) = match credentials {
        Some(c) => match c.split_once(':') {
            Some((u, p)) => (decode(u), Some(decode(p))),
            None => (decode(c), None),
        },
        None => (String::new(), None),
    };
    // IPv6 literals are bracketed: [::1]:2222.
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        let (address, after) = rest.split_once(']')?;
        (address.to_string(), after.strip_prefix(':'))
    } else {
        match host_port.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (host_port.to_string(), None),
        }
    };
    let port = match port {
        Some(p) if !p.is_empty() => Some(p.parse::<u16>().ok()?),
        _ => None,
    };
    if host.is_empty() {
        return None;
    }
    Some(ConnectionUrl { protocol, host: decode(&host), user, password, port, path: decode(path) })
}

/// Percent-decoding. Malformed escapes are kept as typed.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() && text.is_char_boundary(i + 1) && text.is_char_boundary(i + 3) {
            if let Ok(value) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_sf12_sftp_url_fills_every_field() {
        let url = parse("sftp://bob@example.org:2222/var/www").unwrap();
        assert_eq!(url.protocol, Protocol::Sftp);
        assert_eq!(url.user, "bob");
        assert_eq!(url.port, Some(2222));
        assert_eq!(url.host, "example.org");
        assert_eq!(url.path, "/var/www");
        assert_eq!(url.password, None);
    }

    #[test]
    fn password_and_special_characters() {
        let url = parse("sftp://user:pass@host:2222/path").unwrap();
        assert_eq!((url.user.as_str(), url.password.as_deref()), ("user", Some("pass")));
        // "@" and ":" inside a password, plain and percent-encoded.
        let url = parse("ftp://bob:p@ss:word@files.example.org/").unwrap();
        assert_eq!(url.password.as_deref(), Some("p@ss:word"));
        assert_eq!(url.host, "files.example.org");
        let url = parse("sftp://bob%40corp:a%2Fb%20c@h/my%20docs").unwrap();
        assert_eq!(url.user, "bob@corp");
        assert_eq!(url.password.as_deref(), Some("a/b c"));
        assert_eq!(url.path, "/my docs");
    }

    #[test]
    fn schemes_map_to_protocols() {
        for (text, protocol) in [
            ("ftp://h", Protocol::Ftp),
            ("ftps://h", Protocol::Ftps),
            ("https://h/dav", Protocol::WebDavs),
            ("davs://h", Protocol::WebDavs),
            ("http://h", Protocol::WebDav),
            ("s3://bucket/key", Protocol::S3),
            ("smb://server/share", Protocol::Smb),
            ("afp://h", Protocol::Afp),
            ("nfs://h/export", Protocol::Nfs),
            ("vnc://h", Protocol::Vnc),
            ("SFTP://H", Protocol::Sftp),
        ] {
            assert_eq!(parse(text).unwrap().protocol, protocol, "{text}");
        }
    }

    #[test]
    fn hosts_ports_and_paths() {
        let url = parse("sftp://example.org").unwrap();
        assert_eq!((url.host.as_str(), url.port, url.path.as_str(), url.user.as_str()), ("example.org", None, "", ""));
        let url = parse("  sftp://[::1]:2222/srv  ").unwrap();
        assert_eq!((url.host.as_str(), url.port, url.path.as_str()), ("::1", Some(2222), "/srv"));
        let url = parse("sftp://[fe80::1]/x").unwrap();
        assert_eq!((url.host.as_str(), url.port), ("fe80::1", None));
    }

    #[test]
    fn things_that_are_not_connection_urls_are_left_alone() {
        for text in [
            "example.org",
            "bob@example.org",
            "",
            "gopher://h",
            "sftp://",
            "sftp://h:notaport",
            "sftp://h:99999",
            "sftp://[::1",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn malformed_percent_escapes_are_kept() {
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("a%zzb"), "a%zzb");
        assert_eq!(decode("%41%4"), "A%4");
    }
}
