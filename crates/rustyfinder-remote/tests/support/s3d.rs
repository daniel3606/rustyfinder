//! A scripted S3 server for tests: path-style addressing, objects kept in
//! memory, enough of the REST API for listing (with pagination), objects,
//! server-side copy, batch delete and multipart uploads. Signatures are
//! not checked. Every request is logged.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Object {
    pub data: Vec<u8>,
    pub content_type: String,
    /// As sent in `x-amz-storage-class`; empty is STANDARD.
    pub storage_class: String,
}

/// A multipart upload in progress: bucket, key, content type, parts.
pub type Upload = (String, String, String, BTreeMap<u32, Vec<u8>>);

#[derive(Default)]
pub struct State {
    /// Bucket name to its objects by key.
    pub buckets: BTreeMap<String, BTreeMap<String, Object>>,
    /// Multipart uploads in progress, by upload id.
    pub uploads: HashMap<String, Upload>,
    pub log: Vec<String>,
    /// Page size for listings when the client names none.
    pub page: usize,
    /// Buckets that live in another region than us-east-1: requests signed
    /// for any other region get "301, x-amz-bucket-region".
    pub regions: HashMap<String, String>,
    next_upload: u32,
}

pub type Shared = Arc<Mutex<State>>;

/// Listen on a free local port. Returns the port.
pub fn start(state: Shared) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                let _ = serve(stream, &state);
            });
        }
    });
    port
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn respond(
    out: &mut TcpStream,
    status: &str,
    headers: &[(&str, String)],
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    out.write_all(head.as_bytes())?;
    if !head_only {
        out.write_all(body)?;
    }
    Ok(())
}

fn error(out: &mut TcpStream, status: &str, code: &str, head_only: bool) -> std::io::Result<()> {
    let xml = format!("<?xml version=\"1.0\"?><Error><Code>{code}</Code><Message>{code}</Message></Error>");
    respond(out, status, &[("Content-Type", "application/xml".into())], xml.as_bytes(), head_only)
}

/// A body sent as "aws-chunked": `size[;chunk-signature=..]\r\n data \r\n`
/// repeated, a zero-size chunk, then optional trailers.
fn decode_aws_chunked(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let Some(end) = body[at..].windows(2).position(|w| w == b"\r\n") else { break };
        let line = String::from_utf8_lossy(&body[at..at + end]).into_owned();
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        at += end + 2;
        if size == 0 {
            break;
        }
        out.extend_from_slice(&body[at..(at + size).min(body.len())]);
        at += size + 2;
    }
    out
}

fn serve(stream: TcpStream, state: &Shared) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    let mut input = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("/").to_string());
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            let mut header = String::new();
            input.read_line(&mut header)?;
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                headers.push((name.trim().to_lowercase(), value.trim().to_string()));
            }
        }
        let header = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        if header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
            out.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        }
        let mut body = Vec::new();
        if header("transfer-encoding").is_some_and(|v| v.to_lowercase().contains("chunked")) {
            loop {
                let mut size = String::new();
                input.read_line(&mut size)?;
                let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16).unwrap_or(0);
                if n == 0 {
                    loop {
                        let mut trailer = String::new();
                        input.read_line(&mut trailer)?;
                        if trailer.trim().is_empty() {
                            break;
                        }
                    }
                    break;
                }
                let mut chunk = vec![0u8; n];
                input.read_exact(&mut chunk)?;
                body.extend_from_slice(&chunk);
                let mut crlf = [0u8; 2];
                input.read_exact(&mut crlf)?;
            }
        } else if let Some(length) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
            body = vec![0u8; length];
            input.read_exact(&mut body)?;
        }
        if header("x-amz-content-sha256").is_some_and(|v| v.starts_with("STREAMING"))
            || header("content-encoding").is_some_and(|v| v.contains("aws-chunked"))
        {
            body = decode_aws_chunked(&body);
        }

        let (raw_path, raw_query) = target.split_once('?').unwrap_or((&target, ""));
        let path = decode(raw_path);
        let query: HashMap<String, String> = raw_query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| p.split_once('=').map_or((decode(p), String::new()), |(k, v)| (decode(k), decode(v))))
            .collect();
        let (bucket, key) = match path.trim_start_matches('/').split_once('/') {
            Some((b, k)) => (b.to_string(), k.to_string()),
            None => (path.trim_start_matches('/').to_string(), String::new()),
        };
        let head_only = method == "HEAD";
        let mut st = state.lock().unwrap();
        // The region the request was signed for: Credential=AK/date/REGION/s3/…
        let signed_for = header("authorization")
            .and_then(|a| a.split("Credential=").nth(1).map(|c| c.split('/').nth(2).unwrap_or("").to_string()))
            .unwrap_or_default();
        st.log.push(format!(
            "{method} {path}{}{raw_query} region={signed_for}",
            if raw_query.is_empty() { "" } else { "?" }
        ));
        if let Some(home) = st.regions.get(&bucket).cloned().filter(|r| *r != signed_for) {
            drop(st);
            let xml =
                "<?xml version=\"1.0\"?><Error><Code>PermanentRedirect</Code><Message>Wrong region</Message></Error>";
            respond(
                &mut out,
                "301 Moved Permanently",
                &[("x-amz-bucket-region", home), ("Content-Type", "application/xml".into())],
                xml.as_bytes(),
                head_only,
            )?;
            continue;
        }
        let xml = |text: String| (text.into_bytes(), [("Content-Type", "application/xml".to_string())]);

        // --- The service itself: list buckets.
        if bucket.is_empty() {
            let list: String = st
                .buckets
                .keys()
                .map(|b| {
                    format!(
                        "<Bucket><Name>{}</Name><CreationDate>2020-01-01T00:00:00.000Z</CreationDate></Bucket>",
                        escape(b)
                    )
                })
                .collect();
            let (bytes, headers) = xml(format!(
                "<?xml version=\"1.0\"?><ListAllMyBucketsResult><Buckets>{list}</Buckets></ListAllMyBucketsResult>"
            ));
            drop(st);
            respond(&mut out, "200 OK", &headers, &bytes, head_only)?;
            continue;
        }
        // --- A bucket.
        if key.is_empty() {
            match method.as_str() {
                "PUT" => {
                    st.buckets.entry(bucket.clone()).or_default();
                    drop(st);
                    respond(&mut out, "200 OK", &[("Location", format!("/{bucket}"))], b"", false)?;
                }
                "DELETE" => {
                    let outcome = match st.buckets.get(&bucket) {
                        None => ("404 Not Found", "NoSuchBucket"),
                        Some(objects) if !objects.is_empty() => ("409 Conflict", "BucketNotEmpty"),
                        Some(_) => {
                            st.buckets.remove(&bucket);
                            ("204 No Content", "")
                        }
                    };
                    drop(st);
                    if outcome.1.is_empty() {
                        respond(&mut out, outcome.0, &[], b"", false)?
                    } else {
                        error(&mut out, outcome.0, outcome.1, false)?
                    }
                }
                "POST" if query.contains_key("delete") => {
                    let text = String::from_utf8_lossy(&body).into_owned();
                    let keys: Vec<String> = text
                        .split("<Key>")
                        .skip(1)
                        .filter_map(|rest| rest.split("</Key>").next())
                        .map(|k| k.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">"))
                        .collect();
                    if let Some(objects) = st.buckets.get_mut(&bucket) {
                        for key in &keys {
                            objects.remove(key);
                        }
                    }
                    let deleted: String =
                        keys.iter().map(|k| format!("<Deleted><Key>{}</Key></Deleted>", escape(k))).collect();
                    let (bytes, headers) =
                        xml(format!("<?xml version=\"1.0\"?><DeleteResult>{deleted}</DeleteResult>"));
                    drop(st);
                    respond(&mut out, "200 OK", &headers, &bytes, false)?;
                }
                "GET" | "HEAD" => {
                    let Some(objects) = st.buckets.get(&bucket) else {
                        drop(st);
                        error(&mut out, "404 Not Found", "NoSuchBucket", head_only)?;
                        continue;
                    };
                    if head_only {
                        drop(st);
                        respond(&mut out, "200 OK", &[("x-amz-bucket-region", "us-east-1".into())], b"", true)?;
                        continue;
                    }
                    if query.contains_key("uploads") {
                        let list: String = st
                            .uploads
                            .iter()
                            .filter(|(_, u)| u.0 == bucket)
                            .map(|(id, u)| {
                                format!("<Upload><Key>{}</Key><UploadId>{id}</UploadId></Upload>", escape(&u.1))
                            })
                            .collect();
                        let (bytes, headers) = xml(format!("<?xml version=\"1.0\"?><ListMultipartUploadsResult><Bucket>{bucket}</Bucket>{list}</ListMultipartUploadsResult>"));
                        drop(st);
                        respond(&mut out, "200 OK", &headers, &bytes, false)?;
                        continue;
                    }
                    let prefix = query.get("prefix").cloned().unwrap_or_default();
                    let delimiter = query.get("delimiter").cloned().unwrap_or_default();
                    let after = query
                        .get("continuation-token")
                        .or_else(|| query.get("start-after"))
                        .cloned()
                        .unwrap_or_default();
                    let default_page = if st.page == 0 { 1000 } else { st.page };
                    let max = query
                        .get("max-keys")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(default_page)
                        .min(default_page);
                    // Each key or common prefix counts as one listed item.
                    let mut items: Vec<(String, Option<usize>)> = Vec::new();
                    for (key, object) in objects.range(prefix.clone()..).take_while(|(k, _)| k.starts_with(&prefix)) {
                        let rest = &key[prefix.len()..];
                        let item = match (delimiter.is_empty(), rest.find(&delimiter)) {
                            (false, Some(i)) => (format!("{prefix}{}", &rest[..i + delimiter.len()]), None),
                            _ => (key.clone(), Some(object.data.len())),
                        };
                        if items.last().map(|l| &l.0) != Some(&item.0) {
                            items.push(item);
                        }
                    }
                    let remaining: Vec<&(String, Option<usize>)> =
                        items.iter().filter(|(name, _)| after.is_empty() || name.as_str() > after.as_str()).collect();
                    let truncated = remaining.len() > max;
                    let page = &remaining[..remaining.len().min(max)];
                    let mut text = format!("<?xml version=\"1.0\"?><ListBucketResult><Name>{bucket}</Name><Prefix>{}</Prefix><KeyCount>{}</KeyCount><MaxKeys>{max}</MaxKeys><IsTruncated>{truncated}</IsTruncated>", escape(&prefix), page.len());
                    if truncated {
                        text.push_str(&format!(
                            "<NextContinuationToken>{}</NextContinuationToken>",
                            escape(&page[page.len() - 1].0)
                        ));
                    }
                    for (name, size) in page {
                        match size {
                            Some(size) => text.push_str(&format!("<Contents><Key>{}</Key><LastModified>2020-01-01T00:00:00.000Z</LastModified><ETag>&quot;etag&quot;</ETag><Size>{size}</Size><StorageClass>STANDARD</StorageClass></Contents>", escape(name))),
                            None => text.push_str(&format!("<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>", escape(name))),
                        }
                    }
                    text.push_str("</ListBucketResult>");
                    let (bytes, headers) = xml(text);
                    drop(st);
                    respond(&mut out, "200 OK", &headers, &bytes, false)?;
                }
                _ => {
                    drop(st);
                    error(&mut out, "405 Method Not Allowed", "MethodNotAllowed", false)?;
                }
            }
            continue;
        }
        // --- An object.
        if !st.buckets.contains_key(&bucket) {
            drop(st);
            error(&mut out, "404 Not Found", "NoSuchBucket", head_only)?;
            continue;
        }
        let content_type = header("content-type").unwrap_or_else(|| "binary/octet-stream".into());
        match method.as_str() {
            "GET" | "HEAD" => match st.buckets[&bucket].get(&key).cloned() {
                Some(object) => {
                    drop(st);
                    let mut headers = vec![
                        ("Content-Type", object.content_type.clone()),
                        ("Last-Modified", "Wed, 01 Jan 2020 00:00:00 GMT".to_string()),
                        ("ETag", "\"etag\"".to_string()),
                    ];
                    if !object.storage_class.is_empty() {
                        headers.push(("x-amz-storage-class", object.storage_class.clone()));
                    }
                    let from = header("range")
                        .and_then(|r| r.strip_prefix("bytes=").and_then(|r| r.strip_suffix('-')).map(str::to_string))
                        .and_then(|n| n.parse::<usize>().ok());
                    match from {
                        Some(from) if from <= object.data.len() => {
                            let len = object.data.len();
                            headers.push(("Content-Range", format!("bytes {from}-{}/{len}", len.max(1) - 1)));
                            respond(&mut out, "206 Partial Content", &headers, &object.data[from..], head_only)?;
                        }
                        _ => respond(&mut out, "200 OK", &headers, &object.data, head_only)?,
                    }
                }
                None => {
                    drop(st);
                    error(&mut out, "404 Not Found", "NoSuchKey", head_only)?;
                }
            },
            "PUT" if query.contains_key("partNumber") => {
                let id = query.get("uploadId").cloned().unwrap_or_default();
                let part: u32 = query["partNumber"].parse().unwrap_or(0);
                let known = st.uploads.get_mut(&id).map(|upload| upload.3.insert(part, body)).is_some();
                drop(st);
                if known {
                    respond(&mut out, "200 OK", &[("ETag", format!("\"part-{part}\""))], b"", false)?;
                } else {
                    error(&mut out, "404 Not Found", "NoSuchUpload", false)?;
                }
            }
            "PUT" => {
                let object = match header("x-amz-copy-source") {
                    Some(source) => {
                        let source = decode(&source);
                        let (from_bucket, from_key) =
                            source.trim_start_matches('/').split_once('/').unwrap_or(("", ""));
                        st.buckets.get(from_bucket).and_then(|objects| objects.get(from_key)).cloned()
                    }
                    None => Some(Object {
                        data: body,
                        content_type,
                        storage_class: header("x-amz-storage-class").unwrap_or_default(),
                    }),
                };
                let copied = header("x-amz-copy-source").is_some();
                match object {
                    Some(object) => {
                        st.buckets.get_mut(&bucket).unwrap().insert(key, object);
                        drop(st);
                        if copied {
                            let (bytes, headers) = xml("<?xml version=\"1.0\"?><CopyObjectResult><ETag>&quot;etag&quot;</ETag><LastModified>2020-01-01T00:00:00.000Z</LastModified></CopyObjectResult>".into());
                            respond(&mut out, "200 OK", &headers, &bytes, false)?;
                        } else {
                            respond(&mut out, "200 OK", &[("ETag", "\"etag\"".into())], b"", false)?;
                        }
                    }
                    None => {
                        drop(st);
                        error(&mut out, "404 Not Found", "NoSuchKey", false)?;
                    }
                }
            }
            "POST" if query.contains_key("uploads") => {
                st.next_upload += 1;
                let id = format!("upload-{}", st.next_upload);
                st.uploads.insert(id.clone(), (bucket.clone(), key.clone(), content_type, BTreeMap::new()));
                let (bytes, headers) = xml(format!("<?xml version=\"1.0\"?><InitiateMultipartUploadResult><Bucket>{bucket}</Bucket><Key>{}</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>", escape(&key)));
                drop(st);
                respond(&mut out, "200 OK", &headers, &bytes, false)?;
            }
            "POST" if query.contains_key("uploadId") => {
                let finished = st.uploads.remove(&query["uploadId"]);
                match finished {
                    Some((b, k, content_type, parts)) => {
                        let data: Vec<u8> = parts.into_values().flatten().collect();
                        st.buckets
                            .get_mut(&b)
                            .unwrap()
                            .insert(k.clone(), Object { data, content_type, storage_class: String::new() });
                        let (bytes, headers) = xml(format!("<?xml version=\"1.0\"?><CompleteMultipartUploadResult><Bucket>{b}</Bucket><Key>{}</Key><ETag>&quot;etag-1&quot;</ETag></CompleteMultipartUploadResult>", escape(&k)));
                        drop(st);
                        respond(&mut out, "200 OK", &headers, &bytes, false)?;
                    }
                    None => {
                        drop(st);
                        error(&mut out, "404 Not Found", "NoSuchUpload", false)?;
                    }
                }
            }
            "DELETE" if query.contains_key("uploadId") => {
                st.uploads.remove(&query["uploadId"]);
                drop(st);
                respond(&mut out, "204 No Content", &[], b"", false)?;
            }
            "DELETE" => {
                st.buckets.get_mut(&bucket).unwrap().remove(&key);
                drop(st);
                respond(&mut out, "204 No Content", &[], b"", false)?;
            }
            _ => {
                drop(st);
                error(&mut out, "405 Method Not Allowed", "MethodNotAllowed", false)?;
            }
        }
    }
}
