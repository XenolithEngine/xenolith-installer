//! Localisation, shared by the headless CLI and the GUI.
//!
//! Catalogues are Fluent (`.ftl`) files embedded at build time, so a single
//! source of truth serves both Rust front-ends. English is the fallback: a key
//! missing from the active language falls through to English, and a key missing
//! everywhere returns itself (so nothing ever renders blank).

use fluent::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

/// Languages shipped with the installer. The first is the fallback.
pub const AVAILABLE: &[&str] = &["en", "ru", "zh"];
pub const FALLBACK: &str = "en";

const EN_FTL: &str = include_str!("../locales/en.ftl");
const RU_FTL: &str = include_str!("../locales/ru.ftl");
const ZH_FTL: &str = include_str!("../locales/zh.ftl");

fn catalogue(lang: &str) -> Option<&'static str> {
    match lang {
        "en" => Some(EN_FTL),
        "ru" => Some(RU_FTL),
        "zh" => Some(ZH_FTL),
        _ => None,
    }
}

/// Pick the best available language from the user's preferences.
///
/// Matching is by primary language subtag, so `ru-RU` matches `ru`. Falls back
/// to [`FALLBACK`] when nothing matches.
pub fn resolve_locale(preferred: &[&str], available: &[&str], fallback: &str) -> String {
    for pref in preferred {
        let plang = primary(pref);
        if let Some(found) = available.iter().find(|a| primary(a) == plang) {
            return (*found).to_string();
        }
    }
    fallback.to_string()
}

/// Resolve against the real environment (`$LANG`/OS) and the shipped catalogues.
pub fn detect_locale() -> String {
    match sys_locale::get_locale() {
        Some(loc) => resolve_locale(&[loc.as_str()], AVAILABLE, FALLBACK),
        None => FALLBACK.to_string(),
    }
}

fn primary(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or(tag)
        .to_ascii_lowercase()
}

fn bundle_for(lang: &str) -> FluentBundle<FluentResource> {
    let langid: LanguageIdentifier = lang.parse().unwrap_or_else(|_| "en".parse().unwrap());
    let mut bundle = FluentBundle::new(vec![langid]);
    // No Unicode bidi isolation marks — keeps CLI output and tests clean.
    bundle.set_use_isolating(false);
    if let Some(src) = catalogue(lang) {
        if let Ok(res) = FluentResource::try_new(src.to_string()) {
            let _ = bundle.add_resource(res);
        }
    }
    bundle
}

/// A resolved localiser for one active language, with English fallback.
pub struct I18n {
    lang: String,
    active: FluentBundle<FluentResource>,
    fallback: FluentBundle<FluentResource>,
}

impl I18n {
    /// Build for `lang` (normalised to an available language; fallback if not).
    pub fn new(lang: &str) -> Self {
        let resolved = resolve_locale(&[lang], AVAILABLE, FALLBACK);
        I18n {
            active: bundle_for(&resolved),
            fallback: bundle_for(FALLBACK),
            lang: resolved,
        }
    }

    pub fn from_env() -> Self {
        Self::new(&detect_locale())
    }

    pub fn language(&self) -> &str {
        &self.lang
    }

    /// Look up `key`, returning the active translation, then the English
    /// fallback, then the key itself.
    pub fn get(&self, key: &str) -> String {
        self.format(key, None)
    }

    /// Look up `key` substituting `args` (e.g. `[("version", "3.2")]`).
    pub fn get_args(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut fargs = FluentArgs::new();
        for (k, v) in args {
            fargs.set(*k, FluentValue::from(*v));
        }
        self.format(key, Some(&fargs))
    }

    fn format(&self, key: &str, args: Option<&FluentArgs>) -> String {
        format_from(&self.active, key, args)
            .or_else(|| format_from(&self.fallback, key, args))
            .unwrap_or_else(|| key.to_string())
    }
}

fn format_from(
    bundle: &FluentBundle<FluentResource>,
    key: &str,
    args: Option<&FluentArgs>,
) -> Option<String> {
    let msg = bundle.get_message(key)?;
    let pattern = msg.value()?;
    let mut errors = Vec::new();
    let out = bundle.format_pattern(pattern, args, &mut errors);
    if errors.is_empty() {
        Some(out.into_owned())
    } else {
        None
    }
}

/// Map a localised string per group kind, for the package table headers.
pub fn group_label(i18n: &I18n, kind: crate::manifest::Kind) -> String {
    match kind {
        crate::manifest::Kind::Host => i18n.get("group-hosts"),
        crate::manifest::Kind::Target => i18n.get("group-targets"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_matches_primary_subtag() {
        assert_eq!(resolve_locale(&["ru-RU"], AVAILABLE, FALLBACK), "ru");
        assert_eq!(resolve_locale(&["en_US.UTF-8"], AVAILABLE, FALLBACK), "en");
    }

    #[test]
    fn resolve_falls_back_when_unmatched() {
        assert_eq!(resolve_locale(&["fr", "de"], AVAILABLE, FALLBACK), "en");
    }

    #[test]
    fn resolve_respects_preference_order() {
        assert_eq!(
            resolve_locale(&["de", "ru", "en"], AVAILABLE, FALLBACK),
            "ru"
        );
    }

    #[test]
    fn translates_in_the_active_language() {
        let ru = I18n::new("ru");
        assert_eq!(ru.language(), "ru");
        assert_eq!(ru.get("status-installed"), "Установлено");
        let en = I18n::new("en");
        assert_eq!(en.get("status-installed"), "Installed");
    }

    #[test]
    fn substitutes_arguments_without_bidi_marks() {
        let ru = I18n::new("ru");
        assert_eq!(
            ru.get_args("status-update-available", &[("version", "3.2")]),
            "Доступно обновление: 3.2"
        );
    }

    #[test]
    fn unknown_key_returns_itself() {
        let en = I18n::new("en");
        assert_eq!(en.get("no-such-key"), "no-such-key");
    }

    #[test]
    fn unknown_language_falls_back_to_english() {
        let x = I18n::new("fr");
        assert_eq!(x.language(), "en");
        assert_eq!(x.get("action-cancel"), "Cancel");
    }

    #[test]
    fn group_labels_localise() {
        use crate::manifest::Kind;
        let ru = I18n::new("ru");
        assert_eq!(group_label(&ru, Kind::Target), "Платформы рантайма");
        assert_eq!(group_label(&ru, Kind::Host), "Средства разработки");
    }
}
