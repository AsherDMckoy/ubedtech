# Permissions — role × operation matrix

Rule (CLAUDE.md §5): **a row may only appear here if a passing test backs
it.** Operations whose authorization is implemented but not yet test-backed
are listed at the bottom as debt, not in the matrix. Authorization lives in
services/policy modules, never in templates; institution scoping applies to
every operation (an admin's power ends at their institution's boundary).

Roles: `student`, `instructor`, `registrar`, `records_officer`,
`document_officer`, `institution_admin`, `platform_licensing_admin`
(codes as seeded by migration 0007; `shared::actor::Role`).

## Matrix (Phase 2: identity, sessions, licensing)

Legend: ✅ allowed, ❌ denied, `—` not applicable. "any" = any
authenticated actor regardless of role; "anon" = no session required.

| Operation | anon | student | instructor | registrar | records_officer | document_officer | institution_admin | platform_licensing_admin | Proof (tests) |
|---|---|---|---|---|---|---|---|---|---|
| Log in (`POST /api/v1/session/login`) | ✅ | — | — | — | — | — | — | — | `valid_session_reaches_the_handler_with_the_correct_actor`, `login_failures_all_get_the_same_generic_401`, `throttle_locks_an_account_ip_pair_then_expires` |
| Any protected route without a valid session | ❌ 401 | — | — | — | — | — | — | — | `request_without_a_session_is_401`, `garbage_and_forged_cookies_are_401`, `expired_sessions_stop_working_over_http` |
| Log out own session (`POST /api/v1/session/logout`) | ❌ | ✅ any authenticated actor | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | `full_session_lifecycle_login_use_logout` |
| Change **own** password, current password required (`POST /api/v1/me/password`) | ❌ | ✅ any authenticated actor | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | `password_change_rotates_this_session_and_revokes_all_others`, `password_change_rejects_wrong_current_and_short_new` |
| Reset **another** user's password (`POST /api/v1/users/{id}/password`) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ own institution only (else 404) | ❌ | `admin_password_reset_revokes_target_sessions` (grant + non-admin 403), `admin_powers_stop_at_the_institution_boundary` (scope), `policy::only_institution_admin_manages_accounts_and_roles` (all 7 roles exhaustively) |
| Suspend an account (`POST /api/v1/users/{id}/suspend`) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ own institution, never self | ❌ | `suspension_revokes_sessions_and_blocks_relogin`, `admin_powers_stop_at_the_institution_boundary`, `policy::only_institution_admin_manages_accounts_and_roles` |
| Grant/revoke an institution role (`POST /api/v1/users/{id}/roles`, `DELETE .../roles/{code}`) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ own institution, never self, never the platform role | ❌ | `admin_grants_and_revokes_roles_with_session_revocation`, `role_management_guardrails`, `policy::only_institution_admin_manages_accounts_and_roles` |
| Grant/revoke `platform_licensing_admin` via the API | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ (no API path exists) | `role_management_guardrails`, `policy::the_platform_role_is_never_institution_assignable` |
| Create the first platform admin (CLI `bootstrap-platform-admin`, operator-only, refuses once one exists) | — | — | — | — | — | — | — | — | `bootstrap::bootstrap_creates_a_working_platform_admin_once`, `bootstrap::bootstrap_validates_inputs_and_environment` |
| Change license status (`POST /ui/platform/institutions/{id}/license`) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | `platform_admin_flips_the_license_end_to_end` (allow), `non_platform_roles_cannot_touch_the_license` (institution_admin — the strongest non-platform role — denied) |
| Read license status (`GET /license/status`) | ✅ (recovery surface, works while locked) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | `locked_institution_answers_402_and_recovery_stays_reachable` |
| Any non-exempt route while the license is locked | ❌ 402 for everyone | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ (must use the exempt platform UI) | `locked_institution_answers_402_and_recovery_stays_reachable`, `licensing::middleware` unit tests |
| Register for a section | ❌ | ✅ self only | ❌ | ✅ any student in institution | ❌ | ❌ | ❌ | ❌ | partially: `enrollment::policy::require_can_register_for` is exercised by `only_one_student_gets_the_last_seat` (student-self path); the full per-role deny matrix is Phase 4/8 debt |

Additional cross-cutting proofs:

- CSRF applies to every state-changing operation above except login:
  `csrf_missing_wrong_and_cross_session_tokens_are_403`,
  `csrf_form_field_is_accepted_and_the_body_survives_for_the_handler`.
- Privilege changes revoke the target's sessions in the same transaction:
  `admin_grants_and_revokes_roles_with_session_revocation`,
  `admin_password_reset_revokes_target_sessions`,
  `suspension_revokes_sessions_and_blocks_relogin`.
- Suspended accounts cannot log in and their live sessions stop resolving:
  `sessions::suspended_account_sessions_do_not_resolve`,
  `suspension_revokes_sessions_and_blocks_relogin`.

## Implemented but not yet test-backed (must not be trusted; matrix debt)

These checks exist in code but have no per-role deny tests yet. They get
rows when their phases add the tests (Phase 4/5/6/8):

- Grade draft save: instructor assigned to the section, or records officer
  (`records/grades.rs`).
- Grade publish: records officer only (`records/grades.rs`).
- Document request: student for self; approve/reject: document officer
  (`documents/service.rs`, `documents/http.rs` admin queue).
- Registration/drop **deny** cases (instructor/officer roles must be
  rejected; only the student-self and registrar allow paths matter today).
