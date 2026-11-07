//! HTTPS fetching behind a trait, and the real implementation on top of `curl`.
//!
//! `ts-system` has no HTTP library: the Python edition used `urllib`, and a TLS stack is far
//! more code (and more attack surface in a program that ends by running something as root) than
//! the updater justifies. The real fetch spawns the system's `curl` through the same injected
//! [`CommandRunner`] the sensors use, so tests never start a process and never touch the
//! network. `curl` (7.75 or newer, for `%{exitcode}`) is therefore a run-time requirement of
//! updating; when it is missing the update reports that and nothing else happens.
#![forbid(unsafe_code)]

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ts_platform::hw::CommandRunner;

/// One GET request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest<'a> {
    /// The full `https://` URL.
    pub url: &'a str,
    /// An `Accept` header, if the endpoint wants one (GitHub's API does).
    pub accept: Option<&'a str>,
    /// How long the whole transfer may take.
    pub timeout: Duration,
    /// The most bytes of body that are accepted; more is [`FetchError::TooLarge`].
    pub max_bytes: u64,
}

/// Why a request produced no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The server answered, but not with 200.
    Status {
        /// The HTTP status code.
        code: u16,
        /// The URL asked.
        url: String,
    },
    /// The answer was bigger than the request allowed.
    TooLarge {
        /// The limit in bytes.
        limit: u64,
    },
    /// No answer: no network, a timeout, no `curl`, a TLS failure.
    Transport(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status { code, url } => write!(f, "HTTP {code} from {url}"),
            Self::TooLarge { .. } => f.write_str("the download is unexpectedly large; refusing"),
            Self::Transport(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for FetchError {}

/// Fetches a URL. The updater only ever reads: GET, a bounded body, status 200 or an error.
pub trait HttpFetcher: Send + Sync {
    /// Returns the body of a 200 answer.
    fn get(&self, request: &HttpRequest<'_>) -> Result<Vec<u8>, FetchError>;
}

/// Marks the end of the body in `curl`'s output. `curl` prints it after everything else, so a
/// body that happens to contain the same text cannot fake the status: the last mark wins.
const MARK: &[u8] = b"\n--truesense-curl-status--";

/// How many redirects are followed (GitHub sends release downloads to its object store).
const MAX_REDIRECTS: &str = "5";

/// Fetches with the system's `curl`, HTTPS only (also through redirects).
pub struct CurlFetcher {
    runner: Arc<dyn CommandRunner>,
    user_agent: String,
}

impl CurlFetcher {
    /// A fetcher running `curl` through `runner`, identifying itself as `user_agent`
    /// (`truesense/<version>`; GitHub's API refuses requests without one).
    pub fn new(runner: Arc<dyn CommandRunner>, user_agent: impl Into<String>) -> Self {
        Self { runner, user_agent: user_agent.into() }
    }

    /// The arguments for one request. `-q` first: a user's `.curlrc` must not be able to turn
    /// off certificate checks or add a proxy for a download that ends as root.
    fn args(&self, request: &HttpRequest<'_>) -> Vec<String> {
        let mut args: Vec<String> = [
            "-q",
            "--silent",
            "--location",
            "--max-redirs",
            MAX_REDIRECTS,
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
        ]
        .map(String::from)
        .to_vec();
        args.extend(["--max-time".into(), request.timeout.as_secs().max(1).to_string()]);
        args.extend(["--max-filesize".into(), request.max_bytes.to_string()]);
        args.extend(["--user-agent".into(), self.user_agent.clone()]);
        if let Some(accept) = request.accept {
            args.extend(["--header".into(), format!("Accept: {accept}")]);
        }
        args.extend([
            "--write-out".into(),
            format!("{}%{{http_code}} %{{exitcode}}", String::from_utf8_lossy(MARK)),
            "--url".into(),
            request.url.to_owned(),
        ]);
        args
    }
}

impl HttpFetcher for CurlFetcher {
    fn get(&self, request: &HttpRequest<'_>) -> Result<Vec<u8>, FetchError> {
        let args = self.args(request);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        // The runner kills curl at its own deadline; curl's `--max-time` normally fires first.
        let out =
            self.runner.output("curl", &args, request.timeout + Duration::from_secs(5)).map_err(|e| {
                FetchError::Transport(format!(
                    "cannot run curl ({e}); install curl to check for or install updates"
                ))
            })?;
        interpret(&out, request)
    }
}

/// Splits curl's output into body and trailer and decides. The trailer is
/// `<mark><http code> <curl exit code>`.
fn interpret(output: &[u8], request: &HttpRequest<'_>) -> Result<Vec<u8>, FetchError> {
    let odd = || FetchError::Transport("unexpected output from curl".into());
    let at = output.windows(MARK.len()).rposition(|w| w == MARK).ok_or_else(odd)?;
    let trailer = std::str::from_utf8(&output[at + MARK.len()..]).map_err(|_| odd())?;
    let mut fields = trailer.split_whitespace();
    let (code, exit) = (fields.next().ok_or_else(odd)?, fields.next().ok_or_else(odd)?);
    if fields.next().is_some() {
        return Err(odd());
    }
    let code: u16 = code.parse().map_err(|_| odd())?;
    let exit: u32 = exit.parse().map_err(|_| odd())?;
    // 63: --max-filesize exceeded. 28: timed out. Others are described by number: the message
    // is for a person who can look it up in `man curl`.
    match exit {
        0 => {}
        63 => return Err(FetchError::TooLarge { limit: request.max_bytes }),
        28 => return Err(FetchError::Transport("the request timed out".into())),
        n => {
            return Err(FetchError::Transport(format!(
                "curl failed (exit code {n}); is there a network connection?"
            )));
        }
    }
    if code != 200 {
        return Err(FetchError::Status { code, url: request.url.to_owned() });
    }
    let body = &output[..at];
    if body.len() as u64 > request.max_bytes {
        return Err(FetchError::TooLarge { limit: request.max_bytes });
    }
    Ok(body.to_vec())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::Mutex;

    /// Plays one scripted answer and remembers what was asked.
    struct FakeCurl {
        answer: Mutex<io::Result<Vec<u8>>>,
        asked: Mutex<Vec<(String, Vec<String>, Duration)>>,
    }

    impl FakeCurl {
        fn new(answer: io::Result<Vec<u8>>) -> Arc<Self> {
            Arc::new(Self { answer: Mutex::new(answer), asked: Mutex::new(Vec::new()) })
        }
    }

    impl CommandRunner for FakeCurl {
        fn output(&self, program: &str, args: &[&str], timeout: Duration) -> io::Result<Vec<u8>> {
            self.asked.lock().unwrap().push((
                program.to_owned(),
                args.iter().map(|a| (*a).to_owned()).collect(),
                timeout,
            ));
            match &*self.answer.lock().unwrap() {
                Ok(v) => Ok(v.clone()),
                Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
            }
        }
    }

    fn reply(body: &[u8], code: &str, exit: &str) -> Vec<u8> {
        [body, MARK, format!("{code} {exit}").as_bytes()].concat()
    }

    fn request(url: &str) -> HttpRequest<'_> {
        HttpRequest {
            url,
            accept: Some("application/vnd.github+json"),
            timeout: Duration::from_secs(8),
            max_bytes: 100,
        }
    }

    fn fetch(answer: io::Result<Vec<u8>>) -> (Result<Vec<u8>, FetchError>, Arc<FakeCurl>) {
        let runner = FakeCurl::new(answer);
        let fetcher = CurlFetcher::new(runner.clone(), "truesense/0.5.0");
        (fetcher.get(&request("https://api.github.com/x")), runner)
    }

    #[test]
    fn a_200_answer_gives_its_body() {
        let (r, runner) = fetch(Ok(reply(b"{\"a\":1}", "200", "0")));
        assert_eq!(r.unwrap(), b"{\"a\":1}");
        let asked = runner.asked.lock().unwrap();
        let (program, args, timeout) = &asked[0];
        assert_eq!(program, "curl");
        assert_eq!(args[0], "-q", "a user's curlrc is ignored");
        let has = |pair: &[&str]| args.windows(pair.len()).any(|w| w == pair);
        assert!(
            has(&["--proto", "=https"]) && has(&["--proto-redir", "=https"]),
            "https only, also through redirects"
        );
        assert!(has(&["--max-redirs", "5"]) && has(&["--max-time", "8"]) && has(&["--max-filesize", "100"]));
        assert!(has(&["--user-agent", "truesense/0.5.0"]));
        assert!(has(&["--header", "Accept: application/vnd.github+json"]));
        assert_eq!(
            &args[args.len() - 2..],
            ["--url", "https://api.github.com/x"],
            "the URL is passed as a value, never as an option"
        );
        assert!(args.iter().any(|a| a.contains("%{http_code} %{exitcode}")));
        assert_eq!(*timeout, Duration::from_secs(13), "the runner's deadline is a little after curl's own");
    }

    #[test]
    fn no_accept_header_when_none_is_asked_for() {
        let runner = FakeCurl::new(Ok(reply(b"x", "200", "0")));
        let fetcher = CurlFetcher::new(runner.clone(), "ua");
        let req = HttpRequest {
            url: "https://github.com/a",
            accept: None,
            timeout: Duration::from_millis(100),
            max_bytes: 10,
        };
        fetcher.get(&req).unwrap();
        let asked = runner.asked.lock().unwrap();
        assert!(!asked[0].1.iter().any(|a| a == "--header"));
        assert!(
            asked[0].1.windows(2).any(|w| w == ["--max-time", "1"]),
            "never zero: 0 means no limit for curl"
        );
    }

    #[test]
    fn a_body_that_contains_the_mark_cannot_fake_the_status() {
        let sneaky = [b"before".as_slice(), MARK, b"200 0", b"after"].concat();
        let (r, _) = fetch(Ok(reply(&sneaky, "200", "0")));
        assert_eq!(r.unwrap(), sneaky, "everything before the LAST mark is the body");
        let (r, _) = fetch(Ok(reply(&[b"x".as_slice(), MARK, b"200 0"].concat(), "404", "0")));
        assert!(matches!(r, Err(FetchError::Status { code: 404, .. })), "the real trailer says 404");
    }

    #[test]
    fn other_statuses_are_errors_with_the_status_and_url() {
        for code in ["204", "301", "403", "404", "500", "000"] {
            let (r, _) = fetch(Ok(reply(b"body", code, "0")));
            match r.unwrap_err() {
                FetchError::Status { code: c, url } => {
                    assert_eq!(c, code.parse::<u16>().unwrap());
                    assert_eq!(url, "https://api.github.com/x");
                }
                other => panic!("{other:?}"),
            }
        }
        let (r, _) = fetch(Ok(reply(b"", "403", "0")));
        assert_eq!(r.unwrap_err().to_string(), "HTTP 403 from https://api.github.com/x");
    }

    #[test]
    fn curl_failures_never_pass_as_success_even_with_a_200_and_a_body() {
        // A transfer cut off half way still printed its status (200) but exited non-zero.
        let (r, _) = fetch(Ok(reply(b"partial", "200", "18")));
        assert_eq!(
            r.unwrap_err(),
            FetchError::Transport("curl failed (exit code 18); is there a network connection?".into())
        );
        let (r, _) = fetch(Ok(reply(b"", "000", "6")));
        assert!(matches!(r, Err(FetchError::Transport(_))));
        let (r, _) = fetch(Ok(reply(b"", "200", "28")));
        assert_eq!(r.unwrap_err(), FetchError::Transport("the request timed out".into()));
        let (r, _) = fetch(Ok(reply(b"", "200", "63")));
        assert_eq!(r.unwrap_err(), FetchError::TooLarge { limit: 100 });
    }

    #[test]
    fn a_body_over_the_limit_is_too_large() {
        let (r, _) = fetch(Ok(reply(&[b'a'; 101], "200", "0")));
        assert_eq!(r.unwrap_err(), FetchError::TooLarge { limit: 100 });
        let (r, _) = fetch(Ok(reply(&[b'a'; 100], "200", "0")));
        assert_eq!(r.unwrap().len(), 100);
        assert_eq!(
            FetchError::TooLarge { limit: 1 }.to_string(),
            "the download is unexpectedly large; refusing"
        );
    }

    #[test]
    fn output_that_is_not_curls_is_refused() {
        for junk in [
            b"".to_vec(),
            b"plain body".to_vec(),
            [MARK, b"200"].concat(),
            [MARK, b"200 0 extra"].concat(),
            [MARK, b"abc 0"].concat(),
            [MARK, b"200 x"].concat(),
            [MARK, b"200 %{exitcode}"].concat(),
            [MARK, b"99999 0"].concat(),
            [MARK, b"\xff\xfe 0"].concat(),
            [MARK, b"-1 0"].concat(),
        ] {
            let (r, _) = fetch(Ok(junk.clone()));
            assert_eq!(
                r.unwrap_err(),
                FetchError::Transport("unexpected output from curl".into()),
                "{junk:?}"
            );
        }
    }

    #[test]
    fn a_missing_curl_is_reported_as_such() {
        let (r, _) = fetch(Err(io::Error::from(io::ErrorKind::NotFound)));
        let text = r.unwrap_err().to_string();
        assert!(
            text.starts_with("cannot run curl (")
                && text.ends_with("install curl to check for or install updates"),
            "{text}"
        );
        let (r, _) = fetch(Err(io::Error::from(io::ErrorKind::TimedOut)));
        assert!(matches!(r, Err(FetchError::Transport(_))));
    }
}
