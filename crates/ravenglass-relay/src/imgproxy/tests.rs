use super::*;

fn ip(s: &str) -> IpAddr { s.parse().unwrap() }

#[test]
fn internal_and_special_addresses_are_not_public() {
    for refused in [
        "127.0.0.1", "127.255.0.9", "0.0.0.0", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.1", "169.254.169.254",
        "169.254.0.1", "100.64.0.1", "100.127.255.254", "224.0.0.1", "239.1.1.1", "255.255.255.255", "240.0.0.1", "192.0.2.1",
        "198.18.0.1", "198.51.100.7", "203.0.113.9", "192.0.0.8",
        "::", "::1", "::ffff:127.0.0.1", "::ffff:169.254.169.254", "::ffff:10.0.0.1", "::127.0.0.1", "fc00::1", "fd00:ec2::254",
        "fe80::1", "fec0::1", "ff02::1", "2001:db8::1", "64:ff9b::a00:1", "2002:a00:1::1", "2001::1", "100::1",
    ] {
        assert!(!is_public(ip(refused)), "{refused} must be refused");
    }
}

#[test]
fn ordinary_internet_addresses_are_public() {
    for allowed in ["1.1.1.1", "8.8.8.8", "93.184.216.34", "172.32.0.1", "100.128.0.1", "2606:4700:4700::1111", "2a00:1450:4001::1", "::ffff:8.8.8.8", "64:ff9b::808:808"] {
        assert!(is_public(ip(allowed)), "{allowed} must be allowed");
    }
}

fn run<F: std::future::Future>(f: F) -> F::Output { tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f) }

#[test]
fn urls_that_are_not_plain_http_are_refused_before_any_lookup() {
    let allow = public_only();
    for bad in ["file:///etc/passwd", "ftp://example.com/a.png", "gopher://example.com/", "http://user:pw@example.com/a.png", "javascript:alert(1)", "not a url", "data:image/png;base64,AAAA"] {
        assert_eq!(run(fetch(bad, &allow)), Err(ImgError::Fetch(FetchError::BadUrl)), "{bad}");
    }
}

#[test]
fn literal_internal_addresses_are_refused_without_connecting() {
    let allow = public_only();
    for bad in ["http://127.0.0.1/x.png", "http://[::1]/x.png", "http://169.254.169.254/latest/meta-data/", "https://10.0.0.1/a.png", "http://0x7f000001/a.png", "http://2130706433/a.png", "http://[::ffff:7f00:1]/a.png", "http://100.64.1.1/a.png", "http://224.0.0.1/a.png"] {
        assert_eq!(run(fetch(bad, &allow)), Err(ImgError::Fetch(FetchError::Forbidden)), "{bad}");
    }
}

#[test]
fn a_name_that_resolves_to_loopback_is_refused() {
    assert_eq!(run(fetch("http://localhost:9/x.png", &public_only())), Err(ImgError::Fetch(FetchError::Forbidden)));
}
