//! Measures the server-side cost of OPAQUE versus the Argon2id password check.
//!
//! ```bash
//! cargo run --release --example opaque_cost
//! ```
//!
//! The client-side KSF (Argon2) is **not** included: OPAQUE moves that work to
//! the client, which is the whole point. Numbers are per operation on the
//! current core.

use std::path::PathBuf;
use std::time::Instant;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use opaque_ke::{
    CipherSuite, ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialFinalization, CredentialRequest,
    CredentialResponse, RegistrationRequest, RegistrationResponse, RegistrationUpload,
    Ristretto255, TripleDh,
};
use stemson::auth::opaque::OpaqueAuth;
use stemson::utils::hash::{hash_password, verify_password};

struct Suite;

impl CipherSuite for Suite {
    type OprfCs = Ristretto255;
    type KeyExchange = TripleDh<Ristretto255, sha2::Sha512>;
    type Ksf = opaque_ke::argon2::Argon2<'static>;
}

fn main() {
    let mut rng = opaque_ke::rand::rngs::OsRng;
    let path = PathBuf::from(".bench/opaque_cost_setup.bin");
    let auth = OpaqueAuth::load_or_create(&path, 120).expect("create server setup");

    // Register one account (client side included; not part of the measurement).
    let started =
        ClientRegistration::<Suite>::start(&mut rng, b"bench-password").expect("client start");
    let request = URL_SAFE_NO_PAD.encode(started.message.serialize());
    let response = auth
        .registration_start("bench", &request)
        .expect("server registration start");
    let response =
        RegistrationResponse::<Suite>::deserialize(&URL_SAFE_NO_PAD.decode(response).unwrap())
            .unwrap();
    let finished = started
        .state
        .finish(
            &mut rng,
            b"bench-password",
            response,
            ClientRegistrationFinishParameters::default(),
        )
        .unwrap();
    let upload = URL_SAFE_NO_PAD.encode(finished.message.serialize());
    let record = auth.registration_finish(&upload).expect("server finish");

    // Prepare one full login exchange, then time each server step in isolation.
    let client = ClientLogin::<Suite>::start(&mut rng, b"bench-password").unwrap();
    let request = URL_SAFE_NO_PAD.encode(client.message.serialize());
    let start = auth
        .login_start("bench", Some(&record), &request)
        .expect("server login start");
    let credential_response =
        CredentialResponse::<Suite>::deserialize(&URL_SAFE_NO_PAD.decode(&start.response).unwrap())
            .unwrap();
    let finalization = client
        .state
        .finish(
            &mut rng,
            b"bench-password",
            credential_response,
            ClientLoginFinishParameters::default(),
        )
        .unwrap();
    let finalization = URL_SAFE_NO_PAD.encode(finalization.message.serialize());
    // Sanity: the prepared exchange is valid.
    auth.login_finish(&start.session, &finalization)
        .expect("login finish");

    let rounds = 200;
    let start_us = time(rounds, || {
        let _ = auth
            .login_start("bench", Some(&record), &request)
            .expect("login start");
    });
    let finish_us = time(rounds, || {
        let _ = auth
            .login_finish(&start.session, &finalization)
            .expect("login finish");
    });

    // The password path, for comparison.
    let hash = hash_password("bench-password").expect("hash");
    let verify_rounds = 20;
    let verify_us = time(verify_rounds, || {
        assert!(verify_password("bench-password", &hash));
    });
    let hash_us = time(verify_rounds, || {
        let _ = hash_password("bench-password").expect("hash");
    });

    println!("server-side cost per operation (single core)");
    println!("-------------------------------------------");
    println!("OPAQUE login/start   {start_us:>9.1} us");
    println!("OPAQUE login/finish  {finish_us:>9.1} us");
    println!("OPAQUE login total   {:>9.1} us", start_us + finish_us);
    println!("Argon2id verify      {verify_us:>9.1} us");
    println!("Argon2id hash        {hash_us:>9.1} us");
    println!();
    println!(
        "login capacity per core: OPAQUE {:.0}/s vs password {:.0}/s",
        1_000_000.0 / (start_us + finish_us),
        1_000_000.0 / verify_us
    );
}

/// Average microseconds per operation over `rounds` iterations.
fn time(rounds: usize, mut operation: impl FnMut()) -> f64 {
    let started = Instant::now();
    for _ in 0..rounds {
        operation();
    }
    started.elapsed().as_secs_f64() * 1_000_000.0 / rounds as f64
}

// Keep the client types referenced so the example documents the full message
// set even though only the server side is timed.
#[allow(dead_code)]
fn _message_types(
    _: RegistrationRequest<Suite>,
    _: RegistrationUpload<Suite>,
    _: CredentialRequest<Suite>,
    _: CredentialFinalization<Suite>,
) {
}
