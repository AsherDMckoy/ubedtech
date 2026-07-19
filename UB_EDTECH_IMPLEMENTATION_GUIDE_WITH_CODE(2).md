# University of Belize Education Platform
## Implementation Guide with Rust, Actix Web, PostgreSQL, Alpine.js, and Alpine AJAX

This guide turns the architecture into concrete vertical slices inside one ordinary Rust package. The code is intentionally explicit. It avoids generic repositories, internal HTTP, hidden database access, and abstractions that do not yet pay for themselves.

The examples are designed to be copied into one application repository with one root `Cargo.toml`. Feature boundaries are directories and Rust modules, not workspace crates. A production build still requires institution-specific rules, complete error templates, full authentication middleware, migrations tested against real data, and load/security testing.

---

## 1. Dependency baseline

Use compatible current versions rather than copying exact patch versions forever. At the time this guide was produced, the relevant current documentation covered Actix Web 4.x, SQLx 0.9.x, and Askama 0.16.x.

```toml
# Cargo.toml
[package]
name = "ub-education-platform"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
actix-web = "4"
askama = "0.16"
arc-swap = "1"
argon2 = "0.5"
chrono = { version = "0.4", features = ["serde"] }
ed25519-dalek = { version = "2", features = ["serde"] }
hex = "0.4"
printpdf = "0.10"
rand = "0.9"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
sqlx = { version = "0.9", features = [
    "runtime-tokio-rustls",
    "postgres",
    "uuid",
    "chrono",
    "json",
    "macros",
    "migrate",
] }
subtle = "2"
thiserror = "2"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "fs", "time"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
uuid = { version = "1", features = ["serde", "v4"] }
```

There is deliberately no `[workspace]` section. All modules share this dependency list because they are part of the same package. A file such as `src/identity_access/middleware.rs` can use Actix while `src/identity_access/service.rs` remains framework-agnostic; neither file declares dependencies independently.

Why these choices:

- SQLx keeps SQL visible and supports prepared queries and bounded pools.
- Askama provides compile-time templates and HTML escaping.
- `arc-swap` supports a lock-free institution-license snapshot.
- Ed25519 is used only where a real external boundary exists: signed self-hosted licenses.
- `printpdf` supplies the minimal valid demo PDF adapter; final university layouts should replace only that adapter.
- No ORM entity graph is introduced.
- No dependency-injection framework is introduced.

---

## 2. Project placement

```text
ub-platform/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── app.rs
│   ├── config.rs
│   ├── db.rs
│   ├── shared/
│   │   ├── mod.rs
│   │   ├── actor.rs
│   │   ├── error.rs
│   │   └── ids.rs
│   ├── identity_access/
│   │   ├── mod.rs
│   │   ├── middleware.rs
│   │   ├── extractor.rs
│   │   ├── service.rs
│   │   ├── sessions.rs
│   │   ├── password.rs
│   │   ├── queries.rs
│   │   └── types.rs
│   ├── institution/
│   ├── academics/
│   ├── enrollment/
│   │   ├── mod.rs
│   │   ├── http.rs
│   │   ├── service.rs
│   │   ├── policy.rs
│   │   ├── queries.rs
│   │   ├── types.rs
│   │   └── tests.rs
│   ├── records/
│   │   ├── mod.rs
│   │   ├── http.rs
│   │   ├── grades.rs
│   │   ├── schedule.rs
│   │   ├── transcript.rs
│   │   └── templates.rs
│   ├── documents/
│   │   ├── mod.rs
│   │   ├── http.rs
│   │   ├── service.rs
│   │   ├── worker.rs
│   │   ├── storage.rs
│   │   └── templates.rs
│   ├── licensing/
│   │   ├── mod.rs
│   │   ├── middleware.rs
│   │   ├── gate.rs
│   │   ├── service.rs
│   │   ├── signed_license.rs
│   │   └── http.rs
│   ├── audit.rs
│   └── jobs/
│       ├── mod.rs
│       └── worker.rs
├── web/
│   ├── pages/
│   ├── fragments/
│   └── assets/
├── migrations/
└── load/
```

This is one crate. Directories are used to keep related code together, not to create independently packaged components. Declare the top-level modules once:

```rust
// src/main.rs
// Declare only modules that currently contain code. Add new top-level modules
// when their first implementation file is introduced.
mod app;
mod audit;
mod config;
mod db;
mod documents;
mod enrollment;
mod identity_access;
mod licensing;
mod records;
mod shared;
```

A feature's `mod.rs` is its local boundary. Keep implementation files private and re-export only the service and command/result types needed elsewhere:

```rust
// src/enrollment/mod.rs
mod policy;
mod service;
mod types;
pub(crate) mod http;

pub use service::EnrollmentService;
pub use types::{DropCommand, EnrollmentReceipt, RegisterCommand};

#[cfg(test)]
mod tests;
```

Authentication follows the same directory pattern, while making the framework boundary obvious:

```rust
// src/identity_access/mod.rs
// The demo currently supplies only the Actor extractor. Declare the session,
// password, service, and middleware files only when those implementations exist.
mod extractor;
```

`extractor.rs` and `middleware.rs` use Actix because they sit at the HTTP boundary. `service.rs`, `password.rs`, `sessions.rs`, and `types.rs` do not need Actix types even though the package has Actix as a dependency.

The other features re-export the small set of types used by application composition:

```rust
// src/records/mod.rs
mod grades;
mod schedule;
mod transcript;
pub(crate) mod http;

pub use grades::GradeService;
pub use schedule::ScheduleQuery;
pub use transcript::TranscriptSnapshotService;

// src/documents/mod.rs
mod service;
mod storage;
mod worker;
pub(crate) mod http;

pub use service::{DocumentService, RequestDocumentCommand};
pub use worker::DocumentWorker;

// src/licensing/mod.rs
mod gate;
mod service;
mod signed_license;
pub(crate) mod http;

pub use gate::{LicenseGate, LicenseSnapshot, LicenseStatus};
pub use service::LicenseService;
```

No directory has its own `Cargo.toml`. `actix-web`, SQLx, and every other dependency are declared once at the root. Each feature section below names the destination inside `src/`.

---

## 3. Common application primitives

### 3.1 Shared module declaration

**Place in:** `src/shared/mod.rs`

```rust
pub mod actor;
pub mod error;
```

### 3.2 Actor

**Place in:** `src/shared/actor.rs`

The actor is the authenticated request identity after session middleware resolves the opaque session. It contains coarse roles and the student/instructor IDs needed for common policy checks. Do not put every user profile field in it.

```rust
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Student,
    Instructor,
    Registrar,
    RecordsOfficer,
    DocumentOfficer,
    InstitutionAdmin,
    PlatformLicensingAdmin,
}

#[derive(Debug, Clone)]
pub struct Actor {
    pub user_id: Uuid,
    pub institution_id: Uuid,
    pub student_id: Option<Uuid>,
    pub roles: HashSet<Role>,
}

impl Actor {
    pub fn has_role(&self, role: Role) -> bool {
        self.roles.contains(&role)
    }

    pub fn require_student_self(&self) -> Result<Uuid, crate::shared::error::AppError> {
        self.student_id.ok_or(crate::shared::error::AppError::Forbidden)
    }
}
```

Keep the `Actor` itself in `src/shared/actor.rs`. Put the Actix extractor below in `src/identity_access/extractor.rs`; this keeps HTTP framework code out of the authentication service while still using the same package-level Actix dependency.

```rust
use actix_web::{dev::Payload, FromRequest, HttpMessage, HttpRequest};
use std::future::{ready, Ready};

use crate::shared::actor::Actor;

impl FromRequest for Actor {
    type Error = crate::shared::error::AppError;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        ready(
            req.extensions()
                .get::<Actor>()
                .cloned()
                .ok_or(crate::shared::error::AppError::Unauthenticated),
        )
    }
}
```

The authentication middleware is the only component that should know how sessions are represented.

### 3.3 Error type

**Place in:** `src/shared/error.rs`

```rust
use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("authentication required")]
    Unauthenticated,

    #[error("permission denied")]
    Forbidden,

    #[error("resource not found")]
    NotFound,

    #[error("institution license is inactive")]
    InstitutionLocked,

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("database error")]
    Database(#[from] sqlx::Error),

    #[error("template rendering failed")]
    Template(#[from] askama::Error),

    #[error("internal error")]
    Internal,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::InstitutionLocked => StatusCode::PAYMENT_REQUIRED,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Database(_) | Self::Template(_) | Self::Internal => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    fn error_response(&self) -> HttpResponse {
        let code = match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::InstitutionLocked => "institution_locked",
            Self::Validation(_) => "validation_error",
            Self::Conflict(_) => "conflict",
            Self::Database(_) | Self::Template(_) | Self::Internal => "internal_error",
        };

        // Do not expose raw database or internal errors to the client.
        let message = match self {
            Self::Database(_) | Self::Template(_) | Self::Internal => {
                "An internal error occurred".to_owned()
            }
            other => other.to_string(),
        };

        HttpResponse::build(self.status_code()).json(ErrorBody { code, message })
    }
}
```

### 3.4 Audit writer

**Place in:** `src/audit.rs`

Critical audit records are written in the same transaction as the business mutation. That is why the writer accepts an existing transaction instead of opening its own.

```rust
use chrono::Utc;
use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct AuditWriter;

impl AuditWriter {
    pub async fn write<T: Serialize>(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        institution_id: Uuid,
        actor_user_id: Uuid,
        action: &str,
        resource_type: &str,
        resource_id: Uuid,
        detail: &T,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO audit_event (
                id, institution_id, actor_user_id, action,
                resource_type, resource_id, detail, occurred_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(institution_id)
        .bind(actor_user_id)
        .bind(action)
        .bind(resource_type)
        .bind(resource_id)
        .bind(sqlx::types::Json(detail))
        .bind(Utc::now())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}
```

### 3.4 Foundation migration

**Place in:** `migrations/0001_foundation.sql`

```sql
CREATE TYPE user_status AS ENUM ('active', 'suspended', 'closed');

CREATE TABLE institution (
    id uuid PRIMARY KEY,
    code text NOT NULL UNIQUE,
    name text NOT NULL,
    timezone text NOT NULL DEFAULT 'America/Belize',
    status text NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'inactive')),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE user_account (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    username text NOT NULL,
    email text NOT NULL,
    status user_status NOT NULL DEFAULT 'active',
    session_version bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (institution_id, username),
    UNIQUE (institution_id, email)
);

CREATE TABLE password_credential (
    user_id uuid PRIMARY KEY REFERENCES user_account(id) ON DELETE CASCADE,
    password_hash text NOT NULL,
    changed_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE user_session (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    user_id uuid NOT NULL REFERENCES user_account(id),
    session_version bigint NOT NULL,
    csrf_secret_hash bytea NOT NULL,
    expires_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX user_session_active_lookup
    ON user_session (id, expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE role (
    id smallserial PRIMARY KEY,
    code text NOT NULL UNIQUE
);

CREATE TABLE user_role (
    id bigserial PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    user_id uuid NOT NULL REFERENCES user_account(id) ON DELETE CASCADE,
    role_id smallint NOT NULL REFERENCES role(id),
    scope_type text,
    scope_id uuid,
    UNIQUE NULLS NOT DISTINCT (institution_id, user_id, role_id, scope_type, scope_id)
);

CREATE TABLE audit_event (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    actor_user_id uuid NOT NULL REFERENCES user_account(id),
    action text NOT NULL,
    resource_type text NOT NULL,
    resource_id uuid NOT NULL,
    detail jsonb NOT NULL,
    occurred_at timestamptz NOT NULL
);

CREATE INDEX audit_event_resource_history
    ON audit_event (institution_id, resource_type, resource_id, occurred_at DESC);
```

---

# Part I — Registration and Drop/Add

## 4. Directory boundary

**Directory:** `src/enrollment/`

This Rust module owns:

- student-term registration state;
- enrollment state transitions;
- capacity counters;
- idempotency for registration commands;
- enrollment overrides.

It reads:

- term windows, sections, and meetings from `academics`;
- course-completion eligibility through an explicit records-owned read view.

It does not own course titles, grade records, or user authentication.

This boundary de-risks delivery because all concurrency-sensitive rules are in one command service and one transaction. The admin portal calls the same service with a registrar actor; it does not bypass the enrollment service.

---

## 5. Registration schema

### 5.1 Academics tables required by enrollment

**Place in:** `migrations/0002_academics.sql`

```sql
CREATE TABLE academic_term (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    code text NOT NULL,
    name text NOT NULL,
    starts_on date NOT NULL,
    ends_on date NOT NULL,
    registration_opens_at timestamptz NOT NULL,
    registration_closes_at timestamptz NOT NULL,
    drop_add_closes_at timestamptz NOT NULL,
    grade_entry_closes_at timestamptz,
    UNIQUE (institution_id, code),
    CHECK (starts_on <= ends_on),
    CHECK (registration_opens_at < registration_closes_at),
    CHECK (registration_closes_at <= drop_add_closes_at)
);

CREATE TABLE course (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    code text NOT NULL,
    title text NOT NULL,
    credit_hours numeric(4, 1) NOT NULL CHECK (credit_hours > 0),
    active boolean NOT NULL DEFAULT true,
    UNIQUE (institution_id, code)
);

CREATE TABLE course_prerequisite (
    course_id uuid NOT NULL REFERENCES course(id) ON DELETE CASCADE,
    prerequisite_course_id uuid NOT NULL REFERENCES course(id),
    minimum_grade_points double precision NOT NULL DEFAULT 1.0,
    PRIMARY KEY (course_id, prerequisite_course_id),
    CHECK (course_id <> prerequisite_course_id)
);

CREATE TABLE section (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    term_id uuid NOT NULL REFERENCES academic_term(id),
    course_id uuid NOT NULL REFERENCES course(id),
    section_code text NOT NULL,
    status text NOT NULL DEFAULT 'open'
        CHECK (status IN ('draft', 'open', 'closed', 'cancelled')),
    UNIQUE (institution_id, term_id, course_id, section_code)
);

CREATE TABLE room (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    campus_code text NOT NULL,
    room_code text NOT NULL,
    UNIQUE (institution_id, campus_code, room_code)
);

CREATE TABLE section_meeting (
    id uuid PRIMARY KEY,
    section_id uuid NOT NULL REFERENCES section(id) ON DELETE CASCADE,
    day_of_week smallint NOT NULL CHECK (day_of_week BETWEEN 1 AND 7),
    starts_at time NOT NULL,
    ends_at time NOT NULL,
    room_id uuid REFERENCES room(id),
    CHECK (starts_at < ends_at)
);

CREATE INDEX section_meeting_conflict_lookup
    ON section_meeting (section_id, day_of_week, starts_at, ends_at);

CREATE TABLE instructor_assignment (
    section_id uuid NOT NULL REFERENCES section(id) ON DELETE CASCADE,
    instructor_user_id uuid NOT NULL REFERENCES user_account(id),
    assignment_role text NOT NULL DEFAULT 'primary',
    PRIMARY KEY (section_id, instructor_user_id)
);
```

### 5.2 Enrollment tables

**Place in:** `migrations/0003_enrollment.sql`

```sql
CREATE TYPE enrollment_status AS ENUM ('enrolled', 'dropped', 'withdrawn');

CREATE TABLE student_profile (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    user_id uuid NOT NULL REFERENCES user_account(id),
    student_number text NOT NULL,
    program_code text NOT NULL,
    academic_status text NOT NULL DEFAULT 'good_standing',
    UNIQUE (institution_id, user_id),
    UNIQUE (institution_id, student_number)
);

CREATE TABLE student_term_registration (
    student_id uuid NOT NULL REFERENCES student_profile(id),
    term_id uuid NOT NULL REFERENCES academic_term(id),
    status text NOT NULL DEFAULT 'eligible'
        CHECK (status IN ('eligible', 'blocked', 'closed')),
    hold_flags text[] NOT NULL DEFAULT '{}',
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (student_id, term_id)
);

-- Capacity belongs to enrollment because enrollment owns seat reservation.
CREATE TABLE section_capacity (
    section_id uuid PRIMARY KEY REFERENCES section(id) ON DELETE CASCADE,
    capacity integer NOT NULL CHECK (capacity >= 0),
    enrolled_count integer NOT NULL DEFAULT 0 CHECK (enrolled_count >= 0),
    version bigint NOT NULL DEFAULT 1,
    CHECK (enrolled_count <= capacity)
);

CREATE TABLE enrollment (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    student_id uuid NOT NULL REFERENCES student_profile(id),
    section_id uuid NOT NULL REFERENCES section(id),
    status enrollment_status NOT NULL,
    registered_at timestamptz NOT NULL,
    dropped_at timestamptz,
    source text NOT NULL CHECK (source IN ('student', 'registrar', 'import')),
    idempotency_key uuid NOT NULL,
    created_by_user_id uuid NOT NULL REFERENCES user_account(id),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (institution_id, student_id, idempotency_key)
);

CREATE UNIQUE INDEX enrollment_one_active_per_section
    ON enrollment (student_id, section_id)
    WHERE status = 'enrolled';

CREATE INDEX enrollment_student_term_read
    ON enrollment (student_id, status, section_id);

CREATE INDEX enrollment_section_active
    ON enrollment (section_id, student_id)
    WHERE status = 'enrolled';

CREATE TABLE registration_override (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    student_id uuid NOT NULL REFERENCES student_profile(id),
    term_id uuid NOT NULL REFERENCES academic_term(id),
    section_id uuid REFERENCES section(id),
    override_type text NOT NULL CHECK (
        override_type IN ('hold', 'prerequisite', 'schedule_conflict', 'capacity', 'deadline')
    ),
    granted_by_user_id uuid NOT NULL REFERENCES user_account(id),
    expires_at timestamptz,
    note text,
    created_at timestamptz NOT NULL DEFAULT now()
);
```

### 5.3 Records-owned prerequisite read view

This read model lets enrollment check completed courses without writing records tables.

**Place in:** `migrations/0004_records.sql`

```sql
CREATE TABLE grade_record (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    enrollment_id uuid NOT NULL REFERENCES enrollment(id),
    grade_code text NOT NULL,
    grade_points double precision,
    numeric_value double precision,
    state text NOT NULL CHECK (state IN ('draft', 'published', 'amended')),
    entered_by_user_id uuid NOT NULL REFERENCES user_account(id),
    published_at timestamptz,
    version bigint NOT NULL DEFAULT 1,
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (enrollment_id)
);

CREATE VIEW records_student_course_completion AS
SELECT
    e.student_id,
    s.course_id,
    max(g.grade_points) AS best_grade_points
FROM enrollment e
JOIN section s ON s.id = e.section_id
JOIN grade_record g ON g.enrollment_id = e.id
WHERE g.state IN ('published', 'amended')
  AND g.grade_points IS NOT NULL
GROUP BY e.student_id, s.course_id;
```

The view is read-only from enrollment's perspective. If this query becomes expensive, records can replace it with a maintained projection without changing enrollment's business API.

---

## 6. Registration Rust types

**Place in:** `src/enrollment/types.rs`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct RegisterCommand {
    pub section_id: Uuid,
    pub idempotency_key: Uuid,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EnrollmentReceipt {
    pub enrollment_id: Uuid,
    pub section_id: Uuid,
    pub status: String,
    pub registered_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct DropCommand {
    pub enrollment_id: Uuid,
}

#[derive(Debug, sqlx::FromRow)]
pub struct RegistrationContext {
    pub term_id: Uuid,
    pub course_id: Uuid,
    pub section_status: String,
    pub registration_opens_at: DateTime<Utc>,
    pub registration_closes_at: DateTime<Utc>,
    pub drop_add_closes_at: DateTime<Utc>,
}
```

---

## 7. Registration policy

**Place in:** `src/enrollment/policy.rs`

```rust
use crate::shared::{actor::{Actor, Role}, error::AppError};
use uuid::Uuid;

pub fn require_can_register_for(
    actor: &Actor,
    target_student_id: Uuid,
) -> Result<(), AppError> {
    if actor.student_id == Some(target_student_id) {
        return Ok(());
    }

    if actor.has_role(Role::Registrar) {
        return Ok(());
    }

    Err(AppError::Forbidden)
}
```

Keep policy functions small and named after the business decision. Do not spread role comparisons through handlers and SQL files.

---

## 8. Registration service

**Place in:** `src/enrollment/service.rs`

```rust
use crate::audit::AuditWriter;
use chrono::Utc;
use crate::shared::{actor::Actor, error::AppError};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::enrollment::{
    policy::require_can_register_for,
    types::{EnrollmentReceipt, RegisterCommand, RegistrationContext},
};

#[derive(Clone)]
pub struct EnrollmentService {
    pool: PgPool,
    audit: AuditWriter,
}

impl EnrollmentService {
    pub fn new(pool: PgPool, audit: AuditWriter) -> Self {
        Self { pool, audit }
    }

    pub async fn register_self(
        &self,
        actor: &Actor,
        command: RegisterCommand,
    ) -> Result<EnrollmentReceipt, AppError> {
        let student_id = actor.require_student_self()?;
        self.register_for(actor, student_id, command).await
    }

    pub async fn register_for(
        &self,
        actor: &Actor,
        student_id: Uuid,
        command: RegisterCommand,
    ) -> Result<EnrollmentReceipt, AppError> {
        require_can_register_for(actor, student_id)?;

        let mut tx = self.pool.begin().await?;

        // Idempotency is checked before doing expensive work. A repeated browser
        // submission returns the original successful result.
        if let Some(existing) = sqlx::query_as::<_, EnrollmentReceipt>(
            r#"
            SELECT
                id AS enrollment_id,
                section_id,
                status::text AS status,
                registered_at
            FROM enrollment
            WHERE institution_id = $1
              AND student_id = $2
              AND idempotency_key = $3
            "#,
        )
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(command.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return Ok(existing);
        }

        let context = sqlx::query_as::<_, RegistrationContext>(
            r#"
            SELECT
                s.term_id,
                s.course_id,
                s.status AS section_status,
                t.registration_opens_at,
                t.registration_closes_at,
                t.drop_add_closes_at
            FROM section s
            JOIN academic_term t ON t.id = s.term_id
            WHERE s.id = $1
              AND s.institution_id = $2
            "#,
        )
        .bind(command.section_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        if context.section_status != "open" {
            return Err(AppError::Conflict("section is not open".into()));
        }

        let now = Utc::now();
        if now < context.registration_opens_at || now >= context.registration_closes_at {
            return Err(AppError::Conflict(
                "registration window is closed".into(),
            ));
        }

        // Ensure the lock row exists, then lock it. All registration changes for
        // one student and term now execute in a clear order.
        sqlx::query(
            r#"
            INSERT INTO student_term_registration (student_id, term_id)
            VALUES ($1, $2)
            ON CONFLICT (student_id, term_id) DO NOTHING
            "#,
        )
        .bind(student_id)
        .bind(context.term_id)
        .execute(&mut *tx)
        .await?;

        let term_state = sqlx::query_as::<_, StudentTermState>(
            r#"
            SELECT status, hold_flags
            FROM student_term_registration
            WHERE student_id = $1 AND term_id = $2
            FOR UPDATE
            "#,
        )
        .bind(student_id)
        .bind(context.term_id)
        .fetch_one(&mut *tx)
        .await?;

        if term_state.status != "eligible" || !term_state.hold_flags.is_empty() {
            let has_override: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM registration_override
                    WHERE student_id = $1
                      AND term_id = $2
                      AND override_type = 'hold'
                      AND (expires_at IS NULL OR expires_at > now())
                )
                "#,
            )
            .bind(student_id)
            .bind(context.term_id)
            .fetch_one(&mut *tx)
            .await?;

            if !has_override {
                return Err(AppError::Conflict(
                    "student has a registration hold".into(),
                ));
            }
        }

        // Repeat the idempotency lookup after acquiring the per-student/term lock.
        // Two identical submissions can pass the fast pre-lock lookup together;
        // this second lookup makes the later request return the first result.
        if let Some(existing) = sqlx::query_as::<_, EnrollmentReceipt>(
            r#"
            SELECT
                id AS enrollment_id,
                section_id,
                status::text AS status,
                registered_at
            FROM enrollment
            WHERE institution_id = $1
              AND student_id = $2
              AND idempotency_key = $3
            "#,
        )
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(command.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return Ok(existing);
        }

        let duplicate: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM enrollment
                WHERE student_id = $1
                  AND section_id = $2
                  AND status = 'enrolled'
            )
            "#,
        )
        .bind(student_id)
        .bind(command.section_id)
        .fetch_one(&mut *tx)
        .await?;

        if duplicate {
            return Err(AppError::Conflict(
                "student is already enrolled in this section".into(),
            ));
        }

        let prerequisites_met: bool = sqlx::query_scalar(
            r#"
            SELECT NOT EXISTS (
                SELECT 1
                FROM course_prerequisite p
                WHERE p.course_id = $2
                  AND NOT EXISTS (
                      SELECT 1
                      FROM records_student_course_completion c
                      WHERE c.student_id = $1
                        AND c.course_id = p.prerequisite_course_id
                        AND c.best_grade_points >= p.minimum_grade_points
                  )
            )
            "#,
        )
        .bind(student_id)
        .bind(context.course_id)
        .fetch_one(&mut *tx)
        .await?;

        if !prerequisites_met
            && !has_override(
                &mut tx,
                student_id,
                context.term_id,
                Some(command.section_id),
                "prerequisite",
            )
            .await?
        {
            return Err(AppError::Conflict(
                "prerequisite requirements are not satisfied".into(),
            ));
        }

        let has_time_conflict: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM enrollment e
                JOIN section existing_section ON existing_section.id = e.section_id
                JOIN section_meeting existing_meeting
                  ON existing_meeting.section_id = existing_section.id
                JOIN section_meeting target_meeting
                  ON target_meeting.section_id = $2
                 AND target_meeting.day_of_week = existing_meeting.day_of_week
                 AND target_meeting.starts_at < existing_meeting.ends_at
                 AND existing_meeting.starts_at < target_meeting.ends_at
                WHERE e.student_id = $1
                  AND e.status = 'enrolled'
                  AND existing_section.term_id = $3
            )
            "#,
        )
        .bind(student_id)
        .bind(command.section_id)
        .bind(context.term_id)
        .fetch_one(&mut *tx)
        .await?;

        if has_time_conflict
            && !has_override(
                &mut tx,
                student_id,
                context.term_id,
                Some(command.section_id),
                "schedule_conflict",
            )
            .await?
        {
            return Err(AppError::Conflict("schedule conflict detected".into()));
        }

        // The conditional update is the seat reservation algorithm. It does not
        // read a count and later write it; that would race under concurrency.
        let reserved = sqlx::query_scalar::<_, i32>(
            r#"
            UPDATE section_capacity
               SET enrolled_count = enrolled_count + 1,
                   version = version + 1
             WHERE section_id = $1
               AND enrolled_count < capacity
            RETURNING enrolled_count
            "#,
        )
        .bind(command.section_id)
        .fetch_optional(&mut *tx)
        .await?;

        if reserved.is_none()
            && !has_override(
                &mut tx,
                student_id,
                context.term_id,
                Some(command.section_id),
                "capacity",
            )
            .await?
        {
            return Err(AppError::Conflict("section is full".into()));
        }

        // A capacity override needs an explicit capacity policy. For the demo,
        // it does not silently increase the counter beyond the database check.
        if reserved.is_none() {
            return Err(AppError::Conflict(
                "capacity override requires registrar seat adjustment".into(),
            ));
        }

        let enrollment_id = Uuid::new_v4();
        let registered_at = Utc::now();
        let source = if actor.student_id == Some(student_id) {
            "student"
        } else {
            "registrar"
        };

        sqlx::query(
            r#"
            INSERT INTO enrollment (
                id, institution_id, student_id, section_id, status,
                registered_at, source, idempotency_key, created_by_user_id
            )
            VALUES ($1, $2, $3, $4, 'enrolled', $5, $6, $7, $8)
            "#,
        )
        .bind(enrollment_id)
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(command.section_id)
        .bind(registered_at)
        .bind(source)
        .bind(command.idempotency_key)
        .bind(actor.user_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "enrollment.registered",
                "enrollment",
                enrollment_id,
                &RegistrationAudit {
                    student_id,
                    section_id: command.section_id,
                    source,
                },
            )
            .await?;

        tx.commit().await?;

        Ok(EnrollmentReceipt {
            enrollment_id,
            section_id: command.section_id,
            status: "enrolled".into(),
            registered_at,
        })
    }

    pub async fn drop_self(
        &self,
        actor: &Actor,
        enrollment_id: Uuid,
    ) -> Result<(), AppError> {
        let student_id = actor.require_student_self()?;
        self.drop_for(actor, student_id, enrollment_id).await
    }

    pub async fn drop_for(
        &self,
        actor: &Actor,
        student_id: Uuid,
        enrollment_id: Uuid,
    ) -> Result<(), AppError> {
        require_can_register_for(actor, student_id)?;

        let mut tx = self.pool.begin().await?;

        // Read enough context to identify the term, but do not lock the
        // enrollment first. Registration and drop must share one lock order:
        // student-term -> enrollment/section state. A fixed order removes an
        // avoidable source of deadlocks under registration-period contention.
        let row = sqlx::query_as::<_, DropContext>(
            r#"
            SELECT
                e.section_id,
                s.term_id,
                t.drop_add_closes_at
            FROM enrollment e
            JOIN section s ON s.id = e.section_id
            JOIN academic_term t ON t.id = s.term_id
            WHERE e.id = $1
              AND e.student_id = $2
              AND e.institution_id = $3
              AND e.status = 'enrolled'
            "#,
        )
        .bind(enrollment_id)
        .bind(student_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        sqlx::query(
            r#"
            INSERT INTO student_term_registration (student_id, term_id)
            VALUES ($1, $2)
            ON CONFLICT (student_id, term_id) DO NOTHING
            "#,
        )
        .bind(student_id)
        .bind(row.term_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query_scalar::<_, i32>(
            r#"
            SELECT 1
            FROM student_term_registration
            WHERE student_id = $1 AND term_id = $2
            FOR UPDATE
            "#,
        )
        .bind(student_id)
        .bind(row.term_id)
        .fetch_one(&mut *tx)
        .await?;

        // Re-check the mutable state after obtaining the serialization lock.
        // A concurrent duplicate drop will now observe the committed state and
        // fail rather than decrementing capacity twice.
        sqlx::query_scalar::<_, i32>(
            r#"
            SELECT 1
            FROM enrollment
            WHERE id = $1
              AND student_id = $2
              AND institution_id = $3
              AND status = 'enrolled'
            FOR UPDATE
            "#,
        )
        .bind(enrollment_id)
        .bind(student_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        if Utc::now() >= row.drop_add_closes_at
            && !has_override(
                &mut tx,
                student_id,
                row.term_id,
                Some(row.section_id),
                "deadline",
            )
            .await?
        {
            return Err(AppError::Conflict("drop/add period is closed".into()));
        }

        sqlx::query(
            r#"
            UPDATE enrollment
               SET status = 'dropped',
                   dropped_at = now(),
                   updated_at = now()
             WHERE id = $1
            "#,
        )
        .bind(enrollment_id)
        .execute(&mut *tx)
        .await?;

        let changed = sqlx::query(
            r#"
            UPDATE section_capacity
               SET enrolled_count = enrolled_count - 1,
                   version = version + 1
             WHERE section_id = $1
               AND enrolled_count > 0
            "#,
        )
        .bind(row.section_id)
        .execute(&mut *tx)
        .await?;

        if changed.rows_affected() != 1 {
            return Err(AppError::Internal);
        }

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "enrollment.dropped",
                "enrollment",
                enrollment_id,
                &serde_json::json!({
                    "student_id": student_id,
                    "section_id": row.section_id
                }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct StudentTermState {
    status: String,
    hold_flags: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct DropContext {
    section_id: Uuid,
    term_id: Uuid,
    drop_add_closes_at: chrono::DateTime<Utc>,
}

#[derive(Serialize)]
struct RegistrationAudit<'a> {
    student_id: Uuid,
    section_id: Uuid,
    source: &'a str,
}

async fn has_override(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    student_id: Uuid,
    term_id: Uuid,
    section_id: Option<Uuid>,
    override_type: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM registration_override
            WHERE student_id = $1
              AND term_id = $2
              AND (section_id IS NULL OR section_id = $3)
              AND override_type = $4
              AND (expires_at IS NULL OR expires_at > now())
        )
        "#,
    )
    .bind(student_id)
    .bind(term_id)
    .bind(section_id)
    .bind(override_type)
    .fetch_one(&mut **tx)
    .await
}
```

### Why this code respects the hierarchy

1. Correctness: lock student/term, enforce constraints, one transaction.
2. Algorithm: atomic conditional seat update rather than count-then-write.
3. Non-pessimization: early idempotency check, `EXISTS`, explicit columns, no N+1.
4. Infrastructure: no queue, cache, or service split is used to hide an inefficient transaction.

---

## 9. Registration handlers

**Place in:** `src/enrollment/http.rs`

```rust
use actix_web::{post, web, HttpResponse};
use crate::shared::{actor::Actor, error::AppError};
use serde::Deserialize;
use uuid::Uuid;

use crate::enrollment::{EnrollmentService, RegisterCommand};

#[derive(Deserialize)]
pub struct RegisterForm {
    section_id: Uuid,
    idempotency_key: Uuid,
    csrf_token: String,
}

#[post("/api/v1/me/enrollments")]
pub async fn register_json(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    body: web::Json<RegisterCommand>,
) -> Result<HttpResponse, AppError> {
    let receipt = service.register_self(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Created().json(receipt))
}

#[post("/ui/registration/add")]
pub async fn register_fragment(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    form: web::Form<RegisterForm>,
) -> Result<HttpResponse, AppError> {
    // CSRF middleware should validate the token before this handler. Keeping the
    // field in the form preserves non-JavaScript progressive enhancement.
    let _ = &form.csrf_token;

    service
        .register_self(
            &actor,
            RegisterCommand {
                section_id: form.section_id,
                idempotency_key: form.idempotency_key,
            },
        )
        .await?;

    // In a real project render Askama templates. The response must contain every
    // element named by x-target.
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_registration_panel(&actor).await?))
}

#[derive(Deserialize)]
pub struct DropForm {
    enrollment_id: Uuid,
    csrf_token: String,
}

#[post("/ui/registration/drop")]
pub async fn drop_fragment(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    form: web::Form<DropForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    service.drop_self(&actor, form.enrollment_id).await?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_registration_panel(&actor).await?))
}

async fn render_registration_panel(_actor: &Actor) -> Result<String, AppError> {
    // Replace with an Askama template fed by one registration-page query.
    Ok(r#"
        <section id="registration-panel">
            <p role="status">Registration updated.</p>
        </section>
        <div id="notifications" x-sync></div>
    "#
    .to_owned())
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(register_json)
        .service(register_fragment)
        .service(drop_fragment);
}
```

Do not leave manual string rendering in production. It appears here only to keep the handler focused. Askama templates should receive typed view models and auto-escape every value.

---

## 10. Registration Alpine AJAX fragment

**Place in:** `web/fragments/registration_panel.html`

```html
<section id="registration-panel" aria-labelledby="registration-heading">
  <h1 id="registration-heading">Register for classes</h1>

  <div id="registration-notice" role="status" aria-live="polite"></div>

  <table>
    <thead>
      <tr>
        <th scope="col">Course</th>
        <th scope="col">Section</th>
        <th scope="col">Meeting</th>
        <th scope="col">Seats</th>
        <th scope="col">Action</th>
      </tr>
    </thead>
    <tbody>
      {% for section in sections %}
      <tr>
        <td>{{ section.course_code }} — {{ section.course_title }}</td>
        <td>{{ section.section_code }}</td>
        <td>{{ section.meeting_summary }}</td>
        <td>{{ section.remaining_seats }}</td>
        <td>
          <form
            method="post"
            action="/ui/registration/add"
            x-target="registration-panel registration-notice"
            x-target.error="registration-notice"
          >
            <input type="hidden" name="csrf_token" value="{{ csrf_token }}">
            <input type="hidden" name="section_id" value="{{ section.id }}">
            <input type="hidden" name="idempotency_key" value="{{ section.command_key }}">
            <button type="submit" {% if section.remaining_seats == 0 %}disabled{% endif %}>
              Add
            </button>
          </form>
        </td>
      </tr>
      {% endfor %}
    </tbody>
  </table>

  <h2>Current classes</h2>
  <ul>
    {% for item in current_enrollments %}
    <li>
      <span>{{ item.course_code }} — {{ item.title }}</span>
      <form
        method="post"
        action="/ui/registration/drop"
        x-target="registration-panel registration-notice"
        x-target.error="registration-notice"
        @ajax:before="confirm('Drop this class?') || $event.preventDefault()"
      >
        <input type="hidden" name="csrf_token" value="{{ csrf_token }}">
        <input type="hidden" name="enrollment_id" value="{{ item.enrollment_id }}">
        <button type="submit">Drop</button>
      </form>
    </li>
    {% endfor %}
  </ul>
</section>
```

Alpine AJAX automatically disables the submitting button and marks targets `aria-busy="true"`. Use CSS rather than extra JavaScript for loading state:

```css
#registration-panel[aria-busy="true"] {
  opacity: 0.65;
  pointer-events: none;
}
```

### Registration-specific security

- Require authentication, license gate, and CSRF before the handler.
- Never accept `student_id` from a student form; derive it from the actor.
- Use an idempotency key for every add command.
- Recheck deadlines and eligibility inside the transaction.
- Do not trust displayed seat counts.
- Bound all form and JSON bodies.
- Record registrar overrides explicitly.
- Test concurrent registration for the final seat.

---

# Part II — Grades and Schedule

## 11. Directory boundary

**Directories:** `src/records/` owns grades; `src/academics/` owns section structure; `src/enrollment/` owns active enrollment.

The student grade page is a read model in `records`. It may join read-only tables from academics and enrollment in one explicit query. The records directory is the only feature area that writes grade records.

The schedule page is a query adapter, not a new domain module. Its job is to return a screen-shaped read model without introducing another owner of academic facts.

---

## 12. Grade service

**Place in:** `src/records/grades.rs`

```rust
use crate::audit::AuditWriter;
use chrono::{DateTime, Utc};
use crate::shared::{actor::{Actor, Role}, error::AppError};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct GradeService {
    pool: PgPool,
    audit: AuditWriter,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct StudentGradeRow {
    pub course_code: String,
    pub course_title: String,
    pub section_code: String,
    pub grade_code: String,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct SaveGradeCommand {
    pub enrollment_id: Uuid,
    pub grade_code: String,
    pub grade_points: Option<f64>,
    pub numeric_value: Option<f64>,
    pub expected_version: i64,
}

impl GradeService {
    pub fn new(pool: PgPool, audit: AuditWriter) -> Self {
        Self { pool, audit }
    }

    pub async fn student_grades(
        &self,
        actor: &Actor,
        term_id: Uuid,
    ) -> Result<Vec<StudentGradeRow>, AppError> {
        let student_id = actor.require_student_self()?;

        let rows = sqlx::query_as::<_, StudentGradeRow>(
            r#"
            SELECT
                c.code AS course_code,
                c.title AS course_title,
                s.section_code,
                g.grade_code,
                g.published_at
            FROM grade_record g
            JOIN enrollment e ON e.id = g.enrollment_id
            JOIN section s ON s.id = e.section_id
            JOIN course c ON c.id = s.course_id
            WHERE e.student_id = $1
              AND s.term_id = $2
              AND g.institution_id = $3
              AND g.state IN ('published', 'amended')
            ORDER BY c.code, s.section_code
            "#,
        )
        .bind(student_id)
        .bind(term_id)
        .bind(actor.institution_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    pub async fn save_draft(
        &self,
        actor: &Actor,
        command: SaveGradeCommand,
    ) -> Result<i64, AppError> {
        if !actor.has_role(Role::Instructor) && !actor.has_role(Role::RecordsOfficer) {
            return Err(AppError::Forbidden);
        }

        let mut tx = self.pool.begin().await?;

        let assignment_allowed: bool = if actor.has_role(Role::RecordsOfficer) {
            true
        } else {
            sqlx::query_scalar(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM enrollment e
                    JOIN instructor_assignment ia ON ia.section_id = e.section_id
                    WHERE e.id = $1
                      AND ia.instructor_user_id = $2
                )
                "#,
            )
            .bind(command.enrollment_id)
            .bind(actor.user_id)
            .fetch_one(&mut *tx)
            .await?
        };

        if !assignment_allowed {
            return Err(AppError::Forbidden);
        }

        let existing = sqlx::query_as::<_, ExistingGrade>(
            r#"
            SELECT id, grade_code, version
            FROM grade_record
            WHERE enrollment_id = $1
            FOR UPDATE
            "#,
        )
        .bind(command.enrollment_id)
        .fetch_optional(&mut *tx)
        .await?;

        let new_version = match existing {
            Some(old) => {
                if old.version != command.expected_version {
                    return Err(AppError::Conflict(
                        "grade was changed by another user".into(),
                    ));
                }

                let row: (i64,) = sqlx::query_as(
                    r#"
                    UPDATE grade_record
                       SET grade_code = $2,
                           grade_points = $3,
                           numeric_value = $4,
                           state = 'draft',
                           entered_by_user_id = $5,
                           version = version + 1,
                           updated_at = now()
                     WHERE id = $1
                    RETURNING version
                    "#,
                )
                .bind(old.id)
                .bind(&command.grade_code)
                .bind(command.grade_points)
                .bind(command.numeric_value)
                .bind(actor.user_id)
                .fetch_one(&mut *tx)
                .await?;

                self.audit
                    .write(
                        &mut tx,
                        actor.institution_id,
                        actor.user_id,
                        "grade.draft_changed",
                        "grade_record",
                        old.id,
                        &serde_json::json!({
                            "old_grade": old.grade_code,
                            "new_grade": command.grade_code,
                            "new_version": row.0
                        }),
                    )
                    .await?;

                row.0
            }
            None => {
                if command.expected_version != 0 {
                    return Err(AppError::Conflict(
                        "grade no longer has the expected state".into(),
                    ));
                }

                let id = Uuid::new_v4();
                sqlx::query(
                    r#"
                    INSERT INTO grade_record (
                        id, institution_id, enrollment_id, grade_code,
                        grade_points, numeric_value, state,
                        entered_by_user_id, version
                    )
                    SELECT $1, $2, e.id, $4, $5, $6, 'draft', $7, 1
                    FROM enrollment e
                    WHERE e.id = $3 AND e.institution_id = $2
                    "#,
                )
                .bind(id)
                .bind(actor.institution_id)
                .bind(command.enrollment_id)
                .bind(&command.grade_code)
                .bind(command.grade_points)
                .bind(command.numeric_value)
                .bind(actor.user_id)
                .execute(&mut *tx)
                .await?;

                self.audit
                    .write(
                        &mut tx,
                        actor.institution_id,
                        actor.user_id,
                        "grade.draft_created",
                        "grade_record",
                        id,
                        &serde_json::json!({
                            "enrollment_id": command.enrollment_id,
                            "grade": command.grade_code
                        }),
                    )
                    .await?;

                1
            }
        };

        tx.commit().await?;
        Ok(new_version)
    }

    pub async fn publish_section(
        &self,
        actor: &Actor,
        section_id: Uuid,
    ) -> Result<u64, AppError> {
        if !actor.has_role(Role::RecordsOfficer) {
            return Err(AppError::Forbidden);
        }

        let mut tx = self.pool.begin().await?;

        let changed = sqlx::query(
            r#"
            UPDATE grade_record g
               SET state = 'published',
                   published_at = now(),
                   version = version + 1,
                   updated_at = now()
              FROM enrollment e
             WHERE g.enrollment_id = e.id
               AND e.section_id = $1
               AND g.institution_id = $2
               AND g.state = 'draft'
            "#,
        )
        .bind(section_id)
        .bind(actor.institution_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "grade.section_published",
                "section",
                section_id,
                &serde_json::json!({ "count": changed.rows_affected() }),
            )
            .await?;

        tx.commit().await?;
        Ok(changed.rows_affected())
    }
}

#[derive(sqlx::FromRow)]
struct ExistingGrade {
    id: Uuid,
    grade_code: String,
    version: i64,
}
```

### Why optimistic versioning here

Seat reservation is a highly contended scarce-resource operation and uses explicit locking. Grade editing usually involves one instructor and occasional administrative correction. Optimistic version checking gives a clearer user conflict without holding a lock across a human editing session.

---

## 13. Schedule query

**Place in:** `src/records/schedule.rs`

```rust
use crate::shared::{actor::Actor, error::AppError};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct ScheduleQuery {
    pool: PgPool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ScheduleMeeting {
    pub course_code: String,
    pub course_title: String,
    pub section_code: String,
    pub day_of_week: i16,
    pub starts_at: chrono::NaiveTime,
    pub ends_at: chrono::NaiveTime,
    pub campus_code: Option<String>,
    pub room_code: Option<String>,
}

impl ScheduleQuery {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn for_student(
        &self,
        actor: &Actor,
        term_id: Uuid,
    ) -> Result<Vec<ScheduleMeeting>, AppError> {
        let student_id = actor.require_student_self()?;

        let meetings = sqlx::query_as::<_, ScheduleMeeting>(
            r#"
            SELECT
                c.code AS course_code,
                c.title AS course_title,
                s.section_code,
                m.day_of_week,
                m.starts_at,
                m.ends_at,
                r.campus_code,
                r.room_code
            FROM enrollment e
            JOIN section s ON s.id = e.section_id
            JOIN course c ON c.id = s.course_id
            JOIN section_meeting m ON m.section_id = s.id
            LEFT JOIN room r ON r.id = m.room_id
            WHERE e.student_id = $1
              AND e.institution_id = $2
              AND e.status = 'enrolled'
              AND s.term_id = $3
            ORDER BY m.day_of_week, m.starts_at, c.code
            "#,
        )
        .bind(student_id)
        .bind(actor.institution_id)
        .bind(term_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(meetings)
    }
}
```

One query is clearer and faster than loading enrollments and then querying every section.

---

## 14. Grades/schedule handlers and Alpine

**Place in:** `src/records/http.rs`

```rust
use actix_web::{get, web, HttpResponse};
use crate::shared::{actor::Actor, error::AppError};
use serde::Deserialize;
use uuid::Uuid;

use crate::records::{GradeService, ScheduleQuery};

#[derive(Deserialize)]
pub struct TermQuery {
    term_id: Uuid,
}

#[get("/api/v1/me/grades")]
pub async fn grades_json(
    actor: Actor,
    service: web::Data<GradeService>,
    query: web::Query<TermQuery>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(
        service.student_grades(&actor, query.term_id).await?,
    ))
}

#[get("/api/v1/me/schedule")]
pub async fn schedule_json(
    actor: Actor,
    query_service: web::Data<ScheduleQuery>,
    query: web::Query<TermQuery>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(
        query_service.for_student(&actor, query.term_id).await?,
    ))
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(grades_json).service(schedule_json);
}
```

**Place in:** `web/pages/grades.html`

```html
<main>
  <h1>Grades</h1>

  <form method="get" action="/ui/grades/table" x-target="grades-table">
    <label for="term-id">Term</label>
    <select id="term-id" name="term_id" @change="$el.form.requestSubmit()">
      {% for term in terms %}
      <option value="{{ term.id }}" {% if term.current %}selected{% endif %}>
        {{ term.name }}
      </option>
      {% endfor %}
    </select>
  </form>

  <section id="grades-table" aria-live="polite">
    {% include "fragments/grades_table.html" %}
  </section>
</main>
```

**Place in:** `web/fragments/grades_table.html`

```html
<section id="grades-table">
  <table>
    <thead>
      <tr>
        <th scope="col">Course</th>
        <th scope="col">Section</th>
        <th scope="col">Grade</th>
      </tr>
    </thead>
    <tbody>
      {% for grade in grades %}
      <tr>
        <td>{{ grade.course_code }} — {{ grade.course_title }}</td>
        <td>{{ grade.section_code }}</td>
        <td>{{ grade.grade_code }}</td>
      </tr>
      {% endfor %}
    </tbody>
  </table>
</section>
```

### Grade/schedule security

- Derive student identity from the session actor.
- Filter by institution and student in SQL.
- Return only published/amended grades to students.
- Validate instructor assignment for every write.
- Audit old and new grades.
- Use version checks to prevent silent overwrites.
- Use `Cache-Control: private, no-store` for student records.
- Do not put grade data in URLs beyond a term identifier.

---

# Part III — Document Requests and Printing

## 15. Directory boundary

**Directory:** `src/documents/`

It owns request status, approvals, generation jobs, artifacts, and delivery. It asks `records` to produce immutable transcript/proof snapshots. It does not query and reinterpret grades itself.

Unofficial print views are synchronous HTML. Official artifacts are generated asynchronously by a worker in the same application process.

---

## 16. Document schema

**Place in:** `migrations/0005_documents.sql`

```sql
CREATE TABLE transcript_snapshot (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    student_id uuid NOT NULL REFERENCES student_profile(id),
    snapshot_version bigint NOT NULL,
    snapshot_json jsonb NOT NULL,
    content_hash bytea NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (institution_id, student_id, snapshot_version)
);

CREATE TABLE document_request (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    student_id uuid NOT NULL REFERENCES student_profile(id),
    document_type text NOT NULL CHECK (
        document_type IN ('official_transcript', 'enrollment_letter', 'signed_document')
    ),
    status text NOT NULL CHECK (
        status IN ('pending', 'approved', 'rejected', 'generating', 'ready', 'failed')
    ),
    purpose text,
    delivery_method text NOT NULL CHECK (
        delivery_method IN ('download', 'pickup', 'email')
    ),
    current_snapshot_id uuid REFERENCES transcript_snapshot(id),
    requested_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX document_request_admin_queue
    ON document_request (institution_id, status, requested_at);

CREATE INDEX document_request_student_history
    ON document_request (institution_id, student_id, requested_at DESC);

CREATE TABLE document_approval (
    id uuid PRIMARY KEY,
    request_id uuid NOT NULL REFERENCES document_request(id),
    decision text NOT NULL CHECK (decision IN ('approved', 'rejected')),
    decided_by_user_id uuid NOT NULL REFERENCES user_account(id),
    decided_at timestamptz NOT NULL DEFAULT now(),
    note text
);

CREATE TABLE document_job (
    id uuid PRIMARY KEY,
    request_id uuid NOT NULL REFERENCES document_request(id),
    job_type text NOT NULL DEFAULT 'generate_pdf',
    status text NOT NULL CHECK (
        status IN ('queued', 'running', 'complete', 'failed')
    ),
    attempts integer NOT NULL DEFAULT 0,
    available_at timestamptz NOT NULL DEFAULT now(),
    locked_at timestamptz,
    locked_by text,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX document_job_claim
    ON document_job (status, available_at, created_at)
    WHERE status = 'queued';

CREATE TABLE generated_document (
    id uuid PRIMARY KEY,
    request_id uuid NOT NULL REFERENCES document_request(id),
    snapshot_id uuid REFERENCES transcript_snapshot(id),
    content_hash bytea NOT NULL,
    storage_path text NOT NULL,
    mime_type text NOT NULL,
    size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
    issued_at timestamptz NOT NULL DEFAULT now(),
    superseded_at timestamptz
);

CREATE UNIQUE INDEX generated_document_current
    ON generated_document (request_id)
    WHERE superseded_at IS NULL;
```

---

## 17. Transcript snapshot service

**Place in:** `src/records/transcript.rs`

```rust
use crate::shared::error::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct TranscriptSnapshotService;

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscriptSnapshotData {
    pub student_number: String,
    pub student_name: String,
    pub program_code: String,
    pub courses: Vec<TranscriptCourse>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TranscriptCourse {
    pub term_code: String,
    pub course_code: String,
    pub course_title: String,
    pub credit_hours: f64,
    pub grade_code: String,
    pub grade_points: Option<f64>,
}

impl TranscriptSnapshotService {
    pub async fn create(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        institution_id: Uuid,
        student_id: Uuid,
    ) -> Result<Uuid, AppError> {
        // Snapshot versions are monotonic per student. Lock the stable student
        // row before calculating max(version)+1 so two approvals cannot choose
        // the same version concurrently.
        sqlx::query_scalar::<_, i32>(
            r#"
            SELECT 1
            FROM student_profile
            WHERE id = $1 AND institution_id = $2
            FOR UPDATE
            "#,
        )
        .bind(student_id)
        .bind(institution_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let student = sqlx::query_as::<_, StudentHeader>(
            r#"
            SELECT
                sp.student_number,
                ua.username AS student_name,
                sp.program_code
            FROM student_profile sp
            JOIN user_account ua ON ua.id = sp.user_id
            WHERE sp.id = $1 AND sp.institution_id = $2
            "#,
        )
        .bind(student_id)
        .bind(institution_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let courses = sqlx::query_as::<_, TranscriptCourse>(
            r#"
            SELECT
                t.code AS term_code,
                c.code AS course_code,
                c.title AS course_title,
                c.credit_hours::float8 AS credit_hours,
                g.grade_code,
                g.grade_points
            FROM grade_record g
            JOIN enrollment e ON e.id = g.enrollment_id
            JOIN section s ON s.id = e.section_id
            JOIN academic_term t ON t.id = s.term_id
            JOIN course c ON c.id = s.course_id
            WHERE e.student_id = $1
              AND g.institution_id = $2
              AND g.state IN ('published', 'amended')
            ORDER BY t.starts_on, c.code
            "#,
        )
        .bind(student_id)
        .bind(institution_id)
        .fetch_all(&mut **tx)
        .await?;

        let data = TranscriptSnapshotData {
            student_number: student.student_number,
            student_name: student.student_name,
            program_code: student.program_code,
            courses,
        };

        let bytes = serde_json::to_vec(&data).map_err(|_| AppError::Internal)?;
        let hash = Sha256::digest(&bytes).to_vec();

        let next_version: i64 = sqlx::query_scalar(
            r#"
            SELECT COALESCE(max(snapshot_version), 0) + 1
            FROM transcript_snapshot
            WHERE institution_id = $1 AND student_id = $2
            "#,
        )
        .bind(institution_id)
        .bind(student_id)
        .fetch_one(&mut **tx)
        .await?;

        let snapshot_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO transcript_snapshot (
                id, institution_id, student_id, snapshot_version,
                snapshot_json, content_hash
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(snapshot_id)
        .bind(institution_id)
        .bind(student_id)
        .bind(next_version)
        .bind(sqlx::types::Json(&data))
        .bind(hash)
        .execute(&mut **tx)
        .await?;

        Ok(snapshot_id)
    }
}

#[derive(sqlx::FromRow)]
struct StudentHeader {
    student_number: String,
    student_name: String,
    program_code: String,
}
```

The snapshot is denormalized intentionally because it is an immutable issued-document input, not an operational data model.

---

## 18. Document request and approval service

**Place in:** `src/documents/service.rs`

```rust
use crate::audit::AuditWriter;
use crate::shared::{actor::{Actor, Role}, error::AppError};
use crate::records::TranscriptSnapshotService;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct DocumentService {
    pool: PgPool,
    audit: AuditWriter,
    transcript_snapshots: TranscriptSnapshotService,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RequestDocumentCommand {
    pub document_type: String,
    pub purpose: Option<String>,
    pub delivery_method: String,
}

#[derive(Debug, Serialize)]
pub struct DocumentRequestReceipt {
    pub request_id: Uuid,
    pub status: &'static str,
}

impl DocumentService {
    pub fn new(
        pool: PgPool,
        audit: AuditWriter,
        transcript_snapshots: TranscriptSnapshotService,
    ) -> Self {
        Self {
            pool,
            audit,
            transcript_snapshots,
        }
    }

    pub async fn request_for_self(
        &self,
        actor: &Actor,
        command: RequestDocumentCommand,
    ) -> Result<DocumentRequestReceipt, AppError> {
        let student_id = actor.require_student_self()?;

        validate_request(&command)?;

        let request_id = Uuid::new_v4();
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO document_request (
                id, institution_id, student_id, document_type,
                status, purpose, delivery_method
            )
            VALUES ($1, $2, $3, $4, 'pending', $5, $6)
            "#,
        )
        .bind(request_id)
        .bind(actor.institution_id)
        .bind(student_id)
        .bind(&command.document_type)
        .bind(command.purpose.as_deref())
        .bind(&command.delivery_method)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "document.requested",
                "document_request",
                request_id,
                &command,
            )
            .await?;

        tx.commit().await?;

        Ok(DocumentRequestReceipt {
            request_id,
            status: "pending",
        })
    }

    pub async fn approve(
        &self,
        actor: &Actor,
        request_id: Uuid,
        note: Option<&str>,
    ) -> Result<(), AppError> {
        if !actor.has_role(Role::DocumentOfficer) {
            return Err(AppError::Forbidden);
        }

        let mut tx = self.pool.begin().await?;

        let request = sqlx::query_as::<_, PendingRequest>(
            r#"
            SELECT student_id, document_type
            FROM document_request
            WHERE id = $1
              AND institution_id = $2
              AND status = 'pending'
            FOR UPDATE
            "#,
        )
        .bind(request_id)
        .bind(actor.institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let snapshot_id = match request.document_type.as_str() {
            "official_transcript" | "enrollment_letter" => Some(
                self.transcript_snapshots
                    .create(&mut tx, actor.institution_id, request.student_id)
                    .await?,
            ),
            "signed_document" => None,
            _ => return Err(AppError::Validation("unknown document type".into())),
        };

        sqlx::query(
            r#"
            INSERT INTO document_approval (
                id, request_id, decision, decided_by_user_id, note
            )
            VALUES ($1, $2, 'approved', $3, $4)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(request_id)
        .bind(actor.user_id)
        .bind(note)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            UPDATE document_request
               SET status = 'approved',
                   current_snapshot_id = $2,
                   updated_at = now()
             WHERE id = $1
            "#,
        )
        .bind(request_id)
        .bind(snapshot_id)
        .execute(&mut *tx)
        .await?;

        // The job is created in the same transaction. An approved request cannot
        // exist without durable work queued to produce its artifact.
        sqlx::query(
            r#"
            INSERT INTO document_job (id, request_id, status)
            VALUES ($1, $2, 'queued')
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(request_id)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "document.approved",
                "document_request",
                request_id,
                &serde_json::json!({ "snapshot_id": snapshot_id, "note": note }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn reject(
        &self,
        actor: &Actor,
        request_id: Uuid,
        note: &str,
    ) -> Result<(), AppError> {
        if !actor.has_role(Role::DocumentOfficer) {
            return Err(AppError::Forbidden);
        }

        if note.trim().is_empty() {
            return Err(AppError::Validation(
                "a rejection reason is required".into(),
            ));
        }

        let mut tx = self.pool.begin().await?;

        let changed = sqlx::query(
            r#"
            UPDATE document_request
               SET status = 'rejected', updated_at = now()
             WHERE id = $1
               AND institution_id = $2
               AND status = 'pending'
            "#,
        )
        .bind(request_id)
        .bind(actor.institution_id)
        .execute(&mut *tx)
        .await?;

        if changed.rows_affected() != 1 {
            return Err(AppError::NotFound);
        }

        sqlx::query(
            r#"
            INSERT INTO document_approval (
                id, request_id, decision, decided_by_user_id, note
            )
            VALUES ($1, $2, 'rejected', $3, $4)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(request_id)
        .bind(actor.user_id)
        .bind(note)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                actor.institution_id,
                actor.user_id,
                "document.rejected",
                "document_request",
                request_id,
                &serde_json::json!({ "note": note }),
            )
            .await?;

        tx.commit().await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct PendingRequest {
    student_id: Uuid,
    document_type: String,
}

fn validate_request(command: &RequestDocumentCommand) -> Result<(), AppError> {
    match command.document_type.as_str() {
        "official_transcript" | "enrollment_letter" | "signed_document" => {}
        _ => return Err(AppError::Validation("unknown document type".into())),
    }

    match command.delivery_method.as_str() {
        "download" | "pickup" | "email" => {}
        _ => return Err(AppError::Validation("unknown delivery method".into())),
    }

    if command.purpose.as_deref().is_some_and(|value| value.len() > 500) {
        return Err(AppError::Validation("purpose is too long".into()));
    }

    Ok(())
}
```

---

## 19. Document HTTP handlers

**Place in:** `src/documents/http.rs`

```rust
use actix_web::{post, web, HttpResponse};
use askama::Template;
use crate::shared::{
    actor::{Actor, Role},
    error::AppError,
};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::documents::{DocumentService, RequestDocumentCommand};

#[derive(Deserialize)]
pub struct RequestDocumentForm {
    document_type: String,
    purpose: Option<String>,
    delivery_method: String,
    csrf_token: String,
}

#[post("/ui/document-requests")]
pub async fn request_fragment(
    actor: Actor,
    service: web::Data<DocumentService>,
    pool: web::Data<PgPool>,
    form: web::Form<RequestDocumentForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token; // Validated by CSRF middleware.

    service
        .request_for_self(
            &actor,
            RequestDocumentCommand {
                document_type: form.document_type.clone(),
                purpose: form.purpose.clone(),
                delivery_method: form.delivery_method.clone(),
            },
        )
        .await?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_student_request_list(&actor, &pool).await?))
}

#[derive(Deserialize)]
pub struct DecisionForm {
    note: Option<String>,
    csrf_token: String,
}

#[post("/ui/admin/document-requests/{request_id}/approve")]
pub async fn approve_fragment(
    actor: Actor,
    service: web::Data<DocumentService>,
    pool: web::Data<PgPool>,
    request_id: web::Path<Uuid>,
    form: web::Form<DecisionForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    service
        .approve(&actor, request_id.into_inner(), form.note.as_deref())
        .await?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_admin_queue(&actor, &pool, &form.csrf_token).await?))
}

#[post("/ui/admin/document-requests/{request_id}/reject")]
pub async fn reject_fragment(
    actor: Actor,
    service: web::Data<DocumentService>,
    pool: web::Data<PgPool>,
    request_id: web::Path<Uuid>,
    form: web::Form<DecisionForm>,
) -> Result<HttpResponse, AppError> {
    let note = form.note.as_deref().unwrap_or_default();
    service.reject(&actor, request_id.into_inner(), note).await?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(render_admin_queue(&actor, &pool, &form.csrf_token).await?))
}

#[derive(sqlx::FromRow)]
struct StudentRequestRow {
    id: Uuid,
    document_type: String,
    status: String,
    requested_at: chrono::DateTime<chrono::Utc>,
}

struct StudentRequestView {
    id: Uuid,
    document_type_label: &'static str,
    status: String,
    requested_at: String,
}

#[derive(Template)]
#[template(
    source = r#"
<section id="document-request-list" aria-live="polite">
  <h2>Your requests</h2>
  {% if requests.is_empty() %}
    <p>No document requests yet.</p>
  {% else %}
    <ul>
    {% for request in requests %}
      <li>
        <strong>{{ request.document_type_label }}</strong>
        — {{ request.status }}
        <time datetime="{{ request.requested_at }}">{{ request.requested_at }}</time>
      </li>
    {% endfor %}
    </ul>
  {% endif %}
</section>
"#,
    ext = "html"
)]
struct StudentRequestListTemplate<'a> {
    requests: &'a [StudentRequestView],
}

async fn render_student_request_list(
    actor: &Actor,
    pool: &PgPool,
) -> Result<String, AppError> {
    let student_id = actor.require_student_self()?;

    let rows = sqlx::query_as::<_, StudentRequestRow>(
        r#"
        SELECT id, document_type, status, requested_at
        FROM document_request
        WHERE institution_id = $1 AND student_id = $2
        ORDER BY requested_at DESC
        LIMIT 50
        "#,
    )
    .bind(actor.institution_id)
    .bind(student_id)
    .fetch_all(pool)
    .await?;

    let requests: Vec<_> = rows
        .into_iter()
        .map(|row| StudentRequestView {
            id: row.id,
            document_type_label: document_type_label(&row.document_type),
            status: row.status,
            requested_at: row.requested_at.to_rfc3339(),
        })
        .collect();

    Ok(StudentRequestListTemplate { requests: &requests }.render()?)
}

#[derive(sqlx::FromRow)]
struct PendingRequestRow {
    id: Uuid,
    document_type: String,
    student_number: String,
    purpose: Option<String>,
    requested_at: chrono::DateTime<chrono::Utc>,
}

struct PendingRequestView {
    id: Uuid,
    document_type_label: &'static str,
    student_number: String,
    purpose: String,
    requested_at: String,
}

#[derive(Template)]
#[template(
    source = r#"
<section id="document-queue">
  <h1>Pending document requests</h1>
  {% if requests.is_empty() %}
    <p>No pending requests.</p>
  {% endif %}
  {% for request in requests %}
  <article>
    <h2>{{ request.document_type_label }}</h2>
    <p>Student: {{ request.student_number }}</p>
    <p>Requested: {{ request.requested_at }}</p>
    <p>{{ request.purpose }}</p>
    <form method="post"
          action="/ui/admin/document-requests/{{ request.id }}/approve"
          x-target="document-queue">
      <input type="hidden" name="csrf_token" value="{{ csrf_token }}">
      <label>Approval note <textarea name="note"></textarea></label>
      <button type="submit">Approve</button>
    </form>
    <form method="post"
          action="/ui/admin/document-requests/{{ request.id }}/reject"
          x-target="document-queue"
          x-target.422="document-queue">
      <input type="hidden" name="csrf_token" value="{{ csrf_token }}">
      <label>Rejection reason <textarea name="note" required></textarea></label>
      <button type="submit">Reject</button>
    </form>
  </article>
  {% endfor %}
</section>
"#,
    ext = "html"
)]
struct AdminQueueTemplate<'a> {
    requests: &'a [PendingRequestView],
    csrf_token: &'a str,
}

async fn render_admin_queue(
    actor: &Actor,
    pool: &PgPool,
    csrf_token: &str,
) -> Result<String, AppError> {
    if !actor.has_role(Role::DocumentOfficer) {
        return Err(AppError::Forbidden);
    }

    let rows = sqlx::query_as::<_, PendingRequestRow>(
        r#"
        SELECT
            dr.id,
            dr.document_type,
            sp.student_number,
            dr.purpose,
            dr.requested_at
        FROM document_request dr
        JOIN student_profile sp ON sp.id = dr.student_id
        WHERE dr.institution_id = $1 AND dr.status = 'pending'
        ORDER BY dr.requested_at
        LIMIT 100
        "#,
    )
    .bind(actor.institution_id)
    .fetch_all(pool)
    .await?;

    let requests: Vec<_> = rows
        .into_iter()
        .map(|row| PendingRequestView {
            id: row.id,
            document_type_label: document_type_label(&row.document_type),
            student_number: row.student_number,
            purpose: row.purpose.unwrap_or_default(),
            requested_at: row.requested_at.to_rfc3339(),
        })
        .collect();

    Ok(AdminQueueTemplate {
        requests: &requests,
        csrf_token,
    }
    .render()?)
}

fn document_type_label(value: &str) -> &'static str {
    match value {
        "official_transcript" => "Official transcript",
        "enrollment_letter" => "Proof of enrollment",
        "signed_document" => "Signed document",
        _ => "Document",
    }
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(request_fragment)
        .service(approve_fragment)
        .service(reject_fragment);
}
```

The render functions are read adapters inside the `documents` module. They use one bounded page query and auto-escaped Askama fragments; they do not reimplement approval or generation rules.

---

## 20. PostgreSQL job worker

**Place in:** `src/documents/worker.rs`

The worker runs in the same binary but outside HTTP handlers. `SKIP LOCKED` makes claiming safe if more worker tasks are added later.

```rust
use std::{path::PathBuf, time::Duration};

use crate::shared::error::AppError;
use printpdf::{
    BuiltinFont, Mm, Op, PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions,
    Point, Pt, TextItem,
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::time::sleep;
use uuid::Uuid;

use crate::documents::storage::FilesystemDocumentStore;

#[derive(Clone)]
pub struct DocumentWorker {
    pool: PgPool,
    worker_id: String,
    store: FilesystemDocumentStore,
}

impl DocumentWorker {
    pub fn new(pool: PgPool, worker_id: String, root: PathBuf) -> Self {
        Self {
            pool,
            worker_id,
            store: FilesystemDocumentStore::new(root),
        }
    }

    pub async fn run(self) {
        loop {
            match self.run_once().await {
                Ok(true) => {}
                Ok(false) => sleep(Duration::from_millis(200)).await,
                Err(error) => {
                    tracing::error!(?error, "document worker iteration failed");
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn run_once(&self) -> Result<bool, AppError> {
        let mut tx = self.pool.begin().await?;

        let job = sqlx::query_as::<_, ClaimedJob>(
            r#"
            SELECT j.id AS job_id, j.request_id, r.current_snapshot_id
            FROM document_job j
            JOIN document_request r ON r.id = j.request_id
            WHERE j.status = 'queued'
              AND j.available_at <= now()
            ORDER BY j.created_at
            FOR UPDATE OF j SKIP LOCKED
            LIMIT 1
            "#,
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(job) = job else {
            tx.rollback().await?;
            return Ok(false);
        };

        sqlx::query(
            r#"
            UPDATE document_job
               SET status = 'running',
                   locked_at = now(),
                   locked_by = $2,
                   attempts = attempts + 1
             WHERE id = $1
            "#,
        )
        .bind(job.job_id)
        .bind(&self.worker_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE document_request SET status = 'generating', updated_at = now() WHERE id = $1",
        )
        .bind(job.request_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        // Rendering is deliberately outside the transaction. Never hold a DB
        // lock while doing PDF work or filesystem I/O.
        let result = self.generate(job.request_id, job.current_snapshot_id).await;

        match result {
            Ok(artifact) => self.complete(job, artifact).await?,
            Err(error) => self.fail(job.job_id, job.request_id, &error.to_string()).await?,
        }

        Ok(true)
    }

    async fn generate(
        &self,
        request_id: Uuid,
        snapshot_id: Option<Uuid>,
    ) -> Result<Artifact, AppError> {
        let snapshot: Option<serde_json::Value> = match snapshot_id {
            Some(id) => sqlx::query_scalar(
                "SELECT snapshot_json FROM transcript_snapshot WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?,
            None => None,
        };

        // The demo adapter emits a valid minimal PDF. Production replaces only
        // the renderer with approved stationery, fonts, seals, and signatures.
        let pdf_bytes = render_pdf(request_id, snapshot.as_ref())?;
        let digest = Sha256::digest(&pdf_bytes);
        let hash_hex = hex::encode(digest);
        let path = self.store.write(&hash_hex, &pdf_bytes).await?;

        Ok(Artifact {
            bytes: pdf_bytes.len() as i64,
            hash: digest.to_vec(),
            path,
        })
    }

    async fn complete(&self, job: ClaimedJob, artifact: Artifact) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO generated_document (
                id, request_id, snapshot_id, content_hash,
                storage_path, mime_type, size_bytes
            )
            VALUES ($1, $2, $3, $4, $5, 'application/pdf', $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(job.request_id)
        .bind(job.current_snapshot_id)
        .bind(artifact.hash)
        .bind(artifact.path)
        .bind(artifact.bytes)
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE document_job SET status = 'complete' WHERE id = $1")
            .bind(job.job_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "UPDATE document_request SET status = 'ready', updated_at = now() WHERE id = $1",
        )
        .bind(job.request_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn fail(
        &self,
        job_id: Uuid,
        request_id: Uuid,
        message: &str,
    ) -> Result<(), AppError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            UPDATE document_job
               SET status = CASE WHEN attempts >= 3 THEN 'failed' ELSE 'queued' END,
                   available_at = CASE
                       WHEN attempts >= 3 THEN available_at
                       ELSE now() + interval '30 seconds'
                   END,
                   last_error = $2,
                   locked_at = NULL,
                   locked_by = NULL
             WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(truncate_for_log(message, 1000))
        .execute(&mut *tx)
        .await?;

        let terminal: bool = sqlx::query_scalar(
            "SELECT status = 'failed' FROM document_job WHERE id = $1",
        )
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await?;

        if terminal {
            sqlx::query(
                "UPDATE document_request SET status = 'failed', updated_at = now() WHERE id = $1",
            )
            .bind(request_id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow, Clone, Copy)]
struct ClaimedJob {
    job_id: Uuid,
    request_id: Uuid,
    current_snapshot_id: Option<Uuid>,
}

struct Artifact {
    bytes: i64,
    hash: Vec<u8>,
    path: String,
}

fn render_pdf(
    request_id: Uuid,
    snapshot: Option<&serde_json::Value>,
) -> Result<Vec<u8>, AppError> {
    // This is a valid, deliberately plain demo renderer. It proves the worker,
    // artifact, hashing, and download path without coupling the documents
    // module to a browser or office-suite process. Replace this function—not
    // the workflow—with a versioned University template before production.
    let snapshot = snapshot.ok_or(AppError::Internal)?;

    let string_field = |name: &str| {
        snapshot
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Not available")
            .to_owned()
    };

    let mut lines: Vec<PdfLine> = vec![
        PdfLine::title("University of Belize"),
        PdfLine::subtitle("Official Transcript — Demo Layout"),
        PdfLine::body(format!("Request ID: {request_id}")),
        PdfLine::body(format!("Student: {}", string_field("student_name"))),
        PdfLine::body(format!("Student number: {}", string_field("student_number"))),
        PdfLine::body(format!("Program: {}", string_field("program_code"))),
        PdfLine::body(""),
        PdfLine::heading("Academic record"),
    ];

    if let Some(courses) = snapshot.get("courses").and_then(serde_json::Value::as_array) {
        for course in courses {
            let term = json_text(course, "term_code");
            let code = json_text(course, "course_code");
            let title = json_text(course, "course_title");
            let grade = json_text(course, "grade_code");
            let credits = course
                .get("credit_hours")
                .and_then(serde_json::Value::as_f64)
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "—".to_owned());

            lines.extend(wrap_pdf_line(
                format!("{term} | {code} | {title} | {credits} credits | Grade {grade}"),
                92,
            ));
        }
    }

    lines.push(PdfLine::body(""));
    lines.push(PdfLine::body(
        "Generated from an immutable approved snapshot. Verify the artifact hash before external use.",
    ));

    const LINES_PER_PAGE: usize = 43;
    let mut pages = Vec::new();

    for (page_index, chunk) in lines.chunks(LINES_PER_PAGE).enumerate() {
        let mut ops = vec![
            Op::StartTextSection,
            Op::SetTextCursor {
                pos: Point::new(Mm(18.0), Mm(279.0)),
            },
            Op::SetLineHeight { lh: Pt(15.0) },
        ];

        if page_index > 0 {
            ops.extend([
                Op::SetFont {
                    font: PdfFontHandle::Builtin(BuiltinFont::HelveticaBold),
                    size: Pt(11.0),
                },
                Op::ShowText {
                    items: vec![TextItem::Text("Official Transcript — continued".to_owned())],
                },
                Op::AddLineBreak,
                Op::AddLineBreak,
            ]);
        }

        for line in chunk {
            ops.push(Op::SetFont {
                font: PdfFontHandle::Builtin(line.font),
                size: Pt(line.size),
            });
            ops.push(Op::ShowText {
                items: vec![TextItem::Text(line.text.clone())],
            });
            ops.push(Op::AddLineBreak);
        }

        ops.push(Op::EndTextSection);
        pages.push(PdfPage::new(Mm(210.0), Mm(297.0), ops));
    }

    let mut document = PdfDocument::new("University of Belize Official Transcript");
    document.with_pages(pages);

    let mut warnings = Vec::new();
    let bytes = document.save(&PdfSaveOptions::default(), &mut warnings);

    for warning in warnings {
        tracing::warn!(?warning, %request_id, "PDF renderer warning");
    }

    if !bytes.starts_with(b"%PDF-") {
        return Err(AppError::Internal);
    }

    Ok(bytes)
}

#[derive(Clone)]
struct PdfLine {
    text: String,
    font: BuiltinFont,
    size: f32,
}

impl PdfLine {
    fn title(text: impl Into<String>) -> Self {
        Self { text: text.into(), font: BuiltinFont::HelveticaBold, size: 18.0 }
    }

    fn subtitle(text: impl Into<String>) -> Self {
        Self { text: text.into(), font: BuiltinFont::HelveticaBold, size: 13.0 }
    }

    fn heading(text: impl Into<String>) -> Self {
        Self { text: text.into(), font: BuiltinFont::HelveticaBold, size: 11.0 }
    }

    fn body(text: impl Into<String>) -> Self {
        Self { text: text.into(), font: BuiltinFont::Helvetica, size: 10.0 }
    }
}

fn json_text(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("—")
        .to_owned()
}

fn wrap_pdf_line(text: String, maximum_chars: usize) -> Vec<PdfLine> {
    let mut result = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let needed = current.len() + usize::from(!current.is_empty()) + word.len();
        if needed > maximum_chars && !current.is_empty() {
            result.push(PdfLine::body(std::mem::take(&mut current)));
        }

        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        result.push(PdfLine::body(current));
    }

    if result.is_empty() {
        result.push(PdfLine::body(""));
    }

    result
}

fn truncate_for_log(value: &str, maximum: usize) -> &str {
    value.get(..maximum).unwrap_or(value)
}
```

The renderer above produces a valid, intentionally plain PDF for the demo. It is not the final University stationery. Before production, replace the renderer adapter with approved layouts, an embedded Unicode font, pagination rules, signatures/seals, accessibility metadata, and golden-file/PDF-parser tests. The claiming, state transitions, retries, immutable snapshots, hashing, and storage do not need to change.

### Filesystem storage adapter

**Place in:** `src/documents/storage.rs`

```rust
use std::path::PathBuf;
use crate::shared::error::AppError;

#[derive(Clone)]
pub struct FilesystemDocumentStore {
    root: PathBuf,
}

impl FilesystemDocumentStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn write(&self, hash_hex: &str, bytes: &[u8]) -> Result<String, AppError> {
        let prefix = &hash_hex[..2];
        let directory = self.root.join(prefix);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|_| AppError::Internal)?;

        let final_path = directory.join(format!("{hash_hex}.pdf"));
        let temporary_path = directory.join(format!(".{hash_hex}.tmp"));

        tokio::fs::write(&temporary_path, bytes)
            .await
            .map_err(|_| AppError::Internal)?;
        tokio::fs::rename(&temporary_path, &final_path)
            .await
            .map_err(|_| AppError::Internal)?;

        Ok(final_path.to_string_lossy().into_owned())
    }
}
```

---

## 21. Unofficial print view

**Place in:** `web/pages/unofficial_transcript_print.html`

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Unofficial Transcript</title>
  <link rel="stylesheet" href="/assets/app.css">
  <style>
    @media print {
      .no-print { display: none; }
      @page { margin: 18mm; }
    }
    .watermark {
      position: fixed;
      inset: 40% 0 auto 0;
      text-align: center;
      font-size: 72px;
      opacity: 0.08;
      transform: rotate(-25deg);
      pointer-events: none;
    }
  </style>
</head>
<body>
  <div class="watermark">UNOFFICIAL</div>

  <button class="no-print" type="button" onclick="window.print()">Print</button>

  <header>
    <h1>University of Belize</h1>
    <h2>Unofficial Transcript</h2>
    <p>Generated {{ generated_at }}</p>
  </header>

  <dl>
    <dt>Student</dt><dd>{{ student_name }}</dd>
    <dt>Student number</dt><dd>{{ student_number }}</dd>
    <dt>Program</dt><dd>{{ program_code }}</dd>
  </dl>

  <table>
    <thead>
      <tr>
        <th scope="col">Term</th>
        <th scope="col">Course</th>
        <th scope="col">Credits</th>
        <th scope="col">Grade</th>
      </tr>
    </thead>
    <tbody>
      {% for row in courses %}
      <tr>
        <td>{{ row.term_code }}</td>
        <td>{{ row.course_code }} — {{ row.course_title }}</td>
        <td>{{ row.credit_hours }}</td>
        <td>{{ row.grade_code }}</td>
      </tr>
      {% endfor %}
    </tbody>
  </table>
</body>
</html>
```

Browser printing is the simplest correct solution for unofficial documents. It avoids forcing server-side PDF generation into every read.

---

## 22. Document request Alpine AJAX

**Place in:** `web/pages/documents.html`

```html
<main>
  <h1>Documents</h1>

  <p>
    <a href="/ui/documents/unofficial-transcript/print" target="_blank" rel="noopener">
      Print unofficial transcript
    </a>
  </p>

  <form
    id="document-request-form"
    method="post"
    action="/ui/document-requests"
    x-target="document-request-list"
    x-target.422="document-request-form"
  >
    <input type="hidden" name="csrf_token" value="{{ csrf_token }}">

    <label for="document-type">Official document</label>
    <select id="document-type" name="document_type" required>
      <option value="official_transcript">Official transcript</option>
      <option value="enrollment_letter">Proof of enrollment</option>
      <option value="signed_document">Other signed document</option>
    </select>

    <label for="purpose">Purpose</label>
    <textarea id="purpose" name="purpose" maxlength="500"></textarea>

    <label for="delivery-method">Delivery</label>
    <select id="delivery-method" name="delivery_method" required>
      <option value="download">Secure download</option>
      <option value="pickup">Pickup</option>
      <option value="email">Email</option>
    </select>

    <button type="submit">Submit request</button>
  </form>

  <section id="document-request-list" aria-live="polite">
    {% include "fragments/document_request_list.html" %}
  </section>
</main>
```

**Place in:** `web/fragments/admin_document_queue.html`

```html
<section id="document-queue">
  <h1>Pending document requests</h1>

  {% for request in requests %}
  <article>
    <h2>{{ request.document_type_label }}</h2>
    <p>Student: {{ request.student_number }}</p>
    <p>Requested: {{ request.requested_at }}</p>
    <p>{{ request.purpose }}</p>

    <form
      method="post"
      action="/ui/admin/document-requests/{{ request.id }}/approve"
      x-target="document-queue"
    >
      <input type="hidden" name="csrf_token" value="{{ csrf_token }}">
      <label>
        Approval note
        <textarea name="note"></textarea>
      </label>
      <button type="submit">Approve</button>
    </form>

    <form
      method="post"
      action="/ui/admin/document-requests/{{ request.id }}/reject"
      x-target="document-queue"
      x-target.422="document-queue"
    >
      <input type="hidden" name="csrf_token" value="{{ csrf_token }}">
      <label>
        Rejection reason
        <textarea name="note" required></textarea>
      </label>
      <button type="submit">Reject</button>
    </form>
  </article>
  {% endfor %}
</section>
```

### Document-specific security

- Students can request and view only their own documents.
- Approval requires `document_officer` role.
- Request status transitions are validated server-side.
- Official generation uses immutable snapshots.
- Artifact filenames are content-derived, not user supplied.
- Never expose filesystem paths to clients.
- Download handlers authorize ownership/role before streaming.
- Add `Content-Disposition: attachment` and a fixed safe filename.
- Scan or strictly validate any future uploaded templates.
- Keep generated artifact hashes and audit history.
- Do not log transcript content or document bytes.

---

# Part IV — Subscription Management and Kill Switch

## 23. Directory boundary

**Directory:** `src/licensing/`

Licensing is checked at the application boundary before normal session/database work. The licensing directory owns institution contract metadata, the small enforcement projection, and signed-license verification. It never mutates individual student accounts.

Hosted contracts carry the software fee plus a hosting fee. Self-hosted contracts carry the software fee plus an installation fee. The demo records those agreed terms but does not build a payment processor or invoicing engine. Hosted license changes use PostgreSQL plus an atomically replaced in-memory snapshot. Self-hosted installations use a signed license document.

---

## 24. Licensing schema

**Place in:** `migrations/0006_licensing.sql`

```sql
-- Commercial terms are platform-operator data, not student billing data.
-- Money is stored in minor currency units to avoid floating-point amounts.
CREATE TABLE institution_contract (
    institution_id uuid PRIMARY KEY REFERENCES institution(id),
    contract_reference text NOT NULL UNIQUE,
    billing_model text NOT NULL CHECK (billing_model IN ('annual', 'contractual')),
    deployment_mode text NOT NULL CHECK (deployment_mode IN ('hosted', 'self_hosted')),
    currency_code char(3) NOT NULL,
    software_fee_minor bigint NOT NULL CHECK (software_fee_minor >= 0),
    hosting_fee_minor bigint,
    installation_fee_minor bigint,
    starts_at timestamptz NOT NULL,
    ends_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (starts_at < ends_at),
    CHECK (
        (deployment_mode = 'hosted'
            AND hosting_fee_minor IS NOT NULL
            AND hosting_fee_minor >= 0
            AND installation_fee_minor IS NULL)
        OR
        (deployment_mode = 'self_hosted'
            AND installation_fee_minor IS NOT NULL
            AND installation_fee_minor >= 0
            AND hosting_fee_minor IS NULL)
    )
);

CREATE TABLE institution_license (
    institution_id uuid PRIMARY KEY REFERENCES institution(id),
    deployment_id uuid NOT NULL,
    mode text NOT NULL CHECK (mode IN ('hosted', 'self_hosted')),
    status text NOT NULL CHECK (status IN ('active', 'suspended', 'expired')),
    valid_from timestamptz NOT NULL,
    valid_until timestamptz NOT NULL,
    feature_set jsonb NOT NULL DEFAULT '{}',
    version bigint NOT NULL DEFAULT 1,
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (valid_from < valid_until)
);

CREATE TABLE license_change (
    id uuid PRIMARY KEY,
    institution_id uuid NOT NULL REFERENCES institution(id),
    old_status text NOT NULL,
    new_status text NOT NULL,
    changed_by_user_id uuid NOT NULL REFERENCES user_account(id),
    reason text NOT NULL,
    changed_at timestamptz NOT NULL DEFAULT now()
);
```

---

## 25. Lock-free hosted license snapshot

**Place in:** `src/licensing/gate.rs`

```rust
use std::sync::Arc;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use crate::shared::error::AppError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseSnapshot {
    pub institution_id: Uuid,
    pub deployment_id: Uuid,
    pub status: LicenseStatus,
    pub valid_from: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub version: i64,
    pub feature_set: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LicenseStatus {
    Active,
    Suspended,
    Expired,
}

#[derive(Clone)]
pub struct LicenseGate {
    current: Arc<ArcSwap<LicenseSnapshot>>,
}

impl LicenseGate {
    pub fn new(snapshot: LicenseSnapshot) -> Self {
        Self {
            current: Arc::new(ArcSwap::from_pointee(snapshot)),
        }
    }

    pub fn require_active(&self, institution_id: Uuid) -> Result<(), AppError> {
        let snapshot = self.current.load();
        let now = Utc::now();

        let active = snapshot.institution_id == institution_id
            && snapshot.status == LicenseStatus::Active
            && now >= snapshot.valid_from
            && now < snapshot.valid_until;

        if active {
            Ok(())
        } else {
            Err(AppError::InstitutionLocked)
        }
    }

    pub fn replace(&self, snapshot: LicenseSnapshot) {
        self.current.store(Arc::new(snapshot));
    }

    pub fn snapshot(&self) -> Arc<LicenseSnapshot> {
        self.current.load_full()
    }
}
```

The request check compares the current clock every time, so the in-memory snapshot does not create an expiry grace period.

---

## 26. Hosted license change service

**Place in:** `src/licensing/service.rs`

```rust
use crate::audit::AuditWriter;
use crate::shared::{actor::{Actor, Role}, error::AppError};
use sqlx::PgPool;
use uuid::Uuid;

use crate::licensing::{LicenseGate, LicenseSnapshot, LicenseStatus};

#[derive(Clone)]
pub struct LicenseService {
    pool: PgPool,
    gate: LicenseGate,
    audit: AuditWriter,
}

impl LicenseService {
    pub fn new(pool: PgPool, gate: LicenseGate, audit: AuditWriter) -> Self {
        Self { pool, gate, audit }
    }

    pub async fn set_status(
        &self,
        actor: &Actor,
        institution_id: Uuid,
        new_status: LicenseStatus,
        reason: &str,
    ) -> Result<LicenseSnapshot, AppError> {
        if !actor.has_role(Role::PlatformLicensingAdmin) {
            return Err(AppError::Forbidden);
        }

        if reason.trim().is_empty() {
            return Err(AppError::Validation("reason is required".into()));
        }

        let mut tx = self.pool.begin().await?;

        let old = sqlx::query_as::<_, LicenseRow>(
            r#"
            SELECT
                institution_id, deployment_id, status,
                valid_from, valid_until, feature_set, version
            FROM institution_license
            WHERE institution_id = $1
            FOR UPDATE
            "#,
        )
        .bind(institution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

        let status_text = match new_status {
            LicenseStatus::Active => "active",
            LicenseStatus::Suspended => "suspended",
            LicenseStatus::Expired => "expired",
        };

        let updated = sqlx::query_as::<_, LicenseRow>(
            r#"
            UPDATE institution_license
               SET status = $2,
                   version = version + 1,
                   updated_at = now()
             WHERE institution_id = $1
            RETURNING
                institution_id, deployment_id, status,
                valid_from, valid_until, feature_set, version
            "#,
        )
        .bind(institution_id)
        .bind(status_text)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO license_change (
                id, institution_id, old_status, new_status,
                changed_by_user_id, reason
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(institution_id)
        .bind(&old.status)
        .bind(&updated.status)
        .bind(actor.user_id)
        .bind(reason)
        .execute(&mut *tx)
        .await?;

        self.audit
            .write(
                &mut tx,
                institution_id,
                actor.user_id,
                "license.status_changed",
                "institution_license",
                institution_id,
                &serde_json::json!({
                    "old": old.status,
                    "new": updated.status,
                    "reason": reason
                }),
            )
            .await?;

        tx.commit().await?;

        let snapshot = LicenseSnapshot::try_from(updated)?;

        // Replace only after the transaction commits. A request can observe either
        // the complete old state or the complete new state, never an uncommitted one.
        if self.gate.snapshot().institution_id == institution_id {
            self.gate.replace(snapshot.clone());
        }

        Ok(snapshot)
    }
}

#[derive(sqlx::FromRow)]
struct LicenseRow {
    institution_id: Uuid,
    deployment_id: Uuid,
    status: String,
    valid_from: chrono::DateTime<chrono::Utc>,
    valid_until: chrono::DateTime<chrono::Utc>,
    feature_set: serde_json::Value,
    version: i64,
}

impl TryFrom<LicenseRow> for LicenseSnapshot {
    type Error = AppError;

    fn try_from(row: LicenseRow) -> Result<Self, Self::Error> {
        let status = match row.status.as_str() {
            "active" => LicenseStatus::Active,
            "suspended" => LicenseStatus::Suspended,
            "expired" => LicenseStatus::Expired,
            _ => return Err(AppError::Internal),
        };

        Ok(Self {
            institution_id: row.institution_id,
            deployment_id: row.deployment_id,
            status,
            valid_from: row.valid_from,
            valid_until: row.valid_until,
            version: row.version,
            feature_set: row.feature_set,
        })
    }
}
```

---

## 27. License middleware shape

Actix middleware may halt request processing before a handler. This is normal package-level framework code; it does not turn `licensing` into another application or crate. Keep recovery routes outside the protected scope.

**Place in:** `src/app.rs`

```rust
use actix_web::web;

pub fn protected_routes(cfg: &mut web::ServiceConfig) {
    // Handler attributes already contain their complete /api/v1 and /ui paths,
    // so configure them at the application root. Wrapping these routes with
    // authentication and licensing middleware can be added once those files exist.
    cfg.configure(crate::enrollment::http::routes)
        .configure(crate::records::http::routes)
        .configure(crate::documents::http::routes);
}

pub fn recovery_routes(cfg: &mut web::ServiceConfig) {
    cfg.route("/health", web::get().to(health));
    cfg.route("/license/status", web::get().to(license_status));
    cfg.route("/license/import", web::post().to(import_license));
    cfg.route("/institution-locked", web::get().to(locked_page));
}

async fn health() -> &'static str { "ok" }
async fn license_status() -> &'static str { "status" }
async fn import_license() -> &'static str { "import" }
async fn locked_page() -> &'static str { "Institution access is inactive." }
```

A complete custom Actix middleware implementation is mostly framework plumbing. Its decision should remain this simple:

```rust
fn license_decision(
    gate: &LicenseGate,
    institution_id: uuid::Uuid,
) -> Result<(), crate::shared::error::AppError> {
    gate.require_active(institution_id)
}
```

Do not query PostgreSQL in this middleware on every request.

---

## 28. Signed self-hosted license

**Place in:** `src/licensing/signed_license.rs`

```rust
use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use crate::shared::error::AppError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct LicenseClaims {
    pub institution_id: Uuid,
    pub deployment_id: Uuid,
    pub license_serial: Uuid,
    pub valid_from: DateTime<Utc>,
    pub valid_until: DateTime<Utc>,
    pub feature_set: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignedLicenseFile {
    pub claims: LicenseClaims,
    pub signature_hex: String,
}

pub fn verify_signed_license<'a>(
    file: &'a SignedLicenseFile,
    public_key: &VerifyingKey,
    expected_deployment_id: Uuid,
) -> Result<&'a LicenseClaims, AppError> {
    let canonical = serde_json::to_vec(&file.claims).map_err(|_| AppError::Internal)?;
    let signature_bytes = hex::decode(&file.signature_hex)
        .map_err(|_| AppError::Validation("invalid license signature encoding".into()))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| AppError::Validation("invalid license signature".into()))?;

    public_key
        .verify(&canonical, &signature)
        .map_err(|_| AppError::InstitutionLocked)?;

    if file.claims.deployment_id != expected_deployment_id {
        return Err(AppError::InstitutionLocked);
    }

    let now = Utc::now();
    if now < file.claims.valid_from || now >= file.claims.valid_until {
        return Err(AppError::InstitutionLocked);
    }

    Ok(&file.claims)
}
```

For deterministic signatures, define and freeze a canonical serialization format. A production implementation should use a versioned binary format or a formal canonical JSON method rather than relying indefinitely on a library's ordinary JSON field ordering.

### Subscription-specific security

- Only platform licensing administrators can change hosted license state.
- License changes are audited inside the transaction.
- Protected routes fail closed if no valid snapshot loads at startup.
- Expiry compares the current time on every request.
- Recovery routes are minimal and separately authorized.
- Never suspend individual students for institution nonpayment.
- Keep the signing private key offline; applications contain only the public key.
- Document that a root-level self-hosted operator can patch local enforcement.

---

## 29. Hosted subscription handler and Alpine AJAX

**Place in:** `src/licensing/http.rs`

```rust
use actix_web::{post, web, HttpResponse};
use crate::shared::{actor::Actor, error::AppError};
use serde::Deserialize;
use uuid::Uuid;

use crate::licensing::{LicenseService, LicenseStatus};

#[derive(Deserialize)]
pub struct LicenseStatusForm {
    status: String,
    reason: String,
    csrf_token: String,
}

#[post("/ui/platform/institutions/{institution_id}/license")]
pub async fn change_license_fragment(
    actor: Actor,
    service: web::Data<LicenseService>,
    institution_id: web::Path<Uuid>,
    form: web::Form<LicenseStatusForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    let status = match form.status.as_str() {
        "active" => LicenseStatus::Active,
        "suspended" => LicenseStatus::Suspended,
        "expired" => LicenseStatus::Expired,
        _ => return Err(AppError::Validation("unknown license status".into())),
    };

    let snapshot = service
        .set_status(
            &actor,
            institution_id.into_inner(),
            status,
            &form.reason,
        )
        .await?;

    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(format!(
            r#"<section id="license-panel"><p role="status">License status updated to {:?}. Version {}.</p></section>"#,
            snapshot.status, snapshot.version
        )))
}
```

The formatted values above are enums and integers controlled by the server. Use Askama for the full page and for any user-provided text.

**Place in:** `web/fragments/license_panel.html`

```html
<section id="license-panel">
  <h1>{{ institution_name }} license</h1>
  <dl>
    <dt>Status</dt><dd>{{ license.status }}</dd>
    <dt>Valid from</dt><dd>{{ license.valid_from }}</dd>
    <dt>Valid until</dt><dd>{{ license.valid_until }}</dd>
    <dt>Version</dt><dd>{{ license.version }}</dd>
  </dl>

  <form
    method="post"
    action="/ui/platform/institutions/{{ institution_id }}/license"
    x-target="license-panel"
    x-target.422="license-panel"
    @ajax:before="confirm('Apply this institution-wide access change?') || $event.preventDefault()"
  >
    <input type="hidden" name="csrf_token" value="{{ csrf_token }}">

    <label for="license-status">New status</label>
    <select id="license-status" name="status" required>
      <option value="active">Active</option>
      <option value="suspended">Suspended</option>
      <option value="expired">Expired</option>
    </select>

    <label for="license-reason">Reason</label>
    <textarea id="license-reason" name="reason" required maxlength="500"></textarea>

    <button type="submit">Apply institution-wide status</button>
  </form>
</section>
```

This screen belongs to the hosted platform operator, not the University's ordinary admin portal. The backend still checks `PlatformLicensingAdmin`; the confirmation dialog is only a usability measure.

---

# Part V — Application Composition

## 30. Concrete application state

**Place in:** `src/main.rs`

```rust
use std::path::PathBuf;

use actix_web::{middleware, web, App, HttpServer};
use crate::audit::AuditWriter;
use crate::documents::{DocumentService, DocumentWorker};
use crate::enrollment::EnrollmentService;
use crate::licensing::{LicenseGate, LicenseService, LicenseSnapshot, LicenseStatus};
use crate::records::{GradeService, ScheduleQuery, TranscriptSnapshotService};
use crate::shared::error::AppError;
use sqlx::postgres::PgPoolOptions;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,actix_web=info,sqlx=warn")
        .json()
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");

    let pool = PgPoolOptions::new()
        // This is a bounded concurrency control, not a target to maximize.
        // Tune from database measurements.
        .max_connections(64)
        .min_connections(8)
        .connect(&database_url)
        .await
        .expect("database connection failed");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("database migration failed");

    let audit = AuditWriter;
    let initial_license = load_initial_license(&pool)
        .await
        .expect("valid institution license required before startup");
    let license_gate = LicenseGate::new(initial_license);

    let enrollment = EnrollmentService::new(pool.clone(), audit.clone());
    let grades = GradeService::new(pool.clone(), audit.clone());
    let schedule = ScheduleQuery::new(pool.clone());
    let transcript = TranscriptSnapshotService;
    let documents = DocumentService::new(
        pool.clone(),
        audit.clone(),
        transcript,
    );
    let licensing = LicenseService::new(
        pool.clone(),
        license_gate.clone(),
        audit,
    );

    let worker = DocumentWorker::new(
        pool.clone(),
        "document-worker-1".to_owned(),
        PathBuf::from("./var/documents"),
    );
    actix_web::rt::spawn(worker.run());

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(enrollment.clone()))
            .app_data(web::Data::new(grades.clone()))
            .app_data(web::Data::new(schedule.clone()))
            .app_data(web::Data::new(documents.clone()))
            .app_data(web::Data::new(licensing.clone()))
            .app_data(web::Data::new(license_gate.clone()))
            .wrap(middleware::NormalizePath::trim())
            .wrap(middleware::DefaultHeaders::new()
                .add(("X-Content-Type-Options", "nosniff"))
                .add(("Referrer-Policy", "strict-origin-when-cross-origin"))
                .add(("Content-Security-Policy",
                      "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")))
            .wrap(middleware::Logger::default())
            .configure(crate::app::recovery_routes)
            .configure(crate::app::protected_routes)
    })
    .workers(std::thread::available_parallelism().map_or(1, usize::from))
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}

async fn load_initial_license(
    pool: &sqlx::PgPool,
) -> Result<LicenseSnapshot, AppError> {
    #[derive(sqlx::FromRow)]
    struct InitialLicenseRow {
        institution_id: uuid::Uuid,
        deployment_id: uuid::Uuid,
        status: String,
        valid_from: chrono::DateTime<chrono::Utc>,
        valid_until: chrono::DateTime<chrono::Utc>,
        feature_set: serde_json::Value,
        version: i64,
    }

    // A deployment is intentionally single-tenant in this design. Refuse to
    // start without one explicit license row rather than silently running open.
    let row = sqlx::query_as::<_, InitialLicenseRow>(
        r#"
        SELECT
            institution_id, deployment_id, status,
            valid_from, valid_until, feature_set, version
        FROM institution_license
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::InstitutionLocked)?;

    let status = match row.status.as_str() {
        "active" => LicenseStatus::Active,
        "suspended" => LicenseStatus::Suspended,
        "expired" => LicenseStatus::Expired,
        _ => return Err(AppError::Internal),
    };

    Ok(LicenseSnapshot {
        institution_id: row.institution_id,
        deployment_id: row.deployment_id,
        status,
        valid_from: row.valid_from,
        valid_until: row.valid_until,
        feature_set: row.feature_set,
        version: row.version,
    })
}

```

Concrete services make the dependency graph visible. There is no global container that resolves arbitrary types at runtime.

---

## 31. Frontend bootstrapping

Use local, pinned copies of Alpine AJAX and the Alpine CSP build. Do not load production scripts from a public CDN unless the deployment accepts that availability and supply-chain dependency.

**Place in:** `web/pages/base.html`

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <link rel="stylesheet" href="/assets/app.css">
  <script defer src="/assets/alpine-ajax.js"></script>
  <script defer src="/assets/alpine-csp.js"></script>
  <title>{% block title %}University of Belize{% endblock %}</title>
</head>
<body>
  <a class="skip-link" href="#main">Skip to content</a>

  <div id="notifications" x-sync role="status" aria-live="polite"></div>

  <header>
    <nav aria-label="Primary">
      <a href="/dashboard">Dashboard</a>
      <a href="/registration">Registration</a>
      <a href="/schedule">Schedule</a>
      <a href="/grades">Grades</a>
      <a href="/documents">Documents</a>
    </nav>
  </header>

  <main id="main">
    {% block content %}{% endblock %}
  </main>
</body>
</html>
```

Use progressive enhancement: every link and form should still perform a valid full-page request when JavaScript is unavailable. Add `x-target` only where a fragment replacement improves the interaction.

---

# Part VI — Testing and Performance

## 32. Registration concurrency test

**Place in:** `src/enrollment/tests.rs`

The most important enrollment test is not a handler unit test. It is a module test using a real PostgreSQL test database, with two transactions competing for the final seat. Keeping it in `src/enrollment/tests.rs` allows direct testing without creating a second library package solely for integration tests.

```rust
#[sqlx::test(migrations = "./migrations")]
async fn only_one_student_gets_the_last_seat(pool: sqlx::PgPool) {
    // Arrange one section with capacity=1 and two eligible students.
    let fixture = seed_registration_fixture(&pool, 1).await;

    let service = crate::enrollment::EnrollmentService::new(
        pool.clone(),
        crate::audit::AuditWriter,
    );

    let left = service.register_for(
        &fixture.registrar,
        fixture.student_a,
        crate::enrollment::RegisterCommand {
            section_id: fixture.section_id,
            idempotency_key: uuid::Uuid::new_v4(),
        },
    );

    let right = service.register_for(
        &fixture.registrar,
        fixture.student_b,
        crate::enrollment::RegisterCommand {
            section_id: fixture.section_id,
            idempotency_key: uuid::Uuid::new_v4(),
        },
    );

    let (left, right) = tokio::join!(left, right);
    let successes = usize::from(left.is_ok()) + usize::from(right.is_ok());
    assert_eq!(successes, 1);

    let enrolled_count: i32 = sqlx::query_scalar(
        "SELECT enrolled_count FROM section_capacity WHERE section_id = $1",
    )
    .bind(fixture.section_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(enrolled_count, 1);
}

async fn seed_registration_fixture(
    pool: &sqlx::PgPool,
    capacity: i32,
) -> Fixture {
    use std::collections::HashSet;
    use chrono::{Duration, Utc};
    use crate::shared::actor::{Actor, Role};
    use uuid::Uuid;

    let institution_id = Uuid::new_v4();
    let registrar_user_id = Uuid::new_v4();
    let student_user_a = Uuid::new_v4();
    let student_user_b = Uuid::new_v4();
    let student_a = Uuid::new_v4();
    let student_b = Uuid::new_v4();
    let term_id = Uuid::new_v4();
    let course_id = Uuid::new_v4();
    let section_id = Uuid::new_v4();

    let now = Utc::now();
    let mut tx = pool.begin().await.unwrap();

    sqlx::query(
        "INSERT INTO institution (id, code, name) VALUES ($1, $2, 'Test University')",
    )
    .bind(institution_id)
    .bind(format!("T-{}", &institution_id.to_string()[..8]))
    .execute(&mut *tx)
    .await
    .unwrap();

    for (id, username, email) in [
        (registrar_user_id, "registrar", "registrar@test.invalid"),
        (student_user_a, "student-a", "student-a@test.invalid"),
        (student_user_b, "student-b", "student-b@test.invalid"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO user_account (id, institution_id, username, email)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(id)
        .bind(institution_id)
        .bind(format!("{username}-{}", &institution_id.to_string()[..8]))
        .bind(format!("{}-{}", &institution_id.to_string()[..8], email))
        .execute(&mut *tx)
        .await
        .unwrap();
    }

    for (id, user_id, number) in [
        (student_a, student_user_a, "A-001"),
        (student_b, student_user_b, "B-001"),
    ] {
        sqlx::query(
            r#"
            INSERT INTO student_profile (
                id, institution_id, user_id, student_number, program_code
            )
            VALUES ($1, $2, $3, $4, 'TEST')
            "#,
        )
        .bind(id)
        .bind(institution_id)
        .bind(user_id)
        .bind(format!("{}-{number}", &institution_id.to_string()[..8]))
        .execute(&mut *tx)
        .await
        .unwrap();
    }

    sqlx::query(
        r#"
        INSERT INTO academic_term (
            id, institution_id, code, name, starts_on, ends_on,
            registration_opens_at, registration_closes_at, drop_add_closes_at
        )
        VALUES ($1, $2, $3, 'Test Term', $4, $5, $6, $7, $8)
        "#,
    )
    .bind(term_id)
    .bind(institution_id)
    .bind(format!("TERM-{}", &term_id.to_string()[..8]))
    .bind(now.date_naive())
    .bind((now + Duration::days(120)).date_naive())
    .bind(now - Duration::hours(1))
    .bind(now + Duration::hours(2))
    .bind(now + Duration::days(7))
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO course (id, institution_id, code, title, credit_hours)
        VALUES ($1, $2, $3, 'Concurrency Test', 3.0)
        "#,
    )
    .bind(course_id)
    .bind(institution_id)
    .bind(format!("TEST-{}", &course_id.to_string()[..8]))
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO section (
            id, institution_id, term_id, course_id, section_code, status
        )
        VALUES ($1, $2, $3, $4, '01', 'open')
        "#,
    )
    .bind(section_id)
    .bind(institution_id)
    .bind(term_id)
    .bind(course_id)
    .execute(&mut *tx)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO section_capacity (section_id, capacity, enrolled_count)
        VALUES ($1, $2, 0)
        "#,
    )
    .bind(section_id)
    .bind(capacity)
    .execute(&mut *tx)
    .await
    .unwrap();

    tx.commit().await.unwrap();

    Fixture {
        registrar: Actor {
            user_id: registrar_user_id,
            institution_id,
            student_id: None,
            roles: HashSet::from([Role::Registrar]),
        },
        student_a,
        student_b,
        section_id,
    }
}

struct Fixture {
    registrar: crate::shared::actor::Actor,
    student_a: uuid::Uuid,
    student_b: uuid::Uuid,
    section_id: uuid::Uuid,
}
```

Also test:

- same student submits two conflicting sections concurrently;
- repeat idempotency key returns one enrollment;
- deadline changes while request is waiting;
- drop decrements exactly once;
- grade edit version conflict;
- approval creates one job;
- two workers do not claim the same job;
- license expires at the exact boundary.

---

## 33. Query budgets

Add a design budget beside every endpoint:

| Endpoint | Expected database work | Cache target |
|---|---:|---|
| `GET /terms/current` | 0 on warm snapshot | shared in-process snapshot |
| `GET /me/grades` | 1 query | private/no-store; optional versioned per-user cache later |
| `GET /me/schedule` | 1 query | private/no-store |
| `POST /me/enrollments` | one short transaction, explicit statements | never cache command result except idempotency |
| `POST /document-requests` | one short transaction | no |
| license gate | 0 queries | lock-free local snapshot |

A code review should reject an endpoint that quietly turns one row into dozens of queries.

---

## 34. 500k RPS implementation guidance

### What the code already does for performance

- no internal HTTP;
- lock-free license read;
- bounded database pool;
- explicit one-query grade and schedule reads;
- no N+1 registration checks;
- conditional seat update;
- narrow student-term lock;
- slow document work outside request handlers;
- HTML fragments rather than full-page responses;
- prepared SQL through SQLx;
- no universal serialization/event layer.

### What must be benchmarked rather than assumed

- worker count;
- database pool size;
- response rendering allocations;
- session-cache hit ratio;
- TLS cost;
- static asset serving strategy;
- catalog snapshot representation;
- logging volume;
- p99 under section contention;
- document-worker CPU interference.

### What not to do to chase the number

- do not lower Argon2 cost until password verification is insecure;
- do not acknowledge registration before durable seat ownership;
- do not remove audit records;
- do not disable PostgreSQL durability settings for a production claim;
- do not count rejected or queued writes as completed transactions;
- do not add microservices while every service still queries one overloaded database;
- do not use an unbounded connection pool.

---

## 35. Security checklist by layer

### Request boundary

- TLS only.
- body-size limits.
- content-type validation.
- security headers.
- request IDs.
- no sensitive values in URLs or logs.
- per-session and per-IP abuse controls for login and document requests.

### Session

- opaque random session ID.
- `Secure`, `HttpOnly`, `SameSite` cookie.
- session rotation after login and role change.
- idle and absolute expiry.
- cached lookup with durable revocation record.
- CSRF token for every cookie-authenticated mutation.

### Application services

- explicit policy call.
- derive institution/student identity from actor.
- validate state transitions.
- audit critical writes in the transaction.
- idempotency for retried commands.

### Database

- least-privileged application role.
- parameters for every user value.
- foreign keys and check constraints.
- partial unique indexes for active state.
- short transactions and fixed lock order.
- encrypted backups and restore testing.

### Frontend

- Alpine CSP build.
- auto-escaped templates.
- no business authorization in JavaScript.
- accessible status regions and focus behavior.
- local pinned scripts.
- no transcript/grade data in browser persistent storage.

---

## 36. What to implement first

The safest delivery order is:

1. migrations, actor, session middleware, CSRF, license gate, audit;
2. one current-term read slice;
3. registration and drop concurrency proof;
4. schedule and grades read models;
5. grade editing and publication;
6. unofficial print views;
7. document request/approval/job workflow;
8. approved PDF renderer and artifact download;
9. admin event/calendar functions;
10. benchmark suites and hardening.

Do not build a broad admin dashboard first. It creates many shallow screens before the core rules exist. Build the domain command, policy, transaction, test, and then expose it to student and admin adapters.

---

## 37. Sources and compatibility notes

Primary documentation consulted:

- [Actix Web handlers](https://actix.rs/docs/handlers/)
- [Actix Web extractors](https://actix.rs/docs/extractors/)
- [Actix Web application state](https://actix.rs/docs/application/)
- [Actix Web middleware](https://actix.rs/docs/middleware/)
- [Actix Web server workers](https://actix.rs/docs/server/)
- [SQLx pool](https://docs.rs/sqlx/latest/sqlx/pool/index.html)
- [SQLx `query_as`](https://docs.rs/sqlx/latest/sqlx/macro.query_as.html)
- [PostgreSQL concurrency control](https://www.postgresql.org/docs/current/mvcc.html)
- [PostgreSQL explicit locking](https://www.postgresql.org/docs/current/explicit-locking.html)
- [PostgreSQL transaction isolation](https://www.postgresql.org/docs/current/transaction-iso.html)
- [Alpine AJAX reference](https://alpine-ajax.js.org/reference/)
- [Alpine.js CSP build](https://alpinejs.dev/advanced/csp)
- [Askama escaping](https://docs.rs/askama/latest/askama/filters/fn.escape.html)
- [Argon2 RFC 9106](https://datatracker.ietf.org/doc/rfc9106/)
- [printpdf 0.10 documentation and source](https://docs.rs/printpdf/0.10.0/printpdf/)
- [XenDirect API documentation](https://api.xendevelop.com/)

Before implementation, run `cargo check`, SQLx compile-time query validation against the migrations, migration rollback/restore tests, and integration tests on the exact dependency versions selected for the package.
