//! A one-shot HTTP server for tests that need a REAL socket (a hung one, a prompt one, a slow one).
//!
//! The server must read the WHOLE request before it answers. A client may send the headers and the
//! body in separate TCP segments; a server that reads once, answers and closes leaves bytes unread,
//! and the kernel then resets the connection: the client sees `Connection reset by peer` instead of
//! the answer. That happened in CI (1 run in ~300 under load), so reading the whole request is a
//! tested property of this helper, not an assumption.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

/// Read one whole HTTP request: the headers, then `Content-Length` bytes of body. Stops early, with
/// what it has, if the peer closes first.
pub fn read_request(stream: &mut impl Read) -> Vec<u8> {
    let mut got: Vec<u8> = Vec::new();
    let mut buf = [0_u8; 8192];
    let mut need: Option<usize> = None; // total bytes expected once the headers are in
    loop {
        if let Some(total) = need
            && got.len() >= total
        {
            return got;
        }
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            return got;
        }
        got.extend_from_slice(&buf[..n]);
        if need.is_none()
            && let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n")
        {
            let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
            let body = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            need = Some(end + 4 + body);
        }
    }
}

/// A one-shot HTTP server. `delay` before answering; `None` = accept and never answer.
///
/// # Panics
/// If a loopback socket cannot be bound, which a test cannot recover from.
#[must_use]
pub fn serve_once(delay: Option<Duration>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = read_request(&mut stream);
        match delay {
            None => std::thread::sleep(Duration::from_secs(30)), // hung: holds the socket open
            Some(d) => {
                std::thread::sleep(d);
                let body = r#"[{"embedding":[[0.5,0.25]]}]"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        }
    });
    url
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use super::read_request;
    use std::io::Read;

    /// Hands over one prepared chunk per `read()` call, as a network can.
    struct Trickle(Vec<Vec<u8>>);

    impl Read for Trickle {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.0.is_empty() {
                return Ok(0);
            }
            let c = self.0.remove(0);
            out[..c.len()].copy_from_slice(&c);
            Ok(c.len())
        }
    }

    fn head(len: usize) -> Vec<u8> {
        format!("POST /embedding HTTP/1.1\r\nHost: x\r\nContent-Length: {len}\r\n\r\n").into_bytes()
    }

    #[test]
    fn a_request_whose_body_arrives_in_a_later_segment_is_read_whole() {
        let body = br#"{"content":[1,2,3]}"#.to_vec();
        let mut net = Trickle(vec![head(body.len()), body.clone()]);
        let got = read_request(&mut net);
        assert_eq!(got, [head(body.len()), body].concat());
    }

    #[test]
    fn control_a_request_without_a_body_stops_at_the_headers() {
        // The near-miss: a bodyless request must not wait for bytes that will never come, and must not
        // swallow what follows it.
        let h = b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n".to_vec();
        let mut net = Trickle(vec![h.clone(), b"NEXT".to_vec()]);
        assert_eq!(read_request(&mut net), h);
    }

    #[test]
    fn control_a_request_cut_short_returns_what_arrived_instead_of_hanging() {
        let mut part = head(100);
        part.extend_from_slice(b"abc");
        let mut net = Trickle(vec![part.clone()]);
        assert_eq!(read_request(&mut net), part);
    }
}
