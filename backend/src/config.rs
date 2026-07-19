use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Deployment environment. Only two values exist on purpose: anything that is
/// not explicitly production behaves like development (fail closed on
/// production-only conveniences, never the other way around).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Production,
}

impl Environment {
    pub fn is_production(self) -> bool {
        self == Environment::Production
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub environment: Environment,
    /// Never log this value; it may embed credentials.
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub db_max_connections: u32,
    pub db_min_connections: u32,
    pub db_acquire_timeout_secs: u64,
    pub document_storage_path: PathBuf,
    pub worker_id: String,
    pub shutdown_timeout_secs: u64,
    /// How often the background readiness prober re-checks the database.
    pub readiness_interval_secs: u64,
    /// Argon2id parameters. Defaults follow the OWASP Password Storage Cheat
    /// Sheet's first recommended configuration: 19 MiB memory, 2 iterations,
    /// 1 lane. Raise memory/iterations as hardware allows; existing hashes
    /// keep their original parameters until the password is next changed.
    pub argon2_memory_kib: u32,
    pub argon2_time_cost: u32,
    pub argon2_parallelism: u32,
    /// Idle session deadline, refreshed on activity (seconds).
    pub session_idle_secs: u64,
    /// Absolute session deadline fixed at login (seconds); an active session
    /// still ends when it is reached.
    pub session_absolute_secs: u64,
    /// Failed logins allowed per account+IP inside one throttle window.
    pub login_max_failures: u32,
    /// Length of the fixed throttle window (seconds). The counter resets
    /// when the window expires — throttling can never become permanent.
    pub login_throttle_window_secs: u64,
    /// A document_job left 'running' longer than this is presumed orphaned
    /// by a dead worker and is reaped back to the queue (CLAUDE.md §1
    /// item 3). Must comfortably exceed the longest legitimate render.
    pub job_stale_secs: u64,
    /// Ed25519 public key (64 hex characters) for verifying signed license
    /// imports on self-hosted deployments. Unset = imports refused. The
    /// matching PRIVATE key must never be configured or stored on a
    /// deployment (docs/SECURITY.md).
    pub license_public_key: Option<[u8; 32]>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing(&'static str),
    Invalid { key: &'static str, reason: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never echo the offending value: DATABASE_URL and friends may hold
        // secrets, and startup errors end up in logs.
        match self {
            ConfigError::Missing(key) => {
                write!(f, "required environment variable {key} is not set")
            }
            ConfigError::Invalid { key, reason } => {
                write!(f, "environment variable {key} is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl AppConfig {
    /// Load from process environment. `.env` is loaded by main() beforehand,
    /// for development only; production sets real environment variables.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_source(|key| std::env::var(key).ok())
    }

    /// Parse from an arbitrary source so tests never mutate process-global env.
    pub fn from_source(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let environment = match get("APP_ENV").as_deref() {
            Some("production") => Environment::Production,
            Some("development") | None => Environment::Development,
            Some(other) => {
                return Err(ConfigError::Invalid {
                    key: "APP_ENV",
                    reason: format!("expected \"development\" or \"production\", got \"{other}\""),
                });
            }
        };

        let database_url = get("DATABASE_URL").ok_or(ConfigError::Missing("DATABASE_URL"))?;

        let bind_addr = parse_or_default(&get, "APP_BIND_ADDR", "0.0.0.0:8080", |raw| {
            raw.parse::<SocketAddr>().map_err(|e| e.to_string())
        })?;

        let db_max_connections =
            parse_or_default(&get, "APP_DB_MAX_CONNECTIONS", "64", parse_positive_u32)?;
        let db_min_connections =
            parse_or_default(&get, "APP_DB_MIN_CONNECTIONS", "8", parse_positive_u32)?;

        if db_min_connections > db_max_connections {
            return Err(ConfigError::Invalid {
                key: "APP_DB_MIN_CONNECTIONS",
                reason: format!(
                    "must not exceed APP_DB_MAX_CONNECTIONS \
                     ({db_min_connections} > {db_max_connections})"
                ),
            });
        }

        let db_acquire_timeout_secs =
            parse_or_default(&get, "APP_DB_ACQUIRE_TIMEOUT_SECS", "5", parse_positive_u64)?;
        let shutdown_timeout_secs =
            parse_or_default(&get, "APP_SHUTDOWN_TIMEOUT_SECS", "30", parse_positive_u64)?;
        let readiness_interval_secs =
            parse_or_default(&get, "APP_READINESS_INTERVAL_SECS", "5", parse_positive_u64)?;

        let document_storage_path = PathBuf::from(
            get("APP_DOCUMENT_STORAGE_PATH").unwrap_or_else(|| "./var/documents".to_owned()),
        );

        let worker_id = get("APP_WORKER_ID").unwrap_or_else(|| "document-worker-1".to_owned());

        let argon2_memory_kib =
            parse_or_default(&get, "APP_ARGON2_MEMORY_KIB", "19456", parse_positive_u32)?;
        let argon2_time_cost =
            parse_or_default(&get, "APP_ARGON2_TIME_COST", "2", parse_positive_u32)?;
        let argon2_parallelism =
            parse_or_default(&get, "APP_ARGON2_PARALLELISM", "1", parse_positive_u32)?;

        let session_idle_secs =
            parse_or_default(&get, "APP_SESSION_IDLE_SECS", "1800", parse_positive_u64)?;
        let session_absolute_secs = parse_or_default(
            &get,
            "APP_SESSION_ABSOLUTE_SECS",
            "43200",
            parse_positive_u64,
        )?;

        let login_max_failures =
            parse_or_default(&get, "APP_LOGIN_MAX_FAILURES", "10", parse_positive_u32)?;
        let login_throttle_window_secs = parse_or_default(
            &get,
            "APP_LOGIN_THROTTLE_WINDOW_SECS",
            "900",
            parse_positive_u64,
        )?;
        let job_stale_secs =
            parse_or_default(&get, "APP_JOB_STALE_SECS", "300", parse_positive_u64)?;

        let license_public_key = match get("APP_LICENSE_PUBLIC_KEY") {
            None => None,
            Some(raw) => {
                // Never echo the value; a typo'd secret pasted into the
                // wrong variable must not end up in logs.
                let decoded = hex::decode(raw.trim()).map_err(|_| ConfigError::Invalid {
                    key: "APP_LICENSE_PUBLIC_KEY",
                    reason: "must be hex".to_owned(),
                })?;
                let bytes: [u8; 32] = decoded.try_into().map_err(|_| ConfigError::Invalid {
                    key: "APP_LICENSE_PUBLIC_KEY",
                    reason: "must be exactly 32 bytes (64 hex characters)".to_owned(),
                })?;
                Some(bytes)
            }
        };

        if session_idle_secs > session_absolute_secs {
            return Err(ConfigError::Invalid {
                key: "APP_SESSION_IDLE_SECS",
                reason: format!(
                    "must not exceed APP_SESSION_ABSOLUTE_SECS \
                     ({session_idle_secs} > {session_absolute_secs})"
                ),
            });
        }

        Ok(Self {
            environment,
            database_url,
            bind_addr,
            db_max_connections,
            db_min_connections,
            db_acquire_timeout_secs,
            document_storage_path,
            worker_id,
            shutdown_timeout_secs,
            readiness_interval_secs,
            argon2_memory_kib,
            argon2_time_cost,
            argon2_parallelism,
            session_idle_secs,
            session_absolute_secs,
            login_max_failures,
            login_throttle_window_secs,
            job_stale_secs,
            license_public_key,
        })
    }
}

fn parse_or_default<T>(
    get: impl Fn(&str) -> Option<String>,
    key: &'static str,
    default: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<T, ConfigError> {
    let raw = get(key).unwrap_or_else(|| default.to_owned());
    parse(&raw).map_err(|reason| ConfigError::Invalid { key, reason })
}

fn parse_positive_u32(raw: &str) -> Result<u32, String> {
    match raw.parse::<u32>() {
        Ok(0) => Err("must be greater than zero".to_owned()),
        Ok(value) => Ok(value),
        Err(e) => Err(e.to_string()),
    }
}

fn parse_positive_u64(raw: &str) -> Result<u64, String> {
    match raw.parse::<u64>() {
        Ok(0) => Err("must be greater than zero".to_owned()),
        Ok(value) => Ok(value),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn source(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn defaults_apply_when_only_database_url_is_set() {
        let config =
            AppConfig::from_source(source(&[("DATABASE_URL", "postgresql://h/db")])).unwrap();

        assert_eq!(config.environment, Environment::Development);
        assert_eq!(config.bind_addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(config.db_max_connections, 64);
        assert_eq!(config.db_min_connections, 8);
        assert_eq!(config.db_acquire_timeout_secs, 5);
        assert_eq!(config.shutdown_timeout_secs, 30);
        assert_eq!(config.readiness_interval_secs, 5);
        assert_eq!(
            config.document_storage_path,
            PathBuf::from("./var/documents")
        );
        assert_eq!(config.worker_id, "document-worker-1");
        assert_eq!(config.argon2_memory_kib, 19456);
        assert_eq!(config.argon2_time_cost, 2);
        assert_eq!(config.argon2_parallelism, 1);
        assert_eq!(config.session_idle_secs, 1800);
        assert_eq!(config.session_absolute_secs, 43200);
        assert_eq!(config.login_max_failures, 10);
        assert_eq!(config.login_throttle_window_secs, 900);
        assert_eq!(config.license_public_key, None);
    }

    #[test]
    fn license_public_key_parses_and_rejects_bad_input() {
        let config = AppConfig::from_source(source(&[
            ("DATABASE_URL", "postgresql://h/db"),
            ("APP_LICENSE_PUBLIC_KEY", &"ab".repeat(32)),
        ]))
        .unwrap();
        assert_eq!(config.license_public_key, Some([0xab; 32]));

        for bad in ["not-hex", "abcd", &"ab".repeat(33)] {
            let err = AppConfig::from_source(source(&[
                ("DATABASE_URL", "postgresql://h/db"),
                ("APP_LICENSE_PUBLIC_KEY", bad),
            ]))
            .unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        key: "APP_LICENSE_PUBLIC_KEY",
                        ..
                    }
                ),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn idle_session_deadline_may_not_exceed_absolute() {
        let err = AppConfig::from_source(source(&[
            ("DATABASE_URL", "postgresql://h/db"),
            ("APP_SESSION_IDLE_SECS", "600"),
            ("APP_SESSION_ABSOLUTE_SECS", "300"),
        ]))
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                key: "APP_SESSION_IDLE_SECS",
                ..
            }
        ));
    }

    #[test]
    fn zero_argon2_cost_is_rejected() {
        let err = AppConfig::from_source(source(&[
            ("DATABASE_URL", "postgresql://h/db"),
            ("APP_ARGON2_TIME_COST", "0"),
        ]))
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                key: "APP_ARGON2_TIME_COST",
                ..
            }
        ));
    }

    #[test]
    fn missing_database_url_is_an_error() {
        let err = AppConfig::from_source(source(&[])).unwrap_err();
        assert_eq!(err, ConfigError::Missing("DATABASE_URL"));
    }

    #[test]
    fn production_environment_parses() {
        let config = AppConfig::from_source(source(&[
            ("DATABASE_URL", "postgresql://h/db"),
            ("APP_ENV", "production"),
        ]))
        .unwrap();
        assert!(config.environment.is_production());
    }

    #[test]
    fn unknown_environment_is_rejected() {
        let err = AppConfig::from_source(source(&[
            ("DATABASE_URL", "postgresql://h/db"),
            ("APP_ENV", "staging"),
        ]))
        .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { key: "APP_ENV", .. }));
    }

    #[test]
    fn invalid_bind_addr_is_rejected() {
        let err = AppConfig::from_source(source(&[
            ("DATABASE_URL", "postgresql://h/db"),
            ("APP_BIND_ADDR", "not-an-addr"),
        ]))
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                key: "APP_BIND_ADDR",
                ..
            }
        ));
    }

    #[test]
    fn zero_pool_size_is_rejected() {
        let err = AppConfig::from_source(source(&[
            ("DATABASE_URL", "postgresql://h/db"),
            ("APP_DB_MAX_CONNECTIONS", "0"),
        ]))
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                key: "APP_DB_MAX_CONNECTIONS",
                ..
            }
        ));
    }

    #[test]
    fn min_connections_may_not_exceed_max() {
        let err = AppConfig::from_source(source(&[
            ("DATABASE_URL", "postgresql://h/db"),
            ("APP_DB_MAX_CONNECTIONS", "4"),
            ("APP_DB_MIN_CONNECTIONS", "8"),
        ]))
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                key: "APP_DB_MIN_CONNECTIONS",
                ..
            }
        ));
    }

    #[test]
    fn config_error_display_never_echoes_values() {
        // DATABASE_URL may contain credentials; error text must not repeat
        // the value that failed to parse.
        let err = AppConfig::from_source(source(&[
            ("DATABASE_URL", "postgresql://user:s3cret@h/db"),
            ("APP_BIND_ADDR", "s3cret-value"),
        ]))
        .unwrap_err();
        let text = err.to_string();
        assert!(
            !text.contains("s3cret"),
            "error text leaked a value: {text}"
        );
    }
}
