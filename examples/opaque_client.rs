//! Command line OPAQUE client.
//!
//! Reference implementation of the client side of the login protocol, and the
//! driver used by `scripts/e2e-test.sh`. A browser client uses the same
//! messages through `@serenity-kit/opaque`; only the transport differs.
//!
//! ```bash
//! # 1) an administrator creates the account, which returns an invite token
//! #    (POST /user/user_controller/add_user)
//! # 2) the user registers a password
//! cargo run --release --example opaque_client -- register \
//!     --url http://127.0.0.1:8080 --username alice --password 's3cret' --invite TOKEN
//! # 3) login
//! cargo run --release --example opaque_client -- login \
//!     --url http://127.0.0.1:8080 --username alice --password 's3cret'
//! ```

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use opaque_ke::{
    CipherSuite, ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse, Ristretto255,
    TripleDh,
};
use serde_json::{json, Value};

struct Suite;

impl CipherSuite for Suite {
    type OprfCs = Ristretto255;
    type KeyExchange = TripleDh<Ristretto255, sha2::Sha512>;
    type Ksf = opaque_ke::argon2::Argon2<'static>;
}

struct Args {
    command: String,
    url: String,
    username: String,
    password: String,
    invite: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut command = None;
    let mut url = "http://127.0.0.1:8080".to_string();
    let mut username = None;
    let mut password = None;
    let mut invite = None;

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "-h" | "--help" => {
                println!("usage: opaque_client <register|login> --url URL --username NAME --password PW [--invite TOKEN]");
                std::process::exit(0);
            }
            "register" | "login" | "prepare" => {
                if command.is_some() {
                    return Err("exactly one command is required".to_string());
                }
                command = Some(flag);
            }
            "--url" => url = args.next().ok_or("--url needs a value")?,
            "--username" => username = Some(args.next().ok_or("--username needs a value")?),
            "--password" => password = Some(args.next().ok_or("--password needs a value")?),
            "--invite" => invite = Some(args.next().ok_or("--invite needs a value")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }

    Ok(Args {
        command: command.ok_or("missing command: register or login")?,
        url,
        username: username.ok_or("missing --username")?,
        password: password.ok_or("missing --password")?,
        invite,
    })
}

async fn post(client: &reqwest::Client, url: &str, body: Value) -> Result<Value, String> {
    let response = client
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|err| format!("request failed: {err}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|err| format!("invalid JSON: {err}: {text}"))
}

fn data<'a>(response: &'a Value, field: &str) -> Result<&'a str, String> {
    response
        .get("data")
        .and_then(|data| data.get(field))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("response is missing data.{field}: {response}"))
}

#[tokio::main]
async fn main() {
    let args = parse_args().unwrap_or_else(|err| {
        eprintln!("error: {err}");
        std::process::exit(2);
    });
    let mut rng = opaque_ke::rand::rngs::OsRng;
    let client = reqwest::Client::new();

    let result = if args.command == "prepare" {
        prepare(&client, &mut rng, &args).await
    } else if args.command == "register" {
        let invite = args
            .invite
            .as_deref()
            .ok_or_else(|| "register needs --invite".to_string());
        match invite {
            Ok(invite) => register(&client, &mut rng, &args, invite).await,
            Err(err) => Err(err),
        }
    } else {
        login(&client, &mut rng, &args).await
    };

    match result {
        Ok(token) => println!("{token}"),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}

async fn register(
    client: &reqwest::Client,
    rng: &mut opaque_ke::rand::rngs::OsRng,
    args: &Args,
    invite: &str,
) -> Result<String, String> {
    let started = ClientRegistration::<Suite>::start(rng, args.password.as_bytes())
        .map_err(|err| format!("registration start: {err:?}"))?;
    let request = URL_SAFE_NO_PAD.encode(started.message.serialize());

    let response = post(
        client,
        &format!("{}/v1/api/auth/register/start", args.url),
        json!({ "username": args.username, "registration_request": request }),
    )
    .await?;
    let registration_response = URL_SAFE_NO_PAD
        .decode(data(&response, "registration_response")?)
        .map_err(|err| format!("invalid registration_response: {err}"))?;
    let registration_response = RegistrationResponse::<Suite>::deserialize(&registration_response)
        .map_err(|err| format!("invalid registration_response: {err:?}"))?;

    let finished = started
        .state
        .finish(
            rng,
            args.password.as_bytes(),
            registration_response,
            ClientRegistrationFinishParameters::default(),
        )
        .map_err(|err| format!("registration finish: {err:?}"))?;
    let upload = URL_SAFE_NO_PAD.encode(finished.message.serialize());

    post(
        client,
        &format!("{}/v1/api/auth/register/finish", args.url),
        json!({
            "username": args.username,
            "invite_token": invite,
            "registration_upload": upload,
        }),
    )
    .await?;

    Ok(format!("registered {}", args.username))
}

/// Print the three wire messages of one login, for load testing.
async fn prepare(
    client: &reqwest::Client,
    rng: &mut opaque_ke::rand::rngs::OsRng,
    args: &Args,
) -> Result<String, String> {
    let started = ClientLogin::<Suite>::start(rng, args.password.as_bytes())
        .map_err(|err| format!("login start: {err:?}"))?;
    let request = URL_SAFE_NO_PAD.encode(started.message.serialize());

    let response = post(
        client,
        &format!("{}/v1/api/auth/login/start", args.url),
        json!({ "username": args.username, "credential_request": request }),
    )
    .await?;
    let session = data(&response, "session")?.to_string();
    let credential_response = URL_SAFE_NO_PAD
        .decode(data(&response, "credential_response")?)
        .map_err(|err| format!("invalid credential_response: {err}"))?;
    let credential_response = CredentialResponse::<Suite>::deserialize(&credential_response)
        .map_err(|err| format!("invalid credential_response: {err:?}"))?;
    let finished = started
        .state
        .finish(
            rng,
            args.password.as_bytes(),
            credential_response,
            ClientLoginFinishParameters::default(),
        )
        .map_err(|_| "invalid credentials".to_string())?;

    Ok(json!({
        "credential_request": request,
        "session": session,
        "credential_finalization": URL_SAFE_NO_PAD.encode(finished.message.serialize()),
    })
    .to_string())
}

async fn login(
    client: &reqwest::Client,
    rng: &mut opaque_ke::rand::rngs::OsRng,
    args: &Args,
) -> Result<String, String> {
    let started = ClientLogin::<Suite>::start(rng, args.password.as_bytes())
        .map_err(|err| format!("login start: {err:?}"))?;
    let request = URL_SAFE_NO_PAD.encode(started.message.serialize());

    let response = post(
        client,
        &format!("{}/v1/api/auth/login/start", args.url),
        json!({ "username": args.username, "credential_request": request }),
    )
    .await?;
    let session = data(&response, "session")?.to_string();
    let credential_response = URL_SAFE_NO_PAD
        .decode(data(&response, "credential_response")?)
        .map_err(|err| format!("invalid credential_response: {err}"))?;
    let credential_response = CredentialResponse::<Suite>::deserialize(&credential_response)
        .map_err(|err| format!("invalid credential_response: {err:?}"))?;

    let finished = started
        .state
        .finish(
            rng,
            args.password.as_bytes(),
            credential_response,
            ClientLoginFinishParameters::default(),
        )
        .map_err(|_| "invalid credentials".to_string())?;
    let finalization = URL_SAFE_NO_PAD.encode(finished.message.serialize());

    let response = post(
        client,
        &format!("{}/v1/api/auth/login/finish", args.url),
        json!({
            "session": session,
            "credential_finalization": finalization,
        }),
    )
    .await?;

    Ok(data(&response, "access_token")?.to_string())
}
