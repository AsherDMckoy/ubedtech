# HTTP API

Created in Phase 2, when the first real API surface landed. Conventions:

- Cookie-session authentication (`ub_session`, opaque token); no JWTs.
- Every non-safe method except login requires the session's CSRF token:
  `X-CSRF-Token` header or `csrf_token` form field. Failure ⇒ 403.
- No/invalid session on a protected route ⇒ 401. Inactive license on a
  non-exempt route ⇒ 402 (see licensing exemptions below). Denied ⇒ 403;
  out-of-institution targets ⇒ 404; validation ⇒ 422 with
  `{"code":"validation_error","message":…}`; throttled login ⇒ 429.
- Errors are always `{"code": …, "message": …}` and never carry SQL,
  stack traces, or secrets.

## Session

| Method & path | Auth | Body → response |
|---|---|---|
| `POST /api/v1/session/login` | none (CSRF-exempt, license-exempt) | `{"username","password"}` → 200 `{"csrf_token"}` + session cookie; 401 uniform on any failure; 429 when throttled |
| `POST /api/v1/session/logout` | session + CSRF | → 204 + removal cookie |

## Account & roles (identity_access)

| Method & path | Who | Body → response |
|---|---|---|
| `POST /api/v1/me/password` | any authenticated | `{"current_password","new_password"}` → 200 `{"csrf_token"}` + NEW session cookie (all other sessions revoked) |
| `POST /api/v1/users/{id}/password` | institution_admin, own institution | `{"new_password"}` → 204; target's sessions revoked |
| `POST /api/v1/users/{id}/suspend` | institution_admin, own institution, not self | `{"reason"}` → 204; target's sessions revoked, re-login blocked |
| `POST /api/v1/users/{id}/roles` | institution_admin, own institution, not self | `{"role": "<code>"}` → 204; idempotent; real grant revokes target's sessions |
| `DELETE /api/v1/users/{id}/roles/{code}` | institution_admin, own institution, not self | → 204; idempotent; real revoke revokes target's sessions |

Role codes: `student`, `instructor`, `registrar`, `records_officer`,
`document_officer`, `institution_admin`. `platform_licensing_admin` is not
grantable or revocable over HTTP (operator bootstrap only — OPERATIONS.md).

## Licensing & recovery surface (reachable while locked)

| Method & path | Who | Response |
|---|---|---|
| `GET /health/live`, `GET /health/ready` | none | 200 / 503 |
| `GET /license/status` | none | 200 `{status, valid_from, valid_until, version}` |
| `POST /license/import` | none | 501 (Phase 7.1) |
| `GET /institution-locked` | none | 200 HTML |
| `POST /ui/platform/institutions/{id}/license` | platform_licensing_admin | form `status`+`reason`+`csrf_token` → 200 HTML fragment |

## Academics (Phase 3; registrar or institution_admin for mutations)

| Method & path | Who | Body → response |
|---|---|---|
| `POST /api/v1/terms` | registrar, institution_admin | `{code,name,starts_on,ends_on,registration_opens_at,add_drop_closes_at,grade_entry_closes_at?}` → 201 `{term_id}`; duplicate code ⇒ 409 |
| `GET /api/v1/terms/current` | any authenticated | 200 `TermSummary` or 404 |
| `POST /api/v1/courses` | registrar, institution_admin | `{code,title,credit_hours}` → 201 `{course_id}` |
| `POST /api/v1/courses/{id}/prerequisites` | registrar, institution_admin | `{prerequisite_course_id,minimum_grade_points}` → 204 |
| `POST /api/v1/sections` | registrar, institution_admin | `{term_id,course_id,section_code,capacity}` → 201 `{section_id}` (capacity set in the same tx) |
| `PUT /api/v1/sections/{id}/capacity` | registrar, institution_admin | `{capacity}` → 204; below current enrollment ⇒ 409 |
| `POST /api/v1/sections/{id}/meetings` | registrar, institution_admin | `{day_of_week,starts_at,ends_at,room_id?}` → 201 `{meeting_id}` |
| `GET /api/v1/catalog?term_id&q&page` | any authenticated | 200 `[CatalogSection]`, 20/page, institution-scoped |

## Enrollment (Phase 3)

| Method & path | Who | Body → response |
|---|---|---|
| `POST /api/v1/me/enrollments` | student (self) | `{section_id,idempotency_key}` → 201 receipt; resubmitted key ⇒ the original receipt; denial ⇒ 409 with the reason |
| `POST /api/v1/students/{id}/overrides` | registrar | `{term_id,section_id?,override_type,reason,expires_at?}` → 201 `{override_id}`; single-use, consumed by one enrollment |
| `POST /api/v1/students/{id}/holds` | registrar | `{term_id,flag,reason}` → 204 (idempotent) |
| `DELETE /api/v1/students/{id}/terms/{term_id}/holds/{flag}` | registrar | → 204 (idempotent) |

## Student pages (plain HTML forms; work with JavaScript off)

| Method & path | Who | Behavior |
|---|---|---|
| `GET /ui/login`, `POST /ui/login` | anon (CSRF- and license-exempt like JSON login) | form login; failure re-renders with the error (401), success 303 → `/ui/registration` |
| `GET /ui/catalog?q&page` | any authenticated | current-term catalog search/browse with per-row register forms (server-minted idempotency keys) |
| `GET /ui/registration` | student | current enrollments + drop forms |
| `POST /ui/registration/add`, `POST /ui/registration/drop` | student | success 303 back to the page (PRG); typed denial re-renders the page with the reason inline, status 409 |

## Records & grades (Phase 4)

| Method & path | Who | Body → response |
|---|---|---|
| `POST /api/v1/sections/{id}/instructors` | registrar, institution_admin | `{instructor_user_id}` → 204; target must hold the instructor role (422) |
| `GET /api/v1/instructor/sections` | instructor | 200 `[InstructorSection]` — assigned sections only |
| `GET /api/v1/sections/{id}/roster` | assigned instructor; records_officer (any in institution) | 200 `{section, rows}`; unassigned ⇒ 404 |
| `POST /api/v1/grades/draft` | assigned instructor (inside entry window); records_officer (window-exempt) | `{enrollment_id,grade_code,grade_points?,numeric_value?,expected_version}` → 200 `{version}`; published grade ⇒ 409 |
| `POST /api/v1/grades/correct` | records_officer | draft body + `reason` → 200 `{version}`; state becomes `amended`, prior value+author kept in `grade_revision` |
| `POST /api/v1/sections/{id}/grades/publish` | records_officer | → 200 `{published}` |
| `POST /api/v1/students/{id}/transcript-snapshots` | records_officer | → 201 `{snapshot_id}`; artifact immutable, versions monotonic |
| `GET /api/v1/me/grades?term_id` | student (self) | 200 published/amended rows only |
| `GET /api/v1/me/history` | student (self) | 200 `{courses, snapshots}` — published/amended only |
| `GET /api/v1/me/schedule?term_id` | student (self) | 200 meetings |

Pages (plain forms): `GET /ui/instructor`, `GET /ui/instructor/sections/{id}`
(roster + draft entry, states pending/draft/published/amended),
`POST /ui/instructor/grades`, `POST /ui/instructor/sections/{id}/publish`
(officer), `GET /ui/grades`, `GET /ui/history`. Denials re-render inline
(409/422); successes redirect (PRG).

## Documents (Phase 5)

| Method & path | Who | Behavior |
|---|---|---|
| `GET /ui/documents` | student | request form + own requests with statuses (pending/approved/generating/ready/rejected/failed) and download links when ready |
| `POST /ui/documents` | student | form `{document_type, purpose?, delivery_method, idempotency_key}` (key server-minted into the form) → PRG; resubmitting the same key returns the original request; validation inline (422) |
| `GET /ui/documents/{id}/download` | owning student; document_officer (own institution) | ready + current artifact only, else 404; sha256 re-verified against the recorded checksum; `Content-Disposition: attachment; filename="document.pdf"`; `Cache-Control: private, no-store` |
| `GET /ui/admin/documents` | document_officer | pending queue |
| `POST /ui/admin/documents/{id}/approve` | document_officer | reason REQUIRED; commits approval + immutable snapshot + generation job in one transaction → PRG; blank reason / already-decided render inline (422/404) |
| `POST /ui/admin/documents/{id}/reject` | document_officer | reason REQUIRED; recorded on the decision → PRG |

Generation runs in the background worker (`FOR UPDATE SKIP LOCKED`, 3
attempts with recorded reasons, orphan reaper — see OPERATIONS.md).

The authorization behind every row above is test-backed — see
`docs/PERMISSIONS.md` for the matrix and proving tests.

## Institution administration (Phase 6; institution_admin only)

| Method & path | Body → response |
|---|---|
| `GET /api/v1/institution/settings` | 200 `{name, timezone, document_types: [{document_type, enabled}]}` |
| `PUT /api/v1/institution/settings` | `{name, timezone}` → 204; timezone must exist in `pg_timezone_names` (else 422) |
| `PUT /api/v1/institution/document-types/{type}` | `{enabled}` → 204; disabled types refuse NEW student requests (fail closed) |

Pages (plain forms): `GET /ui/admin/calendar` (events/holidays list +
create form), `POST /ui/admin/calendar` (create; 422/409 inline),
`POST /ui/admin/calendar/{id}/delete`. All audited in the same transaction.

## Licensing (Phase 6 additions)

| Method & path | Who | Behavior |
|---|---|---|
| `GET /ui/platform/license` | platform_licensing_admin (license-exempt) | license panel: current status, change history, suspend/activate form |
| `POST /ui/platform/institutions/{id}/license` | platform_licensing_admin | now PRG: 303 → the panel; validation renders inline (422) |
| `POST /license/import` | institution_admin or platform_licensing_admin (license-exempt, NOT anonymous) | signed license file (format v1, docs/SECURITY.md) → 200 `{status, valid_from, valid_until, version}`; any verification failure → 422 with a fixed message |
