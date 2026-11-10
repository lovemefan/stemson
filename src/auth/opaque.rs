//! OPAQUE (RFC 9807) password authentication.
//!
//! Replaces the Argon2id password check with an augmented PAKE: the password
//! never reaches the server, the server stores a registration record instead of
//! a hash, and a database leak alone does not allow an offline dictionary
//! attack (the attacker also needs the server setup secret).
//!
//! # Wire protocol
//!
//! Registration (two client messages):
//!
//! ```text
//! client                                    server
//!   |  RegistrationRequest  (POST /register/start)  |
//!   |---------------------------------------------->|
//!   |  RegistrationResponse                          |
//!   |<----------------------------------------------|
//!   |  RegistrationUpload   (POST /register/finish)  |  + invite token
//!   |---------------------------------------------->|  stores the record
//! ```
//!
//! Login (two client messages):
//!
//! ```text
//! client                                    server
//!   |  CredentialRequest    (POST /login/start)      |
//!   |---------------------------------------------->|
//!   |  CredentialResponse + session                  |
//!   |<----------------------------------------------|
//!   |  CredentialFinalization (POST /login/finish)   |
//!   |---------------------------------------------->|  issues the JWT
//! ```
//!
//! The server keeps **no session state**: the `ServerLogin` state produced by
//! the first round is serialized, encrypted with a key derived from the server
//! setup and handed back to the client as an opaque `session` string (with an
//! embedded expiry). Any instance holding the same setup file can finish the
//! login, so the deployment stays horizontally scalable.

use std::path::Path;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use opaque_ke::argon2::Argon2;
use opaque_ke::{
    CipherSuite, CredentialFinalization, CredentialRequest, RegistrationRequest,
    RegistrationUpload, Ristretto255, ServerLogin, ServerLoginParameters, ServerRegistration,
    ServerSetup, TripleDh,
};
use sha2::{Digest, Sha256};

use crate::exception::{ApiError, ApiResult};

/// Cipher suite: ristretto255 OPRF, triple-DH key exchange, Argon2 KSF on the
/// client. The same suite must be used by clients, including the JavaScript
/// `@serenity-kit/opaque` package.
pub struct OpaqueSuite;

impl CipherSuite for OpaqueSuite {
    type OprfCs = Ristretto255;
    type KeyExchange = TripleDh<Ristretto255, sha2::Sha512>;
    type Ksf = Argon2<'static>;
}

/// Result of the first login round.
pub struct LoginStart {
    /// `CredentialResponse`, base64url.
    pub response: String,
    /// Opaque, encrypted `ServerLogin` state, base64url.
    pub session: String,
}

/// Server side of OPAQUE.
pub struct OpaqueAuth {
    setup: ServerSetup<OpaqueSuite>,
    session_cipher: ChaCha20Poly1305,
    session_ttl: Duration,
}

impl std::fmt::Debug for OpaqueAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpaqueAuth")
            .field("session_ttl_secs", &self.session_ttl.as_secs())
            .finish_non_exhaustive()
    }
}

impl OpaqueAuth {
    /// Load the server setup from `path`, creating it on first run.
    ///
    /// The setup is the long-term secret of the protocol: losing it invalidates
    /// every registration, leaking it weakens the offline-attack resistance.
    /// Keep the file out of version control and back it up separately.
    pub fn load_or_create(
        path: impl AsRef<Path>,
        session_ttl_seconds: i64,
    ) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let setup = if path.exists() {
            let bytes = std::fs::read(path)?;
            ServerSetup::<OpaqueSuite>::deserialize(&bytes).map_err(|err| {
                anyhow::anyhow!("invalid OPAQUE server setup {}: {err:?}", path.display())
            })?
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut rng = opaque_ke::rand::rngs::OsRng;
            let setup = ServerSetup::<OpaqueSuite>::new(&mut rng);
            let bytes = setup.serialize();
            std::fs::write(path, bytes.as_slice())?;
            restrict_permissions(path)?;
            tracing::warn!(
                path = %path.display(),
                "generated a new OPAQUE server setup; back it up, losing it invalidates every registration"
            );
            setup
        };

        // Derive the session-encryption key from the setup: one secret to
        // manage, and rotating the setup invalidates pending sessions anyway.
        let mut hasher = Sha256::new();
        hasher.update(setup.serialize().as_slice());
        hasher.update(b"stemson/opaque/session-key/v1");
        let key = hasher.finalize();

        Ok(Self {
            setup,
            session_cipher: ChaCha20Poly1305::new(Key::from_slice(&key)),
            session_ttl: Duration::from_secs(session_ttl_seconds.max(30) as u64),
        })
    }

    /// Answer the client's `RegistrationRequest`.
    pub fn registration_start(&self, username: &str, request: &str) -> ApiResult<String> {
        let request = RegistrationRequest::<OpaqueSuite>::deserialize(&decode(request)?)
            .map_err(|_| ApiError::invalid_parameter("invalid registration request"))?;
        let response =
            ServerRegistration::<OpaqueSuite>::start(&self.setup, request, username.as_bytes())
                .map_err(|_| ApiError::invalid_parameter("invalid registration request"))?;
        Ok(URL_SAFE_NO_PAD.encode(response.message.serialize()))
    }

    /// Turn the client's `RegistrationUpload` into the stored record.
    pub fn registration_finish(&self, upload: &str) -> ApiResult<Vec<u8>> {
        let upload = RegistrationUpload::<OpaqueSuite>::deserialize(&decode(upload)?)
            .map_err(|_| ApiError::invalid_parameter("invalid registration upload"))?;
        let record = ServerRegistration::<OpaqueSuite>::finish(upload);
        Ok(record.serialize().to_vec())
    }

    /// First login round.
    ///
    /// `registration` is `None` for unknown users; OPAQUE then answers with a
    /// fake record so that the response does not reveal whether the account
    /// exists.
    pub fn login_start(
        &self,
        username: &str,
        registration: Option<&[u8]>,
        request: &str,
    ) -> ApiResult<LoginStart> {
        let request = CredentialRequest::<OpaqueSuite>::deserialize(&decode(request)?)
            .map_err(|_| ApiError::invalid_parameter("invalid credential request"))?;
        let record = registration
            .and_then(|bytes| ServerRegistration::<OpaqueSuite>::deserialize(bytes).ok());

        let mut rng = opaque_ke::rand::rngs::OsRng;
        let start = ServerLogin::start(
            &mut rng,
            &self.setup,
            record,
            request,
            username.as_bytes(),
            ServerLoginParameters::default(),
        )
        .map_err(|_| ApiError::invalid_parameter("invalid credential request"))?;

        let state = start.state.serialize();
        Ok(LoginStart {
            response: URL_SAFE_NO_PAD.encode(start.message.serialize()),
            session: self.seal(username, state.as_slice())?,
        })
    }

    /// Second login round: returns the authenticated username.
    pub fn login_finish(&self, session: &str, finalization: &str) -> ApiResult<String> {
        let (username, state) = self.open(session)?;
        let state = ServerLogin::<OpaqueSuite>::deserialize(&state)
            .map_err(|_| ApiError::bad_credentials())?;
        let finalization =
            CredentialFinalization::<OpaqueSuite>::deserialize(&decode(finalization)?)
                .map_err(|_| ApiError::bad_credentials())?;
        state
            .finish(finalization, ServerLoginParameters::default())
            .map_err(|_| ApiError::bad_credentials())?;
        Ok(username)
    }

    fn seal(&self, username: &str, state: &[u8]) -> ApiResult<String> {
        let expires = chrono::Utc::now().timestamp() + self.session_ttl.as_secs() as i64;
        let mut plaintext = Vec::with_capacity(8 + 2 + username.len() + state.len());
        plaintext.extend_from_slice(&expires.to_be_bytes());
        plaintext.extend_from_slice(&(username.len() as u16).to_be_bytes());
        plaintext.extend_from_slice(username.as_bytes());
        plaintext.extend_from_slice(state);

        let mut nonce = [0u8; 12];
        opaque_ke::rand::RngCore::fill_bytes(&mut opaque_ke::rand::rngs::OsRng, &mut nonce);
        let ciphertext = self
            .session_cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_slice())
            .map_err(|_| ApiError::internal("cannot seal the login session"))?;

        let mut out = nonce.to_vec();
        out.extend_from_slice(&ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(out))
    }

    fn open(&self, session: &str) -> ApiResult<(String, Vec<u8>)> {
        let raw = decode(session)?;
        if raw.len() < 12 + 16 {
            return Err(ApiError::bad_credentials());
        }
        let (nonce, ciphertext) = raw.split_at(12);
        let plaintext = self
            .session_cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| ApiError::bad_credentials())?;
        if plaintext.len() < 10 {
            return Err(ApiError::bad_credentials());
        }
        let expires = i64::from_be_bytes(plaintext[0..8].try_into().expect("8 bytes"));
        if expires < chrono::Utc::now().timestamp() {
            return Err(ApiError::unauthorized("login session expired"));
        }
        let name_len = u16::from_be_bytes(plaintext[8..10].try_into().expect("2 bytes")) as usize;
        if plaintext.len() < 10 + name_len {
            return Err(ApiError::bad_credentials());
        }
        let username = String::from_utf8(plaintext[10..10 + name_len].to_vec())
            .map_err(|_| ApiError::bad_credentials())?;
        Ok((username, plaintext[10 + name_len..].to_vec()))
    }
}

fn decode(input: &str) -> ApiResult<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(input.trim())
        .map_err(|_| ApiError::invalid_parameter("expected base64url data"))
}

fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opaque_ke::{
        ClientLogin, ClientLoginFinishParameters, ClientRegistration,
        ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse,
    };

    fn auth() -> OpaqueAuth {
        let mut rng = opaque_ke::rand::rngs::OsRng;
        let setup = ServerSetup::<OpaqueSuite>::new(&mut rng);
        let mut hasher = Sha256::new();
        hasher.update(setup.serialize().as_slice());
        hasher.update(b"stemson/opaque/session-key/v1");
        OpaqueAuth {
            setup,
            session_cipher: ChaCha20Poly1305::new(Key::from_slice(&hasher.finalize())),
            session_ttl: Duration::from_secs(120),
        }
    }

    fn register(auth: &OpaqueAuth, username: &str, password: &str) -> Vec<u8> {
        let mut rng = opaque_ke::rand::rngs::OsRng;
        let client =
            ClientRegistration::<OpaqueSuite>::start(&mut rng, password.as_bytes()).unwrap();
        let request = URL_SAFE_NO_PAD.encode(client.message.serialize());
        let response = auth.registration_start(username, &request).unwrap();
        let response =
            RegistrationResponse::<OpaqueSuite>::deserialize(&decode(&response).unwrap()).unwrap();
        let finished = client
            .state
            .finish(
                &mut rng,
                password.as_bytes(),
                response,
                ClientRegistrationFinishParameters::default(),
            )
            .unwrap();
        let upload = URL_SAFE_NO_PAD.encode(finished.message.serialize());
        auth.registration_finish(&upload).unwrap()
    }

    #[test]
    fn registers_and_logs_in() {
        let auth = auth();
        let record = register(&auth, "alice", "correct horse battery staple");

        let mut rng = opaque_ke::rand::rngs::OsRng;
        let client =
            ClientLogin::<OpaqueSuite>::start(&mut rng, b"correct horse battery staple").unwrap();
        let request = URL_SAFE_NO_PAD.encode(client.message.serialize());
        let start = auth.login_start("alice", Some(&record), &request).unwrap();
        let response =
            CredentialResponse::<OpaqueSuite>::deserialize(&decode(&start.response).unwrap())
                .unwrap();
        let finished = client
            .state
            .finish(
                &mut rng,
                b"correct horse battery staple",
                response,
                ClientLoginFinishParameters::default(),
            )
            .unwrap();
        let finalization = URL_SAFE_NO_PAD.encode(finished.message.serialize());

        assert_eq!(
            auth.login_finish(&start.session, &finalization).unwrap(),
            "alice"
        );
    }

    #[test]
    fn rejects_a_wrong_password() {
        let auth = auth();
        let record = register(&auth, "alice", "right");

        let mut rng = opaque_ke::rand::rngs::OsRng;
        let client = ClientLogin::<OpaqueSuite>::start(&mut rng, b"wrong").unwrap();
        let request = URL_SAFE_NO_PAD.encode(client.message.serialize());
        let start = auth.login_start("alice", Some(&record), &request).unwrap();
        let response =
            CredentialResponse::<OpaqueSuite>::deserialize(&decode(&start.response).unwrap())
                .unwrap();
        // The client detects the mismatch and refuses to produce a finalization.
        assert!(client
            .state
            .finish(
                &mut rng,
                b"wrong",
                response,
                ClientLoginFinishParameters::default()
            )
            .is_err());
    }

    #[test]
    fn answers_unknown_users_without_leaking_existence() {
        let auth = auth();
        let mut rng = opaque_ke::rand::rngs::OsRng;
        let client = ClientLogin::<OpaqueSuite>::start(&mut rng, b"whatever").unwrap();
        let request = URL_SAFE_NO_PAD.encode(client.message.serialize());
        let start = auth.login_start("nobody", None, &request).unwrap();
        assert!(!start.response.is_empty());
        assert!(!start.session.is_empty());
    }

    #[test]
    fn rejects_tampered_sessions() {
        let auth = auth();
        let record = register(&auth, "alice", "pw");
        let mut rng = opaque_ke::rand::rngs::OsRng;
        let client = ClientLogin::<OpaqueSuite>::start(&mut rng, b"pw").unwrap();
        let request = URL_SAFE_NO_PAD.encode(client.message.serialize());
        let start = auth.login_start("alice", Some(&record), &request).unwrap();

        let mut raw = decode(&start.session).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        let tampered = URL_SAFE_NO_PAD.encode(raw);
        assert!(auth.login_finish(&tampered, "AAAA").is_err());
    }
}
