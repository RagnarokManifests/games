//! One Ragnarok at a time.
//!
//! The X hides the window to the tray instead of quitting — that is what keeps
//! the achievement popups and the Cloud-Sync watcher alive. But opening
//! Ragnarok again from the desktop then started a whole second copy beside the
//! hidden one, and a user reported a row of identical tray icons, one per time
//! they had opened it. Every copy also ran its own watchers against the same
//! Steam folder.
//!
//! The first copy listens on a loopback port. A later one connects, says
//! hello, and when the first answers it asks it to show its window and exits.
//! Loopback TCP rather than a named mutex so the same code works in the Linux
//! build, and so the waiting copy gets a real answer: a port taken by some
//! unrelated program never answers the handshake, and Ragnarok then simply
//! opens as it always did instead of refusing to start.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::Manager;

const PORT: u16 = 47613;
const HELLO: &str = "RAGNAROK_SHOW";
const ACK: &str = "RAGNAROK_OK";

static HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

pub enum Startup {
    /// No other copy is running; this one keeps the port.
    First(TcpListener),
    /// Another copy answered and is showing its window.
    AlreadyRunning,
    /// The port is held by something that is not Ragnarok.
    Unguarded,
}

pub fn claim() -> Startup {
    claim_on(PORT, Duration::from_secs(3))
}

fn claim_on(port: u16, patience: Duration) -> Startup {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpListener::bind(addr) {
        Ok(listener) => Startup::First(listener),
        Err(_) if ask_to_show(addr, patience) => Startup::AlreadyRunning,
        Err(_) => Startup::Unguarded,
    }
}

fn ask_to_show(addr: SocketAddr, patience: Duration) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, patience) else { return false };
    let _ = stream.set_read_timeout(Some(patience));
    if stream.write_all(format!("{HELLO}\n").as_bytes()).is_err() {
        return false;
    }
    let mut reply = String::new();
    let _ = BufReader::new(stream.take(64)).read_line(&mut reply);
    reply.trim() == ACK
}

/// Answers later copies for the life of the process. Started before the
/// window exists, so a copy opened during startup gets its answer at once
/// instead of timing out and opening a second window.
pub fn serve(listener: TcpListener) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut hello = String::new();
            let Ok(clone) = stream.try_clone() else { continue };
            let _ = BufReader::new(clone.take(64)).read_line(&mut hello);
            if hello.trim() != HELLO {
                continue;
            }
            let _ = stream.write_all(format!("{ACK}\n").as_bytes());
            bring_to_front();
        }
    });
}

/// Called once the app exists, so a request can reach the window.
pub fn attach(handle: tauri::AppHandle) {
    let _ = HANDLE.set(handle);
}

fn bring_to_front() {
    let Some(window) = HANDLE.get().and_then(|h| h.get_window("main")) else { return };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
    }

    #[test]
    fn a_second_copy_hands_over_to_the_first() {
        let port = free_port();
        let Startup::First(listener) = claim_on(port, Duration::from_secs(2)) else {
            panic!("la primera copia tiene que quedarse con el puerto");
        };
        serve(listener);
        assert!(matches!(claim_on(port, Duration::from_secs(2)), Startup::AlreadyRunning));
        // And again: the first copy keeps answering, not just once.
        assert!(matches!(claim_on(port, Duration::from_secs(2)), Startup::AlreadyRunning));
    }

    /// Some other program on the port must never stop Ragnarok from opening.
    #[test]
    fn a_port_held_by_something_else_does_not_block_startup() {
        let squatter = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = squatter.local_addr().unwrap().port();
        std::thread::spawn(move || {
            // Accepts and says nothing, like an unrelated service would.
            let held: Vec<_> = squatter.incoming().take(1).collect();
            std::thread::sleep(Duration::from_secs(2));
            drop(held);
        });
        assert!(matches!(claim_on(port, Duration::from_millis(500)), Startup::Unguarded));
    }
}
