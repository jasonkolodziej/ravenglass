//! `--oauth`: the browser sign-in for Google and Microsoft accounts, with `mail_auth::oauth`
//! (authorization code + PKCE, loopback redirect). Ravenglass ships no client id, so the person
//! running the check registers one and passes it through the environment.

use crate::line;
use mail_auth::oauth::{self, Provider, loopback::Redirect};

fn client_variables(provider: Provider) -> (&'static str, &'static str) {
    match provider {
        Provider::Google => ("RAVENGLASS_GOOGLE_CLIENT_ID", "RAVENGLASS_GOOGLE_CLIENT_SECRET"),
        Provider::Microsoft => ("RAVENGLASS_MICROSOFT_CLIENT_ID", "RAVENGLASS_MICROSOFT_CLIENT_SECRET"),
    }
}

/// What to set up before `--oauth` can work, for the report.
pub fn requirements(provider: Provider) -> String {
    let (id, secret) = client_variables(provider);
    match provider {
        Provider::Google => format!("set {id} to the client id of a Google Cloud OAuth client of type \"Desktop app\" with the Gmail API scope https://mail.google.com/, and {secret} to its client secret (Google requires it even for desktop apps)"),
        Provider::Microsoft => format!("set {id} to the application (client) id of a Microsoft Entra app registration with a \"Mobile and desktop\" redirect URI http://localhost and the delegated permissions IMAP.AccessAsUser.All, SMTP.Send and offline_access ({secret} only if the registration is a confidential client)"),
    }
}

/// Sign in through the browser and return an access token. Nothing about the token is printed.
pub fn sign_in(provider: Provider) -> Result<String, String> {
    let (id_variable, secret_variable) = client_variables(provider);
    let Some(client_id) = std::env::var(id_variable).ok().filter(|v| !v.is_empty()) else {
        line("oauth", format!("needs a client id: {}", requirements(provider)));
        return Err(format!("{id_variable} is not set"));
    };
    let client_secret = std::env::var(secret_variable).ok().filter(|v| !v.is_empty());
    let redirect = Redirect::listen().map_err(|e| e.to_string())?;
    let redirect_uri = redirect.uri().map_err(|e| e.to_string())?;
    let attempt = oauth::Attempt::new().map_err(|e| e.to_string())?;
    // No login hint: the URL then holds nothing about the account and is safe to show.
    let url = attempt.authorize_url(provider, &client_id, &redirect_uri, "");
    line("oauth", "open this address in a browser and sign in (waiting for the redirect to this machine):");
    println!("{url}");
    let query = redirect.wait().map_err(|e| e.to_string())?;
    let code = attempt.code(&query).map_err(|e| e.to_string())?;
    let mut body = attempt.token_request(&client_id, &redirect_uri, &code);
    // mail_auth's request has no client secret: Ravenglass's own clients will be public ones.
    if let Some(secret) = client_secret { body.push_str(&format!("&client_secret={}", oauth::form_encode(&secret))); }
    let post = ravenglass_accounts::http::form_post();
    let answer = post(provider.endpoints().token, &body).map_err(|e| format!("the token request failed: {}", e.chars().take(120).collect::<String>()))?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let tokens = oauth::parse_tokens(&answer, now).map_err(|e| e.to_string())?;
    line("oauth", format!("signed in; access token valid for {} minutes, refresh token {}", ((tokens.expires_at - now) / 60.0).round(), if tokens.refresh.is_some() { "received" } else { "not received" }));
    Ok(tokens.access)
}
