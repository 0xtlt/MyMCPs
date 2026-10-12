//! Environment variable names an operator may not set on an npm MCP.
//!
//! The variables of an npm MCP are handed to the `node` process, and to the
//! `npm` that installs its package, not only to the package. These names are
//! read by the dynamic loader, libc or Node itself before the package runs,
//! or are what MyMCPs sets for it. The sandbox holds whatever a variable
//! makes the process do; refusing these keeps a package starting the way
//! MyMCPs started it. Ordinary application settings (API keys, `*_URL`,
//! `NODE_ENV`, proxies, `NPM_CONFIG_REGISTRY`) stay allowed.

struct ReservedNames {
    names: &'static [&'static str],
    prefixes: &'static [&'static str],
    reason: fn(&str) -> String,
}

const RESERVED: &[ReservedNames] = &[
    ReservedNames {
        // Set by the gateway for every npm MCP. PATHEXT is the Windows half of PATH.
        names: &["HOME", "TMPDIR", "NO_COLOR", "PATH", "PATHEXT"],
        prefixes: &[],
        reason: |name| format!("\"{name}\" is set by MyMCPs for the sandbox and cannot be changed"),
    },
    ReservedNames {
        prefixes: &[],
        // Flags of Node itself and where it looks modules up, code it loads
        // before the package (an OpenSSL configuration names libraries), and
        // files it writes on its own: the keys of every TLS session, coverage
        // reports, compiled code it would run the next time.
        names: &[
            "NODE_OPTIONS",
            "NODE_PATH",
            "NODE_COMPILE_CACHE",
            "NODE_V8_COVERAGE",
            "NODE_REPL_EXTERNAL_MODULE",
            "OPENSSL_CONF",
            "OPENSSL_MODULES",
            "SSLKEYLOGFILE",
        ],
        reason: |name| {
            format!(
                "\"{name}\" configures the runtime that runs the package, not the package, and cannot be set"
            )
        },
    },
    ReservedNames {
        // ld.so (LD_PRELOAD, LD_LIBRARY_PATH, LD_AUDIT...) and dyld
        // (DYLD_INSERT_LIBRARIES...) load code into the process. MALLOC covers the
        // glibc `MALLOC_*` tunables and the macOS `Malloc*` switches, which can
        // also write log files.
        prefixes: &["LD_", "DYLD_", "MALLOC"],
        // What glibc itself strips from privileged programs (unsecvars.h), plus
        // the musl counterpart of LOCPATH: they redirect files libc loads or the
        // resolver it uses.
        names: &[
            "GCONV_PATH",
            "GETCONF_DIR",
            "GLIBC_TUNABLES",
            "HOSTALIASES",
            "LOCALDOMAIN",
            "LOCPATH",
            "MUSL_LOCPATH",
            "NIS_PATH",
            "NLSPATH",
            "RESOLV_HOST_CONF",
            "RES_OPTIONS",
            "TZDIR",
        ],
        reason: |name| {
            format!("\"{name}\" changes how the sandbox process itself is loaded and cannot be set")
        },
    },
];

/// Why a name is refused, or `None` when the package may receive it. Names are
/// compared ignoring case because Windows does, and nothing legitimate depends
/// on a lower-case spelling of these.
pub fn reserved_environment_name_reason(name: &str) -> Option<String> {
    let upper = name.to_uppercase();
    RESERVED
        .iter()
        .find(|group| {
            group.names.contains(&upper.as_str())
                || group
                    .prefixes
                    .iter()
                    .any(|prefix| upper.starts_with(prefix))
        })
        .map(|group| (group.reason)(name))
}

pub fn is_reserved_environment_name(name: &str) -> bool {
    reserved_environment_name_reason(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The validator half of these two cases (field `npmEnv.0.name`, rule
    // `npmEnvironmentName`) belongs to the crate that ports `validators/mcp.ts`.

    #[test]
    fn refuses_names_that_steer_the_binary_lookup_the_loader_or_the_node_runtime() {
        let refused = [
            "PATH",
            "Path",
            "PATHEXT",
            "HOME",
            "TMPDIR",
            "NO_COLOR",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "ld_audit",
            "DYLD_INSERT_LIBRARIES",
            "DYLD_LIBRARY_PATH",
            "NODE_OPTIONS",
            "node_options",
            "NODE_PATH",
            "NODE_COMPILE_CACHE",
            "NODE_V8_COVERAGE",
            "OPENSSL_CONF",
            "SSLKEYLOGFILE",
            "sslkeylogfile",
            "GLIBC_TUNABLES",
            "GCONV_PATH",
            "HOSTALIASES",
            "LOCPATH",
            "MALLOC_CHECK_",
            "MallocLogFile",
            "RES_OPTIONS",
        ];
        for name in refused {
            let reason = reserved_environment_name_reason(name)
                .unwrap_or_else(|| panic!("{name} was accepted"));
            assert!(reason.contains(&format!("\"{name}\"")), "{reason}");
            assert!(reason.contains("cannot be"), "{reason}");
            assert!(is_reserved_environment_name(name));
        }
    }

    #[test]
    fn keeps_ordinary_application_variables_usable() {
        let allowed = [
            "API_KEY",
            "DATABASE_URL",
            "NPM_CONFIG_REGISTRY",
            "NODE_ENV",
            "NODE_EXTRA_CA_CERTS",
            "HTTPS_PROXY",
            "NO_PROXY",
            "TZ",
            "LANG",
            "XDG_CONFIG_HOME",
            // Near misses of the reserved names and prefixes.
            "PATH_PREFIX",
            "HOMEPAGE_URL",
            "LDAP_URL",
            "NODE_OPTIONS_EXTRA",
            // Reserved while Deno ran the packages.
            "DENO_DIR",
            "DYLDX",
        ];
        for name in allowed {
            assert_eq!(reserved_environment_name_reason(name), None, "{name}");
            assert!(!is_reserved_environment_name(name));
        }
    }

    #[test]
    fn words_each_reason_as_the_form_shows_it() {
        assert_eq!(
            reserved_environment_name_reason("Home").as_deref(),
            Some("\"Home\" is set by MyMCPs for the sandbox and cannot be changed")
        );
        assert_eq!(
            reserved_environment_name_reason("NODE_PATH").as_deref(),
            Some(
                "\"NODE_PATH\" configures the runtime that runs the package, not the package, and cannot be set"
            )
        );
        assert_eq!(
            reserved_environment_name_reason("TZDIR").as_deref(),
            Some("\"TZDIR\" changes how the sandbox process itself is loaded and cannot be set")
        );
    }
}
