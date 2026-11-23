//! Compose sessions: one `ComposeSession` machine per open draft. Every edit is saved locally at
//! once; the server copy follows after a quiet period; sending goes through the outbox.

use crate::api::{DraftSource, Event};
use crate::engine::{Engine, Events, Outcome, text};
use mail_core::machine::compose::{self, Effect, State};
use mail_core::*;
use mail_render::compose as mime;
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Drafts {
    open: BTreeMap<DraftId, (Draft, State)>,
    /// Makes each `Message-ID` unique within one clock tick.
    counter: u64,
}

impl Engine {
    pub(crate) async fn start_draft(&mut self, id: DraftId, account: AccountId, source: DraftSource, out: &mut Events) -> Outcome {
        let me = self.accounts.get(&account).ok_or("unknown account")?.identity.clone();
        let draft = match source {
            DraftSource::Blank => Draft::new(id.clone(), account, me),
            DraftSource::Answer { message, kind } => {
                let raw = self.raw(&message).await?;
                mime::reply(&mail_render::parse(&raw.bytes).map_err(text)?, kind, id.clone(), account, &me)
            }
        };
        self.drafts.open.insert(id, (draft.clone(), compose::open()));
        out.push(Event::Draft(Box::new(draft)));
        Ok(())
    }

    pub(crate) fn edit_draft(&mut self, draft: Draft, now: f64, out: &mut Events) -> Outcome {
        if !self.accounts.contains_key(&draft.account) { return Err("unknown account".into()); }
        self.store.put_draft(&draft.id, &serde_json::to_string(&draft).map_err(text)?).map_err(text)?;
        let id = draft.id.clone();
        let entry = self.drafts.open.entry(id.clone()).or_insert_with(|| (draft.clone(), compose::open()));
        entry.0 = draft;
        for effect in compose::step(&mut entry.1, compose::Event::Edit, now) {
            if let Effect::WakeAt(t) = effect { out.push(Event::WakeAt(t)); }
        }
        out.push(Event::DraftSaved { id, on_server: false });
        Ok(())
    }

    /// Autosave: drafts whose quiet period has passed go to the account's Drafts folder.
    pub(crate) async fn draft_timers(&mut self, now: f64, out: &mut Events) {
        let ids: Vec<DraftId> = self.drafts.open.keys().cloned().collect();
        for id in ids {
            let Some((draft, state)) = self.drafts.open.get_mut(&id) else { continue };
            if !compose::step(state, compose::Event::Timer(now), now).contains(&Effect::AppendDraft) { continue; }
            self.drafts.counter += 1;
            let raw = mime::build(draft, &mime::message_id(&draft.from, now, self.drafts.counter), now as Timestamp);
            let saved = match self.accounts.get(&draft.account) {
                Some(account) => account.backend.clone().append_draft(&raw).await.is_ok(),
                None => false,
            };
            let Some((_, state)) = self.drafts.open.get_mut(&id) else { continue };
            let event = if saved { compose::Event::DraftSaved } else { compose::Event::SaveFailed };
            for effect in compose::step(state, event, now) {
                match effect {
                    Effect::WakeAt(t) => out.push(Event::WakeAt(t)),
                    Effect::Notify(line) => out.push(Event::Notice(line)),
                    _ => {}
                }
            }
            if saved { out.push(Event::DraftSaved { id, on_server: true }); }
        }
    }

    pub(crate) async fn send_draft(&mut self, id: DraftId, now: f64, out: &mut Events) -> Outcome {
        let (draft, _) = self.drafts.open.get(&id).ok_or("no such draft")?;
        let rcpt = draft.recipients();
        if rcpt.is_empty() { return Err("add a recipient first".into()); }
        self.drafts.counter += 1;
        let raw = mime::build(draft, &mime::message_id(&draft.from, now, self.drafts.counter), now as Timestamp);
        let (account, from) = (draft.account.clone(), draft.from.clone());
        let outbox = self.queue_send(account, raw, from, rcpt, now, out).await?;
        if let Some((_, state)) = self.drafts.open.get_mut(&id) { compose::step(state, compose::Event::Send, now); }
        self.drafts.open.remove(&id);
        self.store.delete_draft(&id).map_err(text)?;
        out.push(Event::DraftClosed { id, outbox: Some(outbox) });
        Ok(())
    }

    pub(crate) fn discard_draft(&mut self, id: DraftId, out: &mut Events) -> Outcome {
        self.drafts.open.remove(&id);
        self.store.delete_draft(&id).map_err(text)?;
        out.push(Event::DraftClosed { id, outbox: None });
        Ok(())
    }

    pub(crate) fn load_drafts(&mut self, out: &mut Events) -> Outcome {
        for json in self.store.drafts().map_err(text)? {
            let Ok(draft) = serde_json::from_str::<Draft>(&json) else { continue };
            self.drafts.open.entry(draft.id.clone()).or_insert_with(|| (draft.clone(), compose::open()));
            out.push(Event::Draft(Box::new(draft)));
        }
        Ok(())
    }
}
