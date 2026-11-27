//! The catalogue as the plans' endpoints send it, built from the stored rows.
//!
//! `GET /v1/plans` (the extension): format `v` 1, the catalogue's version,
//! how long the answer is fresh (`maxAge`), the address where a subscription
//! is managed, and at most six plans in the order of their cards: those that
//! are available, whose offer has not ended and that can be sold (a paid plan
//! needs an active price of the current provider in a currency this
//! environment may serve). English texts only; no seats, nothing about a
//! person.
//!
//! `GET /internal/plans` (the site's pages): the same, with the texts of the
//! asked language (English where a plan has none), every plan whose offer has
//! not ended, and whether each can be bought now (`available`).

use std::fmt::Write as _;

use serde::Serialize;
use sha2::{Digest as _, Sha256};
use time::OffsetDateTime;
use utoipa::ToSchema;

use super::{FORMAT, MAX_PLANS, currency};
use crate::store::plans::{PlanRow, PlanTextRow, PriceRow};

/// What the catalogue is built with besides the rows: from the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogueSettings {
    /// The site's address, `https://<host>` with no slash at the end.
    pub site_url: String,
    /// The provider whose prices are served (`DEVTAB_PAYMENTS`); none: no paid
    /// plan can be sold.
    pub provider: Option<String>,
    /// `maxAge`, seconds.
    pub max_age_s: u32,
    /// Whether a price in the test currency may be served (`dev` only).
    pub test_currencies: bool,
}

impl Default for CatalogueSettings {
    fn default() -> Self {
        Self {
            site_url: "https://web.example.invalid".to_owned(),
            provider: Some(crate::payments::fake::ID.to_owned()),
            max_age_s: 86_400,
            test_currencies: true,
        }
    }
}

/// The stored catalogue: plans in the order of their cards, every plan's
/// texts, the active prices of the current provider, and the version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogueRows {
    pub version: Option<String>,
    pub plans: Vec<PlanRow>,
    pub texts: Vec<PlanTextRow>,
    pub prices: Vec<PriceRow>,
}

/// `free` or `paid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlanKind {
    Free,
    Paid,
}

/// A subscription's billing period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Term {
    Month,
    Year,
}

/// A price: the amount in the currency's minor units, and the currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Price {
    /// Minor units (cents), 0 to 100 000 000.
    pub amount: i64,
    /// ISO 4217, upper case.
    pub currency: String,
}

/// A plan as its card shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// `[a-z0-9-]{1,32}`; never changes.
    pub id: String,
    pub kind: PlanKind,
    /// A paid plan's billing period; null for a free plan.
    #[schema(required = true)]
    pub term: Option<Term>,
    /// 1–32 characters.
    pub name: String,
    /// At most 140 characters.
    #[schema(required = true)]
    pub summary: Option<String>,
    /// 0–8 lines of 1–100 characters, in the order to show.
    pub includes: Vec<String>,
    /// A paid plan's price; null for a free plan.
    #[schema(required = true)]
    pub price: Option<Price>,
    /// At most 60 characters.
    #[schema(required = true)]
    pub price_note: Option<String>,
    /// At most 24 characters.
    #[schema(required = true)]
    pub badge: Option<String>,
    /// Where a paid plan is bought; null for a free plan.
    #[schema(required = true)]
    pub checkout_url: Option<String>,
    /// The end of a limited offer, Unix seconds.
    #[schema(required = true)]
    pub until: Option<i64>,
}

/// The answer of `GET /v1/plans`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalogue {
    pub v: u8,
    pub catalog: String,
    pub max_age: u32,
    pub manage_url: String,
    pub plans: Vec<Plan>,
}

/// A plan as the site's pages show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SitePlan {
    #[serde(flatten)]
    pub plan: Plan,
    /// Whether it can be bought now; the page shows "soon" instead of a
    /// button when not.
    pub available: bool,
    /// The language of its texts: the one asked for, or `en` where the plan
    /// has none in it.
    pub locale: String,
    /// A team plan's smallest team: its price is per member. Null for a
    /// personal plan.
    #[schema(required = true)]
    pub min_members: Option<i16>,
}

/// The answer of `GET /internal/plans`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SiteCatalogue {
    /// The format version, 1.
    pub v: u8,
    /// The catalogue's version.
    pub catalog: String,
    /// Seconds the answer is fresh.
    pub max_age: u32,
    /// Where a subscription is managed.
    pub manage_url: String,
    /// The language asked for.
    pub locale: String,
    /// In the order of the cards.
    pub plans: Vec<SitePlan>,
}

/// A plan's card in `locale` (else English) and whether it can be sold now;
/// `None` when it cannot be shown at all (its offer ended, no texts).
fn card(
    rows: &CatalogueRows,
    plan: &PlanRow,
    locale: &str,
    settings: &CatalogueSettings,
    now: OffsetDateTime,
) -> Option<(Plan, bool, String)> {
    if plan.until.is_some_and(|until| until <= now) {
        return None;
    }
    let texts_in = |locale: &str| {
        rows.texts
            .iter()
            .find(|t| t.plan_id == plan.id && t.locale == locale)
    };
    let texts = texts_in(locale).or_else(|| texts_in("en"))?;
    let (kind, term) = match (plan.kind.as_str(), plan.interval.as_deref()) {
        ("free", _) => (PlanKind::Free, None),
        (_, Some("month")) => (PlanKind::Paid, Some(Term::Month)),
        (_, Some("year")) => (PlanKind::Paid, Some(Term::Year)),
        // A paid plan without a term (one-time): the extension skips it.
        _ => (PlanKind::Paid, None),
    };
    let price = if kind == PlanKind::Free {
        None
    } else {
        rows.prices
            .iter()
            .filter(|p| p.active && p.plan_id == plan.id)
            .filter(|p| settings.provider.as_deref() == Some(p.provider.as_str()))
            .find(|p| currency::servable(&p.currency, settings.test_currencies))
            .map(|p| Price {
                amount: p.amount_minor,
                currency: p.currency.clone(),
            })
    };
    let sellable =
        plan.available && (kind == PlanKind::Free || (term.is_some() && price.is_some()));
    let paid = kind == PlanKind::Paid;
    Some((
        Plan {
            id: plan.id.clone(),
            kind,
            term,
            name: texts.name.clone(),
            summary: texts.summary.clone(),
            includes: texts.includes.clone(),
            price,
            price_note: texts.price_note.clone(),
            badge: texts.badge.clone(),
            checkout_url: paid
                .then(|| format!("{}/pro/checkout/?plan={}", settings.site_url, plan.id)),
            until: plan.until.map(OffsetDateTime::unix_timestamp),
        },
        sellable,
        texts.locale.clone(),
    ))
}

fn version(rows: &CatalogueRows) -> String {
    rows.version.clone().unwrap_or_else(|| "0".to_owned())
}

fn manage_url(settings: &CatalogueSettings) -> String {
    format!("{}/pro/manage/", settings.site_url)
}

/// The answer of `GET /v1/plans` at `now`.
pub fn public(
    rows: &CatalogueRows,
    settings: &CatalogueSettings,
    now: OffsetDateTime,
) -> Catalogue {
    let plans = rows
        .plans
        .iter()
        // Team plans are sold on the site only: the extension's catalogue has no
        // members in its model.
        .filter(|plan| plan.min_members.is_none())
        .filter_map(|plan| card(rows, plan, "en", settings, now))
        .filter(|(_, sellable, _)| *sellable)
        .map(|(plan, _, _)| plan)
        .take(MAX_PLANS)
        .collect();
    Catalogue {
        v: FORMAT,
        catalog: version(rows),
        max_age: settings.max_age_s,
        manage_url: manage_url(settings),
        plans,
    }
}

/// The answer of `GET /internal/plans?locale=<locale>` at `now`.
pub fn site(
    rows: &CatalogueRows,
    settings: &CatalogueSettings,
    locale: &str,
    now: OffsetDateTime,
) -> SiteCatalogue {
    let mut sold = 0;
    let plans = rows
        .plans
        .iter()
        .filter_map(|plan| card(rows, plan, locale, settings, now).map(|c| (c, plan.min_members)))
        .map(|((plan, sellable, texts_locale), min_members)| {
            // The extension gets the first six that can be sold; the page shows the same.
            let available = sellable && sold < MAX_PLANS;
            sold += usize::from(available);
            SitePlan {
                plan,
                available,
                locale: texts_locale,
                min_members,
            }
        })
        .collect();
    SiteCatalogue {
        v: FORMAT,
        catalog: version(rows),
        max_age: settings.max_age_s,
        manage_url: manage_url(settings),
        locale: locale.to_owned(),
        plans,
    }
}

/// A strong `ETag` of an answer's body: the first 16 bytes of its SHA-256.
pub fn etag(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut tag = String::from("\"");
    for byte in &digest[..16] {
        let _ = write!(tag, "{byte:02x}");
    }
    tag.push('"');
    tag
}

/// Whether an `If-None-Match` header matches `etag` (weak comparison, as for
/// `If-None-Match`; `*` matches anything).
pub fn none_match(header: &str, etag: &str) -> bool {
    let opaque = |tag: &str| tag.trim().trim_start_matches("W/").to_owned();
    let ours = opaque(etag);
    header
        .split(',')
        .map(str::trim)
        .any(|tag| tag == "*" || opaque(tag) == ours)
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).unwrap()
    }

    fn plan(id: &str, kind: &str, interval: Option<&str>, sort: i16) -> PlanRow {
        PlanRow {
            id: id.to_owned(),
            kind: kind.to_owned(),
            interval: interval.map(str::to_owned),
            seats: (kind != "free").then_some(3),
            sort,
            available: true,
            until: None,
            min_members: None,
        }
    }

    fn text(plan_id: &str, locale: &str, name: &str) -> PlanTextRow {
        PlanTextRow {
            plan_id: plan_id.to_owned(),
            locale: locale.to_owned(),
            name: name.to_owned(),
            summary: None,
            includes: vec![],
            price_note: None,
            badge: None,
        }
    }

    fn price(plan_id: &str, provider: &str, currency: &str, amount: i64) -> PriceRow {
        PriceRow {
            id: Uuid::new_v4(),
            plan_id: plan_id.to_owned(),
            provider: provider.to_owned(),
            provider_price_id: format!("{plan_id}-{provider}-{currency}"),
            currency: currency.to_owned(),
            amount_minor: amount,
            active: true,
        }
    }

    fn rows() -> CatalogueRows {
        CatalogueRows {
            version: Some("2026-11-24.1".to_owned()),
            plans: vec![
                plan("free", "free", None, 1),
                plan("pro-monthly", "subscription", Some("month"), 2),
                plan("pro-annual", "subscription", Some("year"), 3),
            ],
            texts: vec![
                text("free", "en", "Free"),
                text("pro-monthly", "en", "Monthly"),
                text("pro-monthly", "ru", "Ежемесячно"),
                text("pro-annual", "en", "Annually"),
            ],
            prices: vec![
                price("pro-monthly", "fake", "XTS", 1111),
                price("pro-annual", "fake", "XTS", 11110),
            ],
        }
    }

    fn ids(catalogue: &Catalogue) -> Vec<&str> {
        catalogue.plans.iter().map(|p| p.id.as_str()).collect()
    }

    #[test]
    fn the_extension_gets_the_available_plans_in_order() {
        let c = public(&rows(), &CatalogueSettings::default(), at(1_800_000_000));
        assert_eq!(ids(&c), ["free", "pro-monthly", "pro-annual"]);
        assert_eq!(
            (c.v, c.catalog.as_str(), c.max_age),
            (1, "2026-11-24.1", 86_400)
        );
        assert_eq!(c.manage_url, "https://web.example.invalid/pro/manage/");
        let monthly = &c.plans[1];
        assert_eq!(
            (monthly.kind, monthly.term),
            (PlanKind::Paid, Some(Term::Month))
        );
        assert_eq!(
            monthly.price,
            Some(Price {
                amount: 1111,
                currency: "XTS".to_owned()
            })
        );
        assert_eq!(
            monthly.checkout_url.as_deref(),
            Some("https://web.example.invalid/pro/checkout/?plan=pro-monthly")
        );
        let free = &c.plans[0];
        assert_eq!(
            (free.term, &free.price, &free.checkout_url),
            (None, &None, &None)
        );
    }

    #[test]
    fn unavailable_ended_and_unpriced_plans_stay_out() {
        let mut r = rows();
        r.plans[0].available = false;
        r.plans[2].until = Some(at(1_800_000_000));
        let c = public(&r, &CatalogueSettings::default(), at(1_800_000_000));
        assert_eq!(ids(&c), ["pro-monthly"], "until has come: the offer ended");
        let later = public(&rows(), &CatalogueSettings::default(), at(1_700_000_000));
        assert_eq!(later.plans.len(), 3);

        let none = CatalogueSettings {
            provider: None,
            ..CatalogueSettings::default()
        };
        assert_eq!(
            ids(&public(&rows(), &none, at(0))),
            ["free"],
            "no provider, no price"
        );
        let other = CatalogueSettings {
            provider: Some("other".to_owned()),
            ..CatalogueSettings::default()
        };
        assert_eq!(ids(&public(&rows(), &other, at(0))), ["free"]);
    }

    #[test]
    fn the_test_currency_is_served_only_where_test_currencies_are() {
        let staging = CatalogueSettings {
            test_currencies: false,
            ..CatalogueSettings::default()
        };
        assert_eq!(ids(&public(&rows(), &staging, at(0))), ["free"]);
        let mut real = rows();
        real.prices.push(price("pro-annual", "fake", "EUR", 1111));
        assert_eq!(ids(&public(&real, &staging, at(0))), ["free", "pro-annual"]);
    }

    #[test]
    fn at_most_six_plans() {
        let mut r = rows();
        r.plans = (0..8)
            .map(|i| plan(&format!("p{i}"), "free", None, i))
            .collect();
        r.texts = (0..8).map(|i| text(&format!("p{i}"), "en", "P")).collect();
        let c = public(&r, &CatalogueSettings::default(), at(0));
        assert_eq!(c.plans.len(), 6);
        let s = site(&r, &CatalogueSettings::default(), "en", at(0));
        assert_eq!(s.plans.iter().filter(|p| p.available).count(), 6);
        assert_eq!(s.plans.len(), 8);
    }

    #[test]
    fn the_site_gets_every_plan_in_its_language() {
        let mut r = rows();
        r.plans[2].available = false;
        let s = site(&r, &CatalogueSettings::default(), "ru", at(0));
        let summary: Vec<(&str, &str, bool, &str)> = s
            .plans
            .iter()
            .map(|p| {
                (
                    p.plan.id.as_str(),
                    p.plan.name.as_str(),
                    p.available,
                    p.locale.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("free", "Free", true, "en"),
                ("pro-monthly", "Ежемесячно", true, "ru"),
                ("pro-annual", "Annually", false, "en"),
            ]
        );
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["plans"][2]["available"], false);
        assert_eq!(
            json["plans"][2]["price"]["amount"], 11110,
            "flattened fields"
        );
    }

    #[test]
    fn the_wire_names_and_order_are_the_contract_s() {
        let c = public(&rows(), &CatalogueSettings::default(), at(0));
        let text = serde_json::to_string(&c.plans[1]).unwrap();
        assert_eq!(
            text,
            r#"{"id":"pro-monthly","kind":"paid","term":"month","name":"Monthly","summary":null,"includes":[],"price":{"amount":1111,"currency":"XTS"},"priceNote":null,"badge":null,"checkoutUrl":"https://web.example.invalid/pro/checkout/?plan=pro-monthly","until":null}"#
        );
        let head = serde_json::to_string(&public(
            &CatalogueRows::default(),
            &CatalogueSettings::default(),
            at(0),
        ))
        .unwrap();
        assert_eq!(
            head,
            r#"{"v":1,"catalog":"0","maxAge":86400,"manageUrl":"https://web.example.invalid/pro/manage/","plans":[]}"#
        );
    }

    #[test]
    fn etags_and_if_none_match() {
        let tag = etag(b"{}");
        assert_eq!(tag.len(), 34);
        assert!(tag.starts_with('"') && tag.ends_with('"'));
        assert_ne!(tag, etag(b"{ }"));
        assert!(none_match(&tag, &tag));
        assert!(none_match(&format!("\"x\", W/{tag}"), &tag));
        assert!(none_match("*", &tag));
        assert!(!none_match("\"x\"", &tag));
    }
}
