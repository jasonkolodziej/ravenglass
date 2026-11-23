//! The JMAP session resource (RFC 8620 §2) and the URL templates it carries.

use mail_core::{MailError, Result, Timestamp};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub api_url: String,
    /// The account that holds mail (`primaryAccounts` for the mail capability).
    pub account_id: String,
    download_template: String,
    upload_template: String,
}

impl Session {
    pub fn parse(json: &[u8]) -> Result<Self> {
        let v: Value = serde_json::from_slice(json).map_err(|e| MailError::Server(format!("unreadable JMAP session: {e}")))?;
        let text = |key: &str| v[key].as_str().map(str::to_owned).ok_or_else(|| MailError::Server(format!("JMAP session has no {key}")));
        let account_id = v["primaryAccounts"]["urn:ietf:params:jmap:mail"].as_str().ok_or_else(|| MailError::Server("this JMAP account has no mail".into()))?.to_owned();
        let session = Self { api_url: text("apiUrl")?, account_id, download_template: text("downloadUrl")?, upload_template: text("uploadUrl")? };
        // Every later request carries the credentials, so none may leave HTTPS.
        if [&session.api_url, &session.download_template, &session.upload_template].iter().any(|u| !u.starts_with("https://")) {
            return Err(MailError::Server("the JMAP session names a URL that is not https".into()));
        }
        Ok(session)
    }

    pub fn download_url(&self, blob: &str) -> String {
        self.download_template.replace("{accountId}", &encode(&self.account_id)).replace("{blobId}", &encode(blob)).replace("{name}", "message.eml").replace("{type}", "message%2Frfc822")
    }

    pub fn upload_url(&self) -> String { self.upload_template.replace("{accountId}", &encode(&self.account_id)) }
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') { out.push(b as char); } else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

/// `2026-11-13T16:44:00Z` (JMAP `UTCDate`, or a `Date` with an offset) to Unix seconds.
pub fn parse_utc(s: &str) -> Option<Timestamp> {
    let num = |range: std::ops::Range<usize>| s.get(range)?.parse::<i64>().ok();
    let (y, m, d, h, min, sec) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) { return None; }
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if m <= 2 { y - 1 } else { y };
    let (era, yoe) = (y2.div_euclid(400), y2.rem_euclid(400));
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let tail = s.get(19..)?.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match tail.as_bytes() {
        [b'Z'] | [] => 0,
        [sign @ (b'+' | b'-'), ..] => {
            let (oh, om) = (tail.get(1..3)?.parse::<i64>().ok()?, tail.get(4..6)?.parse::<i64>().ok()?);
            (oh * 3600 + om * 60) * if *sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    Some(days * 86_400 + h * 3600 + min * 60 + sec - offset)
}
