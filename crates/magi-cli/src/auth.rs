//! `magi auth` — signing in to a subscription.
//!
//! The command exists because there is nothing to export. An API key is a string a person can
//! put in their shell profile; a subscription is a token you are given, that expires, and that
//! has to be renewed on your behalf. So "how do I enable this" has a command for an answer,
//! and this is it.
//!
//! The browser is opened rather than embedded. magi has no business rendering a sign-in page,
//! the person's own browser already holds the session they are signing in with, and a terminal
//! program asking for a password is the shape of every credential-phishing attack there is.

use anyhow::{Context, Result, bail};
use magi_provider::oauth::{self, Pkce, Store};
use magi_provider::provider::{Auth, Provider};

/// Sign in to a provider.
pub async fn login(id: &str) -> Result<()> {
    let provider = find(id)?;
    let Auth::OAuth {
        service,
        authorize_url,
        token_url,
        client_id,
        scopes,
    } = &provider.auth
    else {
        bail!(
            "{id} does not sign in; {}",
            requirement_or_nothing(&provider)
        );
    };
    // A provider can be in the catalog before this build knows how to sign in to it. Saying so
    // is better than a sign-in that fails against an endpoint somebody guessed.
    let (Some(authorize_url), Some(token_url), Some(client_id)) = (
        authorize_url.as_ref(),
        token_url.as_ref(),
        client_id.as_ref(),
    ) else {
        bail!(
            "this build has no sign-in details for {id}. Add `authorize_url`, `token_url` and \
             `client_id` to its `auth` block in your own providers.lua to use one you have."
        );
    };

    // Bound to the loopback interface before the URL is built, because the port is part of the
    // redirect the authorization server is told to use and has to be the one actually waiting.
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .context("listening for the browser to come back")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");

    let pkce = Pkce::generate().context("generating a proof key")?;
    // The state is a second random value, checked on the way back. It is what stops a link
    // somebody else crafted from completing a sign-in in this terminal.
    let state = Pkce::generate()
        .context("generating a state value")?
        .verifier;
    let url = oauth::authorize_url(authorize_url, client_id, &redirect, scopes, &pkce, &state);

    println!("Sign in to {service} in your browser:\n\n  {url}\n");
    open_browser(&url);
    println!("Waiting for the callback…");

    let callback = oauth::listen_for_code(&listener)?;
    if callback.state != state {
        bail!("the callback did not match this sign-in; nothing was saved");
    }

    let tokens = oauth::exchange(
        &reqwest::Client::new(),
        token_url,
        &[
            ("grant_type", "authorization_code"),
            ("code", &callback.code),
            ("redirect_uri", &redirect),
            ("client_id", client_id),
            ("code_verifier", &pkce.verifier),
        ],
    )
    .await?;

    let mut store = Store::load()?;
    store.put(id, tokens);
    store.save()?;
    println!(
        "Signed in to {service}. Credentials are in {}.",
        oauth::path().display()
    );
    Ok(())
}

/// Forget a provider's credentials.
pub fn logout(id: &str) -> Result<()> {
    let mut store = Store::load()?;
    if store.forget(id) {
        store.save()?;
        println!("Signed out of {id}.");
    } else {
        println!("Not signed in to {id}.");
    }
    Ok(())
}

/// What is signed in, and what is not.
pub fn status() -> Result<()> {
    let store = Store::load()?;
    let loaded = crate::config::load()?;
    let now = oauth::now();

    let mut any = false;
    for provider in &loaded.providers {
        if !matches!(provider.auth, Auth::OAuth { .. }) {
            continue;
        }
        any = true;
        let state = match store.get(&provider.id) {
            None => "not signed in".to_owned(),
            // Reported rather than hidden, because a stale token is not a problem: it is
            // renewed on the next request. Saying "expired" would send people to a command
            // they do not need to run.
            Some(tokens) if tokens.is_stale(now) => "signed in (will renew)".to_owned(),
            Some(_) => "signed in".to_owned(),
        };
        println!("{:<14} {}", provider.id, state);
    }
    if !any {
        println!("No provider in your configuration signs in with `magi auth`.");
    }
    Ok(())
}

/// One provider from the catalog, by id.
fn find(id: &str) -> Result<Provider> {
    let loaded = crate::config::load()?;
    loaded
        .providers
        .into_iter()
        .find(|p| p.id == id)
        .with_context(|| {
            format!("there is no provider called {id:?}; `magi models --all` lists them")
        })
}

/// How a provider says it is enabled, for the case where it is not by signing in.
fn requirement_or_nothing(provider: &Provider) -> String {
    let requirement = provider.auth.requirement();
    if requirement.is_empty() {
        "it needs no credential".to_owned()
    } else {
        requirement
    }
}

/// Ask the desktop to open a URL, and shrug if it cannot.
///
/// Best effort on purpose: the URL is printed above regardless, so a machine with no browser —
/// a server over ssh, which is where this is most likely — loses nothing but a convenience.
fn open_browser(url: &str) {
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
