//! The network layer: `tiny_http` feeding [`Server`].
//!
//! Everything that decides anything is in `server`; this module only turns sockets into
//! [`Head`]s and [`Outcome`]s into bytes. It listens on the loopback interface only (a listener
//! on any other address is refused before it is created), serves each request on its own thread,
//! and bounds how many run at once.
//!
//! The live stream cannot use `tiny_http`'s ordinary response: it buffers a chunk until 8 KiB are
//! full, which would hold every one-second reading back for minutes. So the stream takes over the
//! raw connection (`Request::upgrade`) and writes server-sent events itself, flushing each one.
//! The answer has no length and is not chunked, so it ends when the connection closes, which is
//! how an event stream is allowed to end.
#![forbid(unsafe_code)]

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tiny_http::{Header, Request, Response, StatusCode};

use super::GuiError;
use super::server::{Head, Outcome, Reply, SECURITY_HEADERS, Server, StreamTicket, stream_headers};

/// How many requests are handled at once; more are told to come back (a page makes a handful).
const MAX_IN_FLIGHT: usize = 64;

/// How often the accept loop looks at its stop flag.
const ACCEPT_POLL: Duration = Duration::from_millis(50);

/// Listens on `addr`, which must be a loopback address: the window server never listens beyond
/// this computer, whatever the caller asked for.
pub(crate) fn bind_loopback(addr: &str) -> Result<TcpListener, GuiError> {
    let parsed: SocketAddr = addr
        .parse()
        .map_err(|_| GuiError::Listen(format!("listen address {addr:?} is not a host:port address")))?;
    if !parsed.ip().is_loopback() {
        return Err(GuiError::Listen(format!(
            "{addr:?} is not a loopback address: the window server never listens beyond this computer"
        )));
    }
    TcpListener::bind(parsed).map_err(|e| GuiError::Listen(format!("listen on {addr}: {e}")))
}

/// The running accept loop.
pub(crate) struct Serving {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Serving {
    /// Whether the accept loop is still going.
    pub(crate) fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Stops accepting and waits for the loop to end. Requests already being handled finish on
    /// their own threads (the open streams end because the server is closing).
    pub(crate) fn shutdown(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Serves `server` on `listener` until [`Serving::shutdown`].
pub(crate) fn serve(listener: TcpListener, server: Arc<Server>) -> Result<Serving, GuiError> {
    let http =
        tiny_http::Server::from_listener(listener, None).map_err(|e| GuiError::Listen(e.to_string()))?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = thread::Builder::new()
        .name("gui-accept".to_owned())
        .spawn(move || accept_loop(&http, &server, &flag))
        .map_err(|e| GuiError::Io(format!("cannot start the server thread: {e}")))?;
    Ok(Serving { stop, thread: Some(thread) })
}

fn accept_loop(http: &tiny_http::Server, server: &Arc<Server>, stop: &AtomicBool) {
    let in_flight = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::SeqCst) {
        let request = match http.recv_timeout(ACCEPT_POLL) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(_) => return,
        };
        if in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
            in_flight.fetch_sub(1, Ordering::SeqCst);
            let busy = Reply {
                status: 503,
                headers: vec![("Content-Type", "text/plain; charset=utf-8".to_owned())],
                body: b"busy".to_vec(),
            };
            let _ = request.respond(response(busy));
            continue;
        }
        let (server, in_flight_here) = (Arc::clone(server), Arc::clone(&in_flight));
        let spawned = thread::Builder::new().name("gui-request".to_owned()).spawn(move || {
            handle(&server, request);
            in_flight_here.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            in_flight.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// The part of a request URL that is the path.
fn path_of(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or("")
}

fn head_of(request: &Request) -> Head {
    Head {
        method: request.method().as_str().to_owned(),
        path: path_of(request.url()).to_owned(),
        headers: request
            .headers()
            .iter()
            .map(|h| (h.field.as_str().as_str().to_owned(), h.value.as_str().to_owned()))
            .collect(),
    }
}

fn handle(server: &Arc<Server>, mut request: Request) {
    let head = head_of(&request);
    let outcome = server.handle(&head, request.as_reader());
    match outcome {
        Outcome::Reply(reply) => {
            let _ = request.respond(response(reply));
        }
        Outcome::Stream(ticket) => stream(server, request, &ticket),
    }
}

fn header(name: &str, value: &str) -> Option<Header> {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
}

fn response(reply: Reply) -> Response<io::Cursor<Vec<u8>>> {
    let mut response = Response::from_data(reply.body).with_status_code(StatusCode(reply.status));
    for (name, value) in SECURITY_HEADERS {
        response.add_header_if_some(header(name, value));
    }
    for (name, value) in &reply.headers {
        response.add_header_if_some(header(name, value));
    }
    response
}

/// `Response::add_header` takes a `Header`; a value that is not valid in a header is dropped.
trait AddHeader {
    fn add_header_if_some(&mut self, h: Option<Header>);
}

impl<R: io::Read> AddHeader for Response<R> {
    fn add_header_if_some(&mut self, h: Option<Header>) {
        if let Some(h) = h {
            self.add_header(h);
        }
    }
}

fn stream(server: &Server, request: Request, ticket: &StreamTicket) {
    let mut headers: Vec<Header> = SECURITY_HEADERS.iter().filter_map(|(n, v)| header(n, v)).collect();
    headers.extend(stream_headers().iter().filter_map(|(n, v)| header(n, v)));
    let answer = Response::new(StatusCode(200), headers, io::empty(), Some(0), None);
    let mut connection = request.upgrade("sse", answer);
    // Ends when the page goes away (a write fails), the server closes, or the connection breaks.
    let _ = server.pump(ticket, &mut connection);
}
