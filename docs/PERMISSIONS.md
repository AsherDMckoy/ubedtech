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
| Register for a section | ❌ | ✅ self only | ❌ | ✅ any student in institution | ❌ | ❌ | ❌ | ❌ | `enrollment::policy::only_the_registrar_registers_other_students` (all 7 roles), `a_student_registers_only_for_themselves`, `only_one_student_gets_the_last_seat` |

## Matrix (Phase 3: academics, enrollment, overrides, holds, student UI)

| Operation | anon | student | instructor | registrar | records_officer | document_officer | institution_admin | platform_licensing_admin | Proof (tests) |
|---|---|---|---|---|---|---|---|---|---|
| Create term/course/section, set capacity, add meeting/prerequisite (`POST /api/v1/terms`, `/courses`, `/sections`, …) | ❌ | ❌ | ❌ | ✅ own institution | ❌ | ❌ | ✅ own institution | ❌ | `academics::policy::only_registrar_and_institution_admin_manage_academics` (all 7 roles), `academics_commands_enforce_the_role_matrix`, `section_creation_sets_capacity_in_the_same_transaction` (cross-institution 404) |
| Grant a registration override (`POST /api/v1/students/{id}/overrides`) | ❌ | ❌ | ❌ | ✅ own institution, reason required | ❌ | ❌ | ❌ | ❌ | `enrollment::policy::only_the_registrar_grants_overrides` (all 7 roles), `override_grants_are_registrar_only_validated_and_scoped` |
| Place/release a registration hold (`POST /api/v1/students/{id}/holds`, `DELETE …/holds/{flag}`) | ❌ | ❌ | ❌ | ✅ own institution, reason required | ❌ | ❌ | ❌ | ❌ | `holds_block_registration_until_released_or_overridden` (403/404/validation + block/release/override behavior) |
| Drop an enrollment | ❌ | ✅ self only | ❌ | ✅ any student in institution | ❌ | ❌ | ❌ | ❌ | `enrollment::policy` role tests (same function as register), `one_deadline_governs_both_adds_and_drops`, `concurrent_duplicate_drops_release_exactly_one_seat` |
| Browse catalog / current term (`GET /ui/catalog`, `GET /api/v1/catalog`, `GET /api/v1/terms/current`) | ❌ 401 | ✅ own institution's rows only | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | `catalog_and_current_term_are_institution_scoped` (two institutions), `ui::login_catalog_register_and_drop_work_as_plain_forms` |
| Student registration page + form register/drop (`/ui/registration…`) | ❌ 401 | ✅ self | ❌ (no student profile ⇒ 403 from `require_student_self`) | ❌ same | ❌ | ❌ | ❌ | ❌ | `ui::login_catalog_register_and_drop_work_as_plain_forms`, `ui::every_rejection_case_renders_inline_feedback` |

## Matrix (Phase 4: instructor assignments, grades, records)

| Operation | anon | student | instructor | registrar | records_officer | document_officer | institution_admin | platform_licensing_admin | Proof (tests) |
|---|---|---|---|---|---|---|---|---|---|
| Assign an instructor to a section (`POST /api/v1/sections/{id}/instructors`) | ❌ | ❌ | ❌ | ✅ target must hold the instructor role | ❌ | ❌ | ✅ same | ❌ | `instructor_assignment_is_validated_scoped_and_idempotent` |
| List own sections / read a roster (`GET /api/v1/instructor/sections`, `GET /api/v1/sections/{id}/roster`, `/ui/instructor…`) | ❌ 401 | ❌ 403 | ✅ assigned sections ONLY — any other real id is 404 | ❌ | ✅ any section in institution | ❌ | ❌ | ❌ | `rosters_are_visible_only_for_assigned_sections` (crafted-request 404s), `ui::instructor_enters_officer_publishes_student_sees_published_only` |
| Save a draft grade (`POST /api/v1/grades/draft`, roster form) | ❌ | ❌ | ✅ assigned section only, inside the grade-entry window | ❌ | ✅ any in institution, window-exempt | ❌ | ❌ | ❌ | `unassigned_instructors_cannot_grade_and_students_never_see_drafts`, `grade_entry_window_binds_instructors_not_the_officer` |
| Rewrite a published grade via draft entry | ❌ | ❌ | ❌ 409 for everyone — published grades only change through correction | ❌ | ❌ 409 | ❌ | ❌ | ❌ | `corrections_preserve_prior_value_and_author_in_history` |
| Publish a section's draft grades (`POST /api/v1/sections/{id}/grades/publish`, roster form) | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | `corrections_preserve_prior_value_and_author_in_history` (officer publish), service role guard + `ui::…` (no publish button ≠ authorization; the service refuses) |
| Correct a published grade (`POST /api/v1/grades/correct`) | ❌ | ❌ | ❌ | ❌ | ✅ reason required; prior value + author preserved in `grade_revision` | ❌ | ❌ | ❌ | `corrections_preserve_prior_value_and_author_in_history` |
| Generate a transcript snapshot (`POST /api/v1/students/{id}/transcript-snapshots`) | ❌ | ❌ | ❌ | ❌ | ✅ own institution (else 404); artifact immutable by DB trigger | ❌ | ❌ | ❌ | `transcript_snapshots_are_immutable_versioned_and_published_only` |
| Read own grades / academic history / snapshots (`GET /api/v1/me/grades`, `/api/v1/me/history`, `/ui/grades`, `/ui/history`) | ❌ 401 | ✅ self only; **published/amended only — the filter is in the query** | ❌ 403 (no student profile) | ❌ same | ❌ same | ❌ same | ❌ same | ❌ same | `unassigned_instructors_cannot_grade_and_students_never_see_drafts`, `academic_history_spans_terms_and_hides_drafts`, `ui::…` (draft invisible on pages) |

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

- Document request: student for self; approve/reject: document officer
  (`documents/service.rs`, `documents/http.rs` admin queue).
