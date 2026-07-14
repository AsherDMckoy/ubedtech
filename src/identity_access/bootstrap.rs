//! First-platform-administrator bootstrap.
//!
//! The platform_licensing_admin role is deliberately unreachable through the
//! role-assignment API (see policy.rs), so the very first holder has to come
//! from somewhere else: this operator-run procedure, invoked as a CLI
//! subcommand before the server ever accepts traffic (see main.rs and
//! docs/OPERATIONS.md). It refuses to run once any platform admin exists —
//! after that, there is no code path that mints another one.

use sqlx::PgPool;
use uuid::Uuid;

use crate::identity_access::password::PasswordService;
use crate::identity_access::service::validate_new_password;
use crate::shared::actor::Role;
use crate::shared::error::AppError;

/// Create the first platform licensing admin: account, credential, role,
/// audit record — one transaction.
///
/// `institution_code` selects the institution when more than one exists;
/// with exactly one institution it may be omitted. The password must meet
/// the same minimum as every other path.
pub async fn bootstrap_platform_admin(
    pool: &PgPool,
    passwords: &PasswordService,
    username: &str,
    email: &str,
    password: &str,
    institution_code: Option<&str>,
) -> Result<Uuid, AppError> {
    let username = username.trim();
    let email = email.trim();
    if username.is_empty() || email.is_empty() {
        return Err(AppError::Validation(
            "username and email are required".into(),
        ));
    }
    validate_new_password(password)?;

    // Hashing before the transaction keeps the transaction short; the cost
    // is one wasted hash if the guard below refuses.
    let password_hash = passwords.hash(password).map_err(|_| AppError::Internal)?;

    let mut tx = pool.begin().await?;

    // The one-shot guard. The FOR UPDATE on role serializes concurrent
    // bootstrap attempts so two cannot both pass the count check.
    sqlx::query("SELECT id FROM role WHERE code = $1 FOR UPDATE")
        .bind(Role::PlatformLicensingAdmin.code())
        .fetch_one(&mut *tx)
        .await?;
    let existing: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_role ur \
         JOIN role r ON r.id = ur.role_id WHERE r.code = $1",
    )
    .bind(Role::PlatformLicensingAdmin.code())
    .fetch_one(&mut *tx)
    .await?;
    if existing > 0 {
        return Err(AppError::Conflict(
            "a platform licensing admin already exists; \
             use the existing account to manage the platform"
                .into(),
        ));
    }

    let institution_id: Uuid = match institution_code {
        Some(code) => sqlx::query_scalar("SELECT id FROM institution WHERE code = $1")
            .bind(code)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::Validation(format!("no institution with code {code}")))?,
        None => {
            let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM institution LIMIT 2")
                .fetch_all(&mut *tx)
                .await?;
            match ids.as_slice() {
                [only] => *only,
                [] => {
                    return Err(AppError::Validation(
                        "no institution exists yet; seed the institution first".into(),
                    ));
                }
                _ => {
                    return Err(AppError::Validation(
                        "multiple institutions exist; pass the institution code".into(),
                    ));
                }
            }
        }
    };

    let taken: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM user_account WHERE institution_id = $1 AND username = $2",
    )
    .bind(institution_id)
    .bind(username)
    .fetch_optional(&mut *tx)
    .await?;
    if taken.is_some() {
        return Err(AppError::Conflict("that username is already taken".into()));
    }

    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO user_account (id, institution_id, username, email, status) \
         VALUES ($1, $2, $3, $4, 'active')",
    )
    .bind(user_id)
    .bind(institution_id)
    .bind(username)
    .bind(email)
    .execute(&mut *tx)
    .await?;

    sqlx::query("INSERT INTO password_credential (user_id, password_hash) VALUES ($1, $2)")
        .bind(user_id)
        .bind(&password_hash)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO user_role (institution_id, user_id, role_id) \
         SELECT $1, $2, id FROM role WHERE code = $3",
    )
    .bind(institution_id)
    .bind(user_id)
    .bind(Role::PlatformLicensingAdmin.code())
    .execute(&mut *tx)
    .await?;

    // Self-referential actor: there is no one else yet. The record proves
    // when and for whom the bootstrap ran.
    sqlx::query(
        r#"
        INSERT INTO audit_event (
            id, institution_id, actor_user_id, action,
            resource_type, resource_id, detail, occurred_at
        )
        VALUES ($1, $2, $3, 'identity.platform_admin_bootstrapped',
                'user_account', $3, '{}', now())
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(institution_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passwords() -> PasswordService {
        PasswordService::new(8, 1, 1).unwrap()
    }

    async fn seed_institution(pool: &PgPool, code: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Boot U')")
            .bind(id)
            .bind(code)
            .execute(pool)
            .await
            .unwrap();
        id
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn bootstrap_creates_a_working_platform_admin_once(pool: PgPool) {
        let institution_id = seed_institution(&pool, "BOOT").await;

        let user_id = bootstrap_platform_admin(
            &pool,
            &passwords(),
            "first.operator",
            "ops@platform.invalid",
            "operator-password-1",
            None,
        )
        .await
        .unwrap();

        // The account is active, credentialed, and holds the platform role.
        let role_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM user_role ur JOIN role r ON r.id = ur.role_id \
             WHERE ur.user_id = $1 AND r.code = 'platform_licensing_admin'",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(role_count, 1);
        let audit_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_event WHERE institution_id = $1 \
             AND action = 'identity.platform_admin_bootstrapped' AND actor_user_id = $2",
        )
        .bind(institution_id)
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit_count, 1);

        // And the credential really works end to end.
        let sessions =
            crate::identity_access::sessions::SessionService::new(pool.clone(), 1800, 43200);
        let auth = crate::identity_access::service::AuthService::new(
            pool.clone(),
            passwords(),
            sessions,
            crate::audit::AuditWriter,
            10,
            900,
        )
        .unwrap();
        let outcome = auth
            .login(
                institution_id,
                "first.operator",
                "operator-password-1",
                "127.0.0.1",
            )
            .await
            .unwrap();
        assert_eq!(outcome.user_id, user_id);

        // A second bootstrap — any username — is refused.
        let refused = bootstrap_platform_admin(
            &pool,
            &passwords(),
            "second.operator",
            "ops2@platform.invalid",
            "operator-password-2",
            None,
        )
        .await;
        assert!(matches!(refused, Err(AppError::Conflict(_))));
        let admins: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM user_role ur JOIN role r ON r.id = ur.role_id \
             WHERE r.code = 'platform_licensing_admin'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(admins, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn bootstrap_validates_inputs_and_environment(pool: PgPool) {
        // No institution yet: refuse with a pointer at the real problem.
        let no_institution = bootstrap_platform_admin(
            &pool,
            &passwords(),
            "op",
            "op@platform.invalid",
            "operator-password-1",
            None,
        )
        .await;
        assert!(matches!(no_institution, Err(AppError::Validation(_))));

        seed_institution(&pool, "ONE").await;
        seed_institution(&pool, "TWO").await;

        // Two institutions and no code: ambiguous, refuse.
        let ambiguous = bootstrap_platform_admin(
            &pool,
            &passwords(),
            "op",
            "op@platform.invalid",
            "operator-password-1",
            None,
        )
        .await;
        assert!(matches!(ambiguous, Err(AppError::Validation(_))));

        // Unknown code: refuse.
        let unknown = bootstrap_platform_admin(
            &pool,
            &passwords(),
            "op",
            "op@platform.invalid",
            "operator-password-1",
            Some("NOPE"),
        )
        .await;
        assert!(matches!(unknown, Err(AppError::Validation(_))));

        // Short password: the shared minimum applies here too.
        let short = bootstrap_platform_admin(
            &pool,
            &passwords(),
            "op",
            "op@platform.invalid",
            "short",
            Some("ONE"),
        )
        .await;
        assert!(matches!(short, Err(AppError::Validation(_))));

        // Blank identity fields: refuse.
        let blank = bootstrap_platform_admin(
            &pool,
            &passwords(),
            "   ",
            "op@platform.invalid",
            "operator-password-1",
            Some("ONE"),
        )
        .await;
        assert!(matches!(blank, Err(AppError::Validation(_))));

        // With a code it works, and picks the RIGHT institution.
        let user_id = bootstrap_platform_admin(
            &pool,
            &passwords(),
            "op",
            "op@platform.invalid",
            "operator-password-1",
            Some("TWO"),
        )
        .await
        .unwrap();
        let code: String = sqlx::query_scalar(
            "SELECT i.code FROM institution i \
             JOIN user_account u ON u.institution_id = i.id WHERE u.id = $1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(code, "TWO");
    }
}
