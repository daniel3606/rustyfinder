//! Runs the scripted FTP test server on a folder, for the app-level checks:
//!
//!     cargo run -p rustyfinder-remote --example ftpd -- <folder> [explicit|implicit]
//!
//! With "explicit" or "implicit" it speaks TLS with the self-signed test
//! certificate.
//! Prints the port it listens on, then serves until killed. User "bob",
//! password "secret". Not for real use.

#[path = "../tests/support/ftpd.rs"]
mod ftpd;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

fn main() {
    let root = std::env::args().nth(1).expect("usage: ftpd <folder> [explicit|implicit]");
    let tls = match std::env::args().nth(2).as_deref() {
        Some("explicit") => Some(ftpd::Tls::Explicit),
        Some("implicit") => Some(ftpd::Tls::Implicit),
        _ => None,
    };
    let port = ftpd::start(
        root.into(),
        ftpd::Config { tls, ..ftpd::Config::default() },
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(AtomicBool::new(false)),
    );
    println!("{port}");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
