//! Authentication use cases (framework-free; HTTP adapters live in http.rs).

use sqlx::PgPool;
use uuid::Uuid;

use crate::identity_access::password::PasswordService;
use crate::identity_access::sessions::{NewSession, SessionService};
use crate::shared::error::AppError;

#[derive(Debug)] // NewSession's Debug redacts the raw token
pub struct LoginOutcome {
    pub user_id: Uuid,
    pub session: NewSession,
}

#[derive(Clone)]
pub struct AuthService {
    pool: PgPool,
    passwords: PasswordService,
    sessions: SessionService,
    /// Verified against when the username or credential does not exist, so a
    /// login probe cannot distinguish "no such user" from "wrong password"
    /// by response time.
    dummy_hash: String,
    max_failures: i64,
    window_secs: f64,
}

impl AuthService {
    /// Hashes a throwaway dummy credential at construction (startup-time
    /// Argon2 cost, once).
    pub fn new(
        pool: PgPool,
        passwords: PasswordService,
        sessions: SessionService,
        max_failures: u32,
        window_secs: u64,
    ) -> Result<Self, AppError> {
        let dummy_hash = passwords
            .hash("dummy-timing-equalizer")
            .map_err(|_| AppError::Internal)?;
        Ok(Self {
            pool,
            passwords,
            sessions,
            dummy_hash,
            max_failures: i64::from(max_failures),
            window_secs: window_secs as f64,
        })
    }

    /// Verify credentials and open a session.
    ///
    /// Every failure — unknown username, wrong password, unusable stored
    /// hash, suspended/closed account — answers with the same generic
    /// `Unauthenticated` and counts against the account+IP throttle window.
    /// Argon2 verification always runs (against a dummy hash if needed) and
    /// runs on the blocking pool, never an async worker thread.
    pub async fn login(
        &self,
        institution_id: Uuid,
        username: &str,
        password: &str,
        client_ip: &str,
    ) -> Result<LoginOutcome, AppError> {
        let username_lower = username.to_lowercase();

        if self
            .throttled(institution_id, &username_lower, client_ip)
            .await?
        {
            return Err(AppError::RateLimited);
        }

        let user = sqlx::query_as::<_, CandidateRow>(
            r#"
            SELECT u.id, u.status::text AS status, u.session_version,
                   c.password_hash
              FROM user_account u
              LEFT JOIN password_credential c ON c.user_id = u.id
             WHERE u.institution_id = $1 AND u.username = $2
            "#,
        )
        .bind(institution_id)
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;

        let (stored_hash, credential_exists) =
            match user.as_ref().and_then(|u| u.password_hash.clone()) {
                Some(hash) => (hash, true),
                None => (self.dummy_hash.clone(), false),
            };

        let passwords = self.passwords.clone();
        let candidate = password.to_owned();
        let verified =
            tokio::task::spawn_blocking(move || passwords.verify(&stored_hash, &candidate))
                .await
                .map_err(|_| AppError::Internal)?;

        let password_ok = match verified {
            Ok(ok) => ok && credential_exists,
            Err(error) => {
                // The stored value is unusable — a server-side fault worth an
                // alert, but the client still gets the generic answer.
                tracing::error!(error = %error, "stored password credential is unusable");
                false
            }
        };
        let account_active = user.as_ref().is_some_and(|u| u.status == "active");

        if !(password_ok && account_active) {
            self.record_failure(institution_id, &username_lower, client_ip)
                .await?;
            return Err(AppError::Unauthenticated);
        }

        let user = user.expect("user exists when password verified");
        self.clear_throttle(institution_id, &username_lower, client_ip)
            .await?;

        let session = self
            .sessions
            .create(user.id, institution_id, user.session_version)
            .await?;

        Ok(LoginOutcome {
            user_id: user.id,
            session,
        })
    }

    /// True while the account+IP pair has exhausted its failure budget for
    /// the current window. Expired windows count as clean.
    async fn throttled(
        &self,
        institution_id: Uuid,
        username_lower: &str,
        client_ip: &str,
    ) -> Result<bool, AppError> {
        let failures: Option<i32> = sqlx::query_scalar(
            r#"
            SELECT failure_count FROM login_throttle
             WHERE institution_id = $1 AND username_lower = $2 AND client_ip = $3
               AND window_started_at > now() - make_interval(secs => $4)
            "#,
        )
        .bind(institution_id)
        .bind(username_lower)
        .bind(client_ip)
        .bind(self.window_secs)
        .fetch_optional(&self.pool)
        .await?;

        Ok(failures.is_some_and(|count| i64::from(count) >= self.max_failures))
    }

    async fn record_failure(
        &self,
        institution_id: Uuid,
        username_lower: &str,
        client_ip: &str,
    ) -> Result<(), AppError> {
        sqlx::query(
            r#"
            INSERT INTO login_throttle (
                institution_id, username_lower, client_ip,
                window_started_at, failure_count
            )
            VALUES ($1, $2, $3, now(), 1)
            ON CONFLICT (institution_id, username_lower, client_ip) DO UPDATE
               SET failure_count = CASE
                       WHEN login_throttle.window_started_at
                            <= now() - make_interval(secs => $4)
                       THEN 1
                       ELSE login_throttle.failure_count + 1
                   END,
                   window_started_at = CASE
                       WHEN login_throttle.window_started_at
                            <= now() - make_interval(secs => $4)
                       THEN now()
                       ELSE login_throttle.window_started_at
                   END
            "#,
        )
        .bind(institution_id)
        .bind(username_lower)
        .bind(client_ip)
        .bind(self.window_secs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn clear_throttle(
        &self,
        institution_id: Uuid,
        username_lower: &str,
        client_ip: &str,
    ) -> Result<(), AppError> {
        sqlx::query(
            "DELETE FROM login_throttle \
             WHERE institution_id = $1 AND username_lower = $2 AND client_ip = $3",
        )
        .bind(institution_id)
        .bind(username_lower)
        .bind(client_ip)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct CandidateRow {
    id: Uuid,
    status: String,
    session_version: i64,
    password_hash: Option<String>,
}
