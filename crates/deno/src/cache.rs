//! What the Deno npm cache holds for a package, read from the files Deno
//! keeps under `$DENO_DIR/npm/registry.npmjs.org`.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde_json::Value;

use crate::text::js_trim;

pub(crate) fn is_safe_path_segment(value: &str) -> bool {
    !value.is_empty() && !value.contains("..") && !value.contains('/') && !value.contains('\\')
}

fn npm_cache_package_dir(deno_dir: &Path, npm_package: &str) -> Option<PathBuf> {
    let package = js_trim(npm_package);
    if package.is_empty()
        || package.contains("..")
        || package.contains('\\')
        || package.starts_with('/')
    {
        return None;
    }
    let segments: Vec<&str> = package
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.is_empty()
        || segments.len() > 2
        || !segments.iter().all(|segment| is_safe_path_segment(segment))
    {
        return None;
    }
    let mut directory = deno_dir.join("npm").join("registry.npmjs.org");
    directory.extend(segments);
    Some(directory)
}

pub(crate) fn is_latest_requested_version(npm_version: Option<&str>) -> bool {
    let version = js_trim(npm_version.unwrap_or_default());
    version.is_empty() || version.eq_ignore_ascii_case("latest")
}

/// The `dist-tags` of a registry document. The rest of the document, which
/// lists every version ever published, is skipped without being kept.
struct DistTags(Option<Value>);

impl<'de> Deserialize<'de> for DistTags {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Document;

        impl<'de> Visitor<'de> for Document {
            type Value = DistTags;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a registry document")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DistTags, A::Error> {
                let mut dist_tags = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "dist-tags" {
                        // As in JavaScript, the last of repeated keys counts.
                        dist_tags = Some(map.next_value::<Value>()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(DistTags(dist_tags))
            }
        }

        deserializer.deserialize_map(Document)
    }
}

fn read_cached_latest_tag(package_dir: &Path) -> Option<String> {
    let content = std::fs::read(package_dir.join("registry.json")).ok()?;
    let DistTags(dist_tags) = serde_json::from_str(&String::from_utf8_lossy(&content)).ok()?;
    let latest = js_trim(dist_tags?.as_object()?.get("latest")?.as_str()?).to_owned();
    is_safe_path_segment(&latest).then_some(latest)
}

/// Semver currently present in the Deno npm cache for this package.
/// For `latest`, uses the cached `dist-tags.latest` when that version folder exists.
/// Pinned versions are returned only when that exact folder is cached.
pub(crate) fn read_cached_npm_package_version(
    deno_dir: &Path,
    npm_package: &str,
    npm_version: Option<&str>,
) -> Option<String> {
    let package_dir = npm_cache_package_dir(deno_dir, npm_package)?;
    if !package_dir.exists() {
        return None;
    }

    let requested = if is_latest_requested_version(npm_version) {
        read_cached_latest_tag(&package_dir)?
    } else {
        js_trim(npm_version.unwrap_or_default()).to_owned()
    };
    if !is_safe_path_segment(&requested) {
        return None;
    }
    package_dir.join(&requested).exists().then_some(requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_package_names_inside_the_registry_directory() {
        let deno_dir = Path::new("/cache/deno");
        let directory = |package: &str| npm_cache_package_dir(deno_dir, package);

        assert_eq!(
            directory("@shopify/dev-mcp"),
            Some(PathBuf::from(
                "/cache/deno/npm/registry.npmjs.org/@shopify/dev-mcp"
            ))
        );
        assert_eq!(
            directory("  mongodb-mcp-server "),
            Some(PathBuf::from(
                "/cache/deno/npm/registry.npmjs.org/mongodb-mcp-server"
            ))
        );
        // Empty segments are dropped, as `split('/').filter(Boolean)` drops them.
        assert_eq!(
            directory("@scope//name/"),
            Some(PathBuf::from(
                "/cache/deno/npm/registry.npmjs.org/@scope/name"
            ))
        );
        for refused in [
            "",
            "   ",
            "..",
            "../outside",
            "@scope/../../outside",
            "a..b",
            "/etc/passwd",
            "a\\b",
            "one/two/three",
            "/",
        ] {
            assert_eq!(directory(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn checks_path_segments() {
        for safe in ["1.14.4", "latest", "2.0.0-beta.1", "."] {
            assert!(is_safe_path_segment(safe), "{safe:?}");
        }
        for unsafe_segment in ["", "..", "1..2", "a/b", "a\\b"] {
            assert!(!is_safe_path_segment(unsafe_segment), "{unsafe_segment:?}");
        }
    }

    #[test]
    fn tells_a_tracked_latest_from_a_pinned_version() {
        for latest in [
            None,
            Some(""),
            Some("  "),
            Some("latest"),
            Some(" LATEST "),
            Some("Latest"),
        ] {
            assert!(is_latest_requested_version(latest), "{latest:?}");
        }
        for pinned in [Some("1.0.0"), Some("next"), Some("latest-1")] {
            assert!(!is_latest_requested_version(pinned), "{pinned:?}");
        }
    }

    #[test]
    fn reads_only_a_usable_latest_tag_from_a_registry_document() {
        let directory = tempfile::tempdir().unwrap();
        let latest_of = |document: &str| {
            std::fs::write(directory.path().join("registry.json"), document).unwrap();
            read_cached_latest_tag(directory.path())
        };

        assert_eq!(
            latest_of(
                r#"{"name":"x","versions":{"1.0.0":{"dist":{}}},"dist-tags":{"latest":" 1.2.3 "}}"#
            ),
            Some("1.2.3".to_owned())
        );
        assert_eq!(
            latest_of(r#"{"dist-tags":{"latest":"1.0.0"},"dist-tags":{"latest":"2.0.0"}}"#),
            Some("2.0.0".to_owned())
        );
        for unusable in [
            "{not json",
            "null",
            "[]",
            "\"text\"",
            "{}",
            r#"{"dist-tags":null}"#,
            r#"{"dist-tags":"1.0.0"}"#,
            r#"{"dist-tags":["1.0.0"]}"#,
            r#"{"dist-tags":{}}"#,
            r#"{"dist-tags":{"latest":7}}"#,
            r#"{"dist-tags":{"latest":"  "}}"#,
            r#"{"dist-tags":{"latest":"../../outside"}}"#,
            r#"{"dist-tags":{"latest":"a/b"}}"#,
        ] {
            assert_eq!(latest_of(unusable), None, "{unusable}");
        }

        std::fs::remove_file(directory.path().join("registry.json")).unwrap();
        assert_eq!(read_cached_latest_tag(directory.path()), None);
    }
}
