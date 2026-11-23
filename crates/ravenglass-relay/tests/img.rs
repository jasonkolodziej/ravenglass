//! The image proxy over real HTTP: what it fetches, what it sends upstream, and every refusal.

mod common;

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use common::*;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really a png";

/// An image host on loopback. Returns its base URL and the headers of every request it got.
fn image_host() -> (String, Arc<Mutex<Vec<HeaderMap>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    let (ready, address) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            ready.send(listener.local_addr().unwrap()).unwrap();
            let app = axum::Router::new()
                .route("/ok.png", get(move |headers: HeaderMap| async move { record.lock().unwrap().push(headers); ([(header::CONTENT_TYPE, "image/png"), (header::SET_COOKIE, "tracker=1")], PNG) }))
                .route("/to-internal", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "http://10.0.0.1/x.png")]) }))
                .route("/to-metadata", get(|| async { (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, "http://169.254.169.254/latest/meta-data/")]) }))
                .route("/chain/{n}", get(|Path(n): Path<u32>| async move {
                    let next = if n == 0 { "/ok.png".to_owned() } else { format!("/chain/{}", n - 1) };
                    (StatusCode::FOUND, [(header::LOCATION, next)])
                }))
                .route("/page", get(|| async { ([(header::CONTENT_TYPE, "text/html")], "<script>alert(1)</script>") }))
                .route("/svgish", get(|| async { ([(header::CONTENT_TYPE, "image/")], "x") }))
                .route("/big.png", get(|| async { ([(header::CONTENT_TYPE, "image/png")], vec![0u8; 11 << 20]) }))
                .route("/missing.png", get(|| async { StatusCode::NOT_FOUND.into_response() }))
                .route("/slow.png", get(|| async { tokio::time::sleep(std::time::Duration::from_secs(15)).await; ([(header::CONTENT_TYPE, "image/png")], PNG) }));
            axum::serve(listener, app).await.unwrap();
        });
    });
    (format!("http://{}", address.recv().unwrap()), seen)
}

fn loopback_only(config: &mut ravenglass_relay::server::UsersConfig, _: &str) {
    config.image_policy = Arc::new(|ip: IpAddr| ip == IpAddr::from([127, 0, 0, 1]));
}

fn proxy(relay: &Relay, session: &Session, url: &str) -> Answer { session.get(&relay.base, &relay_proto::auth::img(url)) }

#[test]
fn an_image_comes_back_privately_cached_without_cookies_or_referrer_sent() {
    let relay = start(loopback_only);
    let (host, seen) = image_host();
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    let answer = proxy(&relay, &alice, &format!("{host}/ok.png"));
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(answer.body, PNG);
    assert_eq!(answer.header("content-type"), Some("image/png"));
    assert_eq!(answer.header("cache-control"), Some("private, max-age=86400"));
    assert!(answer.header("set-cookie").is_none(), "the image host's cookie is not passed on");
    assert_eq!(answer.header("x-content-type-options"), Some("nosniff"));
    let sent = seen.lock().unwrap()[0].clone();
    assert!(sent.get(header::COOKIE).is_none() && sent.get(header::REFERER).is_none() && sent.get(header::AUTHORIZATION).is_none(), "{sent:?}");
    assert_eq!(proxy(&relay, &alice, &format!("{host}/chain/2")).status, 200, "three redirects are fine");
}

#[test]
fn redirects_are_checked_again_and_limited_to_three() {
    let relay = start(loopback_only);
    let (host, _) = image_host();
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    assert_eq!(proxy(&relay, &alice, &format!("{host}/to-internal")).status, 403);
    assert_eq!(proxy(&relay, &alice, &format!("{host}/to-metadata")).status, 403);
    assert_eq!(proxy(&relay, &alice, &format!("{host}/chain/3")).status, 502, "a fourth is not");
}

#[test]
fn only_images_under_ten_megabytes_are_returned() {
    let relay = start(loopback_only);
    let (host, _) = image_host();
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    assert_eq!(proxy(&relay, &alice, &format!("{host}/page")).status, 415);
    assert_eq!(proxy(&relay, &alice, &format!("{host}/svgish")).status, 415);
    assert_eq!(proxy(&relay, &alice, &format!("{host}/big.png")).status, 413);
    assert_eq!(proxy(&relay, &alice, &format!("{host}/missing.png")).status, 502);
}

#[test]
fn a_slow_image_host_is_cut_off_after_ten_seconds() {
    let relay = start(loopback_only);
    let (host, _) = image_host();
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    let started = std::time::Instant::now();
    assert_eq!(proxy(&relay, &alice, &format!("{host}/slow.png")).status, 504);
    assert!(started.elapsed() < std::time::Duration::from_secs(14));
}

#[test]
fn internal_addresses_and_odd_urls_are_refused_with_the_real_policy() {
    let relay = start(|_, _| {});
    let (host, seen) = image_host();
    let alice = sign_in(&relay.base, "alice", ALICE_PW);
    assert_eq!(proxy(&relay, &alice, &format!("{host}/ok.png")).status, 403, "loopback");
    for url in ["http://127.0.0.1/a.png", "http://localhost/a.png", "http://[::1]/a.png", "http://10.1.1.1/a.png", "http://192.168.0.1/a.png", "http://172.16.0.1/a.png", "http://169.254.169.254/latest/meta-data/", "http://100.64.0.1/a.png", "http://[fd00:ec2::254]/", "http://224.0.0.1/a.png", "http://0.0.0.0/a.png", "http://[::ffff:127.0.0.1]/a.png"] {
        assert_eq!(proxy(&relay, &alice, url).status, 403, "{url}");
    }
    for url in ["file:///etc/passwd", "ftp://example.com/a.png", "gopher://example.com/", "http://user:pass@example.com/a.png", "data:image/png;base64,AA=="] {
        assert_eq!(proxy(&relay, &alice, url).status, 400, "{url}");
    }
    assert_eq!(alice.get(&relay.base, "/v1/img").status, 400);
    assert!(seen.lock().unwrap().is_empty(), "nothing reached the loopback host");
}

#[test]
fn the_proxy_is_for_signed_in_users_only() {
    let relay = start(loopback_only);
    let (host, seen) = image_host();
    let url = format!("{}{}", relay.base, relay_proto::auth::img(&format!("{host}/ok.png")));
    assert_eq!(call("GET", &url, &[], None).status, 401);
    assert!(seen.lock().unwrap().is_empty());
}
