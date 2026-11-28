//! The listmonk provider and `admin mail templates` against a stand-in of
//! listmonk's API on a loopback port; no real instance is ever reached.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use devtab_api::Config;
use devtab_api::mail::listmonk::{ApiToken, ListmonkMailer, ListmonkSettings};
use devtab_api::mail::{LOCALES, Mail, MailError, MailKind, MailSettings, Mailer};
use serde_json::{Map, Value, json};

/// What the stand-in was asked, and how it answers.
#[derive(Debug, Default)]
struct StandIn {
    asked: Mutex<Vec<(String, Value, String)>>,
    /// The status of the next `POST /api/tx` answers, in order (200 after).
    tx_status: Mutex<Vec<u16>>,
    /// Subscribers that exist (a second create answers 409).
    subscribers: Mutex<Vec<String>>,
    templates: Mutex<Vec<Value>>,
}

async fn stand_in() -> (String, Arc<StandIn>) {
    let state = Arc::new(StandIn::default());
    let record = |s: &StandIn, what: &str, body: Value, headers: &HeaderMap| {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        s.asked.lock().unwrap().push((what.to_owned(), body, auth));
    };
    let app = Router::new()
        .route(
            "/api/tx",
            post(
                move |State(s): State<Arc<StandIn>>,
                      headers: HeaderMap,
                      Json(body): Json<Value>| async move {
                    record(&s, "tx", body, &headers);
                    let mut statuses = s.tx_status.lock().unwrap();
                    let status = if statuses.is_empty() {
                        200
                    } else {
                        statuses.remove(0)
                    };
                    (
                        StatusCode::from_u16(status).unwrap(),
                        Json(json!({"data": status == 200})),
                    )
                },
            ),
        )
        .route(
            "/api/subscribers",
            post(
                move |State(s): State<Arc<StandIn>>,
                      headers: HeaderMap,
                      Json(body): Json<Value>| async move {
                    let email = body["email"].as_str().unwrap_or("").to_owned();
                    record(&s, "subscribe", body, &headers);
                    let mut known = s.subscribers.lock().unwrap();
                    if known.contains(&email) {
                        return (
                            StatusCode::CONFLICT,
                            Json(json!({"message": "E-mail already exists."})),
                        );
                    }
                    known.push(email);
                    (StatusCode::OK, Json(json!({"data": {"id": known.len()}})))
                },
            ),
        )
        .route(
            "/api/templates",
            get(|State(s): State<Arc<StandIn>>| async move {
                Json(json!({"data": s.templates.lock().unwrap().clone()}))
            })
            .post(
                move |State(s): State<Arc<StandIn>>,
                      headers: HeaderMap,
                      Json(body): Json<Value>| async move {
                    record(&s, "create-template", body.clone(), &headers);
                    let mut all = s.templates.lock().unwrap();
                    let id = 100 + all.len();
                    all.push(json!({"id": id, "name": body["name"], "type": body["type"]}));
                    Json(json!({"data": {"id": id}}))
                },
            ),
        )
        .route(
            "/api/templates/{id}",
            put(
                move |State(s): State<Arc<StandIn>>,
                      Path(id): Path<i64>,
                      headers: HeaderMap,
                      Json(body): Json<Value>| async move {
                    record(&s, &format!("update-template {id}"), body, &headers);
                    Json(json!({"data": {"id": id}}))
                },
            ),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), state)
}

fn settings(url: &str, list_id: Option<i64>) -> ListmonkSettings {
    ListmonkSettings {
        url: url.to_owned(),
        api_user: "devtab".to_owned(),
        token_file: "unused".into(),
        templates: BTreeMap::from([("welcome.ru".to_owned(), 7)]),
        list_id,
        from: Some("DevTab.Dev <no-reply@example.invalid>".to_owned()),
    }
}

fn mailer(url: &str, list_id: Option<i64>) -> ListmonkMailer {
    ListmonkMailer::with_token(&settings(url, list_id), ApiToken::parse("tok3n").unwrap()).unwrap()
}

fn welcome(locale: &str) -> Mail {
    let mut data = Map::new();
    data.insert("plan_name".to_owned(), json!("Pro"));
    Mail {
        kind: MailKind::Welcome,
        to: "buyer@example.invalid".to_owned(),
        locale: locale.to_owned(),
        data,
    }
}

/// `devtab:tok3n` in HTTP basic auth.
const BASIC: &str = "Basic ZGV2dGFiOnRvazNu";

#[tokio::test]
async fn without_a_list_a_mail_goes_in_external_mode_with_its_template_and_data() {
    let (url, stand) = stand_in().await;
    mailer(&url, None).send(&welcome("ru")).await.unwrap();
    let asked = stand.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "no subscriber is made: {asked:?}");
    let (what, body, auth) = &asked[0];
    assert_eq!(what, "tx");
    assert_eq!(auth, BASIC);
    assert_eq!(
        body,
        &json!({
            "subscriber_mode": "external",
            "subscriber_emails": ["buyer@example.invalid"],
            "template_id": 7,
            "data": {"plan_name": "Pro"},
            "content_type": "html",
            "from_email": "DevTab.Dev <no-reply@example.invalid>",
        })
    );
}

#[tokio::test]
async fn with_a_list_the_recipient_joins_it_first_once_and_the_mail_names_the_subscriber() {
    let (url, stand) = stand_in().await;
    let mailer = mailer(&url, Some(4));
    mailer.send(&welcome("ru")).await.unwrap();
    mailer.send(&welcome("ru")).await.unwrap();
    let asked = stand.asked.lock().unwrap().clone();
    let whats: Vec<&str> = asked.iter().map(|(w, _, _)| w.as_str()).collect();
    assert_eq!(
        whats,
        ["subscribe", "tx", "subscribe", "tx"],
        "a 409 is a subscriber that exists"
    );
    assert_eq!(
        asked[0].1,
        json!({"email": "buyer@example.invalid", "name": "", "status": "enabled", "lists": [4],
               "preconfirm_subscriptions": true})
    );
    assert_eq!(asked[1].1["subscriber_email"], "buyer@example.invalid");
    assert!(asked[1].1.get("subscriber_mode").is_none());
}

#[tokio::test]
async fn a_kind_without_a_template_or_a_refusal_is_permanent_and_an_outage_is_retried() {
    let (url, stand) = stand_in().await;
    let mailer = mailer(&url, None);
    let err = mailer.send(&welcome("en")).await.unwrap_err();
    assert!(
        matches!(&err, MailError::Permanent(m) if m.contains("welcome.en")),
        "{err}"
    );
    assert!(stand.asked.lock().unwrap().is_empty(), "nothing asked");
    stand.tx_status.lock().unwrap().extend([400, 503, 429]);
    assert!(matches!(
        mailer.send(&welcome("ru")).await,
        Err(MailError::Permanent(_))
    ));
    assert!(matches!(
        mailer.send(&welcome("ru")).await,
        Err(MailError::Retry(_))
    ));
    assert!(matches!(
        mailer.send(&welcome("ru")).await,
        Err(MailError::Retry(_))
    ));
    let err = devtab_api::mail::listmonk::ListmonkMailer::with_token(
        &settings("http://127.0.0.1:9", None),
        ApiToken::parse("t").unwrap(),
    )
    .unwrap()
    .send(&welcome("ru"))
    .await
    .unwrap_err();
    assert!(matches!(err, MailError::Retry(_)), "unreachable: {err}");
}

#[tokio::test]
async fn the_templates_are_listed_dry_then_created_then_updated_and_their_ids_printed() {
    // Offline: nothing to ask, nothing changed.
    let mut out = Vec::new();
    devtab_api::admin::mail_templates(&Config::default(), false, &mut out)
        .await
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    let count = MailKind::ALL.len() * LOCALES.len();
    assert_eq!(
        text.matches("would create DevTab.Dev: ").count(),
        count,
        "{text}"
    );
    assert!(text.contains("dry run"));
    assert!(
        devtab_api::admin::mail_templates(&Config::default(), true, &mut Vec::new())
            .await
            .is_err(),
        "--apply needs an instance"
    );

    let (url, stand) = stand_in().await;
    let dir = std::env::temp_dir().join(format!("devtab-mail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let token = dir.join("token");
    std::fs::write(&token, "tok3n\n").unwrap();
    let mut listmonk = settings(&url, None);
    listmonk.token_file = token;
    let config = Config {
        mail: MailSettings::Listmonk(listmonk),
        ..Config::default()
    };
    let mut out = Vec::new();
    devtab_api::admin::mail_templates(&config, true, &mut out)
        .await
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(
        text.matches("created  DevTab.Dev: ").count(),
        count,
        "{text}"
    );
    let line = text
        .lines()
        .find(|l| l.starts_with("DEVTAB_MAIL_LISTMONK_TEMPLATES="))
        .expect("the mapping");
    let mapping =
        devtab_api::mail::listmonk::parse_templates(line.split_once('=').unwrap().1).unwrap();
    assert_eq!(mapping.len(), count);
    assert_eq!(mapping.get("trial-started.en"), Some(&100));
    {
        let asked = stand.asked.lock().unwrap();
        let first = &asked[0].1;
        assert_eq!(first["type"], "tx");
        assert_eq!(first["name"], "DevTab.Dev: trial-started (en)");
        assert!(
            first["body"]
                .as_str()
                .unwrap()
                .contains("{{ .Tx.Data.plan_name }}")
        );
        assert!(!first["subject"].as_str().unwrap().is_empty());
    }

    let mut out = Vec::new();
    devtab_api::admin::mail_templates(&config, true, &mut out)
        .await
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(
        text.matches("updated  DevTab.Dev: ").count(),
        count,
        "{text}"
    );
    assert_eq!(
        stand.templates.lock().unwrap().len(),
        count,
        "matched by name, none twice"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
