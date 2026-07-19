# University of Belize Education Platform
## System Design & Architecture — Worked Example from First Principles

**Status:** Demo architecture, designed to preserve a production-quality codebase while product scope is still being validated  
**Backend:** Rust, Actix Web, SQLx, PostgreSQL  
**Frontend:** HTML, Alpine.js CSP build, Alpine AJAX, CSS  
**Architecture:** One Cargo package and one Actix binary, organized into feature directories; separate frontend source boundary; one application server and one database server for the stated deployment constraint  
**Target delivery:** 3–6 months for a complete demo  
**Account population:** 25,000–50,000 initially; possible long-term population of 440,000  
**Active-user assumption used in this document:** 5,000–20,000 simultaneously active users during normal and registration-period operation

---

## 1. Executive decision

Build one Cargo package that produces one backend binary. Organize its source into seven business-capability directories:

1. `identity_access`
2. `institution`
3. `academics`
4. `enrollment`
5. `records`
6. `documents`
7. `licensing`

These are ordinary Rust modules declared from the same crate, not workspace members and not independently versioned libraries. Add two supporting source areas:

- `audit`: append-only evidence written inside critical transactions.
- `jobs`: a PostgreSQL-backed work queue for document generation and other slow side effects.

Do **not** create an `admin` domain module. “Admin” is a user interface and a set of permissions over existing business capabilities. An `admin` module becomes a dumping ground and eventually bypasses the rules owned by enrollment, records, documents, and licensing.

Directories communicate through concrete, typed Rust functions and service structs in the same process. They do not call each other over HTTP. They do not publish every action to an event bus. They do not share generic repositories. Each feature directory owns its writes and exposes a deliberately small surface from its `mod.rs`. Private files remain private to that Rust module.

For the demo, keep the frontend in a separate source tree but package its static assets and templates with the application deployment. This preserves a clean frontend/backend ownership boundary without introducing a mandatory extra network hop on every interaction. The public JSON API and the HTML-fragment endpoints are adapters over the same application services.

The 500,000 request/second constraint must be treated honestly:

- A single application server may be able to accept and answer a very large number of small, locally served requests on sufficiently specialized hardware.
- It cannot truthfully promise 500,000 fully durable, independent PostgreSQL-backed operations per second across registration, drop/add, document requests, and other write-heavy features on an unspecified single database server.
- The architecture therefore separates **local request processing**, **cacheable reads**, and **durable transactional writes**, then benchmarks each separately.
- No code structure can turn 500,000 required database round trips per second into a free operation.

The correct delivery decision is to preserve the target as a benchmark objective while refusing to weaken correctness, password hashing, authorization, enrollment invariants, or durability to manufacture a misleading number.

---

## 2. Normalize the problem before designing it

### 2.1 Write the constraints as equations

Let:

- `R = 500,000 requests/second`
- `F = 5 major feature families`
- `R_feature = R / F = 100,000 requests/second per feature`, because the prompt says load is distributed equally
- `Q = average database round trips per request`

Then:

```text
required_database_round_trips_per_second = R × Q
```

Even at the idealized value `Q = 1`:

```text
500,000 requests/s × 1 round trip/request = 500,000 DB round trips/s
```

Registration and document-request commands are not simple cached reads. They require transactions, constraints, row updates, audit records, and often more than one SQL statement. A naive implementation with six statements per registration would imply:

```text
100,000 registration requests/s × 6 statements = 600,000 statements/s
```

That is before drop/add, grades, schedules, documents, sessions, and audit traffic are included.

**Conclusion:** the first design task is not choosing a Rust trait. It is reducing work per request and defining which work must be durable before a response can be returned.

### 2.2 Separate account count, concurrency, and request rate

These are different dimensions:

- **Registered accounts:** 25,000–50,000, potentially 440,000.
- **Concurrent active users:** assumed 5,000–20,000 for design and demo testing.
- **Request rate:** 500,000 requests/second.

If 20,000 active users produce 500,000 requests/second, each active user averages:

```text
500,000 / 20,000 = 25 requests per second
```

That is not a normal human-driven university portal workload. It is a stress or synthetic requirement. It remains a valid engineering challenge, but the benchmark must state the request mix, response sizes, cache state, hardware, durability expectations, and acceptable error rate. Otherwise, “500k RPS” is not a testable requirement.

### 2.3 Define three performance classes

| Class | Examples | Required behavior | Main bottleneck |
|---|---|---|---|
| Local gate | license check, session cache hit, routing, authorization from cached claims | entirely in process | CPU, memory bandwidth, locks, allocations |
| Read path | course catalog, holidays, shared section metadata, cached schedule snapshot | preferably zero or one database access; conditional responses | serialization, cache locality, network |
| Transaction path | register, drop, grade submission, document request, approval | durable transaction with exact invariants | database locks, WAL, indexes, contention |

The architecture may target 500k RPS for selected local and cached read paths. It must separately report sustainable transactional command throughput. Combining these numbers would be dishonest.

---

## 3. Apply the code hierarchy

The required hierarchy is used as a decision filter at every layer.

### Level 1 — Best clear way to make the feature correct

Examples:

- A registration command owns one transaction.
- Unique constraints prevent duplicate enrollment.
- A student-term row is locked to serialize concurrent changes for the same student.
- A section seat counter is atomically incremented only when capacity remains.
- Authorization occurs in the application service, not only in the button visibility.

### Level 2 — Choose the correct algorithm and data shape

Examples:

- Do not calculate remaining seats with `COUNT(*)` for every registration attempt.
- Maintain an explicit capacity row and update it conditionally.
- Do not load every enrollment and detect time conflicts in Rust.
- Ask PostgreSQL whether an overlap exists using indexed meeting intervals.
- Do not regenerate an unchanged transcript on every print.
- Generate from an immutable snapshot and reuse the resulting artifact.

### Level 3 — Remove pessimization and make small measured optimizations

Examples:

- Use prepared SQL through SQLx.
- Avoid N+1 queries.
- Return only fields needed by the fragment.
- Avoid cloning large models between layers.
- Keep the hot license snapshot lock-free.
- Bound request payloads and database concurrency.
- Cache immutable or versioned shared data in process.

### Level 4 — Change infrastructure only after code warrants it

Examples that are deliberately deferred:

- Read replicas.
- Redis.
- Kafka.
- Microservices.
- Distributed caches.
- Sharding.
- Separate document workers.

If one clear SQL statement and a correct index are missing, adding Kubernetes is not optimization. It is moving the same mistake into more machines.

---

## 4. Find natural module seams step by step

A useful module boundary encloses rules that:

1. change for the same business reason;
2. require the same transaction;
3. use the same vocabulary;
4. should have one owner for writes;
5. can be tested without knowing the entire application.

### Step 1 — List the invariants, not the screens

#### Identity and access invariants

- A session belongs to one user and one institution context.
- A suspended user cannot authenticate.
- A user may have multiple roles.
- A student can read only their own student record unless granted an elevated role.
- An instructor can change grades only for assigned sections and only within allowed periods.

#### Academic structure invariants

- A section belongs to one course and one academic term.
- Section meetings have valid times, rooms, and days.
- Prerequisite rules belong to the course or offering.
- Academic term dates determine registration and drop/add windows.

#### Enrollment invariants

- A student cannot have two active enrollments in the same section.
- A section cannot exceed capacity.
- A student cannot register outside the allowed period.
- A student cannot register with an unresolved schedule conflict unless an authorized override exists.
- Drop/add changes are serialized for one student and term.

#### Records invariants

- A grade belongs to an enrollment.
- Grade changes are attributable and audited.
- Published grades are student-visible; draft grades are not.
- A transcript represents an immutable academic-history snapshot at a specific point in time.

#### Document invariants

- A request has a type, owner, status, and audit history.
- Approval and generation are distinct states.
- An official document is generated from an immutable snapshot.
- An approved artifact cannot silently change after issue.

#### Licensing invariants

- Access is granted or denied at the institution level.
- Individual students are not independently disabled because of subscription state.
- Expiry has no grace period in the stated policy.
- Recovery and license-management routes remain available when the institution is locked.

### Step 2 — Compare boundary strategies

#### Strategy A: module per screen

Examples: `registration_page`, `grade_page`, `admin_page`, `documents_page`.

**Gain:** initially fast navigation from ticket to folder.  
**Loss:** duplicate rules, screen-specific database access, and no single owner of enrollment or grade invariants.  
**Timeline effect:** appears fast for four weeks, then creates rework when the same rule is needed in student, instructor, and admin screens.

#### Strategy B: module per table/entity

Examples: `course`, `section`, `student`, `grade`, `event`, `request`.

**Gain:** folders match nouns and tables.  
**Loss:** business actions span many tiny modules, creating chatty calls and cycles.  
**Timeline effect:** too much ceremony for a 3–6 month demo.

#### Strategy C: technical layers across the whole system

Examples: one global `handlers`, `services`, `repositories`, and `models` folder.

**Gain:** familiar initially.  
**Loss:** all features depend on all layers; module ownership disappears; large files become routing tables.  
**Timeline effect:** low start-up cost, high merge conflicts and growing cognitive load.

#### Strategy D: business-capability directories with vertical slices

Examples: `enrollment`, `records`, and `documents`, each containing its handlers, service, SQL, types, and tests inside the same Cargo package.

**Gain:** rules and writes have an owner; related changes stay together; cross-feature dependencies are visible without introducing package-management ceremony.  
**Loss:** boundaries are enforced by Rust module visibility and team discipline rather than separate crates.  
**Timeline effect:** small organization cost in week one, substantially less rework in months two through six.

**Selected:** Strategy D, implemented as directories and Rust modules in one crate.

### Step 3 — Choose boundaries coarse enough for the timeline

A directory per use case would be too fine. One giant “academics” directory would be too broad. The selected balance is below. Every row maps to one folder under `src/`; it does not imply a Cargo crate or separate deployable.

| Module | Owns | Does not own |
|---|---|---|
| `identity_access` | users, credentials, sessions, roles, actor extraction, policy primitives | academic rules |
| `institution` | institution profile, campuses, rooms, calendar events, holidays, term-level operational settings | enrollment decisions |
| `academics` | terms, courses, sections, meetings, prerequisites, instructor assignments | student enrollment state and final records |
| `enrollment` | student term registration, active enrollment, drop/add, capacity counters, overrides | course definitions, grade publication |
| `records` | grade records, grade publication, academic history, transcript snapshots | PDF generation and request workflow |
| `documents` | document requests, approval, generation jobs, artifacts, print views | grade calculation and enrollment rules |
| `licensing` | institution license state, signed self-hosted license verification, kill-switch decisions | user suspension and academic authorization |

`audit` is a supporting component because every module may write evidence, but audit does not decide business behavior.

### Step 4 — Explain why this works at 5k–20k active users

The active-user count does not require distributed modules. The dominant challenges are:

- registration contention for popular sections;
- correct permission checks;
- efficient schedule and grade reads;
- document generation isolation;
- predictable database access.

All are simpler with in-process calls and one transaction boundary. A modular monolith eliminates network serialization and distributed failure modes while still allowing each capability to own its rules.

At 440,000 accounts, the same boundaries remain valid. Data volume may require better indexes, table partitioning, read models, and eventually deployment changes, but it does not require redefining “enrollment” or “records.”

---

## 5. Dependency rules

The purpose of these rules is readability, not package isolation. There is one `Cargo.toml` and one dependency graph. Rust's module system supplies enough control for the demo: keep implementation files private, re-export only the types and functions another directory actually needs, and review cross-feature imports during code review.

### 5.1 Allowed dependency direction

```text
HTTP/UI adapters
    |
    v
Application command/query functions
    |
    +--> identity_access
    +--> institution
    +--> academics
    +--> enrollment ----reads----> academics
    +--> records -------reads----> academics, enrollment
    +--> documents -----reads----> records
    +--> licensing
    +--> audit
    +--> jobs
```

### 5.2 Rules

1. A feature may call another feature's public function or service re-exported from that feature's `mod.rs`.
2. A feature may not update another feature's tables directly.
3. A module may use a cross-module read view when the view is explicitly owned and versioned.
4. Handlers may not contain business rules.
5. Repositories may not authorize users.
6. A database transaction is owned by the command whose invariant it protects.
7. No module-to-module HTTP calls inside the monolith.
8. No generic `Repository<T>` abstraction.
9. No trait is introduced merely to “decouple” concrete code. Add a trait when there are multiple real implementations, an external boundary, or a test substitution with clear value.
10. No global service locator. `AppState` contains concrete module entry points.

### 5.3 Synchronous calls versus events

Use a direct call when the caller needs the result to decide whether the transaction succeeds.

Examples:

- enrollment asks academics for term and section rules;
- documents asks records to create a transcript snapshot;
- grade submission asks academics whether the actor is assigned to the section.

Use an outbox/job when the action may happen after the durable command succeeds.

Examples:

- generate a PDF;
- send a notification;
- rebuild a shared read snapshot;
- export a batch report.

Do not use an event bus for the core registration decision. A seat either belongs to the student when the command returns success or it does not.

---

## 6. Recommended project structure

```text
ub-platform/
├── Cargo.toml                         # one package; Actix and SQLx declared once
├── src/
│   ├── main.rs                        # composition root and process startup
│   ├── app.rs                         # AppState and Actix route composition
│   ├── config.rs
│   ├── db.rs
│   ├── shared/                        # very small technical primitives only
│   │   ├── mod.rs
│   │   ├── actor.rs
│   │   ├── error.rs
│   │   ├── ids.rs
│   │   └── pagination.rs
│   ├── identity_access/
│   │   ├── mod.rs                     # public surface for authentication
│   │   ├── middleware.rs              # Actix-specific session extraction
│   │   ├── service.rs                 # ordinary Rust authentication logic
│   │   ├── sessions.rs
│   │   ├── password.rs
│   │   ├── queries.rs
│   │   └── types.rs
│   ├── institution/
│   │   ├── mod.rs
│   │   ├── http.rs
│   │   ├── service.rs
│   │   ├── queries.rs
│   │   └── types.rs
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
│   ├── documents/
│   ├── licensing/
│   │   ├── mod.rs
│   │   ├── middleware.rs
│   │   ├── service.rs
│   │   ├── signed_license.rs
│   │   ├── queries.rs
│   │   └── types.rs
│   ├── audit.rs
│   └── jobs/
│       ├── mod.rs
│       └── worker.rs
├── web/                               # HTML, Alpine, Alpine AJAX, and CSS
│   ├── pages/
│   ├── fragments/
│   ├── assets/
│   └── components/
├── migrations/
│   ├── 0001_foundation.sql
│   ├── 0002_academics.sql
│   ├── 0003_enrollment.sql
│   ├── 0004_records.sql
│   ├── 0005_documents.sql
│   └── 0006_licensing.sql
└── load/
    ├── cached_reads.js
    ├── registration_contention.js
    └── mixed_workload.js
```

This is a modular monolith in the practical sense: one package, one binary, one process, and one database, with code grouped by business capability. There is no Cargo workspace and no dependency declaration per directory.

A directory may contain an Actix-facing file such as `http.rs` or `middleware.rs`. That does not give the directory its own Actix dependency; `actix-web` is declared once for the entire package. Keep framework types in those boundary files where convenient, while `service.rs`, `policy.rs`, and most `types.rs` files use ordinary Rust and SQLx types.

Each `mod.rs` should expose only the intended entry points:

```rust
// src/enrollment/mod.rs
mod policy;
mod queries;
mod service;
mod types;
pub(crate) mod http;

pub use service::EnrollmentService;
pub use types::{DropCommand, EnrollmentReceipt, RegisterCommand};
```

Other features can use `EnrollmentService`, but they cannot reach private query helpers or policy implementation details. This is enough isolation for a small team and is easier to refactor during a 3–6 month demo than guessed crate boundaries.

---

## 7. Data model

All institution-owned tables include `institution_id`, even if the first deployment serves only the University of Belize. This is required by the school-level subscription model and prevents a later multi-institution conversion from rewriting every key.

Use UUIDs for externally visible identifiers. Internal hot tables may add compact generated keys later if measurement proves UUID index locality is a problem. Do not preemptively maintain two identifier systems.

### 7.1 Foundation

```text
institution
  id
  code
  name
  status
  timezone

user_account
  id
  institution_id
  username
  email
  status
  session_version

password_credential
  user_id
  password_hash
  changed_at

role
  id
  code

user_role
  institution_id
  user_id
  role_id
  scope_type
  scope_id

session
  id
  institution_id
  user_id
  expires_at
  last_seen_at
  csrf_secret_hash
  revoked_at
```

### 7.2 Institution calendar and facilities

```text
campus
  id
  institution_id
  code
  name

room
  id
  institution_id
  campus_id
  code
  capacity

institution_event
  id
  institution_id
  title
  event_type: holiday | academic | public | administrative
  starts_at
  ends_at
  location nullable
  audience
  status
  created_by
  updated_at
```

Events and holidays stay in `institution`, not `academics`, because they are institution-wide operational facts consumed by schedules, dashboards, and the admin portal. A section meeting may reference a room, but it does not own the room.

### 7.3 Academic structure

```text
academic_term
  id
  institution_id
  code
  name
  starts_on
  ends_on
  registration_opens_at
  registration_closes_at
  drop_add_closes_at
  grade_entry_closes_at

course
  id
  institution_id
  code
  title
  credit_hours
  active

course_prerequisite
  course_id
  prerequisite_course_id
  minimum_grade

section
  id
  institution_id
  term_id
  course_id
  section_code
  capacity
  status

section_capacity
  section_id
  enrolled_count
  version

section_meeting
  id
  section_id
  day_of_week
  starts_at
  ends_at
  room_id

instructor_assignment
  section_id
  instructor_user_id
  role
```

### 7.4 Enrollment

```text
student_profile
  id
  institution_id
  user_id
  student_number
  program_code
  academic_status

student_term_registration
  student_id
  term_id
  status
  hold_flags
  updated_at

enrollment
  id
  institution_id
  student_id
  section_id
  status
  registered_at
  dropped_at
  source
  idempotency_key

registration_override
  id
  student_id
  term_id
  section_id nullable
  type
  granted_by
  expires_at
```


### 7.5 Records

```text
grade_record
  id
  institution_id
  enrollment_id
  grade_code
  numeric_value nullable
  state: draft | published | amended
  entered_by
  published_at
  version

transcript_snapshot
  id
  institution_id
  student_id
  snapshot_version
  snapshot_json
  created_at
  content_hash
```

`transcript_snapshot.snapshot_json` is intentionally immutable. It freezes the facts used to produce a document. Normal operational reads remain relational; the JSON snapshot is not the primary grade store.

### 7.6 Documents

```text
document_request
  id
  institution_id
  student_id
  document_type
  status
  requested_at
  purpose nullable
  delivery_method
  current_snapshot_id nullable

document_approval
  id
  request_id
  decision
  decided_by
  decided_at
  note nullable

document_job
  id
  request_id
  job_type
  status
  attempts
  available_at
  locked_at nullable
  locked_by nullable

generated_document
  id
  request_id
  snapshot_id
  content_hash
  storage_path
  mime_type
  size_bytes
  issued_at
  superseded_at nullable
```

### 7.7 Licensing

```text
institution_contract
  institution_id
  contract_reference
  billing_model: annual | contractual
  deployment_mode: hosted | self_hosted
  currency_code
  software_fee_minor
  hosting_fee_minor nullable
  installation_fee_minor nullable
  starts_at
  ends_at

institution_license
  institution_id
  deployment_id
  mode: hosted | self_hosted
  status: active | suspended | expired
  valid_from
  valid_until
  feature_set
  version
  updated_at

license_change
  id
  institution_id
  old_status
  new_status
  changed_by
  changed_at
  reason
```

---

## 8. Database ownership and schema strategy

For the demo, use one PostgreSQL database and one migration stream. Prefixing every table with a module name adds noise without strong isolation. PostgreSQL schemas can help ownership, but they also complicate SQL search paths and migrations.

Recommended compromise:

- Use one database and the default schema for the demo.
- Name tables unambiguously.
- Keep each module's migration sections together.
- Enforce write ownership in code review and tests.
- Add PostgreSQL schemas only when multiple teams or external integrations need database-level ownership boundaries.

The database remains the final enforcer for invariants that can be represented as constraints:

- unique active enrollment per student and section;
- valid statuses through check constraints or enums;
- foreign keys;
- non-negative capacities;
- unique course codes per institution;
- one current document artifact per request where required.

Application validation improves errors. Database constraints guarantee truth under concurrency.

---

## 9. API boundaries

Expose commands and task-focused queries, not tables.

### Identity

```text
POST /api/v1/session/login
POST /api/v1/session/logout
GET  /api/v1/me
```

### Academic catalog and schedule

```text
GET /api/v1/terms/current
GET /api/v1/terms/{term_id}/sections
GET /api/v1/me/schedule?term_id={term_id}
GET /api/v1/me/grades?term_id={term_id}
```

### Enrollment

```text
GET  /api/v1/me/registration?term_id={term_id}
POST /api/v1/me/enrollments
POST /api/v1/me/enrollments/{enrollment_id}/drop
```

The command body for registration contains `section_id` and an idempotency key. Do not expose `POST /enrollment-table-row` semantics.

### Documents

```text
GET  /api/v1/me/documents
GET  /api/v1/me/documents/unofficial-transcript/print
POST /api/v1/me/document-requests
GET  /api/v1/me/document-requests/{request_id}
POST /api/v1/admin/document-requests/{request_id}/approve
POST /api/v1/admin/document-requests/{request_id}/reject
GET  /api/v1/admin/document-requests?status=pending
```

### Institution operations

```text
GET  /api/v1/events
POST /api/v1/admin/events
PATCH /api/v1/admin/events/{event_id}
```

### Licensing

```text
GET  /api/v1/license/status
POST /api/v1/license/import
POST /api/v1/platform/institutions/{institution_id}/suspend
POST /api/v1/platform/institutions/{institution_id}/activate
```

The last two routes are hosted-platform operations and must not be exposed to ordinary institution administrators.

### HTML-fragment adapter

For Alpine AJAX, mirror only the UI actions that need server-rendered fragments:

```text
GET  /ui/registration/sections
POST /ui/registration/add
POST /ui/registration/drop
GET  /ui/grades/table
POST /ui/document-requests
POST /ui/admin/document-requests/{id}/approve
```

These routes call the same application services as the JSON API. They do not contain separate business logic.

---

## 10. Session and authentication layer

### 10.1 Do not put a broad JWT in browser storage

The simplest secure browser model here is an opaque session cookie:

- `Secure`
- `HttpOnly`
- `SameSite=Lax` or stricter where possible
- host-only domain
- short idle lifetime and bounded absolute lifetime
- session rotation after authentication and privilege changes

The session record contains only identity and revocation data. Roles and scoped permissions are loaded into a compact actor snapshot and cached in process.

### 10.2 Performance shape

A database session lookup on every request would immediately consume the database budget. Use:

1. an opaque random session ID in the cookie;
2. a bounded in-process session cache on the single application server;
3. PostgreSQL as the durable session registry;
4. cache invalidation on logout, password reset, role change, user suspension, and license change;
5. expiry checked against the current clock on every request, so a cache hit does not extend the session.

On cache miss, load once from PostgreSQL. The single-server constraint makes local caching effective and simple. If horizontal scale is introduced later, replace only the session-store adapter or add a shared cache; do not change business modules.

### 10.3 Password handling

Use Argon2id with parameters calibrated to the deployment hardware. Password verification is deliberately expensive. It must not be weakened to meet a synthetic 500k login/second target.

The 500k performance target should apply to authenticated application requests, not independent password verifications. If the benchmark requires 500k fresh logins per second on one server, it conflicts directly with secure password hashing.

### 10.4 CSRF and frontend security

All cookie-authenticated state-changing requests require a CSRF token bound to the session. Alpine AJAX forms carry the token as a hidden field or header.

Use the Alpine CSP build. Standard Alpine expression execution requires a Content Security Policy relaxation; the CSP build allows a stricter policy. Keep user data out of raw HTML strings and render through an auto-escaping template engine.

Suggested response headers:

```text
Content-Security-Policy:
  default-src 'self';
  script-src 'self';
  style-src 'self';
  img-src 'self' data:;
  font-src 'self';
  connect-src 'self';
  frame-ancestors 'none';
  base-uri 'none';
  form-action 'self'

Strict-Transport-Security: max-age=31536000; includeSubDomains
X-Content-Type-Options: nosniff
Referrer-Policy: strict-origin-when-cross-origin
Permissions-Policy: camera=(), microphone=(), geolocation=()
```

---

## 11. Permission boundaries

Use role-based access for coarse capabilities and explicit policy functions for resource scope.

### 11.1 Coarse roles

- `student`
- `instructor`
- `registrar`
- `records_officer`
- `document_officer`
- `institution_admin`
- `platform_licensing_admin`

### 11.2 Resource policy examples

```text
can_view_student_record(actor, student_id)
can_register_for_student(actor, student_id)
can_submit_grade(actor, section_id)
can_publish_grade(actor, section_id)
can_approve_document(actor, request)
can_change_license(actor, institution_id)
```

Handlers extract an authenticated actor. Application services call the policy before any sensitive query or mutation. The policy does not live in the frontend, and hiding a button is never treated as authorization.

### 11.3 Avoid premature database row-level security

PostgreSQL row-level security can be valuable defense in depth, but pooled connections require careful request context management. For the demo timeline:

- use one least-privileged application database role;
- enforce authorization in application services;
- test every policy path;
- use explicit institution filters in every query;
- add RLS later only if it is implemented comprehensively and verified under connection pooling.

A partially applied RLS design creates false confidence.

---

## 12. Registration request flow

### Student action

A student presses **Add** for a section.

### Worked flow

```text
1. Browser submits POST /ui/registration/add
2. License middleware checks institution-level access from local snapshot.
3. Session middleware resolves Actor from local session cache.
4. CSRF middleware validates the session-bound token.
5. Handler parses section_id and idempotency_key.
6. EnrollmentService::register(actor, command) starts a PostgreSQL transaction.
7. Policy confirms actor may register for the target student.
8. Lock student_term_registration FOR UPDATE.
9. Validate registration/drop-add time window.
10. Check holds and student academic status.
11. Check duplicate active enrollment.
12. Check prerequisite and schedule-conflict rules.
13. Atomically reserve a seat:

    UPDATE section_capacity
       SET enrolled_count = enrolled_count + 1,
           version = version + 1
     WHERE section_id = $1
       AND enrolled_count < capacity
    RETURNING enrolled_count;

14. Insert enrollment.
15. Insert audit event in the same transaction.
16. Commit.
17. Return updated enrollment and section fragments.
```

### Why lock per student and term

Two browser tabs could submit different sections at the same time. Without serialization, each transaction could check the old schedule and both could insert conflicting meetings. Locking one `student_term_registration` row makes all registration changes for that student and term line up in a clear order.

This does not serialize the entire university. Different students register concurrently.

### Why not lock the section row first

Lock order matters. Use a consistent order:

1. student-term registration row;
2. section capacity row through conditional update;
3. enrollment insert.

This reduces deadlock risk. For commands affecting two sections, sort section IDs before locking.

### Why not use only serializable isolation

Serializable isolation is valid but creates retry behavior that every command must handle. Explicitly locking the narrow conflict domain is easier to reason about for this use case. Serializable transactions may still be used for more complex future rules, with bounded retries and metrics.

---

## 13. Drop/add request flow

Dropping follows the same student-term lock.

```text
1. Lock student_term_registration.
2. Load active enrollment FOR UPDATE.
3. Verify drop/add deadline and policy.
4. Mark enrollment dropped; do not delete history.
5. Decrement section_capacity with a non-negative constraint.
6. Insert audit event.
7. Commit.
```

Use status transitions rather than deleting enrollment rows. Academic and audit history must remain explainable.

If a student attempts to add one section only after dropping another, support an explicit **swap** command later. A swap is one transaction; two independent browser calls are not atomic and can leave the student with neither section.

---

## 14. Grades and schedule flow

### Schedule

A schedule is a read model composed from:

- active enrollments owned by `enrollment`;
- section/course/meeting data owned by `academics`;
- event and holiday data owned by `institution`.

Do not make the web handler call three repositories and merge data. Put one explicit query function in a `student_schedule_query` adapter. It may use a SQL view or one joined query. Cross-module **reads** are allowed through such deliberate read models; cross-module writes are not.

### Grades

The records module owns grade visibility. A student query returns only published grades. Instructor and records-officer queries may include draft state when policy allows.

A grade write flow is:

```text
1. Authenticate and authorize instructor assignment.
2. Lock or compare grade_record.version.
3. Insert/update grade record.
4. Audit old and new value in the same transaction.
5. Publish only through an explicit command.
```

Use optimistic version checking for ordinary grade editing. Registration needs pessimistic coordination because seats are scarce and highly contended; grade edits usually conflict only when two authorized users edit the same row.

---

## 15. Document generation and printing pipeline

### 15.1 Split unofficial printing from official issuance

#### Unofficial transcript and proof of enrollment

- Render a server-side HTML print view.
- Use print-specific CSS.
- Query the current records directly.
- Add a visible “Unofficial” watermark and generation timestamp.
- Let the browser print or save as PDF.

This is simple, fast, and avoids a server-side PDF dependency for a non-official artifact.

#### Official documents

- Student creates `document_request`.
- Authorized officer approves or rejects.
- Approval asks records to create an immutable snapshot.
- A `document_job` is inserted in the same transaction.
- A worker in the same application process claims the job using `FOR UPDATE SKIP LOCKED`.
- The worker renders a PDF, signs or stamps it where required, writes it to content-addressed storage, and records its hash.
- The request becomes `ready` only after the artifact metadata commits.

### 15.2 Why generation is asynchronous

PDF rendering is CPU- and memory-heavy and may involve fonts, images, and cryptographic signing. Keeping it inside the request handler would:

- increase tail latency;
- consume Actix worker capacity;
- make retries ambiguous;
- make 500k RPS less plausible;
- create duplicate artifacts when clients resubmit.

“Latency-critical” for this feature should mean the request and approval command returns quickly with an exact state. It cannot reasonably mean that an official signed PDF is fully generated before every HTTP response under a 500k mixed workload.

### 15.3 Storage under the single-server constraint

For the demo:

- store generated files in an application-managed filesystem directory;
- use a content hash in the filename;
- store metadata and hash in PostgreSQL;
- back up the directory with the database.

For production, object storage is the natural adapter. The documents module should depend on a small `DocumentStore` interface because filesystem and object-storage implementations are genuinely different external boundaries.

---

## 16. Institution-level subscription kill switch

### 16.1 Hosted deployment

Keep one immutable `LicenseSnapshot` in memory for the institution:

```text
institution_id
status
valid_from
valid_until
feature_set
version
```

Every protected request performs:

```text
allowed = status == active
       && now >= valid_from
       && now < valid_until
```

The current time is checked on every request. Therefore, an in-memory cache does not create an expiry grace period.

License changes are written to PostgreSQL and atomically swapped into memory after commit. On startup, the snapshot is loaded before the server accepts protected traffic.

Allow only these routes while locked:

- health and readiness;
- license status;
- license import or renewal;
- platform operator recovery;
- a static payment-required page.

Return `402 Payment Required` for API clients and a clear institution-level lock page for browser requests.

### 16.2 Self-hosted deployment

Use a signed license document containing:

```text
institution_id
deployment_id
valid_from
valid_until
feature_set
license_serial
```

The vendor signs it with an offline Ed25519 private key. The application embeds only the public verification key. At startup and on each protected request, the application verifies the loaded license snapshot and checks its time bounds.

### 16.3 Security truth for self-hosting

A customer with root access to the machine, database, and executable can patch the binary, modify the clock, or replace the software. No purely local kill switch can be made impossible for a determined self-hoster to bypass.

The signed license protects against accidental or ordinary unauthorized continuation. Contract terms, support, upgrade access, and operational controls remain part of enforcement. The architecture must not claim a cryptographic guarantee that cannot exist on customer-controlled hardware.

### 16.4 Keep licensing out of student identity

Do not mark every student suspended when the contract expires. That would create millions of user-level changes and corrupt the meaning of user status. The request gate denies the institution as a whole while user records remain intact.

### 16.5 Commercial model and pricing boundary

Treat pricing as an institution contract, never as student billing:

| Deployment | Commercial components |
|---|---|
| Hosted by the vendor | annual or negotiated software fee **plus a hosting fee** |
| Self-hosted by the University | annual or negotiated software fee **plus a one-time installation fee** |

The demo does not need a general billing engine. Store a platform-operator-only contract record containing the contract reference, deployment mode, billing basis, currency, agreed fee components, and contract dates. The `institution_license` row is the small enforcement projection derived from that commercial decision.

Payment handling is intentionally simple at this stage:

1. finance or the platform operator determines that the institution is paid or unpaid outside the student system;
2. an authorized platform operator changes the institution license to `active`, `suspended`, or `expired`;
3. the change is committed and audited;
4. the in-memory license snapshot is atomically replaced;
5. the next protected request succeeds or fails for the whole institution.

There is no student-level fee check, grace period, staged escalation, or per-user suspension flow. Contract amounts must not be exposed to University students, instructors, or ordinary school administrators.

---

## 17. How module boundaries affect 500k RPS

### Boundaries that matter

#### 1. Enrollment owns the transaction

The registration path should not bounce through generic service layers or make module-to-module HTTP calls. One command controls one transaction and one lock order.

#### 2. Read models prevent N+1 module composition

A schedule page should use one explicit query, not call academics once per enrollment. The boundary forces a deliberate query adapter rather than hiding expensive access behind methods.

#### 3. Documents are isolated from request workers

The documents boundary makes it obvious which work is slow and asynchronous.

#### 4. Licensing is a local gate

A lock-free snapshot avoids a database query on every request.

#### 5. Identity caches session state

The identity boundary centralizes session lookup and prevents each handler from querying users and roles.

### Boundaries that do not matter materially

- A Rust function call versus a trait-object call is not the main scaling factor compared with a database round trip.
- Separate PostgreSQL schemas do not increase throughput by themselves.
- Splitting modules into microservices does not make one database faster.
- Adding an event bus does not make a seat reservation correct.
- More folders do not reduce allocations or network output.

The performance value of modularity comes from making data access, transaction ownership, and side effects visible—not from the namespace itself.

---

## 18. Performance design from first principles

### 18.1 Request budget

For each endpoint, record:

```text
maximum body bytes
maximum response bytes
allocations/request
SQL statements/request
rows read/request
rows written/request
locks acquired
cache behavior
p50/p95/p99 latency
```

No endpoint is accepted into the 500k benchmark without a declared budget.

### 18.2 Database pool and backpressure

Do not create one database connection per request. Use a bounded SQLx pool. A bounded pool is intentional backpressure: it prevents the application from converting a traffic spike into database collapse.

When the queue is full:

- fail quickly for nonessential reads with a retryable response;
- preserve capacity for registration and critical admin commands;
- expose saturation metrics;
- do not let unbounded futures consume memory.

### 18.3 Shared read snapshots

Good candidates for versioned in-process snapshots:

- active terms and deadlines;
- course catalog summaries;
- section meeting metadata;
- holidays and institution events;
- licensing state;
- role definitions and static permission mapping.

Poor candidates for casual caching:

- seat availability during registration;
- unpublished grades;
- document approval state;
- user suspension state without invalidation.

At 50,000 users, selected student dashboard data may also be cached with per-user versioning. Add this only after query profiles show it is necessary.

### 18.4 HTTP caching

Use:

- immutable hashes for static assets;
- `ETag` for shared catalog and event fragments;
- `Cache-Control: private, no-store` for sensitive student records;
- small, targeted HTML fragments rather than full-page reloads;
- no compression for tiny responses when compression CPU costs more than bytes saved, based on measurement.

### 18.5 Query design

- Select explicit columns.
- Use covering indexes for high-frequency reads where measurement supports them.
- Avoid `OFFSET` pagination for large admin lists; use keyset pagination.
- Use `EXISTS` for eligibility/conflict checks.
- Use partial indexes for active states.
- Keep transactions short; no network calls inside a transaction.
- Audit writes should be compact and append-only.

### 18.6 Registration contention

The throughput of a popular section is bounded by a single capacity row because one seat counter must be serialized. That is a property of the invariant, not a module defect.

Possible later optimizations, in order:

1. confirm the actual contention profile;
2. reduce statements and index work in the transaction;
3. batch or precompute immutable eligibility facts;
4. shard capacity counters only if exact seat semantics can still be proven;
5. change infrastructure only after the transaction is minimal.

Do not distribute one exact counter casually. Correctness is the product.

---

## 19. Benchmark contract

Create three published benchmark suites.

### Suite A — Local/cached throughput

- valid cached session;
- active license snapshot;
- cached course/event snapshot;
- small response;
- zero database queries.

This is the appropriate place to explore the 500k RPS application-server target.

### Suite B — Read-through workload

- realistic grades and schedule reads;
- cold and warm session-cache ratios;
- bounded PostgreSQL pool;
- realistic response sizes;
- no writes.

### Suite C — Transactional mixed workload

Equal proportions of:

- register;
- drop;
- document request;
- grade mutation or publication;
- admin approval.

Report sustainable committed transactions/second and p99 latency. Do not label rejected, queued, or nondurable acknowledgements as completed operations.

### Required benchmark metadata

- CPU model, core count, RAM, NIC;
- application and database host specs;
- PostgreSQL configuration;
- connection count;
- dataset size;
- response sizes;
- TLS on/off;
- durability settings unchanged from production intent;
- error and retry rates;
- hot/cold cache state.

---

## 20. Request flows end to end

### 20.1 Student views grades

```text
Browser
  -> GET /ui/grades/table
  -> license gate
  -> session/actor resolution
  -> RecordsQuery::student_grades(actor, term)
  -> one SQL query restricted by institution and student
  -> auto-escaped HTML fragment
  -> Alpine AJAX replaces #grades-table
```

### 20.2 Student registers

```text
Browser
  -> POST /ui/registration/add
  -> license, session, CSRF, payload limits
  -> EnrollmentService::register
  -> PostgreSQL transaction
  -> audit in same transaction
  -> updated fragments
```

### 20.3 Admin approves official transcript

```text
Browser
  -> POST /ui/admin/document-requests/{id}/approve
  -> license, session, CSRF
  -> DocumentsService::approve
  -> policy: document_officer
  -> lock request
  -> RecordsService::create_transcript_snapshot within command boundary
  -> insert approval + generation job + audit
  -> commit
  -> response shows approved/queued
  -> in-process worker claims job
  -> PDF generated and stored
  -> request marked ready
```

### 20.4 Institution license expires

```text
Request arrives
  -> LicenseSnapshot checked against current time
  -> now >= valid_until
  -> request stops before session and database work
  -> 402 or lock page returned
```

This is both simpler and faster than querying a subscription table after every handler has started.

---

## 21. Complexity controls

### Rules for the first six months

1. One Cargo package and one application binary.
2. One application process.
3. One PostgreSQL database.
4. No internal HTTP.
5. No general event bus.
6. No dependency-injection framework.
7. No ORM entity graph; use explicit SQLx queries.
8. No generic repository base classes.
9. No “utils” dumping ground.
10. No admin domain directory.
11. No asynchronous command result for seat ownership.
12. No hidden database access in model methods.
13. No cross-feature table writes.
14. No business logic in Alpine code.
15. No authorization based on UI visibility.
16. No optimization without an endpoint budget or profile.

### Shared kernel limit

The shared core may contain:

- ID newtypes;
- clock abstraction;
- application error type;
- pagination types;
- request correlation ID.

It may not contain:

- `Student`;
- `Course`;
- `Enrollment`;
- generic database repositories;
- universal event types;
- business policy.

If a type has business meaning, it belongs to a business module.

---

## 22. Demo delivery sequence

### Weeks 1–2 — Foundation and architecture proof

- single-package directory and Rust module skeleton;
- migrations and seed data;
- session, actor, CSRF, security headers;
- institution and licensing gate;
- audit primitive;
- one end-to-end “view current term” vertical slice;
- concurrency test harness.

### Weeks 3–6 — Academics and enrollment

- terms, courses, sections, meetings;
- student profile and holds;
- registration and drop/add;
- schedule conflict and prerequisite rules;
- registration UI fragments;
- contention and idempotency tests.

### Weeks 7–10 — Records and student views

- grade entry/read/publish;
- schedule and grade read models;
- unofficial transcript and proof-of-enrollment print views;
- events and holidays.

### Weeks 11–14 — Official document workflow and admin portal

- document requests;
- approval/rejection;
- transcript snapshots;
- PostgreSQL job queue and generator worker;
- admin task views.

### Weeks 15–18 — Hardening and benchmark work

- authorization matrix tests;
- audit review;
- input limits and abuse cases;
- query plans and index tuning;
- load suites A, B, and C;
- failure recovery and backup test;
- demo documentation.

A three-month demonstration uses the lower end of this sequence and may limit administrative breadth. A six-month demonstration can include more complete grade workflows, document templates, and performance characterization. Code quality is preserved because features are reduced, not foundations bypassed.

---

## 23. Tradeoff summary

### Selected business-capability directories

**Gained**

- clear write ownership;
- simple in-process communication;
- local transactions;
- low deployment complexity;
- easier testing and refactoring;
- seams that remain useful after the demo.

**Lost**

- feature directories are not independently deployable;
- one bad in-process bug can affect the whole application;
- database ownership is enforced by module visibility, tests, and code discipline rather than separate crates;
- horizontal scaling is not available under the stated single-server constraint.

### Separate frontend source, packaged deployment

**Gained**

- frontend files and interaction design remain independent;
- no mandatory BFF network hop;
- Alpine AJAX can receive small server-rendered fragments;
- one deployment for the demo.

**Lost**

- frontend and backend releases are coordinated;
- fragment contracts are coupled to backend adapters;
- a fully independent frontend deployment would require later packaging changes.

### PostgreSQL-backed job queue

**Gained**

- no new infrastructure;
- transactional job creation;
- understandable recovery;
- enough for demo and moderate production document volume.

**Lost**

- not intended for extreme general-purpose messaging throughput;
- document work still shares the application host;
- future dedicated workers may require a deployment split.

### Institution-level signed licensing

**Gained**

- correct school-level semantics;
- immediate expiry check;
- works hosted and self-hosted;
- no mass mutation of users.

**Lost**

- self-hosted root users can ultimately bypass local enforcement;
- strict no-grace expiry depends on correct time and controlled update paths.

---

## 24. Final architecture test

A boundary is accepted only if the following answers remain simple:

- Who owns this rule?
- Who owns the write?
- Which transaction proves the invariant?
- How many SQL statements does the request execute?
- Can another module use the capability without knowing its tables?
- Can the demo omit a secondary feature without cutting through every module?
- Can the feature later move to another process without changing its business vocabulary?

The selected design passes these tests while keeping the deployment intentionally boring.

---

## 25. Source notes

The public XenDirect/Xenegrade material was used only as a replacement-system relationship check, not as the target architecture. Its public API documentation exposes course-oriented paged collections and related administrative/student data concepts, which supports retaining familiar terms such as courses, sections, grades, and fee/academic records while redesigning ownership around University of Belize workflows.

Primary technical references consulted:

- [Actix Web: Application and shared state](https://actix.rs/docs/application/)
- [Actix Web: Extractors](https://actix.rs/docs/extractors/)
- [Actix Web: Middleware](https://actix.rs/docs/middleware/)
- [Actix Web: Server and worker behavior](https://actix.rs/docs/server/)
- [SQLx connection pooling](https://docs.rs/sqlx/latest/sqlx/pool/index.html)
- [PostgreSQL transaction isolation](https://www.postgresql.org/docs/current/transaction-iso.html)
- [PostgreSQL explicit locking and advisory locks](https://www.postgresql.org/docs/current/explicit-locking.html)
- [Alpine AJAX reference](https://alpine-ajax.js.org/reference/)
- [Alpine.js CSP build guidance](https://alpinejs.dev/advanced/csp)
- [Argon2 RFC 9106](https://datatracker.ietf.org/doc/rfc9106/)
- [XenDirect API documentation](https://api.xendevelop.com/)
