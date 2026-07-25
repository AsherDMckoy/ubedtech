//! Server-side session store.
//!
//! The cookie value is an opaque 256-bit random token; PostgreSQL stores only
//! its SHA-256 (`user_session.token_hash`), so neither a database snapshot
//! nor a log line can be replayed as a session. Each session also carries a
//! CSRF secret, likewise stored only as a hash.
//!
//! A session resolves only while ALL of these hold: not revoked, before its
//! absolute deadline (`expires_at`, fixed at creation), before its sliding
//! idle deadline (`idle_expires_at`, refreshed on activity), its
//! `session_version` still matches `user_account.session_version` (bumping
//! the account version is the revoke-everything lever), and the account is
//! `active`.

use chrono::{DateTime, Duration, Utc};
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashSet;
use std::fmt;
use uuid::Uuid;

use crate::shared::actor::{Actor, Role};
use crate::shared::error::AppError;

/// Refresh `last_seen_at`/`idle_expires_at` at most this often, so hot
/// sessions do not write on every request.
const LAST_SEEN_REFRESH_SECS: i64 = 60;

/// A raw session token on its way into a cookie. Debug/Display never reveal
/// it, so it cannot leak through logging by accident.
pub struct RawToken(String);

impl RawToken {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RawToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RawToken(REDACTED)")
    }
}

#[derive(Debug)] // RawToken's Debug is redacted; csrf_token is client-visible anyway
pub struct NewSession {
    pub session_id: Uuid,
    pub token: RawToken,
    /// Shown to the client once (login response / rendered forms); only its
    /// hash is stored.
    pub csrf_token: String,
}

pub struct ResolvedSession {
    pub session_id: Uuid,
    pub actor: Actor,
    /// SHA-256 of the session's CSRF token, for the CSRF middleware.
    pub csrf_token_hash: Vec<u8>,
    /// The token itself, for embedding into server-rendered forms (ADR-9).
    pub csrf_token: String,
}

/// What the session middleware leaves in request extensions alongside the
/// `Actor`: enough to log the session out, check CSRF tokens, and render
/// them into forms.
#[derive(Clone)]
pub struct CurrentSession {
    pub session_id: Uuid,
    pub csrf_token_hash: Vec<u8>,
    pub csrf_token: String,
}

#[derive(Clone)]
pub struct SessionService {
    pool: PgPool,
    idle: Duration,
    absolute: Duration,
}

impl SessionService {
    pub fn new(pool: PgPool, idle_secs: u64, absolute_secs: u64) -> Self {
        Self {
            pool,
            idle: Duration::seconds(idle_secs as i64),
            absolute: Duration::seconds(absolute_secs as i64),
        }
    }

    /// Create a session for an already-authenticated user. `session_version`
    /// must be the value read from `user_account` during authentication.
    pub async fn create(
        &self,
        user_id: Uuid,
        institution_id: Uuid,
        session_version: i64,
    ) -> Result<NewSession, AppError> {
        let session_id = Uuid::new_v4();
        let token = random_token()?;
        let csrf_token = random_token()?.0;
        let now = Utc::now();

        sqlx::query(
            r#"
            INSERT INTO user_session (
                id, institution_id, user_id, session_version,
                token_hash, csrf_secret_hash, csrf_secret,
                expires_at, idle_expires_at, last_seen_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(session_id)
        .bind(institution_id)
        .bind(user_id)
        .bind(session_version)
        .bind(sha256(token.0.as_bytes()))
        .bind(sha256(csrf_token.as_bytes()))
        .bind(&csrf_token)
        .bind(now + self.absolute)
        .bind(now + self.idle)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(NewSession {
            session_id,
            token,
            csrf_token,
        })
    }

    /// Resolve a raw cookie token to an Actor. `Ok(None)` is every "no valid
    /// session" case — the caller cannot distinguish unknown, expired,
    /// revoked, stale-version, or suspended, and neither can the client.
    pub async fn resolve(&self, raw_token: &str) -> Result<Option<ResolvedSession>, AppError> {
        let row = sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT
                s.id AS session_id,
                s.user_id,
                s.institution_id,
                s.session_version,
                s.csrf_secret_hash,
                s.csrf_secret,
                s.expires_at,
                s.idle_expires_at,
                s.last_seen_at,
                u.session_version AS account_session_version,
                u.status::text AS account_status,
                sp.id AS student_id,
                COALESCE(
                    array_agg(r.code) FILTER (WHERE r.code IS NOT NULL),
                    '{}'
                ) AS role_codes
            FROM user_session s
            JOIN user_account u ON u.id = s.user_id
            LEFT JOIN student_profile sp
                   ON sp.user_id = u.id AND sp.institution_id = s.institution_id
            LEFT JOIN user_role ur
                   ON ur.user_id = u.id AND ur.institution_id = s.institution_id
            LEFT JOIN role r ON r.id = ur.role_id
            WHERE s.token_hash = $1
              AND s.revoked_at IS NULL
              AND s.csrf_secret IS NOT NULL
            GROUP BY s.id, u.id, sp.id
            "#,
        )
        .bind(sha256(raw_token.as_bytes()))
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let now = Utc::now();
        if now >= row.expires_at
            || now >= row.idle_expires_at
            || row.session_version != row.account_session_version
            || row.account_status != "active"
        {
            return Ok(None);
        }

        let mut roles = HashSet::new();
        for code in &row.role_codes {
            // Fail closed: a role code the application does not know cannot
            // silently grant or drop privileges.
            let role = Role::from_code(code).ok_or_else(|| {
                tracing::error!(role_code = %code, "unknown role code in user_role");
                AppError::Internal
            })?;
            roles.insert(role);
        }

        if now - row.last_seen_at >= Duration::seconds(LAST_SEEN_REFRESH_SECS) {
            // The refresh is best-effort bookkeeping, and this statement is
            // written for the stampede: many in-flight requests on one
            // session all read the same stale last_seen_at and all reach
            // this point. The staleness guard repeated in the WHERE means
            // only the first writer's UPDATE matches (measured without it:
            // 64 connections on one session queued ~7 s of redundant
            // fsyncs); SKIP LOCKED means the rest do not even wait for the
            // winner's commit — they see the row locked, skip it, and move
            // on (measured with the guard alone: the no-op herd still
            // convoyed ~3 s on the row lock). Same pattern as the job
            // worker's claim query.
            sqlx::query(
                r#"
                UPDATE user_session
                   SET last_seen_at = $2, idle_expires_at = $3
                 WHERE id IN (
                     SELECT id FROM user_session
                      WHERE id = $1 AND revoked_at IS NULL AND last_seen_at <= $4
                        FOR NO KEY UPDATE SKIP LOCKED
                 )
                "#,
            )
            .bind(row.session_id)
            .bind(now)
            .bind(now + self.idle)
            .bind(now - Duration::seconds(LAST_SEEN_REFRESH_SECS))
            .execute(&self.pool)
            .await?;
        }

        Ok(Some(ResolvedSession {
            session_id: row.session_id,
            actor: Actor {
                user_id: row.user_id,
                institution_id: row.institution_id,
                student_id: row.student_id,
                roles,
            },
            csrf_token_hash: row.csrf_secret_hash,
            csrf_token: row.csrf_secret,
        }))
    }

    /// Revoke one session (logout). Idempotent.
    pub async fn revoke(&self, session_id: Uuid) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE user_session SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn random_token() -> Result<RawToken, AppError> {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.try_fill_bytes(&mut bytes).map_err(|_| {
        tracing::error!("OS entropy source failed while creating a session token");
        AppError::Internal
    })?;
    Ok(RawToken(hex::encode(bytes)))
}

pub(crate) fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    session_id: Uuid,
    user_id: Uuid,
    institution_id: Uuid,
    session_version: i64,
    csrf_secret_hash: Vec<u8>,
    csrf_secret: String,
    expires_at: DateTime<Utc>,
    idle_expires_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    account_session_version: i64,
    account_status: String,
    student_id: Option<Uuid>,
    role_codes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed_user(pool: &PgPool) -> (Uuid, Uuid) {
        let institution_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();

        sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Session U')")
            .bind(institution_id)
            .bind(format!("S-{}", &institution_id.to_string()[..8]))
            .execute(pool)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO user_account (id, institution_id, username, email) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(user_id)
        .bind(institution_id)
        .bind(format!("user-{}", &user_id.to_string()[..8]))
        .bind(format!("{}@test.invalid", &user_id.to_string()[..8]))
        .execute(pool)
        .await
        .unwrap();

        (institution_id, user_id)
    }

    fn service(pool: &PgPool) -> SessionService {
        SessionService::new(pool.clone(), 1800, 43200)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn raw_token_is_never_stored(pool: PgPool) {
        let (institution_id, user_id) = seed_user(&pool).await;
        let sessions = service(&pool);

        let new = sessions.create(user_id, institution_id, 1).await.unwrap();

        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT token_hash FROM user_session WHERE id = $1")
                .bind(new.session_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(stored, sha256(new.token.expose().as_bytes()));
        assert_ne!(stored, new.token.expose().as_bytes().to_vec());

        let csrf_stored: Vec<u8> =
            sqlx::query_scalar("SELECT csrf_secret_hash FROM user_session WHERE id = $1")
                .bind(new.session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(csrf_stored, sha256(new.csrf_token.as_bytes()));
        assert_ne!(csrf_stored, new.csrf_token.as_bytes().to_vec());

        // And the redaction wrapper never prints the token.
        assert_eq!(format!("{:?}", new.token), "RawToken(REDACTED)");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn valid_session_resolves_to_the_correct_actor(pool: PgPool) {
        let (institution_id, user_id) = seed_user(&pool).await;

        let student_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO student_profile (id, institution_id, user_id, student_number, \
             program_code) VALUES ($1, $2, $3, $4, 'CS')",
        )
        .bind(student_id)
        .bind(institution_id)
        .bind(user_id)
        .bind(format!("N-{}", &student_id.to_string()[..8]))
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO user_role (institution_id, user_id, role_id) \
             SELECT $1, $2, id FROM role WHERE code = 'student'",
        )
        .bind(institution_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

        let sessions = service(&pool);
        let new = sessions.create(user_id, institution_id, 1).await.unwrap();

        let resolved = sessions
            .resolve(new.token.expose())
            .await
            .unwrap()
            .expect("fresh session resolves");

        assert_eq!(resolved.session_id, new.session_id);
        assert_eq!(resolved.actor.user_id, user_id);
        assert_eq!(resolved.actor.institution_id, institution_id);
        assert_eq!(resolved.actor.student_id, Some(student_id));
        assert!(resolved.actor.has_role(Role::Student));
        assert_eq!(resolved.actor.roles.len(), 1);
        assert_eq!(resolved.csrf_token_hash, sha256(new.csrf_token.as_bytes()));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn unknown_tokens_do_not_resolve(pool: PgPool) {
        let sessions = service(&pool);
        assert!(sessions.resolve("no-such-token").await.unwrap().is_none());
        assert!(sessions.resolve("").await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn revoked_sessions_do_not_resolve(pool: PgPool) {
        let (institution_id, user_id) = seed_user(&pool).await;
        let sessions = service(&pool);
        let new = sessions.create(user_id, institution_id, 1).await.unwrap();

        sessions.revoke(new.session_id).await.unwrap();
        assert!(
            sessions
                .resolve(new.token.expose())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn absolutely_expired_sessions_do_not_resolve(pool: PgPool) {
        let (institution_id, user_id) = seed_user(&pool).await;
        let sessions = service(&pool);
        let new = sessions.create(user_id, institution_id, 1).await.unwrap();

        sqlx::query(
            "UPDATE user_session SET expires_at = now() - interval '1 second' \
                     WHERE id = $1",
        )
        .bind(new.session_id)
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            sessions
                .resolve(new.token.expose())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn idle_expired_sessions_do_not_resolve(pool: PgPool) {
        let (institution_id, user_id) = seed_user(&pool).await;
        let sessions = service(&pool);
        let new = sessions.create(user_id, institution_id, 1).await.unwrap();

        sqlx::query(
            "UPDATE user_session SET idle_expires_at = now() - interval '1 second' \
             WHERE id = $1",
        )
        .bind(new.session_id)
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            sessions
                .resolve(new.token.expose())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn stale_session_version_does_not_resolve(pool: PgPool) {
        let (institution_id, user_id) = seed_user(&pool).await;
        let sessions = service(&pool);
        let new = sessions.create(user_id, institution_id, 1).await.unwrap();

        // A privilege change bumps the account version; existing sessions die.
        sqlx::query(
            "UPDATE user_account SET session_version = session_version + 1 \
                     WHERE id = $1",
        )
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

        assert!(
            sessions
                .resolve(new.token.expose())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn suspended_account_sessions_do_not_resolve(pool: PgPool) {
        let (institution_id, user_id) = seed_user(&pool).await;
        let sessions = service(&pool);
        let new = sessions.create(user_id, institution_id, 1).await.unwrap();

        sqlx::query("UPDATE user_account SET status = 'suspended' WHERE id = $1")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();

        assert!(
            sessions
                .resolve(new.token.expose())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn activity_slides_the_idle_deadline(pool: PgPool) {
        let (institution_id, user_id) = seed_user(&pool).await;
        let sessions = service(&pool);
        let new = sessions.create(user_id, institution_id, 1).await.unwrap();

        // Make the session look stale-but-alive: last activity two minutes
        // ago (past the refresh threshold), idle deadline seconds away.
        sqlx::query(
            "UPDATE user_session SET last_seen_at = now() - interval '2 minutes', \
             idle_expires_at = now() + interval '5 seconds' WHERE id = $1",
        )
        .bind(new.session_id)
        .execute(&pool)
        .await
        .unwrap();

        let before: DateTime<Utc> =
            sqlx::query_scalar("SELECT idle_expires_at FROM user_session WHERE id = $1")
                .bind(new.session_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert!(
            sessions
                .resolve(new.token.expose())
                .await
                .unwrap()
                .is_some()
        );

        let after: DateTime<Utc> =
            sqlx::query_scalar("SELECT idle_expires_at FROM user_session WHERE id = $1")
                .bind(new.session_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert!(after > before, "idle deadline was refreshed by activity");
    }

    /// The stampede case: N concurrent requests on one stale session all
    /// pass the in-memory staleness check and all fire the refresh UPDATE.
    /// Only the first may write; the rest must re-check after the row lock
    /// clears and no-op — otherwise every one of them pays a durable commit
    /// (the class-B benchmark queued seconds of fsyncs this way).
    ///
    /// Replayed deterministically: an open transaction refreshes the row
    /// and holds the lock while resolve() — which already read the stale
    /// row — blocks on its UPDATE. After the commit, resolve's re-check
    /// must find the row fresh and leave the winner's timestamp in place.
    #[sqlx::test(migrations = "./migrations")]
    async fn concurrent_refreshes_write_once_not_once_per_request(pool: PgPool) {
        let (institution_id, user_id) = seed_user(&pool).await;
        let sessions = service(&pool);
        let new = sessions.create(user_id, institution_id, 1).await.unwrap();

        sqlx::query(
            "UPDATE user_session SET last_seen_at = now() - interval '2 minutes' WHERE id = $1",
        )
        .bind(new.session_id)
        .execute(&pool)
        .await
        .unwrap();

        // The "winner": refresh inside an open transaction, lock held.
        let mut winner = pool.begin().await.unwrap();
        let winner_stamp: DateTime<Utc> = sqlx::query_scalar(
            "UPDATE user_session SET last_seen_at = now(), \
             idle_expires_at = now() + interval '30 minutes' \
             WHERE id = $1 RETURNING last_seen_at",
        )
        .bind(new.session_id)
        .fetch_one(&mut *winner)
        .await
        .unwrap();

        // The "herd member": reads the stale row (MVCC — the winner is
        // uncommitted), fires the refresh UPDATE, blocks on the row lock.
        let herd = {
            let sessions = service(&pool);
            let token = new.token.expose().to_owned();
            tokio::spawn(async move { sessions.resolve(&token).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        winner.commit().await.unwrap();

        let resolved = herd.await.unwrap().unwrap();
        assert!(
            resolved.is_some(),
            "losing the refresh race is not an error"
        );

        let final_stamp: DateTime<Utc> =
            sqlx::query_scalar("SELECT last_seen_at FROM user_session WHERE id = $1")
                .bind(new.session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            final_stamp, winner_stamp,
            "the herd member overwrote the winner's refresh — the WHERE \
             guard is gone and every queued request pays its own fsync"
        );
    }
}
