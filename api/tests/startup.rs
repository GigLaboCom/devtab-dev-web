//! `devtab-api serve` refuses to start with a signing key it may not sign
//! with, before it opens a listener or touches the database: a private key
//! that is not the trusted key of its id, an id nobody trusts, no key at all,
//! a test key outside `dev`, and a developer's key list outside `dev`; nor
//! does it start with the fake payment provider in `prod`. The keys are made
//! for each test in a temporary folder.
#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;
use std::process::{Command, Output};

use devtab_api::licence::signer::{self, KeyList};
use time::OffsetDateTime;

/// A temporary folder with a fresh test key: its PEM, a key list with its
/// public entry, and its id.
struct Keys {
    dir: PathBuf,
    kid: String,
    pem: String,
    list: String,
}

impl Keys {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("devtab-startup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let (pem, entry) = signer::generate(None, true, OffsetDateTime::now_utc()).unwrap();
        std::fs::write(dir.join("key.pem"), pem).unwrap();
        let list = KeyList {
            note: None,
            keys: vec![entry.clone()],
        };
        std::fs::write(dir.join("keys.json"), serde_json::to_string(&list).unwrap()).unwrap();
        Self {
            pem: dir.join("key.pem").to_str().unwrap().to_owned(),
            list: dir.join("keys.json").to_str().unwrap().to_owned(),
            dir,
            kid: entry.kid,
        }
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn serve(env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devtab-api"));
    command
        .arg("serve")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        // Nothing listens there: the start must fail before the database matters.
        .env("DEVTAB_DATABASE_URL", "postgres://nobody@127.0.0.1:1/none")
        .env("DEVTAB_PUBLIC_ADDR", "127.0.0.1:0")
        .env("DEVTAB_INTERNAL_ADDR", "127.0.0.1:0");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn refused(output: &Output, words: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "it started: {stderr}");
    assert!(stderr.contains(words), "expected `{words}` in: {stderr}");
    assert!(!stderr.contains("api started"), "{stderr}");
}

const HOST: (&str, &str) = ("DEVTAB_LICENCE_HOST", "licence.example.invalid");
const ORIGINS: (&str, &str) = (
    "DEVTAB_EXTENSION_ORIGINS",
    "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
);
const SITE: (&str, &str) = ("DEVTAB_SITE_URL", "https://web.example.invalid");
const SUPABASE: (&str, &str) = ("DEVTAB_SUPABASE_URL", "https://project.example.invalid");

#[test]
fn a_private_key_that_is_not_the_trusted_one_stops_the_start() {
    let trusted = Keys::new();
    let other = Keys::new();
    // The other key's PEM under the trusted key's id.
    let output = serve(&[
        ("DEVTAB_SIGNING_KEY_FILE", &other.pem),
        ("DEVTAB_SIGNING_KID", &trusted.kid),
        ("DEVTAB_DEV_SIGNING_KEYS_FILE", &trusted.list),
    ]);
    refused(&output, "does not match the public key");
}

#[test]
fn an_id_nobody_trusts_stops_the_start() {
    let keys = Keys::new();
    let output = serve(&[
        ("DEVTAB_SIGNING_KEY_FILE", &keys.pem),
        ("DEVTAB_SIGNING_KID", "p1"),
        ("DEVTAB_DEV_SIGNING_KEYS_FILE", &keys.list),
    ]);
    refused(&output, "is not in config/signing-keys.json");
    // Without the developer's list the published one, empty for now, is all there is.
    let output = serve(&[
        ("DEVTAB_SIGNING_KEY_FILE", &keys.pem),
        ("DEVTAB_SIGNING_KID", &keys.kid),
    ]);
    refused(&output, "no key is published yet");
}

#[test]
fn no_signing_key_stops_the_start() {
    refused(
        &serve(&[]),
        "DEVTAB_SIGNING_KEY_FILE and DEVTAB_SIGNING_KID are required",
    );
    let output = serve(&[
        ("DEVTAB_SIGNING_KEY_FILE", "/nonexistent/signing.pem"),
        ("DEVTAB_SIGNING_KID", "test-00000000"),
    ]);
    refused(&output, "reading the signing key");
}

#[test]
fn a_test_key_stops_the_start_in_staging_and_prod() {
    let keys = Keys::new();
    for env in ["staging", "prod"] {
        let output = serve(&[
            ("DEVTAB_ENV", env),
            ("DEVTAB_SIGNING_KEY_FILE", &keys.pem),
            ("DEVTAB_SIGNING_KID", &keys.kid),
            HOST,
            ORIGINS,
            SITE,
            SUPABASE,
        ]);
        refused(&output, "is a test key");
    }
}

#[test]
fn the_fake_payment_provider_stops_the_start_in_prod() {
    let keys = Keys::new();
    let output = serve(&[
        ("DEVTAB_ENV", "prod"),
        ("DEVTAB_SIGNING_KEY_FILE", &keys.pem),
        ("DEVTAB_SIGNING_KID", &keys.kid),
        HOST,
        ORIGINS,
        SITE,
        SUPABASE,
        ("DEVTAB_PAYMENTS", "fake"),
    ]);
    refused(&output, "DEVTAB_PAYMENTS=fake is refused in prod");
}

#[test]
fn a_developers_key_list_stops_the_start_in_staging_and_prod() {
    let keys = Keys::new();
    for env in ["staging", "prod"] {
        let output = serve(&[
            ("DEVTAB_ENV", env),
            ("DEVTAB_SIGNING_KEY_FILE", &keys.pem),
            ("DEVTAB_SIGNING_KID", &keys.kid),
            ("DEVTAB_DEV_SIGNING_KEYS_FILE", &keys.list),
            HOST,
            ORIGINS,
        ]);
        refused(&output, "accepted only in dev");
    }
}

#[test]
fn no_private_key_is_in_the_repository() {
    for dir in ["config", "contract/licence-v1", "api"] {
        for entry in walk(&support::repo(dir)) {
            let text = std::fs::read(&entry).unwrap_or_default();
            let text = String::from_utf8_lossy(&text);
            // Split so that this file does not match itself.
            assert!(
                !text.contains(concat!("BEGIN", " PRIV", "ATE KEY")),
                "{}",
                entry.display()
            );
        }
    }
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}
