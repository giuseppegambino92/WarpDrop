//! Native-side strings (tray menu, notifications). Uses the same generated
//! JSON as the webview so both always agree.

use crate::settings::LanguagePref;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    En,
    It,
}

pub struct Strings {
    en: HashMap<String, String>,
    it: HashMap<String, String>,
}

impl Strings {
    pub fn load() -> Strings {
        let parse = |s: &str| serde_json::from_str::<HashMap<String, String>>(s).expect("bundled translations are valid JSON");
        Strings { en: parse(include_str!("../../src/i18n/en.json")), it: parse(include_str!("../../src/i18n/it.json")) }
    }

    /// Looks up `key` and substitutes `{0}`, `{1}`… Falls back to English, then the key.
    pub fn tr(&self, lang: Lang, key: &str, args: &[&str]) -> String {
        let table = match lang {
            Lang::En => &self.en,
            Lang::It => &self.it,
        };
        let mut s = table.get(key).or_else(|| self.en.get(key)).cloned().unwrap_or_else(|| key.to_string());
        for (i, a) in args.iter().enumerate() {
            s = s.replace(&format!("{{{i}}}"), a);
        }
        s
    }
}

pub fn system_lang() -> Lang {
    match sys_locale::get_locale() {
        Some(l) if l.to_ascii_lowercase().starts_with("it") => Lang::It,
        _ => Lang::En,
    }
}

pub fn resolve(pref: &LanguagePref) -> Lang {
    match pref {
        LanguagePref::System => system_lang(),
        LanguagePref::En => Lang::En,
        LanguagePref::It => Lang::It,
    }
}

/// Upper-cases the first letter (for sentences that start with a placeholder).
pub fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_and_placeholders() {
        let s = Strings::load();
        assert_eq!(s.tr(Lang::En, "tab_nearby", &[]), "Nearby");
        assert_eq!(s.tr(Lang::It, "tab_nearby", &[]), "Dispositivi vicini");
        assert_eq!(s.tr(Lang::It, "tray_your_code", &["849-201"]), "Il tuo codice: 849-201");
        assert_eq!(s.tr(Lang::En, "no_such_key", &[]), "no_such_key");
        assert_eq!(capitalize("l'altro"), "L'altro");
    }
}
