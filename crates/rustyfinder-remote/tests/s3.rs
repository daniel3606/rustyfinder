//! The S3 backend against a scripted S3 server running in this process
//! (see `support/s3d.rs`).

#[path = "support/s3d.rs"]
mod s3d;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use rustyfinder_core::{Vfs, VfsError};
use rustyfinder_remote::s3::{connect, S3Fs, S3Params};
use rustyfinder_remote::RemoteError;
use s3d::{Object, Shared, State};

struct Server {
    port: u16,
    state: Shared,
}

impl Server {
    /// A server holding bucket "photos" with the given objects.
    fn start(objects: &[(&str, &str)]) -> Server {
        let state: Shared = Arc::new(Mutex::new(State::default()));
        {
            let mut st = state.lock().unwrap();
            let bucket = st.buckets.entry("photos".into()).or_default();
            for (key, data) in objects {
                bucket.insert(
                    key.to_string(),
                    Object {
                        data: data.as_bytes().to_vec(),
                        content_type: "binary/octet-stream".into(),
                        ..Default::default()
                    },
                );
            }
            st.buckets.entry("backups".into()).or_default();
        }
        let port = s3d::start(Arc::clone(&state));
        Server { port, state }
    }

    fn params(&self) -> S3Params {
        S3Params {
            access_key: "test-access".into(),
            secret_key: "test-secret".into(),
            endpoint: format!("http://127.0.0.1:{}", self.port),
            ..S3Params::default()
        }
    }

    fn connect(&self) -> S3Fs {
        connect(self.params()).expect("connect")
    }

    fn keys(&self, bucket: &str) -> Vec<String> {
        self.state.lock().unwrap().buckets[bucket].keys().cloned().collect()
    }

    fn object(&self, bucket: &str, key: &str) -> Object {
        self.state.lock().unwrap().buckets[bucket][key].clone()
    }

    fn uploads_in_progress(&self) -> usize {
        self.state.lock().unwrap().uploads.len()
    }

    fn sent(&self, start: &str) -> bool {
        self.state.lock().unwrap().log.iter().any(|line| line.starts_with(start))
    }
}

fn names(fs: &dyn Vfs, dir: &str) -> Vec<String> {
    let mut names: Vec<String> = fs.list(dir).unwrap().into_iter().map(|e| e.name).collect();
    names.sort();
    names
}

fn download(fs: &dyn Vfs, path: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    fs.open_read(path).unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

fn upload(fs: &dyn Vfs, path: &str, content: &[u8]) {
    let mut writer = fs.create(path).unwrap();
    writer.write_all(content).unwrap();
    writer.flush().unwrap();
}

#[test]
fn s3_01_the_top_lists_buckets_and_a_bucket_lists_its_objects() {
    let server = Server::start(&[("hello.txt", "remote hello"), ("2024/trip/a.jpg", "jpg")]);
    let s3 = server.connect();
    assert_eq!((s3.home(), s3.label()), ("/", format!("127.0.0.1:{}", server.port).as_str()));
    assert_eq!(names(&s3, "/"), ["backups", "photos"]);
    assert!(s3.list("/").unwrap().iter().all(|e| e.is_dir));
    assert_eq!(names(&s3, "/photos"), ["2024", "hello.txt"]);
    assert_eq!(names(&s3, "/photos/2024"), ["trip"]);
    assert_eq!(names(&s3, "/photos/2024/trip"), ["a.jpg"]);
    assert!(matches!(s3.list("/photos/nope"), Err(VfsError::NotFound(_))));
    assert!(matches!(s3.list("/no-such-bucket"), Err(VfsError::NotFound(_))));

    let file = s3.stat("/photos/hello.txt").unwrap();
    assert_eq!((file.size, file.is_dir, file.modified_ms), (12, false, Some(1_577_836_800_000)));
    assert!(s3.stat("/photos/2024").unwrap().is_dir, "a prefix is a folder");
    assert!(s3.stat("/photos").unwrap().is_dir && s3.stat("/").unwrap().is_dir);
    assert!(matches!(s3.stat("/photos/nope"), Err(VfsError::NotFound(_))));
    assert_eq!(download(&s3, "/photos/hello.txt"), b"remote hello");
}

#[test]
fn opening_inside_a_bucket_starts_there() {
    let server = Server::start(&[("a.txt", "a")]);
    let params = S3Params { bucket: "photos".into(), ..server.params() };
    let s3 = connect(params).unwrap();
    assert_eq!(s3.home(), "/photos");
    assert_eq!(names(&s3, s3.home()), ["a.txt"]);
    let missing = S3Params { bucket: "nope".into(), ..server.params() };
    assert!(connect(missing).is_err());
}

#[test]
fn s3_02_listings_longer_than_one_page_are_complete() {
    let server = Server::start(&[]);
    {
        let mut st = server.state.lock().unwrap();
        let bucket = st.buckets.get_mut("photos").unwrap();
        for i in 0..2500 {
            bucket.insert(format!("many/item-{i:04}.dat"), Object::default());
        }
    }
    let s3 = server.connect();
    assert_eq!(s3.list("/photos/many").unwrap().len(), 2500);
    assert!(
        server.state.lock().unwrap().log.iter().any(|l| l.contains("continuation-token")),
        "more than one page was fetched"
    );
}

#[test]
fn s3_03_a_folder_marker_is_the_folder_and_never_a_nameless_file() {
    let server = Server::start(&[("docs/", ""), ("docs/a.txt", "a"), ("empty/", "")]);
    let s3 = server.connect();
    let top = s3.list("/photos").unwrap();
    assert_eq!(names(&s3, "/photos"), ["docs", "empty"]);
    assert!(top.iter().all(|e| e.is_dir && !e.name.is_empty()));
    assert_eq!(names(&s3, "/photos/docs"), ["a.txt"]);
    assert!(s3.list("/photos/empty").unwrap().is_empty(), "a folder with only its marker is empty, not missing");
}

#[test]
fn uploads_set_the_content_type_and_small_files_go_in_one_request() {
    // S3-06
    let server = Server::start(&[]);
    let s3 = server.connect();
    upload(&s3, "/photos/trip/photo.jpg", b"jpeg bytes");
    let object = server.object("photos", "trip/photo.jpg");
    assert_eq!((object.data.as_slice(), object.content_type.as_str()), (&b"jpeg bytes"[..], "image/jpeg"));
    upload(&s3, "/photos/data.unknownext", b"x");
    assert_eq!(server.object("photos", "data.unknownext").content_type, "application/octet-stream");
    assert!(!server.sent("POST /photos/trip/photo.jpg?uploads"), "no multipart for a small file");
    assert!(matches!(s3.create("/photos/data.unknownext"), Err(VfsError::AlreadyExists(_))));
    assert!(s3.create("/loose.txt").is_err(), "files live in buckets");
}

#[test]
fn s3_04_a_large_upload_goes_in_parts_and_leaves_nothing_unfinished() {
    let server = Server::start(&[]);
    let params = S3Params { part_size: Some(1000), ..server.params() };
    let s3 = connect(params).unwrap();
    let content: Vec<u8> = (0..4500u32).map(|i| (i % 251) as u8).collect();
    let mut writer = s3.create("/photos/big.png").unwrap();
    for chunk in content.chunks(700) {
        writer.write_all(chunk).unwrap();
    }
    writer.flush().unwrap();
    let object = server.object("photos", "big.png");
    assert_eq!(object.data, content, "the parts were put together in order");
    assert_eq!(object.content_type, "image/png", "multipart uploads carry the type too");
    assert_eq!(server.uploads_in_progress(), 0);
    assert!(server.sent("POST /photos/big.png?uploads") && server.sent("POST /photos/big.png?uploadId="));
}

#[test]
fn s3_05_a_stopped_large_upload_is_aborted_on_the_server() {
    let server = Server::start(&[]);
    let params = S3Params { part_size: Some(1000), ..server.params() };
    let s3 = connect(params).unwrap();
    {
        let mut writer = s3.create("/photos/never.bin").unwrap();
        writer.write_all(&[1u8; 2500]).unwrap();
        assert_eq!(server.uploads_in_progress(), 1);
        // Dropped without being flushed: the copy was stopped.
    }
    assert_eq!(server.uploads_in_progress(), 0);
    assert!(!server.keys("photos").contains(&"never.bin".to_string()));
    let log = server.state.lock().unwrap().log.clone();
    assert!(log.iter().any(|l| l.starts_with("DELETE /photos/never.bin?") && l.contains("uploadId=")), "{log:?}");
}

#[test]
fn s3_07_renaming_a_folder_moves_every_object_under_it() {
    let server = Server::start(&[("old/a.txt", "a"), ("old/sub/b.txt", "b"), ("old/c d.txt", "c"), ("other.txt", "o")]);
    let s3 = server.connect();
    s3.rename("/photos/old", "/photos/new", false).unwrap();
    assert_eq!(server.keys("photos"), ["new/a.txt", "new/c d.txt", "new/sub/b.txt", "other.txt"]);
    assert_eq!(server.object("photos", "new/sub/b.txt").data, b"b");

    // A file: copy then delete, refused if the name is taken.
    assert!(matches!(s3.rename("/photos/other.txt", "/photos/new/a.txt", false), Err(VfsError::AlreadyExists(_))));
    s3.rename("/photos/other.txt", "/photos/moved.txt", false).unwrap();
    assert_eq!(server.object("photos", "moved.txt").data, b"o");
    assert!(!server.keys("photos").contains(&"other.txt".to_string()));
    // Across buckets too.
    s3.rename("/photos/moved.txt", "/backups/moved.txt", false).unwrap();
    assert_eq!(server.keys("backups"), ["moved.txt"]);
}

#[test]
fn folders_buckets_copies_and_deletes() {
    let server = Server::start(&[("a.txt", "copy me")]);
    let s3 = server.connect();
    s3.mkdir("/photos/new folder").unwrap();
    assert!(server.keys("photos").contains(&"new folder/".to_string()), "a folder is a zero-byte marker");
    assert!(matches!(s3.mkdir("/photos/new folder"), Err(VfsError::AlreadyExists(_))));
    assert!(s3.stat("/photos/new folder").unwrap().is_dir);

    assert!(s3.copy_within("/photos/a.txt", "/photos/a copy.txt", &mut |_| true).unwrap());
    assert_eq!(server.object("photos", "a copy.txt").data, b"copy me");
    assert!(matches!(
        s3.copy_within("/photos/a.txt", "/photos/a copy.txt", &mut |_| true),
        Err(VfsError::AlreadyExists(_))
    ));

    s3.remove_file("/photos/a copy.txt").unwrap();
    s3.remove_dir("/photos/new folder").unwrap();
    assert_eq!(server.keys("photos"), ["a.txt"]);

    s3.mkdir("/fresh-bucket").unwrap();
    assert_eq!(names(&s3, "/"), ["backups", "fresh-bucket", "photos"]);
    s3.remove_dir("/fresh-bucket").unwrap();
    assert_eq!(names(&s3, "/"), ["backups", "photos"]);
}

#[test]
fn s3_08_a_bucket_in_another_region_is_asked_there() {
    let server = Server::start(&[("a.txt", "far away")]);
    server.state.lock().unwrap().regions.insert("photos".into(), "eu-west-1".into());
    let s3 = server.connect(); // the connection's region is us-east-1
    assert_eq!(names(&s3, "/photos"), ["a.txt"]);
    assert_eq!(download(&s3, "/photos/a.txt"), b"far away");
    upload(&s3, "/photos/b.txt", b"also there");
    assert_eq!(server.object("photos", "b.txt").data, b"also there");
    assert_eq!(s3.region_of("photos"), "eu-west-1");
    let log = server.state.lock().unwrap().log.clone();
    assert!(log.iter().any(|l| l.ends_with("region=us-east-1")), "the first try went to the connection's region");
    assert!(log.iter().any(|l| l.ends_with("region=eu-west-1")), "then to the bucket's");
    // Opening inside such a bucket works too.
    let inside = connect(S3Params { bucket: "photos".into(), ..server.params() }).expect("connect inside");
    assert_eq!(names(&inside, "/photos").len(), 2);
}

#[test]
fn uploads_carry_the_chosen_storage_class() {
    let server = Server::start(&[]);
    let s3 = connect(S3Params { storage_class: Some("STANDARD_IA".into()), ..server.params() }).unwrap();
    upload(&s3, "/photos/cold.txt", b"rarely read");
    assert_eq!(server.object("photos", "cold.txt").storage_class, "STANDARD_IA");
    assert_eq!(s3.storage_class("/photos/cold.txt").as_deref(), Some("STANDARD_IA"));
    let plain = server.connect();
    upload(&plain, "/photos/warm.txt", b"x");
    assert_eq!(server.object("photos", "warm.txt").storage_class, "");
    assert_eq!(plain.storage_class("/photos/warm.txt").as_deref(), Some("STANDARD"));
    // Get Info shows it.
    let details = s3.details("/photos/cold.txt");
    assert!(details.contains(&("Storage Class".to_string(), "STANDARD_IA".to_string())), "{details:?}");
}

#[test]
fn a_download_resumes_with_a_range() {
    let server = Server::start(&[]);
    let data: Vec<u8> = (0..50_000u32).map(|i| (i % 249) as u8).collect();
    server.state.lock().unwrap().buckets.get_mut("photos").unwrap().insert(
        "big.bin".into(),
        Object { data: data.clone(), content_type: "application/octet-stream".into(), ..Default::default() },
    );
    let s3 = server.connect();
    assert_eq!(s3.resume_support(), (true, false));
    let mut rest = Vec::new();
    s3.open_read_at("/photos/big.bin", 20_000).unwrap().read_to_end(&mut rest).unwrap();
    assert_eq!(rest, &data[20_000..]);
}

#[test]
fn copy_url_has_no_credentials_and_follows_the_addressing() {
    let server = Server::start(&[]);
    let path_style = server.connect();
    assert_eq!(
        path_style.url_for("/photos/2024/a b.jpg").unwrap(),
        format!("http://127.0.0.1:{}/photos/2024/a%20b.jpg", server.port)
    );
    let hosted = connect(S3Params { path_style: Some(false), ..server.params() });
    // The scripted server only knows path style, so connecting may fail;
    // the address form is what matters here.
    if let Ok(hosted) = hosted {
        assert!(hosted.url_for("/photos/a.jpg").unwrap().starts_with("http://photos.127.0.0.1"));
    }
    let url = path_style.url_for("/photos/x").unwrap();
    assert!(!url.contains("test-access") && !url.contains("test-secret"));
    assert_eq!(path_style.url_for("/"), None);
}

#[test]
fn nothing_listening_is_an_unreachable_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let params = S3Params {
        access_key: "a".into(),
        secret_key: "s".into(),
        endpoint: format!("http://127.0.0.1:{port}"),
        ..S3Params::default()
    };
    assert!(matches!(connect(params), Err(RemoteError::Unreachable(_))));
}
