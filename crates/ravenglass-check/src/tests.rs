use crate::args::{self, Parsed};
use crate::body;
use crate::settings::{describe, host_label, url_label};
use mail_imap::{Endpoint, Security};

fn parse(words: &[&str]) -> Result<Parsed, String> { args::parse(words.iter().map(|w| w.to_string())) }

#[test]
fn arguments_parse_and_secrets_are_refused() {
    let Ok(Parsed::Run(args)) = parse(&["--imap", "mail.example.org:143", "--smtp", "mail.example.org:587", "--write", "--idle", "0", "me@example.org"]) else { panic!() };
    assert_eq!(args.address, "me@example.org");
    assert_eq!(args.imap, Some(Endpoint::new("mail.example.org", 143, Security::StartTls)));
    assert_eq!(args.smtp, Some(Endpoint::new("mail.example.org", 587, Security::StartTls)));
    assert!(args.write && args.idle_secs == 0 && args.limit == 200);
    assert_eq!(parse(&["-h"]), Ok(Parsed::Help));
    for bad in [&["--password", "hunter2", "me@example.org"][..], &["not-an-address"], &[], &["a@example.org", "b@example.org"], &["--jmap", "http://x/session", "me@example.org"],
        &["--imap", "host:993:plain", "me@example.org"], &["--limit", "0", "me@example.org"], &["--token", "--oauth", "me@example.org"], &["--bogus", "me@example.org"]] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
    assert_eq!(args::endpoint("imap.example.org", 993), Ok(Endpoint::new("imap.example.org", 993, Security::Tls)));
    assert_eq!(args::endpoint("smtp.example.org:465:tls", 465), Ok(Endpoint::new("smtp.example.org", 465, Security::Tls)));
}

#[test]
fn the_users_domain_is_masked_and_providers_are_not() {
    assert_eq!(host_label("imap.example.org", "example.org"), "imap.<your domain>");
    assert_eq!(host_label("Mail.Example.org", "example.org"), "mail.<your domain>");
    assert_eq!(host_label("example.org", "example.org"), "<your domain>");
    assert_eq!(host_label("imap.gmail.com", "example.org"), "imap.gmail.com");
    // A host that only ends with the same letters is not the domain.
    assert_eq!(host_label("imap.notexample.org", "example.org"), "imap.notexample.org");
    assert_eq!(describe(&Endpoint::new("smtp.example.org", 587, Security::StartTls), "example.org"), "smtp.<your domain>:587 STARTTLS");
    assert_eq!(url_label("https://jmap.example.org:8443/.well-known/jmap", "example.org"), "https://jmap.<your domain>:8443/.well-known/jmap");
    assert_eq!(url_label("https://api.fastmail.com/jmap/session", "fastmail.com"), "https://api.<your domain>/jmap/session");
}

#[test]
fn a_body_is_described_by_its_shape_only() {
    let raw = b"From: Someone <someone@example.org>\r\nTo: me@example.org\r\nSubject: Secret plans\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=m\r\n\r\n--m\r\nContent-Type: multipart/alternative; boundary=a\r\n\r\n--a\r\nContent-Type: text/plain\r\n\r\nThe plans, see https://example.org/plans\r\n--a\r\nContent-Type: text/html\r\n\r\n<p>The plans</p><img src=\"https://tracker.example/open.gif\" width=1 height=1><img src=\"https://cdn.example/logo.png\">\r\n--a--\r\n--m\r\nContent-Type: application/pdf; name=\"plans.pdf\"\r\nContent-Disposition: attachment; filename=\"plans.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--m--\r\n";
    let shape = body::shape(raw).unwrap();
    assert_eq!(shape.parts, ["multipart/mixed", "  multipart/alternative", "    text/plain", "    text/html", "  application/pdf"]);
    assert!(shape.has_text && shape.has_html);
    assert_eq!((shape.remote_images, shape.trackers_removed, shape.attachments), (1, 1, 1));
    assert_eq!(shape.links, 1);
    // Nothing of the content is in what gets printed.
    let printed = format!("{shape:?}");
    for secret in ["Secret", "plans", "someone", "example.org", "tracker.example", "cdn"] { assert!(!printed.contains(secret), "{secret} in {printed}"); }
}
