//! The security rules: who may talk to the server, and what is refused before anything runs.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::harness::{Live, Scripted, TOKEN};
use crate::gui::assets::FILES;
use crate::gui::bus::CommandError;
use crate::gui::client::{self, Answer};
use crate::gui::server::{CONTENT_SECURITY_POLICY, LAUNCH_CODE_LIFE, MAX_BODY, SECURITY_HEADERS};

fn server() -> (Live, Arc<Scripted>) {
    let commands = Arc::new(Scripted::default());
    (Live::start(commands.clone()), commands)
}

fn calls(c: &Scripted) -> usize {
    c.calls.lock().expect("calls").len()
}

fn json_headers() -> Vec<(&'static str, String)> {
    vec![("Content-Type", "application/json".to_owned())]
}

fn with_token(mut headers: Vec<(&'static str, String)>) -> Vec<(&'static str, String)> {
    headers.push(("Authorization", format!("Bearer {TOKEN}")));
    headers
}

fn error_of(a: &Answer) -> (String, String) {
    let body = a.json().expect("JSON error body");
    assert_eq!(body["ok"], json!(false), "{body}");
    (
        body["error"]["message"].as_str().unwrap_or_default().to_owned(),
        body["error"]["code"].as_str().unwrap_or_default().to_owned(),
    )
}

#[test]
fn every_answer_carries_the_security_headers_and_a_strict_policy() {
    let (live, _) = server();
    let hostile_host = live.send("GET", "/", vec![("Host", "evil.example".to_owned())], "");
    let answers = [
        live.get("/"),
        live.get("/js/app.js"),
        live.get("/nothing-here"),
        live.get("/api/status/get"),
        live.send("DELETE", "/", Vec::new(), ""),
        hostile_host,
        live.post("/api/test/echo", "{}"),
    ];
    for a in &answers {
        for (name, value) in SECURITY_HEADERS {
            assert_eq!(a.header(name), Some(value), "{name} on status {}", a.status);
        }
    }
    // The policy: nothing inline, nothing external, no framing.
    for part in [
        "default-src 'none'",
        "script-src 'self'",
        "style-src 'self'",
        "connect-src 'self'",
        "base-uri 'none'",
        "form-action 'none'",
        "frame-ancestors 'none'",
    ] {
        assert!(CONTENT_SECURITY_POLICY.contains(part), "{part}");
    }
    assert!(
        !CONTENT_SECURITY_POLICY.contains("unsafe-inline")
            && !CONTENT_SECURITY_POLICY.contains("unsafe-eval")
    );
    assert!(!CONTENT_SECURITY_POLICY.contains("http:") && !CONTENT_SECURITY_POLICY.contains('*'));
}

#[test]
fn the_page_has_no_inline_code_that_the_policy_would_block() {
    let html =
        FILES.iter().find(|(p, _)| *p == "index.html").map(|(_, b)| String::from_utf8_lossy(b).into_owned());
    let html = html.expect("index.html is embedded");
    assert!(!html.contains("onclick") && !html.contains("<style"));
    for script in html.match_indices("<script") {
        let tag = &html[script.0..html[script.0..].find('>').map_or(html.len(), |e| script.0 + e)];
        assert!(tag.contains("src=\"js/app.js\""), "an inline script: {tag}");
    }
}

#[test]
fn a_request_for_another_host_is_refused_whatever_else_it_carries() {
    let (live, commands) = server();
    for host in ["evil.example", "evil.example:80", "127.0.0.1", "localhost", "127.0.0.1:1", "[::1]:80", ""] {
        let a = live.send(
            "POST",
            "/api/test/echo",
            with_token(json_headers()).into_iter().chain([("Host", host.to_owned())]).collect(),
            "{}",
        );
        assert_eq!(a.status, 403, "Host {host:?}");
        assert_eq!(error_of(&a).1, "bad-host");
        let page = live.send("GET", "/", vec![("Host", host.to_owned())], "");
        assert_eq!(page.status, 403, "the page itself is refused for Host {host:?}");
    }
    assert_eq!(calls(&commands), 0, "DNS rebinding reached no command");
    assert_eq!(live.get("/").status, 200, "the exact address works");
}

#[test]
fn a_cross_origin_or_cross_site_request_is_refused() {
    let (live, commands) = server();
    for origin in
        ["http://evil.example", "http://127.0.0.1", "https://127.0.0.1:1", "null", "http://localhost"]
    {
        let mut headers = with_token(json_headers());
        headers.push(("Origin", origin.to_owned()));
        let a = live.send("POST", "/api/test/echo", headers, "{}");
        assert_eq!(a.status, 403, "Origin {origin}");
        assert_eq!(error_of(&a).1, "cross-site");
    }
    for site in ["cross-site", "same-site"] {
        let mut headers = with_token(json_headers());
        headers.push(("Sec-Fetch-Site", site.to_owned()));
        let a = live.send("POST", "/api/test/echo", headers, "{}");
        assert_eq!(a.status, 403, "Sec-Fetch-Site {site}");
    }
    assert_eq!(calls(&commands), 0);

    // The page itself, and the user typing the address, are allowed.
    for (site, origin) in [("same-origin", Some(format!("http://{}", live.addr()))), ("none", None)] {
        let mut headers = with_token(json_headers());
        headers.push(("Sec-Fetch-Site", site.to_owned()));
        if let Some(o) = origin {
            headers.push(("Origin", o));
        }
        assert_eq!(live.send("POST", "/api/test/echo", headers, "{\"a\":1}").status, 200, "{site}");
    }
}

#[test]
fn a_command_needs_the_token() {
    let (live, commands) = server();
    for auth in [
        None,
        Some("".to_owned()),
        Some("Bearer ".to_owned()),
        Some("Bearer wrong".to_owned()),
        Some(format!("Bearer {TOKEN}x")),
        Some(format!("Bearer {}", &TOKEN[..TOKEN.len() - 1])),
        Some(format!("bearer {TOKEN}")),
        Some(format!("Basic {TOKEN}")),
        Some(TOKEN.to_owned()),
    ] {
        let mut headers = json_headers();
        if let Some(a) = &auth {
            headers.push(("Authorization", a.clone()));
        }
        let a = live.send("POST", "/api/test/echo", headers, "{}");
        assert_eq!(a.status, 401, "{auth:?}");
        assert_eq!(error_of(&a).1, "unauthorised");
    }
    let stream = live.send("GET", "/api/stream", Vec::new(), "");
    assert_eq!(stream.status, 401, "the live state needs the token as well");
    assert_eq!(calls(&commands), 0);
    assert_eq!(live.post("/api/test/echo", "{\"x\":true}").status, 200);
}

#[test]
fn only_get_and_post_are_served_and_a_command_needs_post() {
    let (live, _) = server();
    for method in ["PUT", "DELETE", "PATCH", "OPTIONS", "HEAD", "TRACE", "CONNECT"] {
        let a = live.send(method, "/api/test/echo", with_token(json_headers()), "");
        assert_eq!(a.status, 405, "{method}");
    }
    let get = live.send("GET", "/api/test/echo", with_token(Vec::new()), "");
    assert_eq!(get.status, 405);
    assert_eq!(get.header("Allow"), Some("POST"));
    let post_stream = live.post("/api/stream", "{}");
    assert_eq!(post_stream.status, 405, "the stream is read with GET");
    let post_page = live.send("POST", "/", Vec::new(), "");
    assert_eq!(post_page.status, 405, "pages are read with GET");
}

#[test]
fn a_command_must_be_json_so_a_cross_origin_form_cannot_send_one() {
    let (live, commands) = server();
    for ct in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "multipart/form-data; boundary=x",
        "application/jsonx",
        "json",
    ] {
        let headers = with_token(vec![("Content-Type", ct.to_owned())]);
        let a = live.send("POST", "/api/test/echo", headers, "{}");
        assert_eq!(a.status, 415, "{ct}");
    }
    let none = live.send("POST", "/api/test/echo", with_token(Vec::new()), "{}");
    assert_eq!(none.status, 415, "no Content-Type");
    assert_eq!(calls(&commands), 0);
    for ct in ["application/json", "application/json; charset=utf-8", "Application/JSON"] {
        let a = live.send("POST", "/api/test/echo", with_token(vec![("Content-Type", ct.to_owned())]), "{}");
        assert_eq!(a.status, 200, "{ct}");
    }
}

#[test]
fn a_body_must_be_a_json_object_of_bounded_size() {
    let (live, commands) = server();
    for body in ["[]", "\"x\"", "3", "true", "null", "{", "{\"a\":1} trailing", "{'a':1}", "not json"] {
        let a = live.post("/api/test/echo", body);
        assert_eq!(a.status, 400, "{body:?}");
    }
    assert_eq!(calls(&commands), 0);
    // An empty body is `{}`.
    let empty = live.post("/api/test/echo", "  \n");
    assert_eq!((empty.status, empty.json().expect("json")["result"].clone()), (200, json!({})));
    // The largest legitimate body (128 colours) is far below the limit; one past it is refused.
    let big = format!("{{\"pad\":\"{}\"}}", "x".repeat(MAX_BODY));
    assert_eq!(live.post("/api/test/echo", &big).status, 413);
    let fits = format!("{{\"pad\":\"{}\"}}", "x".repeat(MAX_BODY / 2));
    assert_eq!(live.post("/api/test/echo", &fits).status, 200);
}

#[test]
fn command_names_are_checked_before_they_reach_the_application() {
    let (live, commands) = server();
    for path in [
        "/api/",
        "/api/test",
        "/api/test/",
        "/api//echo",
        "/api/Test/echo",
        "/api/1test/echo",
        "/api/test/echo/more",
        "/api/test/ec%68o",
        "/api/test/e_cho",
        "/api/../etc/passwd",
        "/api/test/..",
        "/api/a%2Fb/echo",
    ] {
        let a = live.post(path, "{}");
        assert_eq!(a.status, 404, "{path}");
    }
    let long = format!("/api/test/{}", "a".repeat(33));
    assert_eq!(live.post(&long, "{}").status, 404);
    assert_eq!(calls(&commands), 0);
    let ok = format!("/api/test/{}", "a".repeat(32));
    assert_eq!(live.post(&ok, "{}").status, 404, "valid name, unknown command");
    assert_eq!(calls(&commands), 1);
}

#[test]
fn an_unknown_command_is_404_and_a_failing_one_422_with_its_message_and_code() {
    let (live, commands) = server();
    let a = live.post("/api/nothing/here", "{}");
    assert_eq!(a.status, 404);
    assert_eq!(error_of(&a).1, "unknown-command");

    *commands.failing.lock().expect("failing") =
        Some(CommandError::failed("the daemon is not running").with_code("needs-daemon"));
    let a = live.post("/api/test/echo", "{}");
    assert_eq!(a.status, 422);
    assert_eq!(error_of(&a), ("the daemon is not running".to_owned(), "needs-daemon".to_owned()));
}

#[test]
fn a_command_that_panics_or_hangs_ends_its_request_not_the_server() {
    let commands = Arc::new(Scripted::default());
    let live = Live::start_with(commands.clone(), Duration::from_millis(300));
    let a = live.post("/api/test/panic", "{}");
    assert_eq!(a.status, 422);
    assert!(error_of(&a).0.contains("internal error"), "{a:?}");
    assert_eq!(live.post("/api/test/echo", "{}").status, 200, "the server is still up");

    *commands.delay.lock().expect("delay") = Duration::from_millis(1500);
    let a = live.post("/api/test/echo", "{}");
    assert_eq!(a.status, 504);
    assert_eq!(error_of(&a).1, "timeout");
}

#[test]
fn the_page_and_its_files_are_served_with_validators_and_nothing_else_is() {
    let (live, _) = server();
    let page = live.get("/");
    assert_eq!(page.status, 200);
    assert_eq!(page.header("Content-Type"), Some("text/html; charset=utf-8"));
    assert_eq!(page.header("Cache-Control"), Some("no-cache"));
    assert!(page.text().contains("<script type=\"module\" src=\"js/app.js\">"));
    assert_eq!(live.get("/index.html").body, page.body);
    let etag = page.header("ETag").expect("etag").to_owned();
    let again = live.send("GET", "/", vec![("If-None-Match", etag)], "");
    assert_eq!((again.status, again.body.len()), (304, 0));
    assert_eq!(live.get("/js/app.js").header("Content-Type"), Some("text/javascript; charset=utf-8"));
    assert_eq!(live.get("/app.css").header("Content-Type"), Some("text/css; charset=utf-8"));
    assert_eq!(live.get("/logo.svg").header("Content-Type"), Some("image/svg+xml"));
    for path in [
        "/js",
        "/js/",
        "/nothing",
        "/../Cargo.toml",
        "/js/../app.css",
        "//app.css",
        "/%2e%2e/Cargo.toml",
        "/js/pages",
        "/.env",
    ] {
        assert_eq!(live.get(path).status, 404, "{path}");
    }
    // The query string and fragment are not part of the path.
    assert_eq!(live.get("/?x=1").status, 200);
    assert_eq!(live.get("/js/app.js?v=2").status, 200);
}

// ---- the one-time launch code

fn exchange(live: &Live, body: &str) -> Answer {
    client::post_json(live.port, "/api/gui/session", None, body, Duration::from_secs(5)).expect("exchange")
}

fn mint(live: &Live) -> String {
    live.server.mint_code().expect("code")
}

#[test]
fn a_launch_code_is_exchanged_once_for_the_token_without_a_token() {
    let (live, _) = server();
    let code = mint(&live);
    let first = exchange(&live, &json!({ "code": code }).to_string());
    assert_eq!(first.status, 200);
    assert_eq!(first.json().expect("json")["result"]["token"], json!(TOKEN));
    let second = exchange(&live, &json!({ "code": code }).to_string());
    assert_eq!(second.status, 401, "a code works once");
    assert_eq!(error_of(&second).1, "unauthorised");
    // The token itself is not a code.
    assert_eq!(exchange(&live, &json!({ "code": TOKEN }).to_string()).status, 401);
}

#[test]
fn a_launch_code_expires_and_the_unused_ones_are_bounded() {
    let (live, _) = server();
    let old = mint(&live);
    live.clock.advance(LAUNCH_CODE_LIFE + Duration::from_secs(1));
    assert_eq!(exchange(&live, &json!({ "code": old }).to_string()).status, 401, "expired");
    let fresh = mint(&live);
    live.clock.advance(LAUNCH_CODE_LIFE - Duration::from_secs(1));
    assert_eq!(exchange(&live, &json!({ "code": fresh }).to_string()).status, 200, "just before the end");

    let codes: Vec<String> = (0..20).map(|_| mint(&live)).collect();
    assert_eq!(
        exchange(&live, &json!({ "code": codes[0] }).to_string()).status,
        401,
        "the oldest was dropped"
    );
    assert_eq!(exchange(&live, &json!({ "code": codes[19] }).to_string()).status, 200);
}

#[test]
fn the_exchange_refuses_everything_that_is_not_a_json_post_with_a_code() {
    let (live, _) = server();
    let code = mint(&live);
    let get = live.get("/api/gui/session");
    assert_eq!(get.status, 405);
    let wrong_type =
        live.send("POST", "/api/gui/session", vec![("Content-Type", "text/plain".to_owned())], "{}");
    assert_eq!(wrong_type.status, 415);
    for body in ["", "{}", "[]", "{\"code\":5}", "{\"code\":null}", "nope", "{\"code\":\"x\""] {
        assert_eq!(exchange(&live, body).status, 400, "{body:?}");
    }
    let huge = json!({ "code": "x".repeat(5000) }).to_string();
    assert_eq!(exchange(&live, &huge).status, 400);
    // None of those used the real code up.
    assert_eq!(exchange(&live, &json!({ "code": code }).to_string()).status, 200);
    // The exchange is subject to the Host and Origin checks like everything else.
    let rebinding = live.send(
        "POST",
        "/api/gui/session",
        vec![("Content-Type", "application/json".to_owned()), ("Host", "evil.example".to_owned())],
        "{\"code\":\"x\"}",
    );
    assert_eq!(rebinding.status, 403);
}

#[test]
fn gui_launch_gives_a_code_only_to_a_caller_with_the_token() {
    let (live, _) = server();
    let denied =
        client::post_json(live.port, "/api/gui/launch", None, "{}", Duration::from_secs(5)).expect("post");
    assert_eq!(denied.status, 401);
    let code = live.post("/api/gui/launch", "{}").json().expect("json")["result"]["code"]
        .as_str()
        .expect("code")
        .to_owned();
    assert_eq!(exchange(&live, &json!({ "code": code }).to_string()).status, 200);
}

#[test]
fn gui_ping_and_quit_are_answered_by_the_server_itself() {
    let (live, commands) = server();
    let ping = live.post("/api/gui/ping", "{}").json().expect("json");
    assert_eq!(ping["result"]["version"], json!("9.9.9"));
    assert!(!live.server.quit_requested());
    let quit = live.post("/api/gui/quit", "{}").json().expect("json");
    assert_eq!(quit["result"], Value::Bool(true));
    assert!(live.server.quit_requested());
    assert_eq!(calls(&commands), 0, "neither reached the application");
}
