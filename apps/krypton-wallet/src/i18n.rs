//! The wallet's languages. Catalogs (locales/build/<lang>.json, made by
//! `python website/i18n_wallet.py build`) are compiled into the signed binary: a language needs
//! no network, and nobody can swap a catalog on this computer. The page is translated in the
//! browser (design/assets/i18n.js); English is the source and the fallback.

macro_rules! lang {
    ($code:literal) => {
        (
            $code,
            include_str!(concat!("../locales/build/", $code, ".json")),
        )
    };
}

const CATALOGS: &[(&str, &str)] = &[
    lang!("de"),
    lang!("fr"),
    lang!("es"),
    lang!("pt"),
    lang!("ru"),
    lang!("tr"),
    lang!("zh-cn"),
    lang!("ja"),
    lang!("ko"),
];

/// Whether `code` is a language of the wallet ("en" included).
pub fn known(code: &str) -> bool {
    code == "en" || CATALOGS.iter().any(|(c, _)| *c == code)
}

/// The catalog of a language as JSON, None for English (nothing to translate).
pub fn catalog(code: &str) -> Option<&'static str> {
    CATALOGS.iter().find(|(c, _)| *c == code).map(|(_, j)| *j)
}

/// The page in a language: its catalog before the shared head scripts (i18n.js reads it at
/// once) and the language on the html element. `page` has `<!--I18N-->` where the catalog goes.
pub fn page(page: &str, code: &str) -> String {
    let Some(json) = catalog(code) else {
        return page.replace("<!--I18N-->", "");
    };
    let tag = match code {
        "zh-cn" => "zh-CN",
        c => c,
    };
    // A catalog never holds "</script", but it must not be able to end the element either.
    let data = format!(
        r#"<script type="application/json" id="gk-i18n-data">{}</script>"#,
        json.trim().replace("</", "<\\/")
    );
    page.replacen(r#"<html lang="en""#, &format!(r#"<html lang="{tag}""#), 1)
        .replace("<!--I18N-->", &data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_is_json_for_its_language() {
        for (code, json) in CATALOGS {
            let v: serde_json::Value = serde_json::from_str(json).expect(code);
            assert!(
                v["dom"].is_object() && v["js"].is_object() && v["rust"].is_array(),
                "{code}"
            );
        }
    }

    #[test]
    fn the_page_carries_its_language_and_cannot_be_broken_out_of() {
        let base = r#"<html lang="en" data-theme="dark"><head><!--I18N--><script src="/assets/i18n.js"></script>"#;
        let de = page(base, "de");
        assert!(de.starts_with(r#"<html lang="de""#));
        assert!(de.contains(r#"id="gk-i18n-data""#));
        let body = &de[de.find("gk-i18n-data").unwrap()..];
        let end = body.find("</script>").unwrap();
        assert!(
            !body[..end].contains("</"),
            "only the element's own end tag"
        );
        assert_eq!(page(base, "en"), base.replace("<!--I18N-->", ""));
        assert!(page(base, "zh-cn").starts_with(r#"<html lang="zh-CN""#));
        assert!(known("en") && known("ja") && !known("xx") && !known("../de"));
    }
}
