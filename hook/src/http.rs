//! A minimal HTTP/1.1 client: one POST to 127.0.0.1 over a plain TcpStream.
//! Any failure returns `None`; the hook never reports errors (fail open).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Responses larger than this are rejected (the widget only sends tiny JSON).
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Connecting to localhost is instant when the widget is up, so give up fast.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(200);

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// POST `body` to `http://127.0.0.1:<port><path>`. The whole exchange (connect,
/// send, receive) must finish within `total`.
pub fn post(port: u16, path: &str, token: &str, body: &[u8], total: Duration) -> Option<Response> {
    let start = Instant::now();
    // Time left of the budget. `checked_sub` gives None once it is used up, and
    // `?` on an Option returns None from this whole function early.
    let remaining = || total.checked_sub(start.elapsed());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let connect_timeout = CONNECT_TIMEOUT.min(remaining()?);
    let mut stream = TcpStream::connect_timeout(&addr, connect_timeout).ok()?;

    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.set_write_timeout(Some(remaining()?)).ok()?;
    stream.write_all(head.as_bytes()).ok()?;
    stream.write_all(body).ok()?;

    // Read until the server closes the connection (we sent `Connection: close`).
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        // A read timeout of zero is an error, so keep at least 1 ms.
        let left = remaining()?.max(Duration::from_millis(1));
        stream.set_read_timeout(Some(left)).ok()?;
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > MAX_RESPONSE_BYTES {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    parse_response(&raw)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Splits raw bytes into status code and (de-chunked) body.
fn parse_response(raw: &[u8]) -> Option<Response> {
    let head_end = find(raw, b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let mut lines = head.split("\r\n");
    // "HTTP/1.1 200 OK" -> second word.
    let status: u16 = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let chunked = lines.any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    let rest = &raw[head_end + 4..];
    let body = if chunked {
        decode_chunked(rest)?
    } else {
        rest.to_vec()
    };
    Some(Response { status, body })
}

/// Decodes `<hex size>\r\n<data>\r\n ... 0\r\n\r\n`.
fn decode_chunked(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = find(data, b"\r\n")?;
        let size_text = String::from_utf8_lossy(&data[..line_end]).into_owned();
        // Ignore chunk extensions after ';'.
        let size = usize::from_str_radix(size_text.split(';').next()?.trim(), 16).ok()?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Some(out);
        }
        if data.len() < size {
            return None;
        }
        out.extend_from_slice(&data[..size]);
        data = data.get(size + 2..)?; // skip the data and its trailing \r\n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_response() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}").unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"{}");
    }

    #[test]
    fn chunked_response() {
        let raw =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        let r = parse_response(raw).unwrap();
        assert_eq!(r.body, b"abcde");
    }

    #[test]
    fn garbage_is_none() {
        assert!(parse_response(b"nonsense").is_none());
        assert!(parse_response(b"HTTP/1.1 abc\r\n\r\n").is_none());
    }
}
