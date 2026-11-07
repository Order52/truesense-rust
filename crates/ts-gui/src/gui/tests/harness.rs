//! A real server on a loopback port over `Services::simulated`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use ts_api::api::testing::Rig;
use ts_api::api::{Api, Services};

use crate::gui::bus::{ApiCommands, CommandError, Commands, Hooks};
use crate::gui::client::{self, Answer, Request};
use crate::gui::http::{Serving, bind_loopback, serve};
use crate::gui::server::{Clock, Server, Settings};
use crate::gui::testutil::Temp;
use crate::gui::token::Entropy;

/// The session token of the test server.
pub(super) const TOKEN: &str = "test-session-token";

/// A clock that moves only when told to.
pub(super) struct ManualClock {
    base: Instant,
    millis: AtomicU64,
}

impl ManualClock {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self { base: Instant::now(), millis: AtomicU64::new(0) })
    }

    pub(super) fn advance(&self, d: Duration) {
        self.millis.fetch_add(u64::try_from(d.as_millis()).unwrap_or(u64::MAX), Ordering::SeqCst);
    }

    pub(super) fn clock(self: &Arc<Self>) -> Clock {
        let me = Arc::clone(self);
        Arc::new(move || me.base + Duration::from_millis(me.millis.load(Ordering::SeqCst)))
    }
}

/// A random source that counts up, so codes and tokens are predictable and distinct.
pub(super) fn counting_entropy() -> Entropy {
    let n = Arc::new(AtomicU64::new(1));
    Arc::new(move |buf: &mut [u8]| {
        let v = n.fetch_add(1, Ordering::SeqCst);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (v as u8).wrapping_add(i as u8);
        }
        Ok(())
    })
}

/// Commands that answer from a script and record what was asked.
#[derive(Default)]
pub(super) struct Scripted {
    pub calls: Mutex<Vec<(String, String, Value)>>,
    pub failing: Mutex<Option<CommandError>>,
    pub delay: Mutex<Duration>,
    pub reading: Mutex<Option<Value>>,
    pub reads: AtomicU64,
    pub silent: std::sync::atomic::AtomicBool,
}

impl Commands for Scripted {
    fn run(&self, area: &str, verb: &str, args: &Value) -> Result<Value, CommandError> {
        let delay = *self.delay.lock().expect("delay");
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        self.calls.lock().expect("calls").push((area.to_owned(), verb.to_owned(), args.clone()));
        if let Some(e) = self.failing.lock().expect("failing").clone() {
            return Err(e);
        }
        match (area, verb) {
            ("test", "echo") => Ok(args.clone()),
            ("test", "panic") => std::panic::resume_unwind(Box::new("a command panicked")),
            _ => Err(CommandError::unknown(area, verb)),
        }
    }

    fn live(&self) -> Option<Value> {
        let n = self.reads.fetch_add(1, Ordering::SeqCst);
        if self.silent.load(Ordering::SeqCst) {
            return None;
        }
        Some(self.reading.lock().expect("reading").clone().unwrap_or_else(|| json!({ "t": n })))
    }
}

/// A server running on a loopback port, stopped when dropped.
pub(super) struct Live {
    pub port: u16,
    pub server: Arc<Server>,
    pub clock: Arc<ManualClock>,
    serving: Option<Serving>,
}

impl Live {
    pub(super) fn start(commands: Arc<dyn Commands>) -> Live {
        Self::start_with(commands, Duration::from_secs(30))
    }

    pub(super) fn start_with(commands: Arc<dyn Commands>, command_timeout: Duration) -> Live {
        let listener = bind_loopback("127.0.0.1:0").expect("bind");
        let local = listener.local_addr().expect("addr");
        let clock = ManualClock::new();
        let settings = Settings {
            version: "9.9.9".to_owned(),
            command_timeout,
            tick: Duration::from_millis(10),
            ping_every: Duration::from_millis(150),
            log: Arc::new(|_| {}),
            entropy: counting_entropy(),
            clock: clock.clock(),
        };
        let server = Server::new(local.to_string(), TOKEN.to_owned(), commands, settings);
        let serving = serve(listener, Arc::clone(&server)).expect("serve");
        Live { port: local.port(), server, clock, serving: Some(serving) }
    }

    pub(super) fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// A command the way the page sends it.
    pub(super) fn post(&self, path: &str, body: &str) -> Answer {
        client::post_json(self.port, path, Some(TOKEN), body, Duration::from_secs(20)).expect("post")
    }

    /// A command by area and verb with JSON arguments; the JSON envelope.
    pub(super) fn command(&self, area: &str, verb: &str, args: &Value) -> (u16, Value) {
        let answer = self.post(&format!("/api/{area}/{verb}"), &args.to_string());
        (answer.status, answer.json().expect("the answer is JSON"))
    }

    /// Any request.
    pub(super) fn send(&self, method: &str, path: &str, headers: Vec<(&str, String)>, body: &str) -> Answer {
        let request = Request { method, path, headers, body };
        client::send(self.port, &request, Duration::from_secs(20)).expect("send")
    }

    pub(super) fn get(&self, path: &str) -> Answer {
        self.send("GET", path, Vec::new(), "")
    }

    pub(super) fn stop(&mut self) {
        self.server.close();
        if let Some(s) = self.serving.take() {
            s.shutdown();
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The application layer over the simulator, the doubles behind it and a server in front of it.
pub(super) struct Machine {
    pub live: Live,
    pub rig: Rig,
    pub api: Arc<Api>,
    pub root: Temp,
}

impl Machine {
    pub(super) fn start() -> Machine {
        Self::start_with(Hooks::default())
    }

    pub(super) fn start_with(hooks: Hooks) -> Machine {
        let root = Temp::new("machine");
        let (services, rig) = Services::simulated(&root.0).expect("simulated services");
        let api = Arc::new(Api::new(services));
        let commands = Arc::new(ApiCommands::new(Arc::clone(&api), hooks));
        Machine { live: Live::start(commands), rig, api, root }
    }

    /// Runs a command and returns `(status, envelope)`.
    pub(super) fn run(&self, area: &str, verb: &str, args: &Value) -> (u16, Value) {
        self.live.command(area, verb, args)
    }

    /// Runs a command that must succeed and returns its result.
    pub(super) fn ok(&self, area: &str, verb: &str, args: &Value) -> Value {
        let (status, body) = self.run(area, verb, args);
        assert_eq!(status, 200, "{area} {verb} {args}: {body}");
        assert_eq!(body["ok"], json!(true), "{body}");
        body["result"].clone()
    }

    /// Runs a command that must be refused and returns its message.
    pub(super) fn refused(&self, area: &str, verb: &str, args: &Value) -> String {
        let (status, body) = self.run(area, verb, args);
        assert_eq!(status, 422, "{area} {verb} {args}: {body}");
        assert_eq!(body["ok"], json!(false));
        body["error"]["message"].as_str().expect("message").to_owned()
    }
}
