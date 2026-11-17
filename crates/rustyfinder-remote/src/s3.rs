//! Amazon S3 and S3-compatible services, behind the core's `Vfs`.
//!
//! The top level lists buckets; inside a bucket, "folders" are key
//! prefixes ending in `/`. A zero-byte object whose key ends in `/` marks
//! an (otherwise empty) folder and is shown as that folder, never as a
//! file. Renames are a server-side copy followed by a delete.
//!
//! A bucket can live in another region than the connection's. The service
//! then answers with the bucket's region in `x-amz-bucket-region`; the
//! request is sent again to that region, and the bucket remembered there.

use std::collections::HashMap;
use std::future::Future;
use std::io::{Read, Write};
use std::sync::Mutex;

use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier, StorageClass};
use aws_sdk_s3::Client;
use rustyfinder_core::paths;
use rustyfinder_core::{Entry, EntryKind, Vfs, VfsError, VfsResult};
use tokio::runtime::{Handle, Runtime};

use crate::RemoteError;

#[derive(Debug, Clone, Default)]
pub struct S3Params {
    pub access_key: String,
    pub secret_key: String,
    /// Empty for Amazon itself; else `host[:port]` or a full URL of an
    /// S3-compatible service (MinIO, Wasabi, R2…).
    pub endpoint: String,
    /// Empty is `us-east-1`.
    pub region: String,
    /// Open inside this bucket; empty lists all buckets at the top.
    pub bucket: String,
    /// Uploads larger than this go up in parts of this size. `None` is
    /// 16 MB (the service's minimum part size is 5 MB).
    pub part_size: Option<usize>,
    /// Addressing: `Some(true)` path style (`host/bucket/key`), `Some(false)`
    /// virtual-hosted (`bucket.host/key`), `None` automatic: virtual-hosted
    /// on Amazon, path style for compatible services.
    pub path_style: Option<bool>,
    /// Storage class of uploads ("STANDARD_IA", "GLACIER"…); `None` is
    /// STANDARD.
    pub storage_class: Option<String>,
}

/// Bytes per part of a multipart upload.
const PART_SIZE: usize = 16 << 20;

/// Every AWS region: code and the name the console shows.
pub const REGIONS: &[(&str, &str)] = &[
    ("us-east-1", "US East (N. Virginia)"),
    ("us-east-2", "US East (Ohio)"),
    ("us-west-1", "US West (N. California)"),
    ("us-west-2", "US West (Oregon)"),
    ("af-south-1", "Africa (Cape Town)"),
    ("ap-east-1", "Asia Pacific (Hong Kong)"),
    ("ap-east-2", "Asia Pacific (Taipei)"),
    ("ap-south-1", "Asia Pacific (Mumbai)"),
    ("ap-south-2", "Asia Pacific (Hyderabad)"),
    ("ap-southeast-1", "Asia Pacific (Singapore)"),
    ("ap-southeast-2", "Asia Pacific (Sydney)"),
    ("ap-southeast-3", "Asia Pacific (Jakarta)"),
    ("ap-southeast-4", "Asia Pacific (Melbourne)"),
    ("ap-southeast-5", "Asia Pacific (Malaysia)"),
    ("ap-southeast-6", "Asia Pacific (New Zealand)"),
    ("ap-southeast-7", "Asia Pacific (Thailand)"),
    ("ap-northeast-1", "Asia Pacific (Tokyo)"),
    ("ap-northeast-2", "Asia Pacific (Seoul)"),
    ("ap-northeast-3", "Asia Pacific (Osaka)"),
    ("ca-central-1", "Canada (Central)"),
    ("ca-west-1", "Canada West (Calgary)"),
    ("eu-central-1", "Europe (Frankfurt)"),
    ("eu-central-2", "Europe (Zurich)"),
    ("eu-west-1", "Europe (Ireland)"),
    ("eu-west-2", "Europe (London)"),
    ("eu-west-3", "Europe (Paris)"),
    ("eu-south-1", "Europe (Milan)"),
    ("eu-south-2", "Europe (Spain)"),
    ("eu-north-1", "Europe (Stockholm)"),
    ("il-central-1", "Israel (Tel Aviv)"),
    ("me-south-1", "Middle East (Bahrain)"),
    ("me-central-1", "Middle East (UAE)"),
    ("mx-central-1", "Mexico (Central)"),
    ("sa-east-1", "South America (São Paulo)"),
    ("us-gov-east-1", "AWS GovCloud (US-East)"),
    ("us-gov-west-1", "AWS GovCloud (US-West)"),
];

/// Storage classes offered for uploads.
pub const STORAGE_CLASSES: &[&str] = &[
    "STANDARD",
    "REDUCED_REDUNDANCY",
    "STANDARD_IA",
    "ONEZONE_IA",
    "INTELLIGENT_TIERING",
    "GLACIER",
    "GLACIER_IR",
    "DEEP_ARCHIVE",
];

pub struct S3Fs {
    /// The connection's own client and configuration.
    client: Client,
    config: aws_sdk_s3::Config,
    /// Clients for buckets found to live in another region.
    regional: Mutex<HashMap<String, (String, Client)>>,
    runtime: Runtime,
    label: String,
    home: String,
    part_size: usize,
    storage_class: Option<StorageClass>,
    /// `https://host[:port]` of a compatible service; empty for Amazon.
    endpoint: String,
    path_style: bool,
    region: String,
}

/// `/bucket/some/key` as ("bucket", "some/key"); the top is ("", "").
fn split(path: &str) -> (&str, &str) {
    let trimmed = path.trim_matches('/');
    match trimmed.split_once('/') {
        Some((bucket, key)) => (bucket, key),
        None => (trimmed, ""),
    }
}

/// Percent-encode a `bucket/key` for the copy-source header.
fn encode_source(bucket: &str, key: &str) -> String {
    crate::webdav::encode_path(&format!("{bucket}/{key}"))
}

/// The MIME type for a file name, from its extension. Unknown types are
/// `application/octet-stream`.
pub fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// True if a bucket name can be a DNS label, so virtual-hosted addressing
/// works for it (lower case, digits, hyphens; dots break TLS names).
pub fn dns_compatible(bucket: &str) -> bool {
    (3..=63).contains(&bucket.len())
        && bucket.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !bucket.starts_with('-')
        && !bucket.ends_with('-')
}

fn dir_entry(name: &str, created_ms: Option<i64>) -> Entry {
    Entry {
        name: name.to_string(),
        kind: EntryKind::Directory,
        is_dir: true,
        size: 0,
        modified_ms: None,
        created_ms,
        is_hidden: name.starts_with('.'),
        mode: None,
        link_target: None,
        is_broken_link: false,
        ext: Default::default(),
    }
}

fn file_entry(name: &str, size: i64, modified_ms: Option<i64>) -> Entry {
    Entry {
        name: name.to_string(),
        kind: EntryKind::File,
        is_dir: false,
        size: size.max(0) as u64,
        modified_ms,
        created_ms: None,
        is_hidden: name.starts_with('.'),
        mode: None,
        link_target: None,
        is_broken_link: false,
        ext: Default::default(),
    }
}

/// The service's own code for an error ("NoSuchKey") and HTTP status.
fn code_and_status<E: ProvideErrorMetadata>(error: &SdkError<E>) -> (String, u16) {
    let status = error.raw_response().map_or(0, |r| r.status().as_u16());
    (error.code().unwrap_or("").to_string(), status)
}

/// The region a "wrong region" answer names for the bucket.
fn bucket_region<E>(error: &SdkError<E>) -> Option<String> {
    let region = error.raw_response()?.headers().get("x-amz-bucket-region")?;
    Some(region.to_string())
}

fn describe<E: ProvideErrorMetadata + std::error::Error + 'static>(error: &SdkError<E>) -> String {
    match error.message() {
        Some(message) if !message.is_empty() => message.to_string(),
        _ => aws_sdk_s3::error::DisplayErrorContext(error).to_string(),
    }
}

fn vfs_error<E: ProvideErrorMetadata + std::error::Error + 'static>(path: &str, error: SdkError<E>) -> VfsError {
    let (code, status) = code_and_status(&error);
    match (code.as_str(), status) {
        ("NoSuchKey" | "NoSuchBucket" | "NotFound", _) | (_, 404) => VfsError::NotFound(path.to_string()),
        ("AccessDenied", _) | (_, 403) => VfsError::PermissionDenied(path.to_string()),
        ("BucketAlreadyOwnedByYou" | "BucketAlreadyExists", _) => VfsError::AlreadyExists(path.to_string()),
        ("InvalidObjectState", _) => VfsError::Io {
            path: path.to_string(),
            message: "The object is in archive storage (Glacier) and has to be restored before it can be downloaded."
                .into(),
        },
        ("SlowDown" | "RequestLimitExceeded" | "TooManyRequests", _) | (_, 429 | 503) => {
            let after = error
                .raw_response()
                .and_then(|r| r.headers().get("retry-after"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(2);
            VfsError::Throttled {
                path: path.to_string(),
                message: "The service asks to slow down.".into(),
                retry_after_secs: after,
            }
        }
        _ => VfsError::Io { path: path.to_string(), message: describe(&error) },
    }
}

fn remote_error<E: ProvideErrorMetadata + std::error::Error + 'static>(error: SdkError<E>) -> RemoteError {
    let (code, status) = code_and_status(&error);
    match (code.as_str(), status) {
        ("InvalidAccessKeyId" | "SignatureDoesNotMatch" | "AccessDenied" | "InvalidToken", _) | (_, 401 | 403) => {
            RemoteError::Auth("The service refused the access key or secret key.".into())
        }
        ("NoSuchBucket", _) => RemoteError::Other("The bucket does not exist.".into()),
        (_, 0) => RemoteError::Unreachable(format!("The service can’t be reached. {}", describe(&error))),
        _ => RemoteError::Other(describe(&error)),
    }
}

/// Connect and check the keys by listing the starting folder.
pub fn connect(params: S3Params) -> Result<S3Fs, RemoteError> {
    if params.access_key.trim().is_empty() {
        return Err(RemoteError::Auth("Missing access key.".into()));
    }
    if params.secret_key.trim().is_empty() {
        return Err(RemoteError::Auth("Missing secret key.".into()));
    }
    let region =
        if params.region.trim().is_empty() { "us-east-1".to_string() } else { params.region.trim().to_string() };
    let endpoint = params.endpoint.trim();
    let mut builder = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(region.clone()))
        .credentials_provider(Credentials::new(
            params.access_key.trim(),
            params.secret_key.trim(),
            None,
            None,
            "rustyfinder",
        ))
        // Checksums only where the service demands them: compatible
        // services often reject the newer trailing-checksum uploads.
        .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
        .response_checksum_validation(ResponseChecksumValidation::WhenRequired);
    let (label, url) = if endpoint.is_empty() {
        ("Amazon S3".to_string(), String::new())
    } else {
        let url = if endpoint.contains("://") { endpoint.to_string() } else { format!("https://{endpoint}") };
        let url = url.trim_end_matches('/').to_string();
        builder = builder.endpoint_url(url.clone());
        (url.split("://").nth(1).unwrap_or(&url).to_string(), url)
    };
    // Compatible services are addressed as host/bucket/key unless told
    // otherwise; on Amazon the SDK uses bucket.host/key where it can.
    let path_style = params.path_style.unwrap_or(!url.is_empty());
    builder = builder.force_path_style(path_style);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("rustyfinder-s3")
        .build()
        .map_err(|e| RemoteError::Other(e.to_string()))?;
    let bucket = params.bucket.trim().trim_matches('/');
    let config = builder.build();
    let fs = S3Fs {
        client: Client::from_conf(config.clone()),
        config,
        regional: Mutex::new(HashMap::new()),
        runtime,
        label,
        home: if bucket.is_empty() { "/".to_string() } else { format!("/{bucket}") },
        part_size: params.part_size.unwrap_or(PART_SIZE),
        storage_class: params.storage_class.filter(|s| !s.is_empty()).map(|s| StorageClass::from(s.as_str())),
        endpoint: url,
        path_style,
        region,
    };
    if bucket.is_empty() {
        fs.block(fs.client.list_buckets().send()).map_err(remote_error)?;
    } else {
        fs.call(bucket, |c| c.list_objects_v2().bucket(bucket).delimiter("/").max_keys(1).send())
            .map_err(remote_error)?;
    }
    Ok(fs)
}

impl S3Fs {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn home(&self) -> &str {
        &self.home
    }

    fn block<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// The client for a bucket: the connection's, or one for the region
    /// the bucket turned out to be in.
    fn client(&self, bucket: &str) -> Client {
        match self.regional.lock().unwrap().get(bucket) {
            Some((_, client)) => client.clone(),
            None => self.client.clone(),
        }
    }

    /// The region requests for `bucket` go to.
    pub fn region_of(&self, bucket: &str) -> String {
        self.regional.lock().unwrap().get(bucket).map(|(r, _)| r.clone()).unwrap_or_else(|| self.region.clone())
    }

    /// Run a request for `bucket`. If the service says the bucket is in
    /// another region, the request goes there again (S3-08). The SDK's own
    /// error type is returned as is, large as it is.
    #[allow(clippy::result_large_err)]
    fn call<T, E, F, Fut>(&self, bucket: &str, request: F) -> Result<T, SdkError<E>>
    where
        F: Fn(Client) -> Fut,
        Fut: Future<Output = Result<T, SdkError<E>>>,
    {
        match self.block(request(self.client(bucket))) {
            Err(error) => match bucket_region(&error).filter(|r| *r != self.region_of(bucket) && !bucket.is_empty()) {
                Some(region) => {
                    let client =
                        Client::from_conf(self.config.to_builder().region(Region::new(region.clone())).build());
                    self.regional.lock().unwrap().insert(bucket.to_string(), (region, client.clone()));
                    self.block(request(client))
                }
                None => Err(error),
            },
            ok => ok,
        }
    }

    /// Every key under `prefix` in `bucket`, however many pages it takes.
    fn all_keys(&self, bucket: &str, prefix: &str, path: &str) -> VfsResult<Vec<String>> {
        let mut keys = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self
                .call(bucket, |c| {
                    let mut request = c.list_objects_v2().bucket(bucket).prefix(prefix);
                    if let Some(token) = &token {
                        request = request.continuation_token(token);
                    }
                    request.send()
                })
                .map_err(|e| vfs_error(path, e))?;
            keys.extend(page.contents().iter().filter_map(|o| o.key().map(str::to_string)));
            match (page.is_truncated(), page.next_continuation_token()) {
                (Some(true), Some(next)) => token = Some(next.to_string()),
                _ => return Ok(keys),
            }
        }
    }

    /// Server-side copy of one object.
    fn copy_object(&self, from: (&str, &str), to: (&str, &str), path: &str) -> VfsResult<()> {
        let source = encode_source(from.0, from.1);
        self.call(to.0, |c| c.copy_object().copy_source(&source).bucket(to.0).key(to.1).send())
            .map(|_| ())
            .map_err(|e| vfs_error(path, e))
    }

    /// Delete many keys, in batches where the service has them.
    fn delete_keys(&self, bucket: &str, keys: &[String], path: &str) -> VfsResult<()> {
        for batch in keys.chunks(1000) {
            let ids: Vec<ObjectIdentifier> =
                batch.iter().filter_map(|k| ObjectIdentifier::builder().key(k).build().ok()).collect();
            let Ok(delete) = Delete::builder().set_objects(Some(ids)).quiet(true).build() else { continue };
            if self.call(bucket, |c| c.delete_objects().bucket(bucket).delete(delete.clone()).send()).is_err() {
                // No batch delete here: one by one.
                for key in batch {
                    self.call(bucket, |c| c.delete_object().bucket(bucket).key(key).send())
                        .map_err(|e| vfs_error(path, e))?;
                }
            }
        }
        Ok(())
    }

    /// True if anything at all lives under `key/`.
    fn is_folder(&self, bucket: &str, key: &str) -> bool {
        let prefix = format!("{key}/");
        self.call(bucket, |c| c.list_objects_v2().bucket(bucket).prefix(&prefix).max_keys(1).send())
            .is_ok_and(|page| !page.contents().is_empty() || !page.common_prefixes().is_empty())
    }

    /// The object's storage class, as the service reports it.
    pub fn storage_class(&self, path: &str) -> Option<String> {
        let (bucket, key) = split(path);
        let head = self.call(bucket, |c| c.head_object().bucket(bucket).key(key).send()).ok()?;
        Some(head.storage_class().map_or("STANDARD".to_string(), |s| s.as_str().to_string()))
    }
}

impl Vfs for S3Fs {
    fn id(&self) -> String {
        format!("s3://{}", self.label)
    }

    fn list(&self, path: &str) -> VfsResult<Vec<Entry>> {
        let (bucket, key) = split(path);
        if bucket.is_empty() {
            let buckets = self.block(self.client.list_buckets().send()).map_err(|e| vfs_error(path, e))?;
            return Ok(buckets
                .buckets()
                .iter()
                .filter_map(|b| Some(dir_entry(b.name()?, b.creation_date().and_then(|d| d.to_millis().ok()))))
                .collect());
        }
        let prefix = if key.is_empty() { String::new() } else { format!("{key}/") };
        let mut entries = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self
                .call(bucket, |c| {
                    let mut request = c.list_objects_v2().bucket(bucket).delimiter("/");
                    if !prefix.is_empty() {
                        request = request.prefix(&prefix);
                    }
                    if let Some(token) = &token {
                        request = request.continuation_token(token);
                    }
                    request.send()
                })
                .map_err(|e| vfs_error(path, e))?;
            for common in page.common_prefixes() {
                if let Some(name) = common.prefix().and_then(|p| p.strip_prefix(prefix.as_str())) {
                    let name = name.trim_end_matches('/');
                    if !name.is_empty() {
                        entries.push(dir_entry(name, None));
                    }
                }
            }
            for object in page.contents() {
                let Some(name) = object.key().and_then(|k| k.strip_prefix(prefix.as_str())) else { continue };
                // The folder's own marker is the folder, not a file in it.
                if name.is_empty() {
                    continue;
                }
                let modified = object.last_modified().and_then(|d| d.to_millis().ok());
                entries.push(file_entry(name, object.size().unwrap_or(0), modified));
            }
            match (page.is_truncated(), page.next_continuation_token()) {
                (Some(true), Some(next)) => token = Some(next.to_string()),
                _ => break,
            }
        }
        // A folder that does not exist lists as empty; say so instead.
        if entries.is_empty() && !key.is_empty() && !self.is_folder(bucket, key) {
            return Err(VfsError::NotFound(path.to_string()));
        }
        Ok(entries)
    }

    fn stat(&self, path: &str) -> VfsResult<Entry> {
        let (bucket, key) = split(path);
        if bucket.is_empty() {
            return Ok(dir_entry("", None));
        }
        if key.is_empty() {
            self.call(bucket, |c| c.head_bucket().bucket(bucket).send()).map_err(|e| vfs_error(path, e))?;
            return Ok(dir_entry(bucket, None));
        }
        let name = paths::file_name(path);
        match self.call(bucket, |c| c.head_object().bucket(bucket).key(key).send()) {
            Ok(head) => Ok(file_entry(
                name,
                head.content_length().unwrap_or(0),
                head.last_modified().and_then(|d| d.to_millis().ok()),
            )),
            Err(_) if self.is_folder(bucket, key) => Ok(dir_entry(name, None)),
            Err(error) => Err(vfs_error(path, error)),
        }
    }

    fn mkdir(&self, path: &str) -> VfsResult<()> {
        let (bucket, key) = split(path);
        if key.is_empty() {
            return self
                .block(self.client.create_bucket().bucket(bucket).send())
                .map(|_| ())
                .map_err(|e| vfs_error(path, e));
        }
        if self.stat(path).is_ok() {
            return Err(VfsError::AlreadyExists(path.to_string()));
        }
        // An empty object named "folder/" is the folder.
        let marker = format!("{key}/");
        self.call(bucket, |c| c.put_object().bucket(bucket).key(&marker).body(ByteStream::from_static(b"")).send())
            .map(|_| ())
            .map_err(|e| vfs_error(path, e))
    }

    fn rename(&self, from: &str, to: &str, replace: bool) -> VfsResult<()> {
        if !replace && self.stat(to).is_ok() {
            return Err(VfsError::AlreadyExists(to.to_string()));
        }
        let (from_bucket, from_key) = split(from);
        let (to_bucket, to_key) = split(to);
        if from_key.is_empty() || to_key.is_empty() {
            return Err(VfsError::Invalid("A bucket can’t be renamed.".into()));
        }
        if self.stat(from)?.is_dir {
            // Every object under the folder is copied, then deleted.
            let prefix = format!("{from_key}/");
            let keys = self.all_keys(from_bucket, &prefix, from)?;
            for key in &keys {
                let target = format!("{to_key}/{}", &key[prefix.len()..]);
                self.copy_object((from_bucket, key), (to_bucket, &target), from)?;
            }
            if keys.is_empty() {
                let marker = format!("{to_key}/");
                self.call(to_bucket, |c| {
                    c.put_object().bucket(to_bucket).key(&marker).body(ByteStream::from_static(b"")).send()
                })
                .map_err(|e| vfs_error(to, e))?;
            }
            self.delete_keys(from_bucket, &keys, from)
        } else {
            self.copy_object((from_bucket, from_key), (to_bucket, to_key), from)?;
            self.remove_file(from)
        }
    }

    fn remove_file(&self, path: &str) -> VfsResult<()> {
        let (bucket, key) = split(path);
        self.call(bucket, |c| c.delete_object().bucket(bucket).key(key).send())
            .map(|_| ())
            .map_err(|e| vfs_error(path, e))
    }

    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        let (bucket, key) = split(path);
        if key.is_empty() {
            return self
                .call(bucket, |c| c.delete_bucket().bucket(bucket).send())
                .map(|_| ())
                .map_err(|e| vfs_error(path, e));
        }
        // The engine has emptied the folder; only its marker may remain.
        let marker = format!("{key}/");
        self.call(bucket, |c| c.delete_object().bucket(bucket).key(&marker).send())
            .map(|_| ())
            .map_err(|e| vfs_error(path, e))
    }

    fn open_read(&self, path: &str) -> VfsResult<Box<dyn Read + Send>> {
        self.open_read_at(path, 0)
    }

    fn resume_support(&self) -> (bool, bool) {
        // Downloads continue with an HTTP Range; uploads cannot append.
        (true, false)
    }

    fn open_read_at(&self, path: &str, offset: u64) -> VfsResult<Box<dyn Read + Send>> {
        let (bucket, key) = split(path);
        let object = self
            .call(bucket, |c| {
                let request = c.get_object().bucket(bucket).key(key);
                if offset > 0 {
                    request.range(format!("bytes={offset}-")).send()
                } else {
                    request.send()
                }
            })
            .map_err(|e| vfs_error(path, e))?;
        Ok(Box::new(Download { handle: self.runtime.handle().clone(), body: object.body, chunk: Vec::new(), at: 0 }))
    }

    fn create(&self, path: &str) -> VfsResult<Box<dyn Write + Send>> {
        if self.stat(path).is_ok() {
            return Err(VfsError::AlreadyExists(path.to_string()));
        }
        let (bucket, key) = split(path);
        if key.is_empty() {
            return Err(VfsError::Invalid("Files go inside a bucket, not beside the buckets.".into()));
        }
        Ok(Box::new(Upload {
            // `stat` above has learned the bucket's region if it differs.
            client: self.client(bucket),
            handle: self.runtime.handle().clone(),
            bucket: bucket.to_string(),
            key: key.to_string(),
            content_type: content_type(key),
            storage_class: self.storage_class.clone(),
            part_size: self.part_size,
            buffer: Vec::new(),
            upload_id: None,
            parts: Vec::new(),
            finished: false,
        }))
    }

    fn copy_within(&self, from: &str, to: &str, _progress: &mut dyn FnMut(u64) -> bool) -> VfsResult<bool> {
        if self.stat(to).is_ok() {
            return Err(VfsError::AlreadyExists(to.to_string()));
        }
        self.copy_object(split(from), split(to), from).map(|()| true)
    }

    fn nfc_names(&self) -> bool {
        true
    }

    fn details(&self, path: &str) -> Vec<(String, String)> {
        let mut facts = Vec::new();
        if let Some(class) = self.storage_class(path) {
            facts.push(("Storage Class".to_string(), class));
        }
        if let Some(url) = self.url_for(path) {
            facts.push(("URL".to_string(), url));
        }
        facts
    }

    /// `https://bucket.s3.region.amazonaws.com/key`, or the compatible
    /// service's form; never with keys or signatures.
    fn url_for(&self, path: &str) -> Option<String> {
        let (bucket, key) = split(path);
        if bucket.is_empty() {
            return None;
        }
        let key = crate::webdav::encode_path(key);
        if self.endpoint.is_empty() {
            let region = self.region_of(bucket);
            return Some(if self.path_style || !dns_compatible(bucket) {
                format!("https://s3.{region}.amazonaws.com/{bucket}/{key}")
            } else {
                format!("https://{bucket}.s3.{region}.amazonaws.com/{key}")
            });
        }
        let (scheme, host) = self.endpoint.split_once("://").unwrap_or(("https", &self.endpoint));
        Some(if self.path_style {
            format!("{scheme}://{host}/{bucket}/{key}")
        } else {
            format!("{scheme}://{bucket}.{host}/{key}")
        })
    }
}

/// A download: the body arrives in chunks, handed on as they are read.
struct Download {
    handle: Handle,
    body: ByteStream,
    chunk: Vec<u8>,
    at: usize,
}

impl Read for Download {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.at >= self.chunk.len() {
            match self.handle.block_on(self.body.next()) {
                Some(Ok(bytes)) => {
                    self.chunk = bytes.to_vec();
                    self.at = 0;
                }
                Some(Err(e)) => return Err(std::io::Error::other(e.to_string())),
                None => return Ok(0),
            }
        }
        let n = buffer.len().min(self.chunk.len() - self.at);
        buffer[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// An upload. Small files go up in one request when flushed. Larger ones
/// go up in parts as they are written; a part upload that is dropped
/// before the end is aborted, so nothing half-finished stays on the
/// service.
struct Upload {
    client: Client,
    handle: Handle,
    bucket: String,
    key: String,
    content_type: &'static str,
    storage_class: Option<StorageClass>,
    part_size: usize,
    buffer: Vec<u8>,
    upload_id: Option<String>,
    parts: Vec<CompletedPart>,
    finished: bool,
}

impl Upload {
    fn io<E: ProvideErrorMetadata + std::error::Error + 'static>(&self, error: SdkError<E>) -> std::io::Error {
        std::io::Error::other(vfs_error(&format!("/{}/{}", self.bucket, self.key), error).to_string())
    }

    /// Send what is buffered as the next part, starting the multipart
    /// upload if this is the first.
    fn send_part(&mut self) -> std::io::Result<()> {
        let id = match &self.upload_id {
            Some(id) => id.clone(),
            None => {
                let request = self
                    .client
                    .create_multipart_upload()
                    .bucket(&self.bucket)
                    .key(&self.key)
                    .content_type(self.content_type)
                    .set_storage_class(self.storage_class.clone());
                let started = self.handle.block_on(request.send()).map_err(|e| self.io(e))?;
                let id = started.upload_id().unwrap_or_default().to_string();
                self.upload_id = Some(id.clone());
                id
            }
        };
        let number = self.parts.len() as i32 + 1;
        let body = ByteStream::from(std::mem::take(&mut self.buffer));
        let request =
            self.client.upload_part().bucket(&self.bucket).key(&self.key).upload_id(&id).part_number(number).body(body);
        let sent = self.handle.block_on(request.send()).map_err(|e| self.io(e))?;
        self.parts
            .push(CompletedPart::builder().part_number(number).set_e_tag(sent.e_tag().map(str::to_string)).build());
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        if self.finished {
            return Ok(());
        }
        match self.upload_id.clone() {
            None => {
                let body = ByteStream::from(std::mem::take(&mut self.buffer));
                let request = self
                    .client
                    .put_object()
                    .bucket(&self.bucket)
                    .key(&self.key)
                    .content_type(self.content_type)
                    .set_storage_class(self.storage_class.clone())
                    .body(body);
                self.handle.block_on(request.send()).map_err(|e| self.io(e))?;
            }
            Some(id) => {
                if !self.buffer.is_empty() {
                    self.send_part()?;
                }
                let parts =
                    CompletedMultipartUpload::builder().set_parts(Some(std::mem::take(&mut self.parts))).build();
                let request = self
                    .client
                    .complete_multipart_upload()
                    .bucket(&self.bucket)
                    .key(&self.key)
                    .upload_id(id)
                    .multipart_upload(parts);
                self.handle.block_on(request.send()).map_err(|e| self.io(e))?;
            }
        }
        self.finished = true;
        Ok(())
    }
}

impl Write for Upload {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.finished {
            return Err(std::io::Error::other("the upload has already ended"));
        }
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() >= self.part_size {
            self.send_part()?;
        }
        Ok(bytes.len())
    }

    /// The copy engine flushes after the last byte: that completes the
    /// upload and reports what the service said.
    fn flush(&mut self) -> std::io::Result<()> {
        self.finish()
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        // Stopped or failed mid-way: leave no orphaned parts behind.
        if let (false, Some(id)) = (self.finished, self.upload_id.take()) {
            let request = self.client.abort_multipart_upload().bucket(&self.bucket).key(&self.key).upload_id(id);
            let _ = self.handle.block_on(request.send());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_into_bucket_and_key() {
        assert_eq!(split("/"), ("", ""));
        assert_eq!(split("/photos"), ("photos", ""));
        assert_eq!(split("/photos/2024/a b.jpg"), ("photos", "2024/a b.jpg"));
        assert_eq!(split("/photos/2024/"), ("photos", "2024"));
    }

    #[test]
    fn content_types_come_from_the_extension() {
        // S3-06
        assert_eq!(content_type("photo.jpg"), "image/jpeg");
        assert_eq!(content_type("PHOTO.JPEG"), "image/jpeg");
        assert_eq!(content_type("dir/report.pdf"), "application/pdf");
        assert_eq!(content_type("archive.tar.gz"), "application/gzip");
        assert_eq!(content_type("unknown.xyz"), "application/octet-stream");
        assert_eq!(content_type("noextension"), "application/octet-stream");
    }

    #[test]
    fn copy_sources_are_percent_encoded() {
        assert_eq!(encode_source("my-bucket", "old/a b+c.txt"), "my-bucket/old/a%20b%2Bc.txt");
        assert_eq!(encode_source("b", "résumé"), "b/r%C3%A9sum%C3%A9");
    }

    #[test]
    fn missing_keys_are_named() {
        let none = S3Params::default();
        assert!(matches!(connect(none), Err(RemoteError::Auth(m)) if m == "Missing access key."));
        let half = S3Params { access_key: "AK".into(), ..S3Params::default() };
        assert!(matches!(connect(half), Err(RemoteError::Auth(m)) if m == "Missing secret key."));
    }
}
