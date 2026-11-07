//! The live stream and the `show` event, over a real socket.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::harness::{Live, Scripted, TOKEN};
use crate::gui::client;
use crate::gui::server::{MAX_STREAMS, SECURITY_HEADERS};

/// An open `/api/stream` request, read the way the page reads it.
pub(super) struct Stream {
    pub(super) reader: BufReader<TcpStream>,
    pub(super) head: String,
}

impl Stream {
    pub(super) fn open(live: &Live) -> Stream {
        Self::open_with(live, Some(TOKEN))
    }

    fn open_with(live: &Live, token: Option<&str>) -> Stream {
        Self::open_port(live.port, token)
    }

    /// Opens the stream of whatever listens on `port`.
    pub(super) fn open_port(port: u16, token: Option<&str>) -> Stream {
        let tcp = client::connect(port, Duration::from_secs(10)).expect("connect");
        let mut request =
            format!("GET /api/stream HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/event-stream\r\n");
        match token {
            Some(t) => request.push_str(&format!("Authorization: Bearer {t}\r\n")),
            // A refusal is an ordinary answer: asking for the connection to close lets the test read it to the end.
            None => request.push_str("Connection: close\r\n"),
        }
        request.push_str("\r\n");
        let mut writer = tcp.try_clone().expect("clone");
        writer.write_all(request.as_bytes()).expect("write");
        let mut reader = BufReader::new(tcp);
        let mut head = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header line");
            if line == "\r\n" || line.is_empty() {
                break;
            }
            head.push_str(&line);
        }
        Stream { reader, head }
    }

    pub(super) fn status(&self) -> u16 {
        self.head.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    /// The next block of lines up to a blank line.
    fn block(&mut self) -> Option<String> {
        let mut out = String::new();
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) | Err(_) => return None,
                Ok(_) if line == "\n" => return Some(out),
                Ok(_) => out.push_str(&line),
            }
        }
    }

    /// The next event (comments and the retry line are skipped): `(event, data)`.
    pub(super) fn event(&mut self) -> Option<(String, String)> {
        loop {
            let block = self.block()?;
            let mut event = None;
            let mut data = None;
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event: ") {
                    event = Some(v.to_owned());
                } else if let Some(v) = line.strip_prefix("data: ") {
                    data = Some(v.to_owned());
                }
            }
            if let (Some(e), Some(d)) = (event, data) {
                return Some((e, d));
            }
        }
    }
}

fn server() -> (Live, Arc<Scripted>) {
    let commands = Arc::new(Scripted::default());
    (Live::start(commands.clone()), commands)
}

#[test]
fn the_stream_answers_200_as_an_event_stream_with_the_security_headers() {
    let (live, _) = server();
    let stream = Stream::open(&live);
    assert_eq!(stream.status(), 200, "{}", stream.head);
    let head = stream.head.to_ascii_lowercase();
    assert!(head.contains("content-type: text/event-stream"), "{head}");
    assert!(head.contains("cache-control: no-store"), "{head}");
    for (name, value) in SECURITY_HEADERS {
        assert!(
            head.contains(&format!("{}: {}", name.to_ascii_lowercase(), value.to_ascii_lowercase())),
            "{name}\n{head}"
        );
    }
    assert!(!head.contains("content-length"), "an open-ended stream has no length");
    assert!(!head.contains("transfer-encoding"), "and is not chunked (a chunk would buffer 8 KiB)");
}

#[test]
fn readings_arrive_one_by_one_as_they_are_taken() {
    let (live, commands) = server();
    let mut stream = Stream::open(&live);
    let started = Instant::now();
    let mut seen = Vec::new();
    for _ in 0..3 {
        let (event, data) = stream.event().expect("an event");
        assert_eq!(event, "state");
        seen.push(serde_json::from_str::<Value>(&data).expect("JSON")["t"].as_i64().expect("t"));
    }
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "each reading is newer: {seen:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "events are flushed, not buffered");
    assert!(commands.reads.load(Ordering::SeqCst) >= 3);
}

#[test]
fn a_new_page_gets_the_latest_reading_without_waiting() {
    let (live, commands) = server();
    *commands.reading.lock().expect("reading") = Some(json!({ "t": 7, "power_mode": "balanced" }));
    let mut first = Stream::open(&live);
    assert!(first.event().is_some());
    let started = Instant::now();
    let mut second = Stream::open(&live);
    let (_, data) = second.event().expect("event");
    assert_eq!(serde_json::from_str::<Value>(&data).expect("json")["power_mode"], json!("balanced"));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn an_idle_stream_sends_keep_alive_comments() {
    let (live, commands) = server();
    commands.silent.store(true, Ordering::SeqCst);
    let mut stream = Stream::open(&live);
    let block = stream.block().expect("retry");
    assert_eq!(block, "retry: 2000\n");
    assert_eq!(stream.block().as_deref(), Some(": ping\n"), "the server has nothing to say but is alive");
}

#[test]
fn show_reaches_every_open_page_as_an_event_and_says_how_many_heard_it() {
    let (live, _) = server();
    let none = live.post("/api/gui/show", "{\"page\":\"curves\"}").json().expect("json");
    assert_eq!(
        none["result"],
        json!({ "shown": false, "windows": 0 }),
        "no window open: the caller starts one"
    );

    let mut a = Stream::open(&live);
    let mut b = Stream::open(&live);
    assert!(a.event().is_some() && b.event().is_some());
    let answer = live.post("/api/gui/show", "{\"page\":\"curves\"}").json().expect("json");
    assert_eq!(answer["result"], json!({ "shown": true, "windows": 2 }));
    for s in [&mut a, &mut b] {
        loop {
            let (event, data) = s.event().expect("event");
            if event == "show" {
                assert_eq!(data, "\"curves\"");
                break;
            }
        }
    }
}

#[test]
fn a_page_name_that_is_not_a_page_id_shows_home_and_cannot_carry_markup() {
    let (live, _) = server();
    let mut stream = Stream::open(&live);
    assert!(stream.event().is_some());
    let wanted = [
        "../etc",
        "<script>alert(1)</script>",
        "Curves",
        "x".repeat(25).as_str(),
        "a b",
        "1abc",
        "",
        "curves\"}",
    ]
    .map(str::to_owned);
    for page in wanted {
        live.post("/api/gui/show", &json!({ "page": page }).to_string());
        loop {
            let (event, data) = stream.event().expect("event");
            if event == "show" {
                assert_eq!(data, "\"home\"", "{page:?}");
                break;
            }
        }
    }
    live.post("/api/gui/show", "{}");
    loop {
        if let Some((event, data)) = stream.event() {
            if event == "show" {
                assert_eq!(data, "\"home\"", "no page means home");
                break;
            }
        }
    }
    live.post("/api/gui/show", "{\"page\":\"keys-and-more\"}");
    loop {
        let (event, data) = stream.event().expect("event");
        if event == "show" {
            assert_eq!(data, "\"keys-and-more\"", "page ids may contain dashes");
            break;
        }
    }
}

#[test]
fn the_number_of_open_streams_is_bounded() {
    let (live, _) = server();
    let mut open: Vec<Stream> = (0..MAX_STREAMS).map(|_| Stream::open(&live)).collect();
    for s in &mut open {
        assert!(s.event().is_some());
    }
    assert_eq!(Stream::open(&live).status(), 503, "a ninth page is told there are too many");
    open.pop();
    // The freed slot is noticed when the server's next write to the closed connection fails.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let again = Stream::open(&live);
        if again.status() == 200 {
            open.push(again);
            break;
        }
        assert!(Instant::now() < deadline, "the slot of a closed page was not freed");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_idle_clock_runs_only_while_no_page_has_the_stream_open() {
    let (live, _) = server();
    live.clock.advance(Duration::from_secs(30));
    assert_eq!(
        live.server.idle_for(),
        Duration::from_secs(30),
        "a page that never connects counts as closed"
    );
    let mut stream = Stream::open(&live);
    assert!(stream.event().is_some());
    live.clock.advance(Duration::from_secs(30));
    assert_eq!(live.server.idle_for(), Duration::ZERO, "a page is open");
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(5);
    while live.server.idle_for().is_zero() {
        live.clock.advance(Duration::from_millis(100));
        assert!(Instant::now() < deadline, "closing the page did not start the idle clock");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_stream_without_the_token_is_refused_before_anything_is_sent() {
    let (live, _) = server();
    let mut stream = Stream::open_with(&live, None);
    assert_eq!(stream.status(), 401);
    let mut rest = String::new();
    stream.reader.read_to_string(&mut rest).ok();
    assert!(rest.contains("unauthorised") && !rest.contains("event:"), "{rest}");
}
