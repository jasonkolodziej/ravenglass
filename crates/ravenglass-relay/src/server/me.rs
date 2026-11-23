//! The signed-in user's own mail accounts (users mode).

use super::{Mode, Principal, Refusal, Shared, error, json};
use crate::store::StoreError;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use mail_core::AccountId;
use relay_proto::auth::NewAccount;
use relay_proto::ErrorCode;

/// Settings plus one secret: far below this.
const MAX_ACCOUNT_BODY: usize = 64 << 10;
const MAX_ACCOUNT_ID: usize = 200;

fn user<'a>(shared: &'a Shared, method: &Method, headers: &HeaderMap) -> Result<(i64, &'a super::auth::Users), Refusal> {
    let Mode::Users(users) = &shared.mode else { return Err(Refusal(ErrorCode::NotFound, "this relay uses a shared token and has no per-user accounts")) };
    match shared.authorize(method, headers)? {
        Principal::User { id, .. } => Ok((id, users)),
        Principal::Owner => Err(Refusal(ErrorCode::Auth, "sign in first")),
    }
}

pub async fn list(State(shared): State<Shared>, method: Method, headers: HeaderMap) -> Response {
    let (id, users) = match user(&shared, &method, &headers) { Ok(u) => u, Err(r) => return r.into_response() };
    match users.store.accounts(id) {
        Ok(accounts) => json(&accounts),
        Err(_) => error(ErrorCode::Server, "the account store failed"),
    }
}

pub async fn add(State(shared): State<Shared>, method: Method, headers: HeaderMap, body: Bytes) -> Response {
    let (id, users) = match user(&shared, &method, &headers) { Ok(u) => u, Err(r) => return r.into_response() };
    if body.len() > MAX_ACCOUNT_BODY { return error(ErrorCode::TooLarge, "the request is too large"); }
    let Ok(new) = serde_json::from_slice::<NewAccount>(&body) else { return error(ErrorCode::BadRequest, "send {\"config\", \"secret\"}") };
    let config = new.config;
    if config.id.0.is_empty() || config.id.0.len() > MAX_ACCOUNT_ID { return error(ErrorCode::BadRequest, "the account id must be 1 to 200 bytes"); }
    if let Some(problem) = config.problem() { return error(ErrorCode::BadRequest, problem); }
    match users.store.add_account(id, &config) {
        Ok(()) => {}
        Err(StoreError::Conflict) => return error(ErrorCode::Conflict, "you already have an account with this id"),
        Err(_) => return error(ErrorCode::Server, "the account store failed"),
    }
    if users.store.set_secret(id, &config.id, new.secret.kind.into(), &new.secret.value).is_err() {
        let _ = users.store.remove_account(id, &config.id);
        return error(ErrorCode::Server, "the account store failed");
    }
    json(&config)
}

pub async fn remove(State(shared): State<Shared>, method: Method, headers: HeaderMap, Path(account): Path<String>) -> Response {
    let (id, users) = match user(&shared, &method, &headers) { Ok(u) => u, Err(r) => return r.into_response() };
    let account = AccountId::from(account.as_str());
    match users.store.remove_account(id, &account) {
        Ok(true) => {
            users.hub.close(id, &account);
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => error(ErrorCode::NotFound, "no such account"),
        Err(_) => error(ErrorCode::Server, "the account store failed"),
    }
}
