//! A fake GitHub for the `get.sh` tests, standard library only (compile with `rustc -O fakegithub.rs`).
//!
//! `fakegithub DOCROOT` listens on 127.0.0.1 on a free port, prints the port on the first line
//! of its output and answers GET requests until it is killed:
//!
//! * `DOCROOT/a/b` is the body of `/a/b` (the query string is ignored);
//! * `DOCROOT/a/b.status` holding a number answers `/a/b` with that status and no body, to
//!   imitate a rate limit or a server error;
//! * anything else is a 404.
//!
//! Like GitHub's download servers it sends an explicit length and closes the connection.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};

fn main() -> io::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "usage: fakegithub DOCROOT"))?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    println!("{}", listener.local_addr()?.port());
    io::stdout().flush()?;
    for stream in listener.incoming().flatten() {
        // One bad request must not stop the server: the next test step may be fine.
        let _ = serve(stream, &root);
    }
    Ok(())
}

/// Answers one request on `stream`.
fn serve(mut stream: TcpStream, root: &Path) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    // Drain the headers: curl waits for the server to read them before it reads the answer.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim_end().is_empty() {
            break;
        }
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");
    let (status, body) = answer(root, path);
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Status",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    stream.flush()
}

/// The status and body for a URL path.
fn answer(root: &Path, url_path: &str) -> (u16, Vec<u8>) {
    let relative = Path::new(url_path.trim_start_matches('/'));
    // Only plain names below the document root: no `..`, no absolute paths.
    if relative.as_os_str().is_empty() || !relative.components().all(|c| matches!(c, Component::Normal(_))) {
        return (404, Vec::new());
    }
    let file = root.join(relative);
    let mut status_file = file.clone().into_os_string();
    status_file.push(".status");
    if let Some(code) =
        fs::read_to_string(PathBuf::from(status_file)).ok().and_then(|t| t.trim().parse().ok())
    {
        return (code, Vec::new());
    }
    match fs::read(&file) {
        Ok(body) if file.is_file() => (200, body),
        _ => (404, Vec::new()),
    }
}
