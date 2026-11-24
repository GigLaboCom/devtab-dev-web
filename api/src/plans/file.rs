//! `config/plans.toml`: the plans, their cards' texts in each language and
//! their prices by the provider's price id. Every problem of the file is
//! reported at once; an unknown field is a problem.
//!
//! ```toml
//! [[plan]]
//! id = "pro-monthly"          # never changes once synced
//! kind = "subscription"       # free, subscription or one_time
//! interval = "month"          # a subscription's, and only a subscription's
//! seats = 3                   # every plan but a free one
//! sort = 2                    # the order of the cards
//! available = false           # shown and sold now
//! until = "2027-01-01T00:00:00Z"   # optional: the end of a limited offer
//! [plan.texts.en]             # required; [plan.texts.ru] optional
//! name = "Monthly"
//! summary = "Pro, billed every month."
//! includes = ["…"]
//! price_note = "…"
//! badge = "…"
//! [[plan.prices]]
//! provider = "fake"
//! provider_price_id = "pri_TEST_0001"
//! currency = "XTS"            # the fake provider's only: a real provider
//! amount_minor = 1111         # states both for its price id
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context as _, bail};
use serde::Deserialize;

use super::{
    LOCALES, MAX_AMOUNT, MAX_INCLUDES, MAX_PLANS, card_text, currency, forbidden, is_plan_id,
    is_provider, limits,
};
use crate::clock::parse_rfc3339;
use crate::payments::PriceSnapshot;
use crate::payments::fake::{self, FakeProvider};
use crate::store::plans::{PlanRow, PlanTextRow};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSpec {
    #[serde(default)]
    plan: Vec<PlanSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanSpec {
    id: String,
    kind: String,
    interval: Option<String>,
    seats: Option<i32>,
    sort: i16,
    #[serde(default)]
    available: bool,
    until: Option<String>,
    #[serde(default)]
    texts: BTreeMap<String, TextSpec>,
    #[serde(default)]
    prices: Vec<PriceSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TextSpec {
    name: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    includes: Vec<String>,
    price_note: Option<String>,
    badge: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PriceSpec {
    provider: String,
    provider_price_id: String,
    currency: Option<String>,
    amount_minor: Option<i64>,
}

/// A price as the file names it: the provider and its price id, plus the
/// amount and currency for the fake provider, whose prices the file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceRef {
    pub provider: String,
    pub provider_price_id: String,
    pub stated: Option<PriceSnapshot>,
}

/// One plan of the file with its texts and prices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub plan: PlanRow,
    pub texts: Vec<PlanTextRow>,
    pub prices: Vec<PriceRef>,
}

/// A checked plans file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlansFile {
    pub plans: Vec<PlanEntry>,
}

impl PlansFile {
    /// The fake provider with the prices this file states for it.
    pub fn fake_provider(&self) -> FakeProvider {
        FakeProvider::new(
            self.plans
                .iter()
                .flat_map(|entry| &entry.prices)
                .filter(|price| price.provider == fake::ID)
                .filter_map(|price| {
                    price
                        .stated
                        .clone()
                        .map(|snapshot| (price.provider_price_id.clone(), snapshot))
                }),
        )
    }
}

/// Reads and checks a plans file.
pub fn read(path: &Path) -> anyhow::Result<PlansFile> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| path.display().to_string())
}

/// Checks the text of a plans file.
pub fn parse(text: &str) -> anyhow::Result<PlansFile> {
    let spec: FileSpec = toml::from_str(text).context("not a plans file")?;
    let mut problems = Vec::new();
    let mut ids = BTreeSet::new();
    let mut price_ids = BTreeSet::new();
    let mut plans = Vec::new();
    for plan in spec.plan {
        let id = plan.id.clone();
        if !ids.insert(id.clone()) {
            problems.push(format!("plan `{id}` appears twice"));
        }
        let mut say = |problem: String| problems.push(format!("plan `{id}`: {problem}"));
        if let Some(entry) = check_plan(plan, &mut price_ids, &mut say) {
            plans.push(entry);
        }
    }
    let available = plans.iter().filter(|e| e.plan.available).count();
    if available > MAX_PLANS {
        problems.push(format!(
            "{available} plans are available; the extension shows at most {MAX_PLANS}"
        ));
    }
    if problems.is_empty() {
        Ok(PlansFile { plans })
    } else {
        bail!(
            "the plans file breaks its rules:\n  - {}",
            problems.join("\n  - ")
        )
    }
}

/// One plan's rules; `None` when it breaks one (reported through `say`).
fn check_plan(
    spec: PlanSpec,
    price_ids: &mut BTreeSet<(String, String)>,
    say: &mut impl FnMut(String),
) -> Option<PlanEntry> {
    let before = std::cell::Cell::new(0_usize);
    let mut count = |problem: String| {
        before.set(before.get() + 1);
        say(problem);
    };
    let id = spec.id;
    if !is_plan_id(&id) {
        count("the id breaks [a-z0-9-]{1,32}".to_owned());
    }
    let kind = spec.kind.as_str();
    if !matches!(kind, "free" | "subscription" | "one_time") {
        count("kind is free, subscription or one_time".to_owned());
    }
    if (kind == "subscription") != spec.interval.is_some() {
        count("a subscription, and only a subscription, has an interval".to_owned());
    }
    if let Some(interval) = &spec.interval
        && !matches!(interval.as_str(), "month" | "year")
    {
        count("interval is month or year".to_owned());
    }
    match spec.seats {
        Some(seats) if !(1..=100).contains(&seats) => count("seats are 1 to 100".to_owned()),
        None if kind != "free" => count("only a free plan has no seats".to_owned()),
        _ => {}
    }
    if kind == "one_time" && spec.available {
        count(
            "a one-time plan cannot be available: the extension's catalogue (v1) skips a paid plan \
             without a term"
                .to_owned(),
        );
    }
    let until = spec
        .until
        .as_deref()
        .map(parse_rfc3339)
        .transpose()
        .unwrap_or_else(|_| {
            count("until is an RFC 3339 time".to_owned());
            None
        });

    let mut texts = Vec::new();
    if !spec.texts.contains_key("en") {
        count("English texts ([plan.texts.en]) are required".to_owned());
    }
    for (locale, text) in spec.texts {
        if !LOCALES.contains(&locale.as_str()) {
            count(format!("texts in `{locale}`: the languages are en and ru"));
            continue;
        }
        match check_texts(&id, &locale, &text) {
            Ok(row) => texts.push(row),
            Err(found) => found
                .into_iter()
                .for_each(|p| count(format!("{locale}: {p}"))),
        }
    }

    if kind == "free" && !spec.prices.is_empty() {
        count("a free plan has no price".to_owned());
    }
    if kind != "free" && spec.available && spec.prices.is_empty() {
        count("an available paid plan needs a price".to_owned());
    }
    let prices = check_prices(spec.prices, price_ids, &mut count);

    (before.get() == 0).then_some(PlanEntry {
        plan: PlanRow {
            id,
            kind: spec.kind,
            interval: spec.interval,
            seats: spec.seats,
            sort: spec.sort,
            available: spec.available,
            until,
        },
        texts,
        prices,
    })
}

/// A plan's prices; those that break a rule are reported through `count`.
fn check_prices(
    specs: Vec<PriceSpec>,
    price_ids: &mut BTreeSet<(String, String)>,
    count: &mut impl FnMut(String),
) -> Vec<PriceRef> {
    let mut prices = Vec::new();
    let mut currencies = BTreeSet::new();
    for price in specs {
        match check_price(price) {
            Ok(price) => {
                if !price_ids.insert((price.provider.clone(), price.provider_price_id.clone())) {
                    count(format!(
                        "price {} {} appears twice in the file",
                        price.provider, price.provider_price_id
                    ));
                }
                if let Some(stated) = &price.stated
                    && !currencies.insert((price.provider.clone(), stated.currency.clone()))
                {
                    count(format!(
                        "two prices at {} in {}: one per provider and currency",
                        price.provider, stated.currency
                    ));
                }
                prices.push(price);
            }
            Err(problem) => count(problem),
        }
    }
    prices
}

fn check_texts(plan_id: &str, locale: &str, spec: &TextSpec) -> Result<PlanTextRow, Vec<String>> {
    let mut problems = Vec::new();
    let mut field = |name: &str, value: Option<&str>, max: usize, optional: bool| {
        card_text(name, value, max, optional).unwrap_or_else(|problem| {
            problems.push(problem);
            None
        })
    };
    let name = field("name", spec.name.as_deref(), limits::NAME, false);
    let summary = field("summary", spec.summary.as_deref(), limits::SUMMARY, true);
    let price_note = field(
        "price_note",
        spec.price_note.as_deref(),
        limits::PRICE_NOTE,
        true,
    );
    let badge = field("badge", spec.badge.as_deref(), limits::BADGE, true);
    let mut includes = Vec::new();
    for (i, line) in spec.includes.iter().enumerate() {
        if let Some(line) = field(
            &format!("includes[{i}]"),
            Some(line),
            limits::INCLUDE,
            false,
        ) {
            includes.push(line);
        }
    }
    if spec.includes.len() > MAX_INCLUDES {
        problems.push(format!(
            "includes has {} lines, more than {MAX_INCLUDES}",
            spec.includes.len()
        ));
    }
    match name {
        Some(name) if problems.is_empty() => Ok(PlanTextRow {
            plan_id: plan_id.to_owned(),
            locale: locale.to_owned(),
            name,
            summary,
            includes,
            price_note,
            badge,
        }),
        _ => Err(problems),
    }
}

fn check_price(spec: PriceSpec) -> Result<PriceRef, String> {
    let provider = spec.provider;
    if !is_provider(&provider) {
        return Err(format!("provider `{provider}` breaks [a-z0-9-]{{2,32}}"));
    }
    let id = spec.provider_price_id;
    if id.is_empty()
        || id.chars().count() > 255
        || id.chars().any(|c| forbidden(c) || c.is_whitespace())
    {
        return Err(format!(
            "price id `{id}` at {provider}: 1 to 255 characters, no spaces or control characters"
        ));
    }
    let stated = match (provider.as_str(), spec.currency, spec.amount_minor) {
        (fake::ID, Some(code), Some(amount_minor)) => {
            if !currency::allowed_at(&provider, &code) {
                return Err(format!(
                    "price {id}: `{code}` is neither a real currency code nor the test currency"
                ));
            }
            if !(0..=MAX_AMOUNT).contains(&amount_minor) {
                return Err(format!(
                    "price {id}: the amount is 0 to {MAX_AMOUNT} minor units"
                ));
            }
            Some(PriceSnapshot {
                currency: code,
                amount_minor,
            })
        }
        (fake::ID, _, _) => {
            return Err(format!(
                "price {id}: a fake price states its currency and amount_minor"
            ));
        }
        (_, None, None) => None,
        (_, _, _) => {
            return Err(format!(
                "price {id}: {provider} states its own amount and currency; the file names only \
                 the price id"
            ));
        }
    };
    Ok(PriceRef {
        provider,
        provider_price_id: id,
        stated,
    })
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    const MONTHLY: &str = r#"
[[plan]]
id = "pro-monthly"
kind = "subscription"
interval = "month"
seats = 3
sort = 2
available = true
[plan.texts.en]
name = "Monthly"
includes = ["One", "Two"]
[[plan.prices]]
provider = "fake"
provider_price_id = "pri_TEST_0001"
currency = "XTS"
amount_minor = 1111
"#;

    #[test]
    fn the_repository_plans_file_parses_and_sells_nothing() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/plans.toml");
        let file = read(&path).unwrap();
        let ids: Vec<&str> = file.plans.iter().map(|p| p.plan.id.as_str()).collect();
        assert_eq!(ids, ["free", "pro-monthly", "pro-annual"]);
        assert!(
            file.plans.iter().all(|p| !p.plan.available),
            "nothing is sold yet"
        );
        // The only prices are the fake provider's, in the test currency.
        for price in file.plans.iter().flat_map(|p| &p.prices) {
            assert_eq!(price.provider, "fake");
            assert_eq!(price.stated.as_ref().unwrap().currency, "XTS");
        }
    }

    #[test]
    fn a_plan_with_texts_and_a_fake_price() {
        let file = parse(MONTHLY).unwrap();
        let entry = &file.plans[0];
        assert_eq!(entry.texts[0].name, "Monthly");
        assert_eq!(entry.texts[0].includes, ["One", "Two"]);
        assert_eq!(entry.prices[0].stated.as_ref().unwrap().amount_minor, 1111);
    }

    #[test]
    fn the_plan_rules() {
        for broken in [
            "[[plan]]\nid = \"Pro\"\nkind = \"free\"\nsort = 1\n[plan.texts.en]\nname = \"F\"\n",
            "[[plan]]\nid = \"a\"\nkind = \"subscription\"\nseats = 3\nsort = 1\n[plan.texts.en]\nname = \"F\"\n",
            "[[plan]]\nid = \"a\"\nkind = \"one_time\"\nsort = 1\n[plan.texts.en]\nname = \"F\"\n",
            "[[plan]]\nid = \"a\"\nkind = \"free\"\nsort = 1\nprice = 5\n[plan.texts.en]\nname = \"F\"\n",
            "[[plan]]\nid = \"a\"\nkind = \"free\"\nsort = 1\nuntil = \"soon\"\n[plan.texts.en]\nname = \"F\"\n",
            "[[plan]]\nid = \"a\"\nkind = \"free\"\nsort = 1\n",
            "[[plan]]\nid = \"a\"\nkind = \"free\"\nsort = 1\n[plan.texts.de]\nname = \"F\"\n",
            "[[plan]]\nid = \"a\"\nkind = \"one_time\"\nseats = 1\nsort = 1\navailable = true\n[plan.texts.en]\nname = \"F\"\n",
        ] {
            assert!(parse(broken).is_err(), "{broken}");
        }
        let twice = format!("{MONTHLY}{MONTHLY}");
        assert!(parse(&twice).unwrap_err().to_string().contains("twice"));
    }

    #[test]
    fn seven_available_plans_are_refused() {
        let seven = (0..7).fold(String::new(), |mut text, i| {
            let _ = write!(
                text,
                "[[plan]]\nid = \"p{i}\"\nkind = \"free\"\nsort = {i}\navailable = true\n\
                 [plan.texts.en]\nname = \"P{i}\"\n"
            );
            text
        });
        let err = parse(&seven).unwrap_err().to_string();
        assert!(err.contains("at most 6"), "{err}");
    }

    #[test]
    fn texts_longer_than_their_limits_in_code_points_are_refused() {
        let at = |field: &str, value: &str| {
            MONTHLY.replace(
                "name = \"Monthly\"",
                &format!("name = \"Monthly\"\n{field} = {value}"),
            )
        };
        assert!(parse(&at("badge", &format!("\"{}\"", "\u{1f600}".repeat(24)))).is_ok());
        assert!(parse(&at("badge", &format!("\"{}\"", "\u{1f600}".repeat(25)))).is_err());
        assert!(parse(&at("summary", &format!("\"{}\"", "x".repeat(141)))).is_err());
        assert!(parse(&at("price_note", &format!("\"{}\"", "x".repeat(61)))).is_err());
        assert!(
            parse(&MONTHLY.replace("\"Monthly\"", &format!("\"{}\"", "M".repeat(33)))).is_err()
        );
        assert!(parse(&MONTHLY.replace("\"One\"", &format!("\"{}\"", "o".repeat(101)))).is_err());
        assert!(
            parse(&MONTHLY.replace("\"One\"", "\"  \"")).is_err(),
            "an empty line"
        );
        let nine = ["\"x\""; 9].join(", ");
        assert!(parse(&MONTHLY.replace("\"One\", \"Two\"", &nine)).is_err());
        assert!(parse(&MONTHLY.replace("\"Monthly\"", "\"Month\\u202Ely\"")).is_err());
    }

    #[test]
    fn the_test_currency_is_the_fake_provider_s_alone() {
        assert!(parse(&MONTHLY.replace("\"fake\"", "\"other\"")).is_err());
        let real = MONTHLY
            .replace("\"fake\"", "\"other\"")
            .replace("currency = \"XTS\"\n", "")
            .replace("amount_minor = 1111\n", "");
        assert!(
            parse(&real).is_ok(),
            "a real provider's price names only its id"
        );
        assert!(parse(&MONTHLY.replace("\"XTS\"", "\"ABC\"")).is_err());
        assert!(parse(&MONTHLY.replace("1111", "100000001")).is_err());
        assert!(parse(&MONTHLY.replace("currency = \"XTS\"\n", "")).is_err());
    }

    #[test]
    fn prices_belong_to_paid_plans() {
        let free = "[[plan]]\nid = \"free\"\nkind = \"free\"\nsort = 1\n[plan.texts.en]\nname = \"Free\"\n\
                    [[plan.prices]]\nprovider = \"fake\"\nprovider_price_id = \"x\"\ncurrency = \"XTS\"\namount_minor = 0\n";
        assert!(parse(free).is_err());
        let unpriced = MONTHLY.split("[[plan.prices]]").next().unwrap();
        assert!(
            parse(unpriced).is_err(),
            "an available paid plan without a price"
        );
    }
}
