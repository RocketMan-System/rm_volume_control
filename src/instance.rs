//! Single instance guard over a loopback TCP port: a second launch asks the running
//! instance to show its window and exits.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

const ADDR: &str = "127.0.0.1:47613";
const SHOW: &[u8] = b"show";

/// Returns the listener if this is the only instance. With `wait` (used right after a
/// self-update) it keeps retrying while the previous process is shutting down.
pub fn acquire(wait: bool) -> Option<TcpListener> {
    let attempts = if wait { 50 } else { 1 };
    for _ in 0..attempts {
        if let Ok(listener) = TcpListener::bind(ADDR) {
            return Some(listener);
        }
        if wait {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    if let Ok(mut stream) = TcpStream::connect_timeout(&ADDR.parse().unwrap(), Duration::from_secs(1)) {
        let _ = stream.write_all(SHOW);
        return None;
    }
    // The port is taken by something else: better to run twice than not at all.
    TcpListener::bind("127.0.0.1:0").ok()
}

pub fn listen(listener: TcpListener, on_show: impl Fn() + Send + 'static) {
    std::thread::Builder::new()
        .name("instance".into())
        .spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut buf = [0u8; 8];
                if let Ok(n) = stream.read(&mut buf) {
                    if &buf[..n] == SHOW {
                        on_show();
                    }
                }
            }
        })
        .expect("failed to spawn instance thread");
}
