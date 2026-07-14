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

Everything else (enrollment/records/documents `/ui/*` and `/api/v1/*`
routes from earlier phases) sits behind session + CSRF + license gate; those
endpoints get documented here as their phases harden them.

The authorization behind every row above is test-backed — see
`docs/PERMISSIONS.md` for the matrix and proving tests.
