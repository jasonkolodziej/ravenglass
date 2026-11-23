//! The compose pane through Rune's `Runtime`: keys go where a window would send them (the
//! focused field's editor included), the draft reaches the inline engine, and the outbox's undo
//! window passes on the virtual clock.

use super::ui::{keyed, press, runtime, runtime_with, scene, settle, text_in, type_keys};
use crate::compose::{Field, format_addresses, parse_addresses};
use crate::demo::DemoBackend;
use crate::link::Link;
use crate::modes::Mode;
use crate::panes::Kind;
use crate::state::{AppState, Mail};
use mail_core::Address;
use mail_store::Store;
use ravenglass_engine::{Command, Config, Engine};
use rune::{Key, KeyEvent, Mods, Runtime};
use std::sync::Arc;

/// The send chord, `mod+shift+d`, as a window delivers it.
fn send_chord(rt: &mut Runtime<AppState>) {
    rt.key(KeyEvent { key: Key::Command('d'), mods: Mods { shift: true, cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() } });
    rt.frame(1.0 / 60.0);
}

/// Let `secs` of app time pass, a second a frame.
fn wait(rt: &mut Runtime<AppState>, secs: usize) {
    for _ in 0..secs { rt.frame(1.0); }
}

/// Whether an element keyed `key` is on screen and not on its way out.
fn shown(rt: &Runtime<AppState>, key: &str) -> bool { rt.elements().iter().any(|e| e.key.as_deref() == Some(key) && !e.exiting) }

fn draft(rt: &Runtime<AppState>) -> &crate::compose::Composer { rt.state.compose.current().expect("a draft is open") }

/// The app over an inline engine whose backends the test keeps, to see what was sent.
fn app_with_backends() -> (AppState, Vec<Arc<DemoBackend>>) {
    let backends: Vec<Arc<DemoBackend>> = DemoBackend::accounts().into_iter().map(Arc::new).collect();
    let mut engine = Engine::new(Store::open_in_memory().expect("an in-memory store"), Config::default());
    for backend in &backends { engine.add_backend(backend.clone()); }
    let mut s = AppState::demo_inline();
    s.link = Link::Inline { engine: Box::new(engine), now: mail_mock::NOW as f64 };
    s.mail = Mail::default();
    for ws in &mut s.workspaces { ws.folder = None; }
    s.open_accounts();
    assert!(!s.rows().is_empty(), "the swapped engine loaded the inbox");
    (s, backends)
}

fn sent(backends: &[Arc<DemoBackend>]) -> Vec<String> {
    backends.iter().flat_map(|b| b.sent()).map(|raw| String::from_utf8_lossy(&raw).into_owned()).collect()
}

#[test]
fn addresses_round_trip_through_a_field() {
    let list = vec![Address::new("Odin Allfather", "odin@asgard.example"), Address { name: None, email: "mom@family.example".into() }];
    let text = format_addresses(&list);
    assert_eq!(text, "Odin Allfather <odin@asgard.example>, mom@family.example");
    assert_eq!(parse_addresses(&text), list);
    // What is still being typed is not an address yet.
    assert_eq!(parse_addresses("mom@family.example, odi"), list[1..]);
    assert_eq!(parse_addresses("  ,; "), []);
    assert_eq!(Field::To.next(true), Field::Body);
    assert_eq!(Field::Body.next(false), Field::To);
}

#[test]
fn r_opens_a_reply_to_the_sender_with_the_message_quoted() {
    let mut rt = runtime();
    let sender = rt.state.selected().unwrap().from.clone();
    let subject = rt.state.selected().unwrap().subject.clone();
    type_keys(&mut rt, "r");
    settle(&mut rt);
    assert_eq!(rt.state.mode, Mode::Compose);
    assert_eq!(rt.state.focused_kind(), Kind::Compose);
    let c = draft(&rt);
    assert_eq!(c.draft.to, std::slice::from_ref(&sender));
    assert_eq!(c.draft.subject, format!("Re: {subject}"));
    assert!(c.draft.in_reply_to.is_some());
    assert!(c.body.text.lines().any(|l| l.starts_with("> ")), "the original is quoted: {:?}", c.body.text);
    assert_eq!(c.field, Field::Body);
    assert!(c.body.focused, "the body has the keyboard");
    let els = rt.elements();
    assert_eq!(text_in(&els, "status").split_whitespace().next(), Some("COMPOSE"));
    let title = text_in(&els, "compose-title");
    assert!(title.contains(&format!("reply ▸ {}", sender.display())) && title.contains("draft ✓"), "{title}");
    assert_eq!(keyed(&els, "tab:draft:0").and_then(|e| e.label.clone()), Some(format!("Draft: Re: {subject}")));
    // The compose pane is below the reader, both on the grid.
    let g = rt.state.frame.get().grid;
    let (reader, compose) = (keyed(&els, "pane:2").expect("the reader"), keyed(&els, "pane:3").expect("the compose pane"));
    assert!(compose.rect[1] > reader.rect[1] && (compose.rect[0] - reader.rect[0]).abs() < 0.01);
    for e in [compose, keyed(&els, "compose-title").unwrap(), keyed(&els, Field::Body.key()).unwrap(), keyed(&els, Field::To.key()).unwrap()] {
        let on_grid = |v: f32, cell: f32| ((v / cell).round() * cell - v).abs() < 0.01;
        assert!(on_grid(e.rect[0], g.cell_w) && on_grid(e.rect[1], g.cell_h), "{:?} at {:?}", e.key, e.rect);
    }
    assert!(keyed(&els, "compose-caret").is_some(), "the block caret is drawn");
}

#[test]
fn typing_edits_the_draft_and_the_engine_keeps_it() {
    let mut rt = runtime();
    type_keys(&mut rt, "r");
    settle(&mut rt);
    type_keys(&mut rt, "Thanks, on its way.");
    press(&mut rt, Key::Enter);
    type_keys(&mut rt, "J");
    press(&mut rt, Key::Backspace);
    let c = draft(&rt);
    assert!(c.body.text.starts_with("Thanks, on its way.\n\n"), "{:?}", c.body.text);
    assert_eq!(c.draft.body, c.body.text, "every edit reaches the draft");
    assert_eq!(c.unsaved, 0, "and the engine answered each with DraftSaved");
    assert_eq!(rt.state.ws().sel, 0, "list keys typed into the draft do not move the list");
    // `tab` walks the fields; typing lands in the one that has the keyboard.
    press(&mut rt, Key::Tab);
    assert_eq!(draft(&rt).field, Field::To);
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Tab);
    assert_eq!(draft(&rt).field, Field::Subject);
    type_keys(&mut rt, "!");
    assert!(draft(&rt).draft.subject.ends_with('!'));
    rt.key(KeyEvent { key: Key::Tab, mods: Mods { shift: true, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
    assert_eq!(draft(&rt).field, Field::Cc);
    // Pasting goes through the field's editor too.
    rt.paste("huginn@asgard.example");
    rt.frame(1.0 / 60.0);
    assert_eq!(draft(&rt).draft.cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["huginn@asgard.example"]);

    // The engine has what was typed: forget the draft here and ask for the kept ones.
    let (id, body) = (draft(&rt).draft.id.clone(), draft(&rt).draft.body.clone());
    rt.state.compose.drafts.clear();
    rt.state.dispatch(Command::LoadDrafts);
    let restored = draft(&rt);
    assert_eq!((&restored.draft.id, &restored.draft.body), (&id, &body));
    assert_eq!(restored.draft.cc.len(), 1);
    rt.frame(1.0 / 60.0);
    assert!(keyed(&rt.elements(), "tab:draft:0").is_some(), "a restored draft is a tab");
}

#[test]
fn escape_keeps_the_draft_and_c_goes_back_to_it() {
    let mut rt = runtime();
    type_keys(&mut rt, "c");
    settle(&mut rt);
    assert_eq!(rt.state.mode, Mode::Compose);
    assert_eq!(draft(&rt).field, Field::To, "a new message starts with its recipients");
    assert_eq!(text_in(&rt.elements(), "compose-title").split("  ").find(|s| !s.trim().is_empty()).map(str::trim), Some("new message"));
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Tab);
    type_keys(&mut rt, "Gnomon sync");
    press(&mut rt, Key::Escape);
    settle(&mut rt);
    assert_eq!(rt.state.mode, Mode::default());
    assert_eq!(rt.state.compose.drafts.len(), 1, "esc keeps the draft");
    assert!(Field::ALL.iter().all(|f| !draft(&rt).edit(*f).focused), "no field has the keyboard in NORMAL");
    assert_eq!(keyed(&rt.elements(), "tab:draft:0").and_then(|e| e.label.clone()).as_deref(), Some("Draft: Gnomon sync"));
    // NORMAL keys are the list's again.
    press(&mut rt, Key::Escape);
    type_keys(&mut rt, "j");
    assert_eq!(rt.state.ws().sel, 1);
    assert_eq!(draft(&rt).draft.subject, "Gnomon sync");
    // `c` returns to the draft rather than starting another.
    type_keys(&mut rt, "c");
    settle(&mut rt);
    assert_eq!((rt.state.mode.clone(), rt.state.compose.drafts.len()), (Mode::Compose, 1));
    type_keys(&mut rt, " notes");
    assert_eq!(draft(&rt).draft.subject, "Gnomon sync notes");
    // `:new` starts a second one; `:discard` drops it.
    press(&mut rt, Key::Escape);
    type_keys(&mut rt, ":new");
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!((rt.state.compose.drafts.len(), rt.state.compose.active), (2, 1));
    press(&mut rt, Key::Escape);
    type_keys(&mut rt, ":discard");
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert_eq!(rt.state.compose.drafts.len(), 1);
    assert_eq!(text_in(&rt.elements(), "toast"), "draft discarded");
    assert!(rt.state.ws().panes.find(Kind::Compose).is_some(), "the other draft keeps the pane");
    type_keys(&mut rt, ":discard");
    press(&mut rt, Key::Enter);
    settle(&mut rt);
    assert!(rt.state.compose.drafts.is_empty());
    assert_eq!(rt.state.ws().panes.find(Kind::Compose), None, "the pane goes with the last draft");
    assert_eq!(rt.state.ws().panes.leaves().len(), 2, "and the reader has its place back");
}

#[test]
fn sending_counts_down_then_the_backend_gets_the_message() {
    let (state, backends) = app_with_backends();
    let mut rt = runtime_with(state, (1400.0, 880.0));
    type_keys(&mut rt, "r");
    settle(&mut rt);
    type_keys(&mut rt, "The pass is clear.");
    send_chord(&mut rt);
    assert_eq!(rt.state.mode, Mode::default(), "the keys are NORMAL's while it waits");
    assert!(!draft(&rt).editing());
    let els = rt.elements();
    assert!(text_in(&els, "status").contains("sending in 10s · u undo"), "{}", text_in(&els, "status"));
    assert!(text_in(&els, "compose-title").contains("sending in 10s · u undo"));
    assert!(!shown(&rt, Field::Body.key()), "a queued draft is not editable");
    wait(&mut rt, 4);
    assert!(text_in(&rt.elements(), "status").contains("sending in 6s"), "{}", text_in(&rt.elements(), "status"));
    assert!(sent(&backends).is_empty(), "nothing leaves inside the undo window");
    wait(&mut rt, 7);
    let out = sent(&backends);
    assert_eq!(out.len(), 1, "the tick after the window sends it");
    assert!(out[0].contains("The pass is clear.") && out[0].contains("In-Reply-To:"), "{}", out[0]);
    assert!(rt.state.compose.drafts.is_empty());
    assert_eq!(rt.state.ws().panes.find(Kind::Compose), None);
    let els = rt.elements();
    assert!(!text_in(&els, "status").contains("sending"));
    assert!(text_in(&els, "toast").eq_ignore_ascii_case("sent"));
    assert!(!shown(&rt, "tab:draft:0"));
}

#[test]
fn u_in_the_undo_window_cancels_the_send_and_reopens_the_draft() {
    let (state, backends) = app_with_backends();
    let mut rt = runtime_with(state, (1400.0, 880.0));
    type_keys(&mut rt, "r");
    settle(&mut rt);
    type_keys(&mut rt, "Not yet.");
    send_chord(&mut rt);
    wait(&mut rt, 3);
    assert!(rt.state.sending_label().is_some_and(|l| l.starts_with("sending in 7s")));
    type_keys(&mut rt, "u");
    settle(&mut rt);
    assert_eq!(rt.state.sending_label(), None);
    assert_eq!(rt.state.mode, Mode::Compose, "the draft is open for editing again");
    let c = draft(&rt);
    assert!(c.editing() && c.body.text.starts_with("Not yet."));
    assert_eq!(text_in(&rt.elements(), "toast"), "send cancelled · the draft is back");
    assert!(rt.state.undo.is_empty() && rt.state.undoing.is_none(), "`u` took back the send, not a list action");
    // It stays unsent however long we wait, and the engine has the session again.
    rt.state.advance_engine(60.0);
    wait(&mut rt, 2);
    assert!(sent(&backends).is_empty());
    type_keys(&mut rt, " Soon.");
    assert_eq!(draft(&rt).unsaved, 0);
    send_chord(&mut rt);
    rt.state.advance_engine(11.0);
    rt.frame(1.0 / 60.0);
    let out = sent(&backends);
    assert_eq!(out.len(), 1);
    assert!(out[0].contains("Not yet. Soon."), "{}", out[0]);
}

#[test]
fn sending_needs_a_recipient() {
    let mut rt = runtime();
    type_keys(&mut rt, "c");
    settle(&mut rt);
    send_chord(&mut rt);
    assert!(rt.state.toast.as_ref().is_some_and(|t| t.error && t.text == "add a recipient first"));
    assert!(draft(&rt).editing());
    assert_eq!(draft(&rt).field, Field::To);
}

#[test]
fn an_address_field_suggests_contacts_and_tab_accepts_one() {
    let mut rt = runtime();
    type_keys(&mut rt, "c");
    settle(&mut rt);
    type_keys(&mut rt, "odi");
    let list = draft(&rt).completion.clone().expect("suggestions for what was typed");
    assert_eq!(list.prefix, "odi");
    assert_eq!(list.addresses[0].email, "odin@asgard.example");
    let els = rt.elements();
    let (popup, field) = (keyed(&els, "compose-completion").expect("the popup list"), keyed(&els, Field::To.key()).unwrap());
    assert_eq!(popup.role, "listbox");
    let g = rt.state.frame.get().grid;
    assert!((popup.rect[1] - (field.rect[1] + g.cell_h)).abs() < 0.01 && (popup.rect[0] - field.rect[0]).abs() < 0.01, "the popup sits under the field");
    assert!(draft(&rt).draft.to.is_empty(), "a half-typed name is not a recipient");
    press(&mut rt, Key::Tab);
    let c = draft(&rt);
    assert_eq!(c.to.text, "Odin Allfather <odin@asgard.example>, ");
    assert_eq!(c.draft.to, [Address::new("Odin Allfather", "odin@asgard.example")]);
    assert_eq!((c.field, c.completion.is_none()), (Field::To, true), "tab took the suggestion, not the next field");
    assert!(!shown(&rt, "compose-completion"));
    // A second recipient after the first; `enter` accepts as well, and `esc` only closes the list.
    type_keys(&mut rt, "mun");
    press(&mut rt, Key::Escape);
    assert_eq!((rt.state.mode.clone(), draft(&rt).completion.is_none()), (Mode::Compose, true));
    type_keys(&mut rt, "i");
    press(&mut rt, Key::Enter);
    assert_eq!(draft(&rt).draft.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["odin@asgard.example", "muninn@ravens.example"]);
}

#[test]
fn the_compose_scenes_show_the_draft() {
    let rt = scene("compose-reply", (1400.0, 880.0));
    let c = draft(&rt);
    assert!(c.body.text.starts_with("Thanks — sending the build north today."));
    assert!(text_in(&rt.elements(), "compose-title").contains("reply ▸"));
    let narrow = scene("compose-narrow", (800.0, 700.0));
    let panes: Vec<String> = narrow.elements().iter().filter_map(|e| e.key.clone()).filter(|k| k.starts_with("pane:")).collect();
    assert_eq!(panes, ["pane:3"], "a narrow window shows the draft alone");
    let sending = scene("compose-sending", (1400.0, 880.0));
    assert!(text_in(&sending.elements(), "status").contains("sending in 10s · u undo"));
    let new = scene("compose", (1400.0, 880.0));
    assert_eq!(draft(&new).draft.subject, "Gnomon sync");
    assert_eq!(new.state.ws().panes.leaves().len(), 2, "with no message open the draft takes the reader's pane");
}
