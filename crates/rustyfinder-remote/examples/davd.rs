//! Runs the scripted WebDAV test server on a folder, for the app-level
//! checks:
//!
//!     cargo run -p rustyfinder-remote --example davd -- <folder>
//!
//! Prints the port it listens on, then serves until killed. User "bob",
//! password "secret" (Basic). Not for real use.

#[path = "../tests/support/davd.rs"]
mod davd;

use std::sync::{Arc, Mutex};

fn main() {
    let root = std::env::args().nth(1).expect("usage: davd <folder>");
    let port = davd::start(root.into(), davd::Config::default(), Arc::new(Mutex::new(Vec::new())));
    println!("{port}");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
