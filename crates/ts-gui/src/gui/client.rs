//! A minimal HTTP/1.1 client for the loopback server.
//!
//! A second `truesense gui` talks to the window server that is already running (to ask for a
//! fresh launch code, or to have it show a page). The exchange is one small request and one small
//! answer on 127.0.0.1, so a hand-written client over `TcpStream` is enough and no HTTP client
//! crate is pulled into the binary. Tests use it to talk to the real server over a real socket.
#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// The answers are small; anything bigger is cut off rather than buffered.
const MAX_ANSWER: u64 = 1 << 20;

/// What to send.
#[derive(Debug, Clone, Default)]
pub(crate) struct Request<'a> {
    /// `GET` or `POST`.
    pub method: &'a str,
    /// The path.
    pub path: &'a str,
    /// Headers besides `Content-Length` and `Connection`. `Host` is added when absent.
    pub headers: Vec<(&'a str, String)>,
    /// The body.
    pub body: &'a str,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Answer {
    /// The status code.
    pub status: u16,
    /// Header names (lower case) and values.
    pub headers: Vec<(String, String)>,
    /// The body (chunked bodies are joined).
    pub body: Vec<u8>,
}

impl Answer {
    /// A header by name, ignoring case.
    #[cfg(test)]
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The body as text.
    #[cfg(test)]
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body as JSON.
    pub(crate) fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

/// Connects to the server on `port`, with a short connect timeout (nothing is there when the
/// server has gone, and the user is waiting).
pub(crate) fn connect(port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let stream = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_millis(500)))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

/// Sends one request and reads the whole answer (the connection is closed after it).
pub(crate) fn send(port: u16, request: &Request<'_>, timeout: Duration) -> io::Result<Answer> {
    let mut stream = connect(port, timeout)?;
    let mut text = format!("{} {} HTTP/1.1\r\n", request.method, request.path);
    if !request.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        text.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (k, v) in &request.headers {
        text.push_str(&format!("{k}: {v}\r\n"));
    }
    text.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", request.body.len()));
    text.push_str(request.body);
    stream.write_all(text.as_bytes())?;
    let mut raw = Vec::new();
    stream.take(MAX_ANSWER).read_to_end(&mut raw)?;
    parse_answer(&raw)
}

/// A JSON `POST` with an optional bearer token, the way the page sends a command.
pub(crate) fn post_json(
    port: u16,
    path: &str,
    token: Option<&str>,
    body: &str,
    timeout: Duration,
) -> io::Result<Answer> {
    let mut headers = vec![("Content-Type", "application/json".to_owned())];
    if let Some(t) = token {
        headers.push(("Authorization", format!("Bearer {t}")));
    }
    send(port, &Request { method: "POST", path, headers, body }, timeout)
}

fn invalid(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.to_owned())
}

/// Splits a raw answer into status, headers and body.
pub(crate) fn parse_answer(raw: &[u8]) -> io::Result<Answer> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| invalid("no header end"))?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| invalid("headers are not text"))?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| invalid("bad status line"))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let rest = &raw[split + 4..];
    let chunked = headers.iter().any(|(k, v)| k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked"));
    let body = if chunked { unchunk(rest)? } else { rest.to_vec() };
    Ok(Answer { status, headers, body })
}

/// Joins a chunked body.
fn unchunk(mut rest: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = rest.windows(2).position(|w| w == b"\r\n").ok_or_else(|| invalid("bad chunk"))?;
        let size = std::str::from_utf8(&rest[..eol])
            .ok()
            .and_then(|s| usize::from_str_radix(s.split(';').next().unwrap_or("").trim(), 16).ok())
            .ok_or_else(|| invalid("bad chunk size"))?;
        rest = &rest[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size.saturating_add(2) {
            return Err(invalid("short chunk"));
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_answer_is_split_into_status_headers_and_body() {
        let a = parse_answer(
            b"HTTP/1.1 422 Unprocessable\r\nContent-Type: application/json\r\nX-A:  b \r\n\r\n{\"ok\":false}",
        )
        .ok();
        let a = a.unwrap_or_else(|| Answer { status: 0, headers: vec![], body: vec![] });
        assert_eq!(a.status, 422);
        assert_eq!(a.header("content-type"), Some("application/json"));
        assert_eq!(a.header("X-A"), Some("b"));
        assert_eq!(a.text(), "{\"ok\":false}");
    }

    #[test]
    fn a_chunked_body_is_joined() {
        let raw =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(parse_answer(raw).map(|a| a.text()).ok().as_deref(), Some("hello world"));
    }

    #[test]
    fn hostile_answers_are_errors_not_panics() {
        for raw in [
            &b""[..],
            b"garbage",
            b"HTTP/1.1\r\n\r\n",
            b"HTTP/1.1 abc OK\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n99999999999999999999\r\nx",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhi",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffff\r\nx",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        ] {
            assert!(parse_answer(raw).is_err(), "{raw:?}");
        }
    }
}
