//! Minimal single-file HTTP server for the browser "studio" page.
//!
//! Serves the embedded studio HTML (three.js + WebRTC + MediaPipe AR hats) on
//! its own port. The page connects back to the existing WebSocket hub for
//! signaling + chat, so browser peers and terminal peers share ONE room.

use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

const STUDIO_HTML: &str = include_str!("studio.html");
/// Cap on concurrent HTTP connections so a connection flood / slow-loris can't
/// pile up detached tasks or exhaust file descriptors on the host.
const MAX_STUDIO_CONNS: usize = 64;
/// Per-connection read/write budget; a client that never sends or never reads
/// can't park a task (and its connection slot) indefinitely.
const CONN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct StudioServer {
    port: u16,
    ws: String,
    stop: Arc<AtomicBool>,
}

impl StudioServer {
    /// Bind synchronously (so "port in use" is reported to the caller
    /// immediately) then spawn the accept loop on the tokio runtime. `ws_url`
    /// (the room's signaling hub) is injected into the page so it dials the
    /// right hub whether the room is hosted locally or joined remotely.
    pub fn start(http_port: u16, ws_url: &str) -> Result<Self> {
        let std_listener = std::net::TcpListener::bind(("0.0.0.0", http_port))?;
        let bound_port = std_listener.local_addr()?.port(); // resolves http_port==0
        std_listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_loop = stop.clone();
        // pre-render the page with the signaling hub baked in
        let page: Arc<[u8]> = Arc::from(
            STUDIO_HTML
                .replace("__WS_URL__", ws_url)
                .into_bytes()
                .into_boxed_slice(),
        );

        let sem = Arc::new(Semaphore::new(MAX_STUDIO_CONNS));

        tokio::spawn(async move {
            let listener = match TcpListener::from_std(std_listener) {
                Ok(l) => l,
                Err(_) => return,
            };
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(400)) => {
                        if stop_loop.load(Ordering::Relaxed) { break; }
                    }
                    accepted = listener.accept() => {
                        let (mut sock, _) = match accepted {
                            Ok(s) => s,
                            // back off on accept errors (e.g. EMFILE) instead of
                            // busy-spinning the loop at 100% CPU
                            Err(_) => { tokio::time::sleep(Duration::from_millis(50)).await; continue; }
                        };
                        // bound concurrency: at capacity, drop the new socket
                        // immediately (closing it) rather than spawning a task
                        let permit = match Arc::clone(&sem).try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        let page = page.clone();
                        tokio::spawn(async move {
                            let _permit = permit; // released when this task ends
                            let mut scratch = [0u8; 2048];
                            // drain the request (we serve the same page for any GET);
                            // timeout so a silent client can't hold the slot forever
                            let _ = tokio::time::timeout(CONN_TIMEOUT, sock.read(&mut scratch)).await;
                            let header = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                                page.len()
                            );
                            let _ = tokio::time::timeout(CONN_TIMEOUT, async {
                                let _ = sock.write_all(header.as_bytes()).await;
                                let _ = sock.write_all(&page).await;
                                let _ = sock.flush().await;
                            })
                            .await;
                        });
                    }
                }
            }
        });

        Ok(Self {
            port: bound_port,
            ws: ws_url.to_string(),
            stop,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The signaling hub URL this server baked into the page.
    pub fn ws(&self) -> &str {
        &self.ws
    }
}

impl Drop for StudioServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_page_with_injected_ws_url() {
        let server = StudioServer::start(0, "ws://test-host:4321").expect("bind");
        let port = server.port();
        assert_ne!(port, 0);
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect to studio");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            match stream.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
            if buf.len() > 300_000 {
                break;
            }
        }
        let text = String::from_utf8_lossy(&buf);
        assert!(text.contains("200 OK"), "expected a 200 response");
        assert!(text.contains("ASCIIVISION"), "expected the studio page body");
        assert!(
            text.contains("ws://test-host:4321"),
            "the signaling hub url must be injected"
        );
        assert!(!text.contains("__WS_URL__"), "placeholder must be replaced");
    }
}
