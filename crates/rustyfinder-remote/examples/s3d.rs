//! Runs the scripted S3 test server, for the app-level checks:
//!
//!     cargo run -p rustyfinder-remote --example s3d
//!
//! It starts with a bucket "photos" holding `hello.txt` and a folder
//! `docs/`, and an empty bucket "backups". Prints the port it listens on,
//! then serves until killed. Any keys are accepted. Not for real use.

#[path = "../tests/support/s3d.rs"]
mod s3d;

use std::sync::{Arc, Mutex};

fn main() {
    let state: s3d::Shared = Arc::new(Mutex::new(s3d::State::default()));
    {
        let mut st = state.lock().unwrap();
        let photos = st.buckets.entry("photos".into()).or_default();
        photos.insert(
            "hello.txt".into(),
            s3d::Object { data: b"remote hello".to_vec(), content_type: "text/plain".into(), ..Default::default() },
        );
        photos.insert("docs/".into(), s3d::Object::default());
        st.buckets.entry("backups".into()).or_default();
    }
    let port = s3d::start(state);
    println!("{port}");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
