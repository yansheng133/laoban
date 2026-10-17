//! The language of text people read: error messages, round log events,
//! round failure messages, commit messages and the generated README.
//!
//! Set once by the environment variable `LAOBAN_LANGUAGE` (chart
//! `config.language`): `en` (the default, also for an unset or unknown
//! value) or `zh-Hant`. Codes, log codes and JSON shapes do not depend on
//! it. Each call site keeps the English and the Chinese side by side:
//! [`tr`] for fixed text, [`trf!`](crate::trf) for formatted text.

use std::sync::OnceLock;

/// The environment variable that chooses the language.
pub const ENV: &str = "LAOBAN_LANGUAGE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    ZhHant,
}

impl Lang {
    /// `zh-Hant` (case does not matter) is Traditional Chinese; anything
    /// else, or nothing, is English.
    pub fn parse(value: Option<&str>) -> Lang {
        match value.map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("zh-Hant") => Lang::ZhHant,
            _ => Lang::En,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::ZhHant => "zh-Hant",
        }
    }
}

/// The language for this process, read from [`ENV`] on first use.
pub fn current() -> Lang {
    static LANG: OnceLock<Lang> = OnceLock::new();
    *LANG.get_or_init(|| Lang::parse(std::env::var(ENV).ok().as_deref()))
}

/// `en` or `zh`, whichever matches `lang`.
pub fn pick<T>(lang: Lang, en: T, zh: T) -> T {
    match lang {
        Lang::En => en,
        Lang::ZhHant => zh,
    }
}

/// Fixed text in the current language.
pub fn tr(en: &'static str, zh: &'static str) -> &'static str {
    pick(current(), en, zh)
}

/// Formatted text in the current language:
/// `trf!(en: "Gave {count} opinions", zh: "提出 {count} 則意見")`.
/// Both strings take the same arguments, inline (`{count}`) or after them
/// (positional or `name = value`).
#[macro_export]
macro_rules! trf {
    (en: $en:literal, zh: $zh:literal $($args:tt)*) => {
        match $crate::lang::current() {
            $crate::lang::Lang::En => format!($en $($args)*),
            $crate::lang::Lang::ZhHant => format!($zh $($args)*),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_defaults_to_english() {
        assert_eq!(Lang::parse(None), Lang::En);
        assert_eq!(Lang::parse(Some("")), Lang::En);
        assert_eq!(Lang::parse(Some("en")), Lang::En);
        assert_eq!(Lang::parse(Some("fr")), Lang::En);
        assert_eq!(Lang::parse(Some("zh-Hant")), Lang::ZhHant);
        assert_eq!(Lang::parse(Some(" zh-hant ")), Lang::ZhHant);
    }

    #[test]
    fn pick_returns_chinese_for_zh_hant() {
        assert_eq!(pick(Lang::ZhHant, "Done.", "已完成。"), "已完成。");
        assert_eq!(pick(Lang::En, "Done.", "已完成。"), "Done.");
    }

    #[test]
    fn tr_and_trf_follow_the_process_language() {
        // Tests run without LAOBAN_LANGUAGE, so English.
        assert_eq!(current(), Lang::En);
        assert_eq!(tr("Done.", "已完成。"), "Done.");
        let n = 3;
        assert_eq!(trf!(en: "{n} left.", zh: "剩 {n} 個。"), "3 left.");
        assert_eq!(trf!(en: "{} left.", zh: "剩 {} 個。", n + 1), "4 left.");
    }
}
