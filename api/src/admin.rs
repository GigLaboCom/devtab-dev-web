//! `devtab-api admin …`: administration from the command line on the server,
//! never a web page. Every change is recorded in `app.admin_audit` with the
//! machine's user name, in the same transaction as the change.
//!
//! Licence keys are printed once, when made; only their hashes are stored.

use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail, ensure};
use clap::Subcommand;
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::clock::{parse_rfc3339, rfc3339};
use crate::config::Env;
use crate::licence::key;
use crate::licence::signer::{self, KeyList, Signer};
use crate::plans::catalogue::CatalogueSettings;
use crate::plans::{file as plans_file, sync as plans_sync};
use crate::store::licences::{LicenceRow, NewLicence};
use crate::store::{audit, installations, licences, plans, users};

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// Licences: issue one free of charge, show, block, revoke, replace the key.
    #[command(subcommand)]
    Licence(LicenceCommand),
    /// Users: block or unblock a user of the site.
    #[command(subcommand)]
    User(UserCommand),
    /// Installations: free a seat.
    #[command(subcommand)]
    Installation(InstallationCommand),
    /// Plans: create or update them from config/plans.toml.
    #[command(subcommand)]
    Plans(PlansCommand),
    /// Signing keys: make a key pair, check the server's key.
    #[command(subcommand)]
    Keys(KeysCommand),
}

#[derive(Debug, Subcommand)]
pub enum LicenceCommand {
    /// Issue a licence without payment (a store reviewer, the owner); prints
    /// its key once.
    Comp {
        /// The plan whose seats the licence gets, such as pro-monthly.
        #[arg(long)]
        plan: String,
        /// Issue it to the account with this e-mail address (shown on the
        /// account's pages); without it, to nobody.
        #[arg(long)]
        user: Option<String>,
        /// When it ends: a date (midnight UTC at its start) or an RFC 3339
        /// time. Without it, never.
        #[arg(long)]
        until: Option<String>,
        /// A note for administrators, at most 500 characters, no personal data.
        #[arg(long)]
        note: Option<String>,
    },
    /// Show a licence by its id, or the licences of an account by its e-mail
    /// address: installations and recent administrator actions.
    Show { target: String },
    /// Block the licence's key (abuse): installations get `revoked:
    /// key-blocked` at their next check, and the key activates nothing.
    Block {
        lid: String,
        #[arg(long)]
        reason: String,
    },
    /// Record a refund that the provider did not report: the licence is
    /// revoked (`refund`).
    Revoke {
        lid: String,
        #[arg(long, required = true)]
        refund: bool,
    },
    /// Replace the key and print the new one once; the old key stops
    /// activating, activated installations keep working.
    RotateKey { lid: String },
}

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Block the user: 403 on the site's pages. Their licences keep working
    /// (`licence block` stops a key).
    Block {
        email: String,
    },
    Unblock {
        email: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum InstallationCommand {
    /// Free the seat of an active installation (its id from `licence show`);
    /// its next check gets `revoked: deactivated`.
    Release { id: Uuid },
}

#[derive(Debug, Subcommand)]
pub enum PlansCommand {
    /// Make the catalogue what the file says for the plans it lists: their
    /// rows, their cards' texts, and their prices read from the provider by
    /// price id (the fake provider's from the file). Prints the changes and
    /// asks first; writes a new catalogue version.
    Sync {
        #[arg(long, default_value = "config/plans.toml")]
        file: PathBuf,
        /// Apply without asking.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum KeysCommand {
    /// Make an ES256 key pair: the private key into `--out` (PKCS#8 PEM, a
    /// new file, mode 600), the public key printed as the entry for
    /// config/signing-keys.json.
    Generate {
        /// The key's id: p1, p2, … never reused.
        #[arg(long)]
        kid: Option<String>,
        /// A test key instead (id test-<hex>): development and tests only,
        /// never published.
        #[arg(long)]
        test: bool,
        #[arg(long)]
        out: PathBuf,
        /// Also write a key list holding just the public entry (a new file),
        /// such as the one `DEVTAB_DEV_SIGNING_KEYS_FILE` names in dev.
        #[arg(long)]
        list_out: Option<PathBuf>,
    },
    /// Check `DEVTAB_SIGNING_KEY_FILE` and `DEVTAB_SIGNING_KID` against
    /// config/signing-keys.json (in dev, plus `DEVTAB_DEV_SIGNING_KEYS_FILE`)
    /// as `serve` does at start.
    Check,
}

/// Who runs a command and where its output goes.
pub struct Context<'a> {
    pub actor: String,
    pub now: OffsetDateTime,
    pub out: &'a mut dyn Write,
}

/// The machine's user name the command runs as.
pub fn actor() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Runs a command that uses the database, in the caller's transaction.
/// `confirm` is asked before `plans sync` changes anything.
pub async fn execute(
    command: &AdminCommand,
    conn: &mut PgConnection,
    ctx: &mut Context<'_>,
    confirm: &mut dyn FnMut() -> bool,
) -> anyhow::Result<()> {
    match command {
        AdminCommand::Licence(command) => licence(command, conn, ctx).await,
        AdminCommand::User(UserCommand::Block { email }) => {
            user_status(email, "blocked", conn, ctx).await
        }
        AdminCommand::User(UserCommand::Unblock { email }) => {
            user_status(email, "active", conn, ctx).await
        }
        AdminCommand::Installation(InstallationCommand::Release { id }) => {
            release(*id, conn, ctx).await
        }
        AdminCommand::Plans(PlansCommand::Sync { file, yes }) => {
            let wanted = plans_file::read(file)?;
            let mut ask = || *yes || confirm();
            // No real provider's adapter exists yet: only the fake provider's
            // prices, which the file states, can be read.
            plans_sync::sync(
                &wanted,
                &[],
                &CatalogueSettings::default(),
                conn,
                ctx,
                &mut ask,
            )
            .await
        }
        AdminCommand::Keys(_) => bail!("the keys commands do not use the database"),
    }
}

async fn licence(
    command: &LicenceCommand,
    conn: &mut PgConnection,
    ctx: &mut Context<'_>,
) -> anyhow::Result<()> {
    match command {
        LicenceCommand::Comp {
            plan,
            user,
            until,
            note,
        } => {
            comp(
                plan,
                user.as_deref(),
                until.as_deref(),
                note.as_deref(),
                conn,
                ctx,
            )
            .await
        }
        LicenceCommand::Show { target } => show(target, conn, ctx).await,
        LicenceCommand::Block { lid, reason } => {
            let licence = find_licence(conn, lid).await?;
            licences::block_key(&mut *conn, licence.id, ctx.now).await?;
            audit::record(
                &mut *conn,
                &ctx.actor,
                "licence.block",
                Some(licence.id),
                &json!({ "reason": reason }),
                ctx.now,
            )
            .await?;
            writeln!(
                ctx.out,
                "{lid}: key blocked; installations get revoked: key-blocked"
            )?;
            Ok(())
        }
        LicenceCommand::Revoke { lid, refund } => {
            ensure!(*refund, "say why: --refund");
            let licence = find_licence(conn, lid).await?;
            licences::revoke_for_refund(&mut *conn, licence.id, ctx.now).await?;
            audit::record(
                &mut *conn,
                &ctx.actor,
                "licence.revoke",
                Some(licence.id),
                &json!({ "reason": "refund" }),
                ctx.now,
            )
            .await?;
            writeln!(ctx.out, "{lid}: revoked (refund)")?;
            Ok(())
        }
        LicenceCommand::RotateKey { lid } => {
            let licence = find_licence(conn, lid).await?;
            ensure!(
                licence.status != "revoked" && licence.key_blocked_at.is_none(),
                "{lid} is revoked or its key is blocked: a new key would activate nothing"
            );
            let new_key = key::generate()?;
            let version =
                licences::replace_key(&mut *conn, licence.id, &new_key.hash, &new_key.hint).await?;
            audit::record(
                &mut *conn,
                &ctx.actor,
                "licence.rotate-key",
                Some(licence.id),
                &json!({ "key_version": version }),
                ctx.now,
            )
            .await?;
            writeln!(
                ctx.out,
                "{lid}: new key (version {version}); the old one no longer activates"
            )?;
            writeln!(ctx.out, "key  {}", new_key.display)?;
            writeln!(ctx.out, "     shown once: it is not stored")?;
            Ok(())
        }
    }
}

/// A date (midnight UTC at its start) or an RFC 3339 time.
fn parse_until(text: &str) -> anyhow::Result<OffsetDateTime> {
    if let Ok(at) = parse_rfc3339(text) {
        return Ok(at);
    }
    let date = || -> Option<time::Date> {
        let mut parts = text.split('-');
        let year = parts.next().filter(|y| y.len() == 4)?.parse().ok()?;
        let month = parts.next().filter(|m| m.len() == 2)?.parse::<u8>().ok()?;
        let day = parts.next().filter(|d| d.len() == 2)?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        time::Date::from_calendar_date(year, time::Month::try_from(month).ok()?, day).ok()
    };
    let date = date().with_context(|| {
        format!("--until `{text}` is neither a date (2027-01-01) nor an RFC 3339 time")
    })?;
    Ok(date.midnight().assume_utc())
}

async fn find_licence(conn: &mut PgConnection, lid: &str) -> anyhow::Result<LicenceRow> {
    licences::find_by_lid(&mut *conn, lid)
        .await?
        .with_context(|| format!("no licence {lid}"))
}

async fn user_by_email(conn: &mut PgConnection, email: &str) -> anyhow::Result<Uuid> {
    users::id_by_email(&mut *conn, email)
        .await?
        .context("no user with this e-mail address")
}

async fn comp(
    plan_id: &str,
    user: Option<&str>,
    until: Option<&str>,
    note: Option<&str>,
    conn: &mut PgConnection,
    ctx: &mut Context<'_>,
) -> anyhow::Result<()> {
    let plan = plans::find(&mut *conn, plan_id)
        .await?
        .with_context(|| format!("no plan `{plan_id}`: run `devtab-api admin plans sync` first"))?;
    let seats = plan
        .seats
        .with_context(|| format!("plan `{plan_id}` has no seats (a free plan)"))?;
    let user_id = match user {
        Some(email) => Some(user_by_email(conn, email).await?),
        None => None,
    };
    let paid_until = until.map(parse_until).transpose()?;
    if let Some(end) = paid_until {
        ensure!(end > ctx.now, "--until {end} has passed");
    }
    if let Some(note) = note {
        ensure!(
            note.chars().count() <= 500,
            "--note is longer than 500 characters"
        );
    }
    let lid = key::new_lid()?;
    let new_key = key::generate()?;
    let id = licences::insert(
        &mut *conn,
        &NewLicence {
            lid: &lid,
            user_id,
            customer_id: None,
            plan_id,
            kind: "complimentary",
            seats,
            paid_until,
            cancel_at_period_end: false,
            provider: None,
            provider_subscription_id: None,
            key_hash: &new_key.hash,
            key_hint: &new_key.hint,
            note,
        },
    )
    .await?;
    audit::record(
        &mut *conn,
        &ctx.actor,
        "licence.comp",
        Some(id),
        &json!({
            "plan": plan_id,
            "user": user_id,
            "until": paid_until.map(rfc3339),
            "note": note.is_some(),
        }),
        ctx.now,
    )
    .await?;
    writeln!(ctx.out, "lid  {lid}")?;
    writeln!(ctx.out, "key  {}", new_key.display)?;
    writeln!(ctx.out, "     shown once: it is not stored")?;
    Ok(())
}

fn when(at: Option<OffsetDateTime>) -> String {
    at.map_or_else(|| "-".to_owned(), |t| rfc3339(t).replace('T', " "))
}

async fn show(target: &str, conn: &mut PgConnection, ctx: &mut Context<'_>) -> anyhow::Result<()> {
    let found = if target.starts_with("lic_") {
        vec![find_licence(conn, target).await?]
    } else if target.contains('@') {
        let user_id = user_by_email(conn, target).await?;
        let found = licences::of_user(&mut *conn, user_id).await?;
        if found.is_empty() {
            writeln!(ctx.out, "user {user_id}: no licences")?;
        }
        found
    } else {
        bail!("name a licence (lic_…) or an account's e-mail address");
    };
    for licence in found {
        let list = installations::of_licence(&mut *conn, licence.id).await?;
        let recent = audit::of_licence(&mut *conn, licence.id, 10).await?;
        ctx.out
            .write_all(describe_licence(&licence, &list, &recent)?.as_bytes())?;
    }
    Ok(())
}

/// What `licence show` prints of one licence: never the key, only its hint,
/// and only the first four characters of an installation id.
fn describe_licence(
    licence: &LicenceRow,
    list: &[installations::InstallationRow],
    recent: &[audit::AuditRow],
) -> Result<String, std::fmt::Error> {
    let in_brackets = |text: Option<&str>| text.map(|t| format!(" ({t})")).unwrap_or_default();
    let or_dash = |id: Option<Uuid>| id.map_or("-".to_owned(), |id| id.to_string());
    let active = list.iter().filter(|i| i.status == "active").count();
    let term = licence
        .term
        .as_deref()
        .filter(|_| licence.kind == "subscription")
        .map(|t| format!(", {t}"))
        .unwrap_or_default();
    let mut text = String::new();
    writeln!(text, "licence {}", licence.lid)?;
    writeln!(
        text,
        "  plan          {} ({}{term})",
        licence.plan_id, licence.kind
    )?;
    writeln!(
        text,
        "  status        {}{}",
        licence.status,
        in_brackets(licence.revoke_reason.as_deref())
    )?;
    writeln!(text, "  seats         {} ({active} active)", licence.seats)?;
    writeln!(text, "  paid until    {}", when(licence.paid_until))?;
    if licence.kind == "subscription" {
        writeln!(text, "  renews        {}", !licence.cancel_at_period_end)?;
    }
    writeln!(
        text,
        "  key           DTD-…-{} (version {})",
        licence.key_hint, licence.key_version
    )?;
    writeln!(text, "  key blocked   {}", when(licence.key_blocked_at))?;
    writeln!(text, "  user          {}", or_dash(licence.user_id))?;
    writeln!(text, "  customer      {}", or_dash(licence.customer_id))?;
    writeln!(text, "  created       {}", when(Some(licence.created_at)))?;
    if let Some(note) = &licence.note {
        writeln!(text, "  note          {note}")?;
    }
    writeln!(text, "  installations")?;
    if list.is_empty() {
        writeln!(text, "    none")?;
    }
    for i in list {
        writeln!(
            text,
            "    {}  {}…  {}{}  activated {}  last seen {}",
            i.id,
            crate::licence::wire::short(&i.inst),
            i.status,
            in_brackets(i.released_reason.as_deref()),
            when(Some(i.activated_at)),
            when(Some(i.last_seen_at)),
        )?;
    }
    writeln!(text, "  recent administrator actions")?;
    if recent.is_empty() {
        writeln!(text, "    none")?;
    }
    for entry in recent {
        writeln!(
            text,
            "    {}  {}  {}",
            when(Some(entry.at)),
            entry.actor,
            entry.action
        )?;
    }
    Ok(text)
}

async fn user_status(
    email: &str,
    status: &str,
    conn: &mut PgConnection,
    ctx: &mut Context<'_>,
) -> anyhow::Result<()> {
    let user_id = user_by_email(conn, email).await?;
    ensure!(
        users::set_status(&mut *conn, user_id, status).await?,
        "user {user_id} has no row in app.users"
    );
    let action = if status == "blocked" {
        "user.block"
    } else {
        "user.unblock"
    };
    audit::record(
        &mut *conn,
        &ctx.actor,
        action,
        None,
        &json!({ "user": user_id }),
        ctx.now,
    )
    .await?;
    writeln!(ctx.out, "user {user_id}: {status}")?;
    Ok(())
}

async fn release(id: Uuid, conn: &mut PgConnection, ctx: &mut Context<'_>) -> anyhow::Result<()> {
    let licence_id = installations::release(&mut *conn, id, "admin", ctx.now)
        .await?
        .with_context(|| format!("no active installation {id}"))?;
    audit::record(
        &mut *conn,
        &ctx.actor,
        "installation.release",
        Some(licence_id),
        &json!({ "installation": id }),
        ctx.now,
    )
    .await?;
    writeln!(ctx.out, "installation {id}: released; its seat is free")?;
    Ok(())
}

/// `keys generate`: writes the private key into a new file (mode 600) and
/// prints the public entry for config/signing-keys.json.
pub fn keys_generate(
    kid: Option<&str>,
    test: bool,
    out_path: &Path,
    list_out: Option<&Path>,
    now: OffsetDateTime,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let (pem, entry) = signer::generate(kid, test, now)?;
    let create = |path: &Path, mode: u32, bytes: &[u8]| -> anyhow::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
        #[cfg(not(unix))]
        let _ = mode;
        let mut file = options
            .open(path)
            .with_context(|| format!("creating {} (it must not exist yet)", path.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    };
    if let Some(list_path) = list_out {
        anyhow::ensure!(
            !list_path.exists(),
            "{} exists already",
            list_path.display()
        );
    }
    create(out_path, 0o600, pem.as_bytes())?;
    if let Some(list_path) = list_out {
        let list = KeyList {
            note: Some("Test public keys made on this machine; never committed.".to_owned()),
            keys: vec![entry.clone()],
        };
        create(
            list_path,
            0o600,
            format!("{}\n", serde_json::to_string_pretty(&list)?).as_bytes(),
        )?;
        writeln!(out, "key list: {}", list_path.display())?;
    }
    writeln!(
        out,
        "private key: {} (kid {})",
        out_path.display(),
        entry.kid
    )?;
    writeln!(out, "the entry for config/signing-keys.json:")?;
    writeln!(out, "{}", serde_json::to_string_pretty(&entry)?)?;
    if !test {
        writeln!(
            out,
            "fill in shipped_in and sign_from when the extension ships the key; keep the private key \
             off this repository"
        )?;
    }
    Ok(())
}

/// `keys check`: the server's key against the published list, as `serve`
/// checks it at start.
pub fn keys_check(
    env: Env,
    key_file: Option<&Path>,
    kid: Option<&str>,
    dev_keys: Option<&Path>,
    now: OffsetDateTime,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let (Some(key_file), Some(kid)) = (key_file, kid) else {
        bail!("set DEVTAB_SIGNING_KEY_FILE and DEVTAB_SIGNING_KID");
    };
    let signer = Signer::load(kid, key_file)?;
    let keys = KeyList::trusted(env, dev_keys)?;
    signer::check(&signer, &keys, env, now, false).map_err(|problems| {
        anyhow::anyhow!("the signing key cannot be used: {}", problems.join("; "))
    })?;
    writeln!(
        out,
        "signing key `{kid}` matches the trusted keys and may sign now"
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_takes_a_date_or_a_time() {
        assert_eq!(
            rfc3339(parse_until("2027-01-01").unwrap()),
            "2027-01-01T00:00:00Z"
        );
        assert_eq!(
            rfc3339(parse_until("2027-01-01T12:30:00+02:00").unwrap()),
            "2027-01-01T10:30:00Z"
        );
        assert!(parse_until("next year").is_err());
        assert!(parse_until("2027-02-30").is_err());
        assert!(parse_until("2027-1-1").is_err());
    }
}
