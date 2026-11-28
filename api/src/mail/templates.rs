//! The skeletons of the mails' templates, `mail/listmonk/`, built into the
//! binary so that `admin mail templates` uploads them from the server without
//! the repository: a body for each kind and language, and the subjects in
//! `templates.toml`. A test holds them to [`MailKind::ALL`] and [`LOCALES`].

use std::collections::BTreeMap;

use anyhow::{Context as _, anyhow};

use super::{LOCALES, MailKind};

macro_rules! body {
    ($kind:literal, $locale:literal) => {
        (
            $kind,
            $locale,
            include_str!(concat!(
                "../../../mail/listmonk/",
                $kind,
                ".",
                $locale,
                ".html"
            )),
        )
    };
}

/// Every body: kind, locale, HTML.
const BODIES: [(&str, &str, &str); 20] = [
    body!("trial-started", "en"),
    body!("trial-started", "ru"),
    body!("trial-ending", "en"),
    body!("trial-ending", "ru"),
    body!("trial-ended", "en"),
    body!("trial-ended", "ru"),
    body!("welcome", "en"),
    body!("welcome", "ru"),
    body!("licence-added", "en"),
    body!("licence-added", "ru"),
    body!("renewal-reminder", "en"),
    body!("renewal-reminder", "ru"),
    body!("payment-failed", "en"),
    body!("payment-failed", "ru"),
    body!("subscription-cancelled", "en"),
    body!("subscription-cancelled", "ru"),
    body!("subscription-ended", "en"),
    body!("subscription-ended", "ru"),
    body!("team-invite", "en"),
    body!("team-invite", "ru"),
];

/// The subjects, by kind and locale.
const SUBJECTS: &str = include_str!("../../../mail/listmonk/templates.toml");

/// One template of the instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub kind: MailKind,
    pub locale: &'static str,
    /// What the instance calls it: `DevTab.Dev: <kind> (<locale>)`.
    pub name: String,
    pub subject: String,
    pub body: &'static str,
}

/// Every template, in the order of [`MailKind::ALL`] and [`LOCALES`].
pub fn all() -> anyhow::Result<Vec<Template>> {
    let subjects: BTreeMap<String, BTreeMap<String, String>> =
        toml::from_str(SUBJECTS).context("mail/listmonk/templates.toml")?;
    let mut out = Vec::new();
    for kind in MailKind::ALL {
        for locale in LOCALES {
            let body = BODIES
                .iter()
                .find(|(k, l, _)| *k == kind.as_str() && *l == locale)
                .map(|(_, _, body)| *body)
                .ok_or_else(|| anyhow!("no body for {kind}.{locale}"))?;
            let subject = subjects
                .get(kind.as_str())
                .and_then(|by| by.get(locale))
                .ok_or_else(|| anyhow!("no subject for {kind}.{locale} in templates.toml"))?;
            out.push(Template {
                kind,
                locale,
                name: format!("DevTab.Dev: {kind} ({locale})"),
                subject: subject.clone(),
                body,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_subject_and_a_body_in_every_language() {
        let all = all().unwrap();
        assert_eq!(all.len(), MailKind::ALL.len() * LOCALES.len());
        for template in &all {
            assert!(
                template
                    .body
                    .contains(&format!("<html lang=\"{}\">", template.locale))
            );
            assert!(
                !template.body.contains(".Subscriber"),
                "{}: data only",
                template.name
            );
            assert!(!template.subject.is_empty());
        }
    }
}
