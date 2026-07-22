use actix_web::http::StatusCode;
use actix_web::{HttpResponse, get, post, put, web};
use askama::Template;
use serde::Deserialize;
use uuid::Uuid;

use crate::academics::AcademicsService;
use crate::academics::service::{
    AddMeetingCommand, AddPrerequisiteCommand, CatalogSection, CreateCourseCommand,
    CreateSectionCommand, CreateTermCommand, SectionOverviewRow, TermSummary,
};
use crate::identity_access::sessions::CurrentSession;
use crate::shared::{actor::Actor, error::AppError};

#[post("/api/v1/terms")]
async fn create_term(
    actor: Actor,
    service: web::Data<AcademicsService>,
    body: web::Json<CreateTermCommand>,
) -> Result<HttpResponse, AppError> {
    let id = service.create_term(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "term_id": id })))
}

#[get("/api/v1/terms/current")]
async fn current_term(
    actor: Actor,
    service: web::Data<AcademicsService>,
) -> Result<HttpResponse, AppError> {
    match service.current_term(&actor).await? {
        Some(term) => Ok(HttpResponse::Ok().json(term)),
        None => Err(AppError::NotFound),
    }
}

#[post("/api/v1/courses")]
async fn create_course(
    actor: Actor,
    service: web::Data<AcademicsService>,
    body: web::Json<CreateCourseCommand>,
) -> Result<HttpResponse, AppError> {
    let id = service.create_course(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "course_id": id })))
}

#[post("/api/v1/courses/{course_id}/prerequisites")]
async fn add_prerequisite(
    actor: Actor,
    service: web::Data<AcademicsService>,
    course_id: web::Path<Uuid>,
    body: web::Json<AddPrerequisiteCommand>,
) -> Result<HttpResponse, AppError> {
    service
        .add_prerequisite(&actor, course_id.into_inner(), body.into_inner())
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

#[post("/api/v1/sections")]
async fn create_section(
    actor: Actor,
    service: web::Data<AcademicsService>,
    body: web::Json<CreateSectionCommand>,
) -> Result<HttpResponse, AppError> {
    let id = service.create_section(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "section_id": id })))
}

#[derive(Deserialize)]
struct CapacityBody {
    capacity: i32,
}

#[put("/api/v1/sections/{section_id}/capacity")]
async fn set_capacity(
    actor: Actor,
    service: web::Data<AcademicsService>,
    section_id: web::Path<Uuid>,
    body: web::Json<CapacityBody>,
) -> Result<HttpResponse, AppError> {
    service
        .set_section_capacity(&actor, section_id.into_inner(), body.capacity)
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

#[post("/api/v1/sections/{section_id}/meetings")]
async fn add_meeting(
    actor: Actor,
    service: web::Data<AcademicsService>,
    section_id: web::Path<Uuid>,
    body: web::Json<AddMeetingCommand>,
) -> Result<HttpResponse, AppError> {
    let id = service
        .add_meeting(&actor, section_id.into_inner(), body.into_inner())
        .await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "meeting_id": id })))
}

#[derive(Deserialize)]
struct AssignInstructorBody {
    instructor_user_id: Uuid,
}

#[post("/api/v1/sections/{section_id}/instructors")]
async fn assign_instructor(
    actor: Actor,
    service: web::Data<AcademicsService>,
    section_id: web::Path<Uuid>,
    body: web::Json<AssignInstructorBody>,
) -> Result<HttpResponse, AppError> {
    service
        .assign_instructor(&actor, section_id.into_inner(), body.instructor_user_id)
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

#[derive(Deserialize)]
struct CatalogQuery {
    term_id: Uuid,
    q: Option<String>,
    #[serde(default)]
    page: u32,
}

#[get("/api/v1/catalog")]
async fn catalog(
    actor: Actor,
    service: web::Data<AcademicsService>,
    query: web::Query<CatalogQuery>,
) -> Result<HttpResponse, AppError> {
    let sections = service
        .search_catalog(&actor, query.term_id, query.q.as_deref(), query.page)
        .await?;
    Ok(HttpResponse::Ok().json(sections))
}

#[derive(Template)]
#[template(path = "pages/catalog.html")]
struct CatalogPage {
    csrf_token: String,
    term: Option<TermSummary>,
    q: String,
    page: u32,
    rows: Vec<RegistrationRow>,
    /// A named condition that blocks every registration right now (hold,
    /// window not open, window closed) — rendered as one loud banner.
    blocked: Option<String>,
}

/// Seats at or below this remaining count render as "low" (amber) — still
/// open, but the warning is honest signal, not decoration.
const LOW_SEATS: i32 = 3;

/// One registration row's view model: the section's committed state plus the
/// two HTTP-only fields the row form needs. The state helpers keep the four
/// row states (available, enrolled, low-seats, blocked) out of the template.
pub struct RegistrationRow {
    pub section: CatalogSection,
    /// A fresh key per rendered form: refreshing after a submit replays the
    /// same key and gets the original receipt instead of a second seat.
    pub idempotency_key: Uuid,
    /// Set only on a fragment re-render after a rejected register — the
    /// specific reason (full, prerequisite, hold, conflict, …) to name in
    /// the row.
    pub denial: Option<String>,
}

impl RegistrationRow {
    pub fn new(section: CatalogSection, denial: Option<String>) -> Self {
        Self {
            section,
            idempotency_key: Uuid::new_v4(),
            denial,
        }
    }

    pub fn remaining(&self) -> i32 {
        (self.section.capacity - self.section.enrolled_count).max(0)
    }
    pub fn is_enrolled(&self) -> bool {
        self.section.enrolled_enrollment_id.is_some()
    }
    pub fn is_full(&self) -> bool {
        !self.is_enrolled() && self.remaining() == 0
    }
    pub fn is_low(&self) -> bool {
        !self.is_enrolled() && self.remaining() > 0 && self.remaining() <= LOW_SEATS
    }
    /// Lowercase haystack for the in-page instant filter (course code, title,
    /// section) — matches the read-path search without a round trip.
    pub fn search_key(&self) -> String {
        format!(
            "{} {} {}",
            self.section.course_code, self.section.course_title, self.section.section_code
        )
        .to_lowercase()
    }
}

/// A single registration row rendered alone, for the Alpine-AJAX fragment
/// swap after a register/drop. The same markup the page uses per row, so the
/// swapped-in state is identical to a fresh page load.
#[derive(Template)]
#[template(path = "components/section_row.html")]
pub struct SectionRowFragment {
    pub row: RegistrationRow,
    pub csrf_token: String,
}

#[derive(Deserialize)]
struct CatalogPageQuery {
    q: Option<String>,
    #[serde(default)]
    page: u32,
}

/// Catalog search + section browse for the current term. Plain forms; the
/// register buttons post to /ui/registration/add with a server-minted
/// idempotency key.
#[get("/ui/catalog")]
async fn catalog_page(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<AcademicsService>,
    enrollment: web::Data<crate::enrollment::EnrollmentService>,
    query: web::Query<CatalogPageQuery>,
) -> Result<HttpResponse, AppError> {
    let q = query.q.as_deref().unwrap_or("");
    let term = service.current_term(&actor).await?;
    let rows = match &term {
        Some(term) => service
            .search_catalog(&actor, term.id, Some(q), query.page)
            .await?
            .into_iter()
            .map(|section| RegistrationRow::new(section, None))
            .collect(),
        None => Vec::new(),
    };

    // Conditions that block EVERY registration are named up front, loudly,
    // instead of being discovered one rejected click at a time. Rows stay
    // live — the enrollment service remains the enforcement point.
    let mut blocked = None;
    if let Some(term) = &term {
        let now = chrono::Utc::now();
        if now < term.registration_opens_at {
            blocked = Some("Registration has not opened yet for this term.".to_owned());
        } else if now >= term.add_drop_closes_at {
            blocked = Some("Add/drop has closed for this term.".to_owned());
        } else if actor.student_id.is_some()
            && !enrollment.own_holds(&actor, term.id).await?.is_empty()
        {
            blocked = Some(
                "Registration is on hold on your account. Clear the hold before adding or dropping classes."
                    .to_owned(),
            );
        }
    }

    let body = CatalogPage {
        csrf_token: current.csrf_token.clone(),
        term,
        q: q.to_owned(),
        page: query.page,
        rows,
        blocked,
    }
    .render()?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(body))
}

/// Renders the registration screen with all four row states and no
/// database, for the frontend axe harness (`render-pages`).
pub fn sample_catalog_html() -> Result<String, askama::Error> {
    use chrono::{Duration, Utc};
    let now = Utc::now();
    let section = |code: &str, title: &str, sec: &str, capacity, enrolled, mine: Option<Uuid>| {
        CatalogSection {
            section_id: Uuid::new_v4(),
            course_code: code.to_owned(),
            course_title: title.to_owned(),
            credit_hours: 3.0,
            section_code: sec.to_owned(),
            capacity,
            enrolled_count: enrolled,
            meetings: "Mon/Wed 09:00-10:15".to_owned(),
            enrolled_enrollment_id: mine,
        }
    };
    CatalogPage {
        csrf_token: "sample".to_owned(),
        term: Some(TermSummary {
            id: Uuid::nil(),
            code: "FA26".into(),
            name: "Fall 2026".into(),
            starts_on: now.date_naive(),
            ends_on: (now + Duration::days(100)).date_naive(),
            registration_opens_at: now - Duration::days(20),
            add_drop_closes_at: now + Duration::days(14),
            grade_entry_closes_at: None,
        }),
        q: String::new(),
        page: 0,
        rows: vec![
            RegistrationRow::new(
                section("CMPS 3141", "Software engineering", "01", 30, 12, None),
                None,
            ),
            RegistrationRow::new(
                section(
                    "CMPS 2131",
                    "Data structures",
                    "01",
                    30,
                    22,
                    Some(Uuid::nil()),
                ),
                None,
            ),
            RegistrationRow::new(
                section("PHYS 2101", "Mechanics", "01", 40, 40, None),
                Some("prerequisite requirements are not satisfied".to_owned()),
            ),
            RegistrationRow::new(
                section("MATH 3201", "Linear algebra", "02", 25, 22, None),
                None,
            ),
        ],
        blocked: None,
    }
    .render()
}

// ---- Registrar overview (scanning density, FRONTEND.md §6) ---------------

/// One dense-table row wrapping the query row with its display helpers.
pub struct OverviewRow {
    pub section: SectionOverviewRow,
}

impl OverviewRow {
    pub fn fill_pct(&self) -> i32 {
        if self.section.capacity <= 0 {
            return 0;
        }
        (self.section.enrolled_count * 100) / self.section.capacity
    }
    /// Same thresholds the student registration row uses: full is danger,
    /// three seats or fewer is amber.
    pub fn fill_state(&self) -> &'static str {
        let remaining = self.section.capacity - self.section.enrolled_count;
        if remaining <= 0 {
            "full"
        } else if remaining <= 3 {
            "low"
        } else {
            "ok"
        }
    }
    pub fn unassigned(&self) -> bool {
        self.section.instructors.is_empty()
    }
    pub fn search_key(&self) -> String {
        format!(
            "{} {} {} {}",
            self.section.course_code,
            self.section.course_title,
            self.section.section_code,
            self.section.instructors
        )
        .to_lowercase()
    }
}

/// A problem the registrar should look at, derived from the same committed
/// rows the table shows — never a separately maintained flag.
pub struct AttentionItem {
    pub severity: &'static str, // "danger" | "warn"
    pub title: String,
    pub detail: String,
}

/// One term window with its computed state; `state` feeds the ONE badge map.
pub struct WindowRow {
    pub label: &'static str,
    pub range: String,
    pub state: &'static str, // "open" | "upcoming" | "closed"
    pub detail: String,
}

impl WindowRow {
    pub fn badge_class(&self) -> &'static str {
        crate::shared::assets::badge_class(self.state)
    }
}

fn day(at: &chrono::DateTime<chrono::Utc>) -> String {
    at.format("%b %-d, %Y").to_string()
}

/// The term's windows with honest open/upcoming/closed states. Registration
/// and add/drop are ONE shared window (ADR: single add/drop deadline).
fn window_rows(term: &TermSummary, now: chrono::DateTime<chrono::Utc>) -> Vec<WindowRow> {
    let mut rows = Vec::new();
    let (state, detail) = if now < term.registration_opens_at {
        (
            "upcoming",
            format!("Opens {}", day(&term.registration_opens_at)),
        )
    } else if now < term.add_drop_closes_at {
        let days_left = (term.add_drop_closes_at - now).num_days();
        ("open", format!("Open · {days_left} days left"))
    } else {
        ("closed", "Closed".to_owned())
    };
    rows.push(WindowRow {
        label: "Registration and add/drop",
        range: format!(
            "{} – {}",
            day(&term.registration_opens_at),
            day(&term.add_drop_closes_at)
        ),
        state,
        detail,
    });
    if let Some(closes) = term.grade_entry_closes_at {
        let (state, detail) = if now < closes {
            ("open", format!("Open · closes {}", day(&closes)))
        } else {
            ("closed", "Closed".to_owned())
        };
        rows.push(WindowRow {
            label: "Grade entry",
            range: format!("Closes {}", day(&closes)),
            state,
            detail,
        });
    }
    rows
}

#[derive(Template)]
#[template(path = "pages/registrar_overview.html")]
struct RegistrarOverviewPage {
    term: Option<TermSummary>,
    q: String,
    rows: Vec<OverviewRow>,
    course_count: usize,
    seats_filled: i64,
    seats_total: i64,
    holds_total: i64,
    holds_detail: String,
    attention: Vec<AttentionItem>,
    windows: Vec<WindowRow>,
    truncated: bool,
}

impl RegistrarOverviewPage {
    pub fn seats_pct(&self) -> i64 {
        if self.seats_total <= 0 {
            return 0;
        }
        (self.seats_filled * 100) / self.seats_total
    }
}

#[derive(Deserialize)]
struct OverviewQuery {
    q: Option<String>,
}

/// The registrar landing page: term at a glance, the needs-attention
/// worklist, window states, and the dense sections table. Read path — every
/// number comes from the same committed rows the table shows.
#[get("/ui/registrar")]
async fn registrar_overview_page(
    actor: Actor,
    service: web::Data<AcademicsService>,
    enrollment: web::Data<crate::enrollment::EnrollmentService>,
    query: web::Query<OverviewQuery>,
) -> Result<HttpResponse, AppError> {
    // The same policy the mutations use; checked here too so a termless
    // institution still 403s non-registrars instead of rendering.
    crate::academics::policy::require_can_manage_academics(&actor)?;
    let q = query.q.as_deref().unwrap_or("");
    let term = service.current_term(&actor).await?;

    let mut rows = Vec::new();
    let mut hold_counts = Vec::new();
    if let Some(term) = &term {
        rows = service
            .term_sections_overview(&actor, term.id, Some(q))
            .await?
            .into_iter()
            .map(|section| OverviewRow { section })
            .collect::<Vec<_>>();
        if actor.has_role(crate::shared::actor::Role::Registrar) {
            hold_counts = enrollment.term_hold_counts(&actor, term.id).await?;
        }
    }

    let course_count = {
        let mut codes: Vec<&str> = rows
            .iter()
            .map(|r| r.section.course_code.as_str())
            .collect();
        codes.dedup(); // rows arrive sorted by course code
        codes.len()
    };
    let seats_total: i64 = rows.iter().map(|r| i64::from(r.section.capacity)).sum();
    let seats_filled: i64 = rows
        .iter()
        .map(|r| i64::from(r.section.enrolled_count))
        .sum();
    let holds_total: i64 = hold_counts.iter().map(|(_, n)| n).sum();
    let holds_detail = hold_counts
        .iter()
        .take(3)
        .map(|(flag, n)| format!("{n} {flag}"))
        .collect::<Vec<_>>()
        .join(" · ");

    let mut attention: Vec<AttentionItem> = Vec::new();
    for row in &rows {
        let name = format!(
            "{} {} · {}",
            row.section.course_code, row.section.section_code, row.section.course_title
        );
        if row.fill_state() == "full" {
            attention.push(AttentionItem {
                severity: "danger",
                title: name.clone(),
                detail: format!(
                    "Section full · {} of {} seats taken",
                    row.section.enrolled_count, row.section.capacity
                ),
            });
        }
        if row.unassigned() {
            attention.push(AttentionItem {
                severity: "warn",
                title: name,
                detail: "No instructor assigned".to_owned(),
            });
        }
    }

    let windows = match &term {
        Some(term) => window_rows(term, chrono::Utc::now()),
        None => Vec::new(),
    };

    let truncated = rows.len() >= 500;
    let body = RegistrarOverviewPage {
        term,
        q: q.to_owned(),
        rows,
        course_count,
        seats_filled,
        seats_total,
        holds_total,
        holds_detail,
        attention,
        windows,
        truncated,
    }
    .render()?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(body))
}

/// Renders the registrar overview with every tile, queue, window, and row
/// state and no database, for the frontend axe harness.
pub fn sample_registrar_overview_html() -> Result<String, askama::Error> {
    use chrono::{Duration, Utc};
    let now = Utc::now();
    let section = |code: &str, title: &str, sec: &str, who: &str, capacity, enrolled| OverviewRow {
        section: SectionOverviewRow {
            course_code: code.to_owned(),
            course_title: title.to_owned(),
            section_code: sec.to_owned(),
            status: "open".to_owned(),
            meetings: "1 09:00-10:15, 3 09:00-10:15".to_owned(),
            instructors: who.to_owned(),
            capacity,
            enrolled_count: enrolled,
        },
    };
    let term = TermSummary {
        id: Uuid::nil(),
        code: "FA26".into(),
        name: "Fall 2026".into(),
        starts_on: now.date_naive(),
        ends_on: (now + Duration::days(100)).date_naive(),
        registration_opens_at: now - Duration::days(20),
        add_drop_closes_at: now + Duration::days(14),
        grade_entry_closes_at: Some(now + Duration::days(80)),
    };
    let windows = window_rows(&term, now);
    let mut rows = vec![
        section("CMPS 2131", "Data structures", "01", "alvarez", 40, 40),
        section("CMPS 3141", "Software engineering", "01", "nunez", 30, 12),
        section("ENGL 1101", "Composition", "03", "", 35, 18),
        section("MATH 3201", "Linear algebra", "02", "chen", 25, 22),
    ];
    rows[3].section.status = "cancelled".to_owned();
    RegistrarOverviewPage {
        term: Some(term),
        q: String::new(),
        rows,
        course_count: 4,
        seats_filled: 92,
        seats_total: 130,
        holds_total: 238,
        holds_detail: "203 advising · 35 financial".to_owned(),
        attention: vec![
            AttentionItem {
                severity: "danger",
                title: "CMPS 2131 01 · Data structures".into(),
                detail: "Section full · 40 of 40 seats taken".into(),
            },
            AttentionItem {
                severity: "warn",
                title: "ENGL 1101 03 · Composition".into(),
                detail: "No instructor assigned".into(),
            },
        ],
        windows,
        truncated: false,
    }
    .render()
}

// ---- Terms & windows management -------------------------------------------

/// One term row with its computed window state for the badge.
pub struct TermRow {
    pub term: TermSummary,
    pub state: &'static str, // "open" | "upcoming" | "closed"
}

impl TermRow {
    fn new(term: TermSummary, now: chrono::DateTime<chrono::Utc>) -> Self {
        let state = if now < term.registration_opens_at {
            "upcoming"
        } else if now < term.add_drop_closes_at {
            "open"
        } else {
            "closed"
        };
        Self { term, state }
    }
    pub fn badge_class(&self) -> &'static str {
        crate::shared::assets::badge_class(self.state)
    }
    /// Values for the datetime-local inputs of the edit form.
    pub fn opens_value(&self) -> String {
        self.term
            .registration_opens_at
            .format("%Y-%m-%dT%H:%M")
            .to_string()
    }
    pub fn closes_value(&self) -> String {
        self.term
            .add_drop_closes_at
            .format("%Y-%m-%dT%H:%M")
            .to_string()
    }
    pub fn grade_entry_value(&self) -> String {
        self.term
            .grade_entry_closes_at
            .map(|at| at.format("%Y-%m-%dT%H:%M").to_string())
            .unwrap_or_default()
    }
}

#[derive(Template)]
#[template(path = "pages/registrar_terms.html")]
struct RegistrarTermsPage<'a> {
    csrf_token: &'a str,
    terms: Vec<TermRow>,
    notice: Option<&'a str>,
    error: Option<&'a str>,
}

async fn render_terms(
    actor: &Actor,
    current: &CurrentSession,
    service: &AcademicsService,
    notice: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let now = chrono::Utc::now();
    let terms = service
        .list_terms(actor)
        .await?
        .into_iter()
        .map(|term| TermRow::new(term, now))
        .collect();
    Ok(RegistrarTermsPage {
        csrf_token: &current.csrf_token,
        terms,
        notice,
        error,
    }
    .render()?)
}

#[derive(Deserialize)]
struct TermsNoticeQuery {
    notice: Option<String>,
}

#[get("/ui/registrar/terms")]
async fn registrar_terms_page(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<AcademicsService>,
    query: web::Query<TermsNoticeQuery>,
) -> Result<HttpResponse, AppError> {
    crate::academics::policy::require_can_manage_academics(&actor)?;
    let notice = match query.notice.as_deref() {
        Some("created") => Some("Term created."),
        Some("windows") => Some("Windows updated."),
        _ => None,
    };
    let body = render_terms(&actor, &current, &service, notice, None).await?;
    Ok(html(StatusCode::OK, body))
}

/// `datetime-local` values ("2026-08-25T09:00"); stored as UTC — the labels
/// say UTC (assumption A30: institution-timezone entry is a later
/// refinement, storage is UTC either way per CLAUDE.md §3).
fn parse_utc_local(value: &str, field: &str) -> Result<chrono::DateTime<chrono::Utc>, AppError> {
    let value = value.trim();
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .map(|naive| naive.and_utc())
        .map_err(|_| AppError::Validation(format!("{field} must be a date and time")))
}

fn parse_optional_utc_local(
    value: Option<&str>,
    field: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, AppError> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => Ok(Some(parse_utc_local(value, field)?)),
        None => Ok(None),
    }
}

#[derive(Deserialize)]
struct CreateTermForm {
    code: String,
    name: String,
    starts_on: chrono::NaiveDate,
    ends_on: chrono::NaiveDate,
    registration_opens_at: String,
    add_drop_closes_at: String,
    grade_entry_closes_at: Option<String>,
    csrf_token: String,
}

#[post("/ui/registrar/terms")]
async fn create_term_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<AcademicsService>,
    form: web::Form<CreateTermForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token; // validated by the CSRF middleware

    let outcome = async {
        let command = CreateTermCommand {
            code: form.code.clone(),
            name: form.name.clone(),
            starts_on: form.starts_on,
            ends_on: form.ends_on,
            registration_opens_at: parse_utc_local(
                &form.registration_opens_at,
                "registration opens",
            )?,
            add_drop_closes_at: parse_utc_local(&form.add_drop_closes_at, "add/drop closes")?,
            grade_entry_closes_at: parse_optional_utc_local(
                form.grade_entry_closes_at.as_deref(),
                "grade entry closes",
            )?,
        };
        service.create_term(&actor, command).await
    }
    .await;

    match outcome {
        Ok(_) => Ok(see_other("/ui/registrar/terms?notice=created")),
        Err(AppError::Validation(message)) => {
            let body = render_terms(&actor, &current, &service, None, Some(&message)).await?;
            Ok(html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(AppError::Conflict(message)) => {
            let body = render_terms(&actor, &current, &service, None, Some(&message)).await?;
            Ok(html(StatusCode::CONFLICT, body))
        }
        Err(other) => Err(other),
    }
}

#[derive(Deserialize)]
struct UpdateWindowsForm {
    registration_opens_at: String,
    add_drop_closes_at: String,
    grade_entry_closes_at: Option<String>,
    csrf_token: String,
}

#[post("/ui/registrar/terms/{term_id}/windows")]
async fn update_windows_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<AcademicsService>,
    term_id: web::Path<Uuid>,
    form: web::Form<UpdateWindowsForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;

    let outcome = async {
        let command = crate::academics::service::UpdateWindowsCommand {
            registration_opens_at: parse_utc_local(
                &form.registration_opens_at,
                "registration opens",
            )?,
            add_drop_closes_at: parse_utc_local(&form.add_drop_closes_at, "add/drop closes")?,
            grade_entry_closes_at: parse_optional_utc_local(
                form.grade_entry_closes_at.as_deref(),
                "grade entry closes",
            )?,
        };
        service
            .update_term_windows(&actor, term_id.into_inner(), command)
            .await
    }
    .await;

    match outcome {
        Ok(()) => Ok(see_other("/ui/registrar/terms?notice=windows")),
        Err(AppError::Validation(message)) => {
            let body = render_terms(&actor, &current, &service, None, Some(&message)).await?;
            Ok(html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(other) => Err(other),
    }
}

fn html(status: StatusCode, body: String) -> HttpResponse {
    HttpResponse::build(status)
        .content_type("text/html; charset=utf-8")
        .body(body)
}

fn see_other(location: &str) -> HttpResponse {
    HttpResponse::SeeOther()
        .insert_header(("Location", location.to_owned()))
        .finish()
}

/// Renders the terms page with open/upcoming/closed terms and no database,
/// for the frontend axe harness.
pub fn sample_registrar_terms_html() -> Result<String, askama::Error> {
    use chrono::{Duration, Utc};
    let now = Utc::now();
    let term = |code: &str, name: &str, opens: i64, closes: i64| TermSummary {
        id: Uuid::nil(),
        code: code.into(),
        name: name.into(),
        starts_on: (now + Duration::days(opens)).date_naive(),
        ends_on: (now + Duration::days(closes + 90)).date_naive(),
        registration_opens_at: now + Duration::days(opens),
        add_drop_closes_at: now + Duration::days(closes),
        grade_entry_closes_at: Some(now + Duration::days(closes + 60)),
    };
    RegistrarTermsPage {
        csrf_token: "sample",
        terms: vec![
            TermRow::new(term("SP27", "Spring 2027", 120, 140), now),
            TermRow::new(term("FA26", "Fall 2026", -20, 14), now),
            TermRow::new(term("SP26", "Spring 2026", -200, -180), now),
        ],
        notice: Some("Windows updated."),
        error: None,
    }
    .render()
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(create_term)
        .service(current_term)
        .service(create_course)
        .service(add_prerequisite)
        .service(create_section)
        .service(set_capacity)
        .service(add_meeting)
        .service(assign_instructor)
        .service(catalog)
        .service(catalog_page)
        .service(registrar_overview_page)
        .service(registrar_terms_page)
        .service(create_term_form)
        .service(update_windows_form);
}
