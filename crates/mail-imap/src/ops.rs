//! The IMAP side of each `MailBackend` call, as functions over an open [`Conn`].

use crate::conn::{Conn, FETCH_FULL, FETCH_NO_STRUCTURE, FlagCache, Inner, quote};
use crate::envelope::{self, Fetched, HEADER_FIELDS, PREVIEW_BYTES};
use crate::folders::{self, Listed};
use crate::ids::{MessageRef, SyncState, chunked_sets, folder_id, mailbox_of, message_id, parse_message_id};
use crate::net::{Failure, timed};
use async_imap::imap_proto::{AttributeValue, MailboxDatum, MessageSection, Response, SectionPath, StatusAttribute};
use mail_core::*;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;

/// On a server that cannot do partial fetches, only messages up to this size have their text
/// fetched for a preview. Larger ones get none rather than a download during sync.
const WHOLE_TEXT_LIMIT: u32 = 128 * 1024;

type Outcome<T> = std::result::Result<T, Failure>;

fn not_found(what: &str) -> Failure { MailError::NotFound(what.into()).into() }

async fn list(conn: &mut Conn) -> Outcome<Vec<Listed>> {
    let mut listed = Vec::new();
    conn.run("LIST \"\" \"*\"", |response| {
        if let Response::MailboxData(MailboxDatum::List(entry)) = response {
            listed.extend(folders::listed(&entry.name, entry.delimiter.as_deref(), &entry.name_attributes));
        }
    }).await?;
    Ok(listed)
}

/// LIST with roles, remembered for [`role_mailbox`].
async fn list_with_roles(inner: &Inner, conn: &mut Conn) -> Outcome<Vec<(Listed, FolderRole)>> {
    let listed = list(conn).await?;
    let roles = folders::assign_roles(&listed);
    let known: Vec<_> = listed.into_iter().zip(roles).collect();
    *inner.folders.lock().unwrap() = Some(known.clone());
    Ok(known)
}

pub async fn list_folders(inner: &Inner, conn: &mut Conn) -> Outcome<Vec<Folder>> {
    let known = list_with_roles(inner, conn).await?;
    let mut out = Vec::with_capacity(known.len());
    for (listed, role) in &known {
        let mut folder = folders::to_folder(&inner.config.account, listed, *role);
        let (total, unread) = (&mut folder.total, &mut folder.unread);
        let counted = conn.run(&format!("STATUS {} (MESSAGES UNSEEN)", quote(&listed.mailbox)?), |response| {
            let Response::MailboxData(MailboxDatum::Status { status, .. }) = response else { return };
            for attribute in status {
                match attribute {
                    StatusAttribute::Messages(count) => *total = *count,
                    StatusAttribute::Unseen(count) => *unread = *count,
                    _ => {}
                }
            }
        }).await;
        // A folder the server will not count is still a folder.
        if let Err(failure) = counted && !failure.reusable { return Err(failure); }
        out.push(folder);
    }
    Ok(out)
}

/// The parts of one FETCH response this crate asks for.
#[derive(Default)]
struct Item<'a> {
    uid: Option<u32>,
    flags: Option<Flags>,
    received: Option<Timestamp>,
    size: Option<u32>,
    header: Option<&'a [u8]>,
    text: Option<&'a [u8]>,
    /// `BODY[]`: the whole message.
    whole: Option<&'a [u8]>,
    attachments: Vec<Attachment>,
}

fn item<'a>(response: &'a Response<'_>) -> Option<Item<'a>> {
    let Response::Fetch(_, attributes) = response else { return None };
    let mut item = Item::default();
    for attribute in attributes {
        match attribute {
            AttributeValue::Uid(uid) => item.uid = Some(*uid),
            AttributeValue::Flags(names) => item.flags = Some(envelope::flags_from(names.iter().map(|n| n.as_ref()))),
            AttributeValue::InternalDate(date) => item.received = envelope::internal_date(date),
            AttributeValue::Rfc822Size(size) => item.size = Some(*size),
            AttributeValue::BodyStructure(structure) => item.attachments = envelope::attachments(structure),
            AttributeValue::BodySection { section: Some(SectionPath::Full(MessageSection::Header)), data: Some(data), .. } => item.header = Some(data),
            AttributeValue::BodySection { section: Some(SectionPath::Full(MessageSection::Text)), data, .. } => item.text = Some(data.as_deref().unwrap_or_default()),
            AttributeValue::BodySection { section: None, data: Some(data), .. } => item.whole = Some(data),
            _ => {}
        }
    }
    Some(item)
}

/// A message whose headers have arrived and whose preview text has not.
struct Waiting { header: Vec<u8>, flags: Flags, received: Option<Timestamp>, attachments: Vec<Attachment>, small: bool }

/// Envelopes for the UIDs in `set` that are at least `min_uid` (`n:*` always returns the last
/// message, even when its UID is below `n`). What is asked of the server depends on
/// `Inner::fetch_level`.
async fn fetch_envelopes(inner: &Inner, conn: &mut Conn, folder: &FolderId, validity: u32, set: &str, min_uid: u32) -> Outcome<Vec<(u32, Envelope)>> {
    let level = inner.fetch_level.load(Ordering::Relaxed);
    let structure = if level < FETCH_NO_STRUCTURE { " BODYSTRUCTURE" } else { "" };
    let common = format!("UID FLAGS INTERNALDATE{structure} BODY.PEEK[HEADER.FIELDS ({HEADER_FIELDS})]");
    let mut out = Vec::new();
    let envelope_for = |uid: u32, fetched: Fetched<'_>| (uid, envelope::build(message_id(folder, validity, uid), folder.clone(), fetched));

    if level == FETCH_FULL {
        conn.run(&format!("UID FETCH {set} ({common} BODY.PEEK[TEXT]<0.{PREVIEW_BYTES}>)"), |response| {
            // Without a header this is a flag update for some other message, not an answer.
            let Some(Item { uid: Some(uid), header: Some(header), flags, received, text, attachments, .. }) = item(response) else { return };
            if uid >= min_uid { out.push(envelope_for(uid, Fetched { header, text: text.unwrap_or_default(), flags: flags.unwrap_or_default(), received, attachments })); }
        }).await?;
        return Ok(out);
    }

    let mut waiting: BTreeMap<u32, Waiting> = BTreeMap::new();
    conn.run(&format!("UID FETCH {set} ({common} RFC822.SIZE)"), |response| {
        let Some(Item { uid: Some(uid), header: Some(header), flags, received, size, attachments, .. }) = item(response) else { return };
        if uid >= min_uid {
            waiting.insert(uid, Waiting { header: header.to_vec(), flags: flags.unwrap_or_default(), received, attachments, small: size.is_some_and(|s| s <= WHOLE_TEXT_LIMIT) });
        }
    }).await?;
    let small: Vec<u32> = waiting.iter().filter(|(_, w)| w.small).map(|(uid, _)| *uid).collect();
    let mut texts: HashMap<u32, Vec<u8>> = HashMap::new();
    for set in chunked_sets(&small) {
        conn.run(&format!("UID FETCH {set} (UID BODY.PEEK[TEXT])"), |response| {
            if let Some(Item { uid: Some(uid), text: Some(text), .. }) = item(response) { texts.insert(uid, text[..text.len().min(PREVIEW_BYTES)].to_vec()); }
        }).await?;
    }
    for (uid, w) in waiting {
        let text = texts.get(&uid).map_or(&[][..], Vec::as_slice);
        out.push(envelope_for(uid, Fetched { header: &w.header, text, flags: w.flags, received: w.received, attachments: w.attachments }));
    }
    Ok(out)
}

/// Current flags of the messages in `set`; with `changed_since`, only those CONDSTORE says changed.
async fn fetch_flags(conn: &mut Conn, set: &str, changed_since: Option<u64>) -> Outcome<HashMap<u32, Flags>> {
    let command = match changed_since { Some(modseq) => format!("UID FETCH {set} (UID FLAGS) (CHANGEDSINCE {modseq})"), None => format!("UID FETCH {set} (UID FLAGS)") };
    let mut out = HashMap::new();
    conn.run(&command, |response| if let Some(Item { uid: Some(uid), flags: Some(flags), .. }) = item(response) { out.insert(uid, flags); }).await?;
    Ok(out)
}

async fn search(conn: &mut Conn, criteria: &str) -> Outcome<Vec<u32>> {
    let mut found = Vec::new();
    conn.run(&format!("UID SEARCH {criteria}"), |response| if let Response::MailboxData(MailboxDatum::Search(uids)) = response { found.extend(uids); }).await?;
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

/// See `docs/ARCHITECTURE.md` §5 and the token description in [`crate::ids`].
pub async fn sync_folder(inner: &Inner, conn: &mut Conn, folder: &FolderId, since: &SyncToken) -> Outcome<FolderDelta> {
    let mailbox = mailbox_of(&inner.config.account, folder)?;
    let selected = conn.select(mailbox).await?;
    let validity = selected.validity;
    let mut delta = FolderDelta::default();

    let previous = match SyncState::decode(since) {
        Some(state) if state.validity == validity => Some(state),
        Some(stale) => {
            // UIDVALIDITY changed: every id handed out before is void. The token remembers them.
            delta.removed = stale.uids.iter().map(|uid| message_id(folder, stale.validity, *uid)).collect();
            None
        }
        None => None,
    };

    // New mail, or everything on a full sync.
    let first_new = previous.as_ref().map_or(1, |p| p.uid_next.max(1));
    // UIDNEXT says whether anything arrived; the count is a second opinion in case it is stale.
    let known = previous.as_ref().map_or(0, |p| p.uids.len());
    let may_have_new = selected.exists > 0 && (selected.uid_next.is_none_or(|next| next > first_new) || selected.exists as usize > known);
    let added = if may_have_new { fetch_envelopes(inner, conn, folder, validity, &format!("{first_new}:*"), first_new).await? } else { Vec::new() };

    // Messages already known: which are gone, and whose flags changed.
    let mut flags_now: Option<HashMap<u32, Flags>> = None;
    let mut kept: Vec<u32> = Vec::new();
    if let Some(previous) = previous.as_ref().filter(|p| !p.uids.is_empty()) {
        let old_range = format!("1:{}", first_new - 1);
        let condstore = match selected.highest_modseq { Some(now) if conn.caps.condstore && previous.modseq > 0 => Some(now), _ => None };
        let present: Option<Vec<u32>> = if let Some(modseq_now) = condstore {
            if modseq_now != previous.modseq {
                let changed = fetch_flags(conn, &old_range, Some(previous.modseq)).await?;
                delta.changed_flags = previous.uids.iter().filter_map(|uid| changed.get(uid).map(|flags| (message_id(folder, validity, *uid), *flags))).collect();
            }
            // EXISTS counts the known messages that are left plus the new ones, so equality means none went.
            if selected.exists as usize == previous.uids.len() + added.len() { None } else { Some(search(conn, &format!("UID {old_range}")).await?) }
        } else {
            // The portable path: one FLAGS fetch of the known range answers both questions.
            let current = fetch_flags(conn, &old_range, None).await?;
            {
                let cache = inner.flag_cache.lock().unwrap();
                let before = cache.get(folder).filter(|c| &c.token == since).map(|c| &c.flags);
                delta.changed_flags = previous.uids.iter().filter_map(|uid| {
                    let flags = current.get(uid)?;
                    (before.and_then(|b| b.get(uid)) != Some(flags)).then(|| (message_id(folder, validity, *uid), *flags))
                }).collect();
            }
            let mut found: Vec<u32> = current.keys().copied().collect();
            found.sort_unstable();
            flags_now = Some(current);
            Some(found)
        };
        match present {
            None => kept = previous.uids.clone(),
            Some(found) => {
                for uid in &previous.uids {
                    if found.binary_search(uid).is_ok() { kept.push(*uid); } else { delta.removed.push(message_id(folder, validity, *uid)); }
                }
            }
        }
    }

    let highest_added = added.iter().map(|(uid, _)| *uid).max().unwrap_or(0);
    let mut uids = kept;
    uids.extend(added.iter().map(|(uid, _)| *uid));
    uids.sort_unstable();
    uids.dedup();
    let state = SyncState {
        validity,
        uid_next: selected.uid_next.unwrap_or(0).max(highest_added.saturating_add(1)).max(first_new),
        modseq: if conn.caps.condstore { selected.highest_modseq.unwrap_or(0) } else { 0 },
        uids,
    };
    delta.token = state.encode();

    // Remember the flags this token stands for, so the next portable sync reports only changes.
    if state.modseq == 0 {
        let mut flags = flags_now.unwrap_or_default();
        flags.retain(|uid, _| state.uids.binary_search(uid).is_ok());
        flags.extend(added.iter().map(|(uid, envelope)| (*uid, envelope.flags)));
        inner.flag_cache.lock().unwrap().insert(folder.clone(), FlagCache { token: delta.token.clone(), flags });
    } else {
        inner.flag_cache.lock().unwrap().remove(folder);
    }

    delta.added = added.into_iter().map(|(_, envelope)| envelope).collect();
    Ok(delta)
}

/// Message ids as UIDs per folder and UIDVALIDITY.
fn grouped(msgs: &[MessageId]) -> Outcome<BTreeMap<(FolderId, u32), Vec<u32>>> {
    let mut groups: BTreeMap<(FolderId, u32), Vec<u32>> = BTreeMap::new();
    for id in msgs {
        let MessageRef { folder, validity, uid } = parse_message_id(id)?;
        groups.entry((folder, validity)).or_default().push(uid);
    }
    Ok(groups)
}

/// Select `folder`. An id from an older UIDVALIDITY no longer names anything.
async fn select_for(inner: &Inner, conn: &mut Conn, folder: &FolderId, validity: u32) -> Outcome<()> {
    let mailbox = mailbox_of(&inner.config.account, folder)?;
    if conn.ensure_selected(mailbox).await? != validity { return Err(not_found("message")); }
    Ok(())
}

pub async fn fetch_body(inner: &Inner, conn: &mut Conn, msg: &MessageId) -> Outcome<RawMessage> {
    let MessageRef { folder, validity, uid } = parse_message_id(msg)?;
    select_for(inner, conn, &folder, validity).await?;
    let mut bytes = None;
    // A whole message can be large and arrives as one response.
    conn.run_within(conn.limit * 10, &format!("UID FETCH {uid} (UID BODY.PEEK[])"), |response| {
        if let Some(Item { uid: Some(found), whole: Some(whole), .. }) = item(response) && found == uid { bytes = Some(whole.to_vec()); }
    }).await?;
    Ok(RawMessage { id: msg.clone(), bytes: bytes.ok_or_else(|| not_found("message"))? })
}

pub async fn set_flags(inner: &Inner, conn: &mut Conn, msgs: &[MessageId], add: Flags, remove: Flags) -> Outcome<()> {
    for ((folder, validity), uids) in grouped(msgs)? {
        select_for(inner, conn, &folder, validity).await?;
        for set in chunked_sets(&uids) {
            if let Some(list) = envelope::flag_list(add) { conn.run(&format!("UID STORE {set} +FLAGS.SILENT {list}"), |_| {}).await?; }
            if let Some(list) = envelope::flag_list(remove) { conn.run(&format!("UID STORE {set} -FLAGS.SILENT {list}"), |_| {}).await?; }
        }
    }
    Ok(())
}

pub async fn move_to(inner: &Inner, conn: &mut Conn, msgs: &[MessageId], to: &FolderId) -> Outcome<()> {
    let target = quote(mailbox_of(&inner.config.account, to)?)?;
    for ((folder, validity), uids) in grouped(msgs)? {
        if &folder == to { continue; }
        select_for(inner, conn, &folder, validity).await?;
        for set in chunked_sets(&uids) {
            if conn.caps.move_ {
                conn.run(&format!("UID MOVE {set} {target}"), |_| {}).await?;
                continue;
            }
            conn.run(&format!("UID COPY {set} {target}"), |_| {}).await?;
            conn.run(&format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)"), |_| {}).await?;
            // Without UIDPLUS, EXPUNGE cannot be limited to these messages: anything else already
            // marked \Deleted in the folder goes with them.
            let expunge = if conn.caps.uidplus { format!("UID EXPUNGE {set}") } else { "EXPUNGE".to_owned() };
            conn.run(&expunge, |_| {}).await?;
        }
    }
    Ok(())
}

/// The mailbox that has `role`, created under its usual name if the account has none.
async fn role_mailbox(inner: &Inner, conn: &mut Conn, role: FolderRole, usual_name: &str) -> Outcome<String> {
    let cached = inner.folders.lock().unwrap().clone();
    let known = match cached { Some(known) => known, None => list_with_roles(inner, conn).await? };
    if let Some((listed, _)) = known.iter().find(|(_, r)| *r == role) { return Ok(listed.mailbox.clone()); }
    let listed: Vec<Listed> = known.into_iter().map(|(listed, _)| listed).collect();
    let name = folders::name_for_new(&listed, usual_name);
    conn.run(&format!("CREATE {}", quote(&name)?), |_| {}).await?;
    *inner.folders.lock().unwrap() = None;
    Ok(name)
}

async fn append(inner: &Inner, conn: &mut Conn, mailbox: &str, flags: &str, raw: &[u8]) -> Outcome<()> {
    // async-imap's APPEND handles the literal, and does check the tagged reply.
    let appended = timed(conn.limit * 10, conn.session.append(mailbox, Some(flags), None, raw)).await;
    // The remembered folder may have been deleted or renamed since the last LIST.
    if appended.is_err() { *inner.folders.lock().unwrap() = None; }
    appended
}

pub async fn append_draft(inner: &Inner, conn: &mut Conn, raw: &[u8]) -> Outcome<MessageId> {
    let mailbox = role_mailbox(inner, conn, FolderRole::Drafts, "Drafts").await?;
    let before = conn.select(&mailbox).await?;
    let floor = before.uid_next.unwrap_or(1).max(1);
    append(inner, conn, &mailbox, "(\\Draft \\Seen)", raw).await?;
    // async-imap does not surface APPENDUID, so look for what arrived at or after the old UIDNEXT.
    conn.run("NOOP", |_| {}).await?;
    let uid = search(conn, &format!("UID {floor}:*")).await?.into_iter().rfind(|uid| *uid >= floor)
        .ok_or_else(|| Failure::from(MailError::Server("the draft was saved but the server did not list it".into())))?;
    Ok(message_id(&folder_id(&inner.config.account, &mailbox), before.validity, uid))
}

pub async fn append_sent(inner: &Inner, conn: &mut Conn, raw: &[u8]) -> Outcome<()> {
    let mailbox = role_mailbox(inner, conn, FolderRole::Sent, "Sent").await?;
    append(inner, conn, &mailbox, "(\\Seen)", raw).await
}
