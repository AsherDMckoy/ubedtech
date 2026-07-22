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
| Registrar overview / terms / sections / courses pages + management forms (`/ui/registrar`, `/ui/registrar/terms…`, `/ui/registrar/sections…`, `/ui/registrar/courses…`) | ❌ 401 | ❌ 403 | ❌ | ✅ own institution (foreign section id 404) | ❌ | ❌ | ✅ same policy fn as the JSON API | ❌ | `ui::registrar_overview_scans_the_term_and_denies_students`, `ui::registrar_manages_terms_and_the_window_governs_registration`, `ui::registrar_manages_sections_meetings_instructors_and_courses` |
| Registrar student lookup, holds, academic standing (`/ui/registrar/students…`) | ❌ 401 | ❌ 403 | ❌ | ✅ own institution (foreign student id 404), hold reason required, standing from a fixed set | ❌ | ❌ | ❌ (hold authority is registrar-only, same as the API) | ❌ | `ui::registrar_manages_holds_and_academic_status` |
| Override grant form + review list (`/ui/registrar/students/{id}/overrides`, `/ui/registrar/overrides`) | ❌ 401 | ❌ 403 | ❌ | ✅ reason required, rule from the fixed set, full record shown | ❌ | ❌ | ❌ | ❌ | `ui::registrar_grants_recorded_overrides_and_reviews_them` |
| Any registrar-form rule bypass because the request came from a staff page | ❌ for every role — refused mutations write nothing; successes commit before the redirect | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | `ui::staff_pages_commit_on_the_server_and_grant_no_rule_bypass` |
| Institution settings page + document-type toggles (`/ui/admin/settings…`) | ❌ 401 | ❌ 403 | ❌ | ❌ | ❌ | ❌ | ✅ own institution; unknown timezone refused inline, writes nothing | ❌ | `settings_page_works_as_plain_forms` |
| Account pages: lookup, password reset, role grant/revoke, suspend (`/ui/admin/accounts…`) | ❌ 401 | ❌ 403 | ❌ | ❌ | ❌ | ❌ | ✅ own institution (foreign id 404), never self-roles, never the platform role (403) | ❌ | `admin_account_pages_work_as_plain_forms` (reset actually rotates the login; suspension blocks the next login) |

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

## Matrix (Phase 5: documents)

| Operation | anon | student | instructor | registrar | records_officer | document_officer | institution_admin | platform_licensing_admin | Proof (tests) |
|---|---|---|---|---|---|---|---|---|---|
| Request a document (`POST /ui/documents`) | ❌ 401 | ✅ self | ❌ 403 (no student profile) | ❌ same | ❌ same | ❌ same | ❌ same | ❌ same | `ui::request_review_generate_download_works_as_plain_forms`, `approval_and_rejection_are_reasoned_scoped_and_atomic` |
| Read the review queue / approve / reject (`/ui/admin/documents…`) | ❌ 401 | ❌ 403 | ❌ | ❌ | ❌ | ✅ own institution; reason required for BOTH decisions; snapshot + job committed with the approval | ❌ | ❌ | `approval_and_rejection_are_reasoned_scoped_and_atomic` (403/422/404 + atomicity), `ui::…` (student 403 on the queue, blank reason inline) |
| Download an artifact (`GET /ui/documents/{id}/download`) | ❌ 401 | ✅ own request only (else 404) | ❌ 403 | ❌ | ❌ | ✅ any in institution (else 404) | ❌ | ❌ | `downloads_are_authorized_and_checksum_verified` (owner/officer allow; other student, foreign officer 404; checksum verified), `ui::…` (HTTP attachment) |

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

## Matrix (Phase 6: institution administration & licensing)

| Operation | anon | student | instructor | registrar | records_officer | document_officer | institution_admin | platform_licensing_admin | Proof (tests) |
|---|---|---|---|---|---|---|---|---|---|
| Manage events/holidays (`/ui/admin/calendar`, create/delete) | ❌ 401 | ❌ 403 | ❌ | ❌ | ❌ | ❌ | ✅ own institution only (foreign event ⇒ 404); audited in-tx | ❌ | `events_are_admin_only_validated_scoped_and_audited`, `ui::calendar_admin_works_as_plain_forms` (student 403) |
| Read/update institution settings (`GET/PUT /api/v1/institution/settings`) | ❌ 401 | ❌ 403 | ❌ | ❌ | ❌ | ❌ | ✅ name + timezone (validated against `pg_timezone_names`); audited in-tx | ❌ | `settings_and_document_types_are_admin_only_validated_and_audited` |
| Enable/disable a document type (`PUT /api/v1/institution/document-types/{type}`) | ❌ 401 | ❌ 403 | ❌ | ❌ | ❌ | ❌ | ✅ audited; new requests fail closed on a disabled or missing row | ❌ | `settings_and_document_types_are_admin_only_validated_and_audited`, `disabling_a_document_type_blocks_new_requests_fail_closed` |
| View the license panel (`GET /ui/platform/license`, reachable while locked) | ❌ 401 | ❌ 403 | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ | `a_disabled_license_locks_the_institution_but_suspends_nobody` |
| Change license status (`POST /ui/platform/institutions/{id}/license`) | ❌ 401 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ 403 | ✅ reason required; change record + audit in one tx | `platform_admin_flips_the_license_end_to_end`, `non_platform_roles_cannot_touch_the_license` |
| Import a signed license (`POST /license/import`, reachable while locked) | ❌ 401 | ❌ 403 | ❌ | ❌ | ❌ | ❌ | ✅ Ed25519 signature verified against the deployment public key — the signature is the authority | ✅ same | `import::a_signed_license_import_unlocks_a_locked_deployment`, `import::bad_or_misdirected_license_files_are_rejected` |

`institution_admin` explicitly holds NO power inside enrollment, grades, or
documents — the admin surfaces call the same services as every other
interface, and those services refuse the role at their own boundary:
`institution_admin_does_not_bypass_domain_rules` (registration for self and
others, grade drafts/corrections/publication, document approval/rejection/
download — all denied, nothing written). Disabling a license suspends no
account and revokes no session: `a_disabled_license_locks_the_institution_
but_suspends_nobody`.

## Matrix debt

None. As of Phase 8 every operation exposed over HTTP appears in a matrix
above with its proving tests; there are no authorization checks that exist
in code but lack a deny test. New operations must land with their row and
tests in the same commit (CLAUDE.md §5).

