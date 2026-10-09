//! Icons, inlined in the page so they take the colour of their text.

use std::collections::HashMap;
use std::sync::LazyLock;

use maud::{Markup, PreEscaped, html};

use crate::assets::{asset_names, asset_text};

/// What is between `<svg ...>` and `</svg>` in each icon file, by icon name.
/// Brand marks are under `brands/<name>`.
static ICONS: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    asset_names()
        .filter_map(|path| {
            let name = path
                .strip_prefix("icons/")?
                .strip_suffix(".svg")?
                .to_string();
            let svg = asset_text(&path)?;
            let start = svg.find('>')? + 1;
            let end = svg.rfind("</svg>")?;
            Some((name, svg[start..end].trim().to_string()))
        })
        .collect()
});

/// The icon `icons/<name>.svg` at 16px. A name with no file draws nothing,
/// and says so in the log rather than breaking the page.
pub fn icon(name: &str) -> Markup {
    icon_with(name, "")
}

/// An icon with modifier classes, such as `icon--lg` or `icon--critical`.
pub fn icon_with(name: &str, modifiers: &str) -> Markup {
    let Some(paths) = ICONS.get(name) else {
        tracing::warn!(icon = name, "Unknown icon");
        return html! {};
    };
    let class = if modifiers.is_empty() {
        "icon".to_string()
    } else {
        format!("icon {modifiers}")
    };
    html! {
        svg class=(class) viewBox="0 0 24 24" aria-hidden="true" { (PreEscaped(paths)) }
    }
}

/// Whether `icons/<name>.svg` exists.
pub fn has_icon(name: &str) -> bool {
    ICONS.contains_key(name)
}

const LOGO_TILE: &str = "M0 8C0 3.58172 3.58172 0 8 0H20C24.4183 0 28 3.58172 28 8V20C28 24.4183 24.4183 28 20 28H8C3.58172 28 0 24.4183 0 20V8Z";

/// The second path of `brand/logo-mark.svg`: the letter on the tile.
static LOGO_LETTER: LazyLock<String> = LazyLock::new(|| {
    asset_text("brand/logo-mark.svg")
        .and_then(|svg| {
            let second = svg.match_indices("<path d=\"").nth(1)?.0 + "<path d=\"".len();
            let end = svg[second..].find('"')? + second;
            Some(svg[second..end].to_string())
        })
        .unwrap_or_default()
});

fn logo_mark() -> Markup {
    html! {
        svg class="logo__mark" viewBox="0 0 28 28" aria-hidden="true" {
            path class="logo__tile" d=(LOGO_TILE) {}
            path class="logo__letter" d=(LOGO_LETTER.as_str()) {}
        }
    }
}

/// The logo with its wordmark, on the dark frame, linking home.
pub fn logo_on_frame() -> Markup {
    html! {
        a class="logo logo--on-frame" href="/" { (logo_mark()) span class="logo__wordmark" { "MyMCPs" } }
    }
}

/// The logo with its wordmark on a light surface.
pub fn logo() -> Markup {
    html! {
        span class="logo" { (logo_mark()) span class="logo__wordmark" { "MyMCPs" } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inlines_the_paths_of_an_icon() {
        let house = icon("house").into_string();
        assert!(house.starts_with(
            "<svg class=\"icon\" viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M15 21v-8"
        ));
        assert!(!house.contains("xmlns"));
        assert!(
            icon_with("check", "icon--lg icon--success")
                .into_string()
                .starts_with("<svg class=\"icon icon--lg icon--success\"")
        );
        assert_eq!(icon("no-such-icon").into_string(), "");
        assert!(has_icon("plug") && !has_icon("../css/app"));
    }

    #[test]
    fn draws_the_logo_from_the_brand_file() {
        let logo = logo_on_frame().into_string();
        assert!(logo.contains("class=\"logo__letter\" d=\"M7 8.36501"));
        assert!(logo.contains("MyMCPs"));
    }
}
