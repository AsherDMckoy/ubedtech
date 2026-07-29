//! Identity use cases: authentication, credential and account
//! administration, role assignment (framework-free; HTTP adapters live in
//! http.rs, authorization decisions in policy.rs).

use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::AuditWriter;
use crate::identity_access::password::PasswordService;
use crate::identity_access::policy;
use crate::identity_access::sessions::{NewSession, SessionService};
use crate::shared::actor::{Actor, Role};
use crate::shared::error::AppError;

/// Minimum accepted password length (characters). Documented in SECURITY.md;
/// applies to self-service changes, admin resets, and bootstrap alike.
pub const MIN_PASSWORD_CHARS: usize = 12;

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
    audit: AuditWriter,
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
        audit: AuditWriter,
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
            audit,
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
    /// Role codes for a user — the HTML login uses this to pick a landing
    /// page the account is actually allowed to open.
    pub async fn role_codes(&self, user_id: Uuid) -> Result<Vec<String>, AppError> {
        Ok(sqlx::query_scalar(
            "SELECT r.code FROM user_role ur JOIN role r ON r.id = ur.role_id \
             WHERE ur.user_id = $1",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?)
    }

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

    /// Self-service password change.
    ///
    /// Requires the current password even though the caller is
    /// authenticated (a stolen session must not be enough to take over the
    /// account). On success every session dies (`session_version` bump +
    /// row revocation, one transaction with the audit record) and a fresh
    /// session is issued to this client — rotation, not survival.
    pub async fn change_own_password(
        &self,
        actor: &Actor,
        current_password: &str,
        new_password: &str,
    ) -> Result<NewSession, AppError> {
        validate_new_password(new_password)?;

        let stored: Option<String> =
            sqlx::query_scalar("SELECT password_hash FROM password_credential WHERE user_id = $1")
                .bind(actor.user_id)
                .fetch_optional(&self.pool)
                .await?;
        // An authenticated user without a credential row cannot prove the
        // current password; same answer as a wrong one.
        let stored = stored.ok_or_else(incorrect_current_password)?;

        let passwords = self.passwords.clone();
        let candidate = current_password.to_owned();
        let verified = tokio::task::spawn_blocking(move || passwords.verify(&stored, &candidate))
            .await
            .map_err(|_| AppError::Internal)?;
        match verified {
            Ok(true) => {}
            Ok(false) => return Err(incorrect_current_password()),
            Err(error) => {
                tracing::error!(error = %error, "stored password credential is unusable");
                return Err(incorrect_current_password());
            }
        }

        let passwords = self.passwords.clone();
        let fresh = new_password.to_owned();
        let new_hash = tokio::task::spawn_blocking(move || passwords.hash(&fresh))
            .await
            .map_err(|_| AppError::Internal)?
            .map_err(|_| AppError::Internal)?;

        let new_version = self
            .replace_credential_and_revoke(
                actor.user_id,
                actor.institution_id,
                actor.user_id,
                &new_hash,
                "identity.password_changed",
            )
            .await?;

        self.sessions
            .create(actor.user_id, actor.institution_id, new_version)
            .await
    }

    /// Institution-admin password reset. The target's sessions all die; the
    /// target logs in again with the new password. No session is created —
    /// the admin is not the target.
    pub async fn reset_password(
        &self,
        actor: &Actor,
        target_user_id: Uuid,
        new_password: &str,
    ) -> Result<(), AppError> {
        policy::require_can_manage_accounts(actor)?;
        validate_new_password(new_password)?;

        // Institution scoping: an admin can only touch accounts of their own
        // institution; anything else looks like a missing user.
        let target: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM user_account WHERE id = $1 AND institution_id = $2")
                .bind(target_user_id)
                .bind(actor.institution_id)
                .fetch_optional(&self.pool)
                .await?;
        target.ok_or(AppError::NotFound)?;

        let passwords = self.passwords.clone();
        let fresh = new_password.to_owned();
        let new_hash = tokio::task::spawn_blocking(move || passwords.hash(&fresh))
            .await
            .map_err(|_| AppError::Internal)?
            .map_err(|_| AppError::Internal)?;

        self.replace_credential_and_revoke(
            target_user_id,
            actor.institution_id,
            actor.user_id,
            &new_hash,
            "identity.password_reset",
        )
        .await?;
        Ok(())
    }

    /// Suspend an account: status flip, every session revoked, audited — one
    /// transaction. Fails closed on scope (other institutions look like a
    /// missing user) and refuses self-suspension so the last admin cannot
    /// brick the deployment by accident.
    pub async fn suspend_user(
        &self,
        actor: &Actor,
        target_user_id: Uuid,
        reason: &str,
    ) -> Result<(), AppError> {
        policy::require_can_manage_accounts(actor)?;
        if reason.trim().is_empty() {
            return Err(AppError::Validation("reason is required".into()));
        }
        if target_user_id == actor.user_id {
            return Err(AppError::Validation(
                "you cannot suspend your own account".into(),
            ));
        }

        let mut tx = self.pool.begin().await?;

        let updated: Option<Uuid> = sqlx::query_scalar(
            r#"
            UPDATE user_account
               SET status = 'suspended', session_version = session_version + 1
             WHERE id = $1 AND institution_id = $2
            RETURNING id
            "#,
        )
        .bind(target_user_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?;
        updated.ok_or(AppError::NotFound)?;

        sqlx::query(
            "UPDATE user_session SET revoked_at = now() \
             WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(target_user_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "identity.user_suspended",
                "user_account",
                target_user_id,
                &serde_json::json!({ "reason": reason }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Grant a role. Idempotent: granting an already-held role succeeds
    /// without side effects. A real grant is a privilege change, so the
    /// target's sessions are revoked (they sign in again and get the new
    /// privileges on a fresh session) and the grant is audited — one
    /// transaction.
    pub async fn assign_role(
        &self,
        actor: &Actor,
        target_user_id: Uuid,
        role: Role,
    ) -> Result<(), AppError> {
        let mut tx = self.check_role_change(actor, target_user_id, role).await?;

        let inserted = sqlx::query(
            r#"
            INSERT INTO user_role (institution_id, user_id, role_id)
            SELECT $1, $2, id FROM role WHERE code = $3
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(actor.institution_id)
        .bind(target_user_id)
        .bind(role.code())
        .execute(&mut *tx)
        .await?
        .rows_affected();

        if inserted == 0 {
            // Already held: a retried request must not revoke sessions again.
            return Ok(());
        }

        self.finish_role_change(tx, actor, target_user_id, role, "identity.role_granted")
            .await
    }

    /// Revoke a role. Idempotent like [`Self::assign_role`]; a real
    /// revocation revokes the target's sessions and is audited in the same
    /// transaction.
    pub async fn revoke_role(
        &self,
        actor: &Actor,
        target_user_id: Uuid,
        role: Role,
    ) -> Result<(), AppError> {
        let mut tx = self.check_role_change(actor, target_user_id, role).await?;

        let deleted = sqlx::query(
            "DELETE FROM user_role \
             WHERE institution_id = $1 AND user_id = $2 \
               AND role_id = (SELECT id FROM role WHERE code = $3)",
        )
        .bind(actor.institution_id)
        .bind(target_user_id)
        .bind(role.code())
        .execute(&mut *tx)
        .await?
        .rows_affected();

        if deleted == 0 {
            return Ok(());
        }

        self.finish_role_change(tx, actor, target_user_id, role, "identity.role_revoked")
            .await
    }

    /// Shared head of both role changes: authorization, the
    /// no-self-service rule, and the institution-scoped target lookup
    /// (inside the transaction the change will use).
    async fn check_role_change(
        &self,
        actor: &Actor,
        target_user_id: Uuid,
        role: Role,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, AppError> {
        policy::require_can_manage_roles(actor)?;
        if !policy::assignable_by_institution_admin(role) {
            return Err(AppError::Forbidden);
        }
        // Admins do not edit their own privileges: no self-escalation, and
        // no revoking your own admin role by accident.
        if target_user_id == actor.user_id {
            return Err(AppError::Validation(
                "you cannot change your own roles".into(),
            ));
        }

        let mut tx = self.pool.begin().await?;
        let target: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM user_account WHERE id = $1 AND institution_id = $2")
                .bind(target_user_id)
                .bind(actor.institution_id)
                .fetch_optional(&mut *tx)
                .await?;
        target.ok_or(AppError::NotFound)?;
        Ok(tx)
    }

    /// Shared tail of both role changes: privilege changed, so revoke the
    /// target's sessions (session_version bump + row revocation) and audit,
    /// then commit.
    async fn finish_role_change(
        &self,
        mut tx: sqlx::Transaction<'static, sqlx::Postgres>,
        actor: &Actor,
        target_user_id: Uuid,
        role: Role,
        audit_action: &str,
    ) -> Result<(), AppError> {
        sqlx::query("UPDATE user_account SET session_version = session_version + 1 WHERE id = $1")
            .bind(target_user_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "UPDATE user_session SET revoked_at = now() \
             WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(target_user_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                audit_action,
                "user_account",
                target_user_id,
                &serde_json::json!({ "role": role.code() }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Shared tail of both password paths: swap the credential, bump the
    /// account's session_version, revoke session rows, audit — one
    /// transaction. Returns the new session_version.
    async fn replace_credential_and_revoke(
        &self,
        target_user_id: Uuid,
        institution_id: Uuid,
        acting_user_id: Uuid,
        new_hash: &str,
        audit_action: &str,
    ) -> Result<i64, AppError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO password_credential (user_id, password_hash, changed_at)
            VALUES ($1, $2, now())
            ON CONFLICT (user_id) DO UPDATE
               SET password_hash = EXCLUDED.password_hash, changed_at = now()
            "#,
        )
        .bind(target_user_id)
        .bind(new_hash)
        .execute(&mut *tx)
        .await?;

        let new_version: i64 = sqlx::query_scalar(
            "UPDATE user_account SET session_version = session_version + 1 \
             WHERE id = $1 RETURNING session_version",
        )
        .bind(target_user_id)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE user_session SET revoked_at = now() \
             WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(target_user_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                institution_id,
                acting_user_id,
                audit_action,
                "user_account",
                target_user_id,
                // Never the password or its hash — the fact is enough.
                &serde_json::json!({ "sessions_revoked": true }),
            )
            .await?;

        tx.commit().await?;
        Ok(new_version)
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

    /// Search accounts by username or email for the admin's lookup page.
    pub async fn search_accounts(
        &self,
        actor: &Actor,
        query: &str,
    ) -> Result<Vec<AccountHit>, AppError> {
        policy::require_can_manage_accounts(actor)?;
        let pattern = format!("%{}%", query.trim().replace('%', "\\%").replace('_', "\\_"));
        Ok(sqlx::query_as::<_, AccountHit>(
            r#"
            SELECT ua.id AS user_id, ua.username, ua.email, ua.status::text AS status,
                   COALESCE(r.codes, '') AS roles
            FROM user_account ua
            LEFT JOIN LATERAL (
                SELECT string_agg(role.code, ', ' ORDER BY role.code) AS codes
                FROM user_role ur
                JOIN role ON role.id = ur.role_id
                WHERE ur.user_id = ua.id AND ur.institution_id = $1
            ) r ON true
            WHERE ua.institution_id = $1
              AND (ua.username ILIKE $2 OR ua.email ILIKE $2)
            ORDER BY ua.username
            LIMIT 50
            "#,
        )
        .bind(actor.institution_id)
        .bind(pattern)
        .fetch_all(&self.pool)
        .await?)
    }

    /// One account for the admin's detail page; `None` (→404) outside the
    /// institution.
    pub async fn account_admin(
        &self,
        actor: &Actor,
        user_id: Uuid,
    ) -> Result<Option<AccountHit>, AppError> {
        policy::require_can_manage_accounts(actor)?;
        Ok(sqlx::query_as::<_, AccountHit>(
            r#"
            SELECT ua.id AS user_id, ua.username, ua.email, ua.status::text AS status,
                   COALESCE(r.codes, '') AS roles
            FROM user_account ua
            LEFT JOIN LATERAL (
                SELECT string_agg(role.code, ', ' ORDER BY role.code) AS codes
                FROM user_role ur
                JOIN role ON role.id = ur.role_id
                WHERE ur.user_id = ua.id AND ur.institution_id = $1
            ) r ON true
            WHERE ua.institution_id = $1 AND ua.id = $2
            "#,
        )
        .bind(actor.institution_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?)
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

pub(crate) fn validate_new_password(new_password: &str) -> Result<(), AppError> {
    if new_password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(AppError::Validation(format!(
            "password must be at least {MIN_PASSWORD_CHARS} characters"
        )));
    }
    Ok(())
}

fn incorrect_current_password() -> AppError {
    AppError::Validation("current password is incorrect".into())
}

/// One account in the admin's lookup and detail views.
#[derive(Debug, sqlx::FromRow)]
pub struct AccountHit {
    pub user_id: Uuid,
    pub username: String,
    pub email: String,
    pub status: String,
    /// Comma-joined role codes; empty when the user holds none.
    pub roles: String,
}

#[derive(sqlx::FromRow)]
struct CandidateRow {
    id: Uuid,
    status: String,
    session_version: i64,
    password_hash: Option<String>,
}
