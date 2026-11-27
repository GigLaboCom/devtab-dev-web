//! `devtab-api admin plans sync`: makes the database's catalogue what the
//! plans file says, for the plans the file lists. It reads every price's
//! id, amount and currency from its provider by the price id or lookup key
//! (the fake provider's from the file), checks a recurring price's interval
//! against its plan's, checks the result as the extension would read
//! it, prints the changes, asks, and writes them with a new catalogue version
//! and one `app.admin_audit` row, in the caller's transaction.
//!
//! Plans the database has and the file does not are left as they are. A
//! listed plan's texts and prices are the file's: texts of a language the
//! file no longer has are deleted, prices it no longer has are deactivated.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use anyhow::{Context as _, bail};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;

use super::catalogue::{self, CatalogueRows, CatalogueSettings};
use super::file::PlansFile;
use super::{MAX_AMOUNT, MAX_ANSWER_BYTES, MAX_PLANS, currency};
use crate::admin::Context;
use crate::clock::rfc3339;
use crate::payments::{Billing, PaymentProvider, fake};
use crate::store::audit;
use crate::store::plans::{self as store, NewPrice, PlanRow, PlanTextRow, PriceRow};

/// Reads every price of the file from its provider: the fake provider with
/// the file's own prices, the others from `providers`.
pub async fn resolve_prices(
    file: &PlansFile,
    providers: &[&dyn PaymentProvider],
) -> anyhow::Result<Vec<NewPrice>> {
    let fake = file.fake_provider();
    let mut problems = Vec::new();
    let mut prices = Vec::new();
    let mut per_currency = BTreeSet::new();
    for entry in &file.plans {
        for price in &entry.prices {
            let provider: Option<&dyn PaymentProvider> = if price.provider == fake::ID {
                Some(&fake)
            } else {
                providers.iter().copied().find(|p| p.id() == price.provider)
            };
            let Some(provider) = provider else {
                problems.push(format!(
                    "plan `{}`: no adapter for the provider `{}` on this server",
                    entry.plan.id, price.provider
                ));
                continue;
            };
            let found = match provider.resolve(&price.key).await {
                Ok(found) => found,
                Err(err) => {
                    problems.push(format!(
                        "plan `{}`: price {} at {}: {err}",
                        entry.plan.id, price.key, price.provider
                    ));
                    continue;
                }
            };
            let billed = match &found.billing {
                Billing::Unknown => None,
                Billing::OneTime => Some(None),
                Billing::Every(interval) => Some(Some(interval)),
            };
            if let Some(billed) = billed
                && billed != entry.plan.interval.as_ref()
            {
                let every = |interval: Option<&String>| {
                    interval.map_or("once".to_owned(), |i| format!("every {i}"))
                };
                problems.push(format!(
                    "plan `{}`: price {} at {} bills {}, the plan {}",
                    entry.plan.id,
                    price.key,
                    price.provider,
                    every(billed),
                    every(entry.plan.interval.as_ref())
                ));
                continue;
            }
            let snapshot = found.snapshot;
            if !currency::allowed_at(&price.provider, &snapshot.currency) {
                problems.push(format!(
                    "plan `{}`: price {} at {} is in `{}`, which is not a real currency code \
                     (the test currency is the fake provider's alone)",
                    entry.plan.id, price.key, price.provider, snapshot.currency
                ));
                continue;
            }
            if !(0..=MAX_AMOUNT).contains(&snapshot.amount_minor) {
                problems.push(format!(
                    "plan `{}`: price {} at {} is {} minor units, outside 0 to {MAX_AMOUNT}",
                    entry.plan.id, price.key, price.provider, snapshot.amount_minor
                ));
                continue;
            }
            if !per_currency.insert((
                entry.plan.id.clone(),
                price.provider.clone(),
                snapshot.currency.clone(),
            )) {
                problems.push(format!(
                    "plan `{}`: two prices at {} in {}: one per provider and currency",
                    entry.plan.id, price.provider, snapshot.currency
                ));
                continue;
            }
            prices.push(NewPrice {
                plan_id: entry.plan.id.clone(),
                provider: price.provider.clone(),
                provider_price_id: found.id,
                currency: snapshot.currency,
                amount_minor: snapshot.amount_minor,
            });
        }
    }
    if problems.is_empty() {
        Ok(prices)
    } else {
        bail!(
            "the prices cannot be read:\n  - {}",
            problems.join("\n  - ")
        )
    }
}

/// The next catalogue version after `current` on `now`'s date:
/// `<date>.<n>`, `n` counting the day's syncs from 1.
pub fn next_version(current: Option<&str>, now: OffsetDateTime) -> String {
    let date = now.date();
    let today = format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    );
    let n = current
        .and_then(|v| v.strip_prefix(&today))
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|n| n.parse::<u32>().ok())
        .map_or(1, |n| n + 1);
    format!("{today}.{n}")
}

fn describe_plan(plan: &PlanRow) -> String {
    format!(
        "kind={} interval={} seats={} sort={} available={} until={} min_members={}",
        plan.kind,
        plan.interval.as_deref().unwrap_or("-"),
        plan.seats.map_or("-".to_owned(), |s| s.to_string()),
        plan.sort,
        plan.available,
        plan.until.map_or("-".to_owned(), rfc3339),
        plan.min_members.map_or("-".to_owned(), |m| m.to_string()),
    )
}

fn describe_text(text: &PlanTextRow) -> String {
    let quoted = |v: &Option<String>| v.as_ref().map_or("-".to_owned(), |v| format!("{v:?}"));
    format!(
        "name={:?} summary={} includes={:?} price_note={} badge={}",
        text.name,
        quoted(&text.summary),
        text.includes,
        quoted(&text.price_note),
        quoted(&text.badge),
    )
}

/// What a sync changes.
#[derive(Debug, Default)]
struct Changes<'a> {
    plans: Vec<&'a PlanRow>,
    texts: Vec<&'a PlanTextRow>,
    gone_texts: Vec<(String, String)>,
    new_prices: Vec<&'a NewPrice>,
    /// An existing row and what the provider states now.
    changed_prices: Vec<(&'a PriceRow, &'a NewPrice)>,
    gone_prices: Vec<&'a PriceRow>,
}

impl Changes<'_> {
    fn is_empty(&self) -> bool {
        self.plans.is_empty()
            && self.texts.is_empty()
            && self.gone_texts.is_empty()
            && self.new_prices.is_empty()
            && self.changed_prices.is_empty()
            && self.gone_prices.is_empty()
    }
}

/// The changes `file` (with its `prices` read) makes to the stored catalogue,
/// printed into `out`.
fn compare<'a>(
    file: &'a PlansFile,
    prices: &'a [NewPrice],
    current: &'a CatalogueRows,
    stored_prices: &'a [PriceRow],
    out: &mut String,
) -> anyhow::Result<Changes<'a>> {
    let mut changes = Changes::default();
    for entry in &file.plans {
        let plan = &entry.plan;
        match current.plans.iter().find(|p| p.id == plan.id) {
            None => {
                writeln!(out, "+ plan {}  {}", plan.id, describe_plan(plan))?;
                changes.plans.push(plan);
            }
            Some(old) if old != plan => {
                writeln!(out, "~ plan {}  {}", plan.id, describe_plan(old))?;
                writeln!(
                    out,
                    "       {}  {}",
                    " ".repeat(plan.id.len()),
                    describe_plan(plan)
                )?;
                changes.plans.push(plan);
            }
            Some(_) => {}
        }
        for text in &entry.texts {
            let label = format!("{}/{}", text.plan_id, text.locale);
            match current
                .texts
                .iter()
                .find(|t| t.plan_id == text.plan_id && t.locale == text.locale)
            {
                None => {
                    writeln!(out, "+ texts {label}  {}", describe_text(text))?;
                    changes.texts.push(text);
                }
                Some(old) if old != text => {
                    writeln!(out, "~ texts {label}  {}", describe_text(old))?;
                    writeln!(
                        out,
                        "        {}  {}",
                        " ".repeat(label.len()),
                        describe_text(text)
                    )?;
                    changes.texts.push(text);
                }
                Some(_) => {}
            }
        }
        for old in current
            .texts
            .iter()
            .filter(|t| t.plan_id == plan.id && entry.texts.iter().all(|n| n.locale != t.locale))
        {
            writeln!(out, "- texts {}/{}", old.plan_id, old.locale)?;
            changes
                .gone_texts
                .push((old.plan_id.clone(), old.locale.clone()));
        }
        compare_prices(&plan.id, prices, stored_prices, &mut changes, out)?;
    }
    for old in current
        .plans
        .iter()
        .filter(|p| file.plans.iter().all(|e| e.plan.id != p.id))
    {
        writeln!(out, "  plan {}  not in the file: left as it is", old.id)?;
    }
    Ok(changes)
}

/// The price changes of one listed plan.
fn compare_prices<'a>(
    plan_id: &str,
    prices: &'a [NewPrice],
    stored_prices: &'a [PriceRow],
    changes: &mut Changes<'a>,
    out: &mut String,
) -> anyhow::Result<()> {
    for price in prices.iter().filter(|p| p.plan_id == plan_id) {
        let label = format!(
            "{} {} {}",
            price.plan_id, price.provider, price.provider_price_id
        );
        match stored_prices.iter().find(|p| {
            p.provider == price.provider && p.provider_price_id == price.provider_price_id
        }) {
            Some(old) if old.plan_id != price.plan_id => bail!(
                "price {} at {} belongs to plan `{}` in the database: a price id never moves \
                 to another plan",
                price.provider_price_id,
                price.provider,
                old.plan_id
            ),
            None => {
                writeln!(
                    out,
                    "+ price {label}  {} {}",
                    price.currency, price.amount_minor
                )?;
                changes.new_prices.push(price);
            }
            Some(old)
                if !old.active
                    || old.currency != price.currency
                    || old.amount_minor != price.amount_minor =>
            {
                writeln!(
                    out,
                    "~ price {label}  {} {}{} -> {} {}",
                    old.currency,
                    old.amount_minor,
                    if old.active { "" } else { " (inactive)" },
                    price.currency,
                    price.amount_minor
                )?;
                changes.changed_prices.push((old, price));
            }
            Some(_) => {}
        }
    }
    for old in stored_prices.iter().filter(|p| {
        p.active
            && p.plan_id == plan_id
            && prices
                .iter()
                .all(|n| n.provider != p.provider || n.provider_price_id != p.provider_price_id)
    }) {
        writeln!(
            out,
            "- price {} {} {}  {} {} (deactivated)",
            old.plan_id, old.provider, old.provider_price_id, old.currency, old.amount_minor
        )?;
        changes.gone_prices.push(old);
    }
    Ok(())
}

/// The stored catalogue after the sync, to check it as a whole.
fn after(
    file: &PlansFile,
    prices: &[NewPrice],
    current: &CatalogueRows,
    stored_prices: &[PriceRow],
) -> CatalogueRows {
    let listed = |id: &str| file.plans.iter().any(|e| e.plan.id == id);
    let mut plans: Vec<PlanRow> = current
        .plans
        .iter()
        .filter(|p| !listed(&p.id))
        .cloned()
        .chain(file.plans.iter().map(|e| e.plan.clone()))
        .collect();
    plans.sort_by(|a, b| (a.sort, &a.id).cmp(&(b.sort, &b.id)));
    let texts = current
        .texts
        .iter()
        .filter(|t| !listed(&t.plan_id))
        .cloned()
        .chain(file.plans.iter().flat_map(|e| e.texts.iter().cloned()))
        .collect();
    let prices = stored_prices
        .iter()
        .filter(|p| p.active && !listed(&p.plan_id))
        .cloned()
        .chain(prices.iter().map(|p| PriceRow {
            id: uuid::Uuid::nil(),
            plan_id: p.plan_id.clone(),
            provider: p.provider.clone(),
            provider_price_id: p.provider_price_id.clone(),
            currency: p.currency.clone(),
            amount_minor: p.amount_minor,
            active: true,
        }))
        .collect();
    CatalogueRows {
        version: Some("0000-00-00.0".to_owned()),
        plans,
        texts,
        prices,
    }
}

/// The catalogue after the sync as the extension would get it: at most six
/// available plans, and an answer within its size limit for every provider
/// that has prices (the test currency counted as served).
fn check_after(
    rows: &CatalogueRows,
    settings: &CatalogueSettings,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    let available = rows.plans.iter().filter(|p| p.available).count();
    if available > MAX_PLANS {
        bail!(
            "{available} plans would be available (counting those not in the file); the extension \
             shows at most {MAX_PLANS}"
        );
    }
    let providers: BTreeSet<Option<String>> = rows
        .prices
        .iter()
        .map(|p| Some(p.provider.clone()))
        .chain([None])
        .collect();
    for provider in providers {
        let settings = CatalogueSettings {
            provider: provider.clone(),
            test_currencies: true,
            ..settings.clone()
        };
        let body = serde_json::to_vec(&catalogue::public(rows, &settings, now))?;
        if body.len() > MAX_ANSWER_BYTES {
            bail!(
                "GET /v1/plans would answer {} bytes with the provider {}; the extension reads at \
                 most {MAX_ANSWER_BYTES}: shorten the texts",
                body.len(),
                provider.as_deref().unwrap_or("none")
            );
        }
    }
    Ok(())
}

/// Writes `changes` with the new catalogue `version` and its audit row.
async fn apply(
    changes: &Changes<'_>,
    version: &str,
    conn: &mut PgConnection,
    ctx: &mut Context<'_>,
) -> anyhow::Result<()> {
    for plan in &changes.plans {
        store::upsert(&mut *conn, plan).await?;
    }
    // Deactivate first: one active price per plan, provider and currency.
    for old in &changes.gone_prices {
        store::deactivate_price(&mut *conn, old.id).await?;
    }
    for (old, now) in &changes.changed_prices {
        store::update_price(&mut *conn, old.id, &now.currency, now.amount_minor)
            .await
            .with_context(|| format!("updating price {}", now.provider_price_id))?;
    }
    for price in &changes.new_prices {
        store::insert_price(&mut *conn, price)
            .await
            .with_context(|| format!("adding price {}", price.provider_price_id))?;
    }
    for text in &changes.texts {
        store::upsert_text(&mut *conn, text).await?;
    }
    for (plan_id, locale) in &changes.gone_texts {
        store::delete_text(&mut *conn, plan_id, locale).await?;
    }
    store::set_catalog_version(&mut *conn, version).await?;

    let plan_ids: Vec<&str> = changes.plans.iter().map(|p| p.id.as_str()).collect();
    let text_ids: Vec<String> = changes
        .texts
        .iter()
        .map(|t| format!("{}/{}", t.plan_id, t.locale))
        .chain(changes.gone_texts.iter().map(|(p, l)| format!("-{p}/{l}")))
        .collect();
    let price_ids: Vec<String> = changes
        .new_prices
        .iter()
        .map(|p| format!("{}:{}", p.provider, p.provider_price_id))
        .chain(
            changes
                .changed_prices
                .iter()
                .map(|(_, p)| format!("{}:{}", p.provider, p.provider_price_id)),
        )
        .chain(
            changes
                .gone_prices
                .iter()
                .map(|p| format!("-{}:{}", p.provider, p.provider_price_id)),
        )
        .collect();
    audit::record(
        &mut *conn,
        &ctx.actor,
        "plans.sync",
        None,
        &json!({
            "plans": plan_ids,
            "texts": text_ids,
            "prices": price_ids,
            "catalog": version,
        }),
        ctx.now,
    )
    .await?;
    writeln!(
        ctx.out,
        "written: {} plan(s), {} text(s), {} price(s); catalogue {version}",
        changes.plans.len(),
        changes.texts.len() + changes.gone_texts.len(),
        changes.new_prices.len() + changes.changed_prices.len() + changes.gone_prices.len(),
    )?;
    Ok(())
}

/// Syncs the catalogue with `file`, asking `confirm` before writing. The
/// prices are read from the fake provider of the file and from `providers`.
pub async fn sync(
    file: &PlansFile,
    providers: &[&dyn PaymentProvider],
    settings: &CatalogueSettings,
    conn: &mut PgConnection,
    ctx: &mut Context<'_>,
    confirm: &mut dyn FnMut() -> bool,
) -> anyhow::Result<()> {
    let prices = resolve_prices(file, providers).await?;
    let current = CatalogueRows {
        version: store::catalog_version(&mut *conn).await?,
        plans: store::all(&mut *conn).await?,
        texts: store::texts(&mut *conn).await?,
        prices: Vec::new(),
    };
    let stored_prices = store::prices(&mut *conn).await?;
    check_after(
        &after(file, &prices, &current, &stored_prices),
        settings,
        ctx.now,
    )?;

    let mut report = String::new();
    let changes = compare(file, &prices, &current, &stored_prices, &mut report)?;
    ctx.out.write_all(report.as_bytes())?;
    if changes.is_empty() {
        writeln!(ctx.out, "the catalogue is up to date")?;
        return Ok(());
    }
    let version = next_version(current.version.as_deref(), ctx.now);
    writeln!(
        ctx.out,
        "catalogue version {} -> {version}",
        current.version.as_deref().unwrap_or("-")
    )?;
    if !confirm() {
        writeln!(ctx.out, "nothing changed")?;
        return Ok(());
    }

    apply(&changes, &version, conn, ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payments::{FoundPrice, PriceKey, PriceSnapshot, ProviderError, ProviderFuture};
    use crate::plans::file;

    fn at(text: &str) -> OffsetDateTime {
        crate::clock::parse_rfc3339(text).unwrap()
    }

    #[test]
    fn versions_count_the_day_s_syncs() {
        let now = at("2026-11-24T10:00:00Z");
        assert_eq!(next_version(None, now), "2026-11-24.1");
        assert_eq!(next_version(Some("2026-11-24.1"), now), "2026-11-24.2");
        assert_eq!(next_version(Some("2026-11-23.7"), now), "2026-11-24.1");
        assert_eq!(next_version(Some("0"), now), "2026-11-24.1");
        assert!(crate::plans::is_catalog_id(&next_version(
            Some("2026-11-24.41"),
            now
        )));
    }

    /// A provider that answers like a remote one: later, with its own ids.
    #[derive(Debug)]
    struct Remote;

    impl PaymentProvider for Remote {
        fn id(&self) -> &'static str {
            "remote"
        }

        fn price<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PriceSnapshot> {
            Box::pin(async move {
                tokio::task::yield_now().await;
                match id {
                    "price_A" => Ok(PriceSnapshot {
                        currency: "EUR".to_owned(),
                        amount_minor: 1111,
                    }),
                    "price_T" => Ok(PriceSnapshot {
                        currency: "XTS".to_owned(),
                        amount_minor: 1111,
                    }),
                    other => Err(ProviderError::NotFound(format!("price {other}"))),
                }
            })
        }
    }

    const PLAN: &str = "[[plan]]\nid = \"pro\"\nkind = \"subscription\"\ninterval = \"month\"\nseats = 1\nsort = 1\n\
                        [plan.texts.en]\nname = \"Pro\"\n[[plan.prices]]\nprovider = \"remote\"\nprovider_price_id = \"ID\"\n";

    #[tokio::test]
    async fn a_real_provider_s_amount_comes_from_its_price() {
        let file = file::parse(&PLAN.replace("ID", "price_A")).unwrap();
        let prices = resolve_prices(&file, &[&Remote]).await.unwrap();
        assert_eq!(
            (prices[0].currency.as_str(), prices[0].amount_minor),
            ("EUR", 1111)
        );
        let err = resolve_prices(&file, &[]).await.unwrap_err().to_string();
        assert!(err.contains("no adapter"), "{err}");
        let unknown = file::parse(&PLAN.replace("ID", "price_X")).unwrap();
        assert!(resolve_prices(&unknown, &[&Remote]).await.is_err());
        let test = file::parse(&PLAN.replace("ID", "price_T")).unwrap();
        let err = resolve_prices(&test, &[&Remote])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("fake provider's alone"), "{err}");
    }

    /// A provider with lookup keys, which says how its prices bill.
    #[derive(Debug)]
    struct Keyed;

    impl PaymentProvider for Keyed {
        fn id(&self) -> &'static str {
            "keyed"
        }

        fn price<'a>(&'a self, id: &'a str) -> ProviderFuture<'a, PriceSnapshot> {
            Box::pin(async move { Err(ProviderError::NotFound(format!("price {id}"))) })
        }

        fn resolve<'a>(&'a self, key: &'a PriceKey) -> ProviderFuture<'a, FoundPrice> {
            Box::pin(async move {
                let billing = match key {
                    PriceKey::LookupKey(k) if k == "monthly" => Billing::Every("month".to_owned()),
                    PriceKey::LookupKey(k) if k == "yearly" => Billing::Every("year".to_owned()),
                    PriceKey::LookupKey(k) if k == "once" => Billing::OneTime,
                    other => return Err(ProviderError::NotFound(other.to_string())),
                };
                Ok(FoundPrice {
                    id: format!("price_{key}").replace(' ', "_"),
                    snapshot: PriceSnapshot {
                        currency: "EUR".to_owned(),
                        amount_minor: 900,
                    },
                    billing,
                })
            })
        }
    }

    #[tokio::test]
    async fn a_lookup_key_stores_the_price_s_id_and_its_billing_must_be_the_plan_s() {
        let keyed = |key: &str| {
            file::parse(&format!(
                "[[plan]]\nid = \"pro\"\nkind = \"subscription\"\ninterval = \"month\"\nseats = 1\n\
                 sort = 1\n[plan.texts.en]\nname = \"Pro\"\n[[plan.prices]]\nprovider = \"keyed\"\n\
                 lookup_key = \"{key}\"\n"
            ))
            .unwrap()
        };
        let prices = resolve_prices(&keyed("monthly"), &[&Keyed]).await.unwrap();
        assert_eq!(prices[0].provider_price_id, "price_lookup_key_monthly");
        for (key, want) in [
            ("yearly", "bills every year, the plan every month"),
            ("once", "bills once"),
        ] {
            let err = resolve_prices(&keyed(key), &[&Keyed])
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains(want), "{key}: {err}");
        }
        let err = resolve_prices(&keyed("monthly"), &[&Remote])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no adapter"), "{err}");
        let unsupported =
            file::parse(&PLAN.replace("provider_price_id = \"ID\"", "lookup_key = \"k\"")).unwrap();
        let err = resolve_prices(&unsupported, &[&Remote])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not support lookup keys"), "{err}");
    }

    #[tokio::test]
    async fn the_fake_provider_s_prices_are_the_file_s() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/plans.toml");
        let file = file::read(&path).unwrap();
        let prices = resolve_prices(&file, &[]).await.unwrap();
        let summary: Vec<(&str, &str, i64)> = prices
            .iter()
            .map(|p| (p.plan_id.as_str(), p.currency.as_str(), p.amount_minor))
            .collect();
        assert_eq!(
            summary,
            [("pro-monthly", "XTS", 1111), ("pro-annual", "XTS", 11110)]
        );
    }
}
