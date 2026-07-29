use crate::academics::AcademicsService;
use crate::academics::service::TermSummary;
use crate::identity_access::sessions::CurrentSession;
use crate::institution::InstitutionService;
use crate::institution::service::InstitutionEvent;
use crate::shared::{actor::Actor, error::AppError};
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, delete, get, post, web};
use askama::Template;
use serde::Deserialize;
use uuid::Uuid;

use crate::enrollment::types::{EnrollError, EnrolledSection};
use crate::enrollment::{EnrollmentService, GrantOverrideCommand, RegisterCommand};

#[derive(Template)]
#[template(path = "pages/dashboard.html")]
struct DashboardPage {
    term: Option<TermSummary>,
    holds: Vec<String>,
    enrollments: Vec<EnrolledSection>,
    /// Completed past terms only, published/amended grades only — a course
    /// in progress has no final grade, so the current term never appears.
    gpa_terms: Vec<crate::records::TermGpa>,
    events: Vec<InstitutionEvent>,
    cal: MiniCal,
}

impl DashboardPage {
    /// Whole days until add/drop closes (negative once it has closed).
    pub fn deadline_days(&self) -> i64 {
        self.term
            .as_ref()
            .map(|term| {
                (term.add_drop_closes_at.date_naive() - chrono::Utc::now().date_naive()).num_days()
            })
            .unwrap_or(0)
    }

    /// Credit-weighted GPA across all completed terms.
    pub fn cumulative_gpa(&self) -> String {
        let credits: f64 = self.gpa_terms.iter().map(|t| t.credits).sum();
        let quality: f64 = self.gpa_terms.iter().map(|t| t.quality_points).sum();
        if credits > 0.0 {
            format!("{:.2}", quality / credits)
        } else {
            "0.00".to_owned()
        }
    }
}

/// One cell of the dashboard mini calendar.
pub struct CalDay {
    pub day: u32,
    pub iso: String,
    pub today: bool,
    pub has_event: bool,
}

/// The current month as a Monday-first grid, event days marked. A visual
/// echo of the events list (aria-hidden in the template — the list itself
/// carries the accessible dates).
pub struct MiniCal {
    pub label: String,
    /// Blank leading cells before the 1st.
    pub lead: usize,
    pub days: Vec<CalDay>,
}

fn mini_cal(events: &[InstitutionEvent]) -> MiniCal {
    use chrono::{Datelike, Utc};
    // ponytail: "today" is the UTC date, not institution-local — the
    // marker can lag a few hours around midnight; thread the institution
    // timezone through if anyone ever notices.
    let today = Utc::now().date_naive();
    let this_first = today.with_day(1).expect("day 1 exists in every month");
    let next_of_this = this_first
        .checked_add_months(chrono::Months::new(1))
        .expect("next month exists");
    // Anchor on the current month when it has event days; otherwise jump
    // to the first upcoming event's month, so the grid always has
    // something to say (events is upcoming-only, sorted by start).
    let event_this_month = events.iter().any(|e| e.starts_on < next_of_this);
    let first = if events.is_empty() || event_this_month {
        this_first
    } else {
        events[0]
            .starts_on
            .with_day(1)
            .expect("day 1 exists in every month")
    };
    let next_month = first
        .checked_add_months(chrono::Months::new(1))
        .expect("next month exists");
    let count = (next_month - first).num_days() as u32;
    let days = (1..=count)
        .map(|n| {
            let date = first.with_day(n).expect("n is within the month");
            CalDay {
                day: n,
                iso: date.format("%Y-%m-%d").to_string(),
                today: date == today,
                has_event: events
                    .iter()
                    .any(|e| e.starts_on <= date && date <= e.ends_on),
            }
        })
        .collect();
    MiniCal {
        label: first.format("%B %Y").to_string(),
        lead: first.weekday().num_days_from_monday() as usize,
        days,
    }
}

/// Renders the dashboard with representative data and no database, for the
/// frontend axe harness (`render-pages`).
pub fn sample_dashboard_html() -> Result<String, askama::Error> {
    use chrono::{Duration, Utc};
    let now = Utc::now();
    let events = vec![
        InstitutionEvent {
            id: Uuid::nil(),
            title: "Independence Day".into(),
            event_type: "holiday".into(),
            starts_on: now.date_naive() + Duration::days(3),
            ends_on: now.date_naive() + Duration::days(3),
        },
        InstitutionEvent {
            id: Uuid::nil(),
            title: "Mid-semester examinations".into(),
            event_type: "academic".into(),
            starts_on: now.date_naive() + Duration::days(7),
            ends_on: now.date_naive() + Duration::days(11),
        },
    ];
    DashboardPage {
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
        holds: vec!["financial".into()],
        enrollments: vec![EnrolledSection {
            enrollment_id: Uuid::nil(),
            course_code: "CMPS 2131".into(),
            course_title: "Data structures".into(),
            section_code: "01".into(),
            credit_hours: 3.0,
            meetings: "Mon/Wed 09:00-10:15 · FSB 214".into(),
            instructors: "d.thompson".into(),
        }],
        gpa_terms: vec![
            crate::records::TermGpa {
                term_name: "Spring 2025".into(),
                credits: 9.0,
                quality_points: 21.0,
            },
            crate::records::TermGpa {
                term_name: "Fall 2025".into(),
                credits: 12.0,
                quality_points: 42.0,
            },
        ],
        cal: mini_cal(&events),
        events,
    }
    .render()
}

/// The student's home screen: current schedule strip, the add/drop deadline,
/// any registration holds (surfaced loudly), and upcoming campus events —
/// server-rendered, useful the instant the HTML arrives.
#[get("/ui/dashboard")]
pub async fn dashboard_page(
    actor: Actor,
    enrollment: web::Data<EnrollmentService>,
    academics: web::Data<AcademicsService>,
    institution: web::Data<InstitutionService>,
    grades: web::Data<crate::records::GradeService>,
) -> Result<HttpResponse, AppError> {
    let term = academics.current_term(&actor).await?;
    let (holds, enrollments) = match &term {
        Some(term) => (
            enrollment.own_holds(&actor, term.id).await?,
            enrollment.list_own_active(&actor, term.id).await?,
        ),
        None => (Vec::new(), Vec::new()),
    };
    let events = institution.upcoming_events(&actor).await?;
    let page = DashboardPage {
        term,
        holds,
        enrollments,
        gpa_terms: grades.own_term_gpas(&actor).await?,
        cal: mini_cal(&events),
        events,
    };
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(page.render()?))
}

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
    let receipt = service
        .register_self(&actor, body.into_inner())
        .await
        .map_err(AppError::from)?;
    Ok(HttpResponse::Created().json(receipt))
}

/// Registrar grants a registration override — the explicit, auditable record
/// (who, rule, why, expiry) that later gets stamped with the enrollment that
/// consumed it. Never a hidden boolean.
#[post("/api/v1/students/{student_id}/overrides")]
pub async fn grant_override(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    student_id: web::Path<Uuid>,
    body: web::Json<GrantOverrideCommand>,
) -> Result<HttpResponse, AppError> {
    let override_id = service
        .grant_override(&actor, student_id.into_inner(), body.into_inner())
        .await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "override_id": override_id })))
}

/// The student registration page: current-term enrollments with drop forms.
/// Plain HTML over full-page requests — no JavaScript required.
#[get("/ui/registration")]
pub async fn registration_page(
    actor: Actor,
    current: CurrentSession,
    enrollment: web::Data<EnrollmentService>,
    academics: web::Data<AcademicsService>,
    query: web::Query<NoticeQuery>,
) -> Result<HttpResponse, AppError> {
    // Notices are fixed strings selected by key — never echoed client input.
    let notice = match query.notice.as_deref() {
        Some("registered") => Some("You are registered."),
        Some("dropped") => Some("The enrollment was dropped."),
        _ => None,
    };
    render_registration(&actor, &current, &enrollment, &academics, notice, None)
        .await
        .map(|body| {
            HttpResponse::Ok()
                .content_type("text/html; charset=utf-8")
                .body(body)
        })
}

#[derive(Deserialize)]
pub struct NoticeQuery {
    notice: Option<String>,
}

/// Register from a form post. JavaScript-off path: success redirects (PRG);
/// a typed denial re-renders the registration page with the reason inline
/// and an honest 409. Enhanced path (`X-Fragment` header from app.js):
/// returns just the re-rendered section row in its committed state — the
/// server outcome, never an optimistic "Enrolled" (FRONTEND.md §3).
#[post("/ui/registration/add")]
pub async fn register_form(
    actor: Actor,
    current: CurrentSession,
    enrollment: web::Data<EnrollmentService>,
    academics: web::Data<AcademicsService>,
    req: actix_web::HttpRequest,
    form: web::Form<RegisterForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token; // validated by the CSRF middleware

    let section_id = form.section_id;
    let outcome = enrollment
        .register_self(
            &actor,
            RegisterCommand {
                section_id,
                idempotency_key: form.idempotency_key,
            },
        )
        .await;

    if is_fragment(&req) {
        return row_fragment(
            &actor,
            &current,
            &academics,
            section_id,
            denial_of(outcome)?,
        )
        .await;
    }
    finish_ui_action(
        &actor,
        &current,
        &enrollment,
        &academics,
        outcome.map(|_| "registered"),
    )
    .await
}

#[post("/ui/registration/drop")]
pub async fn drop_form(
    actor: Actor,
    current: CurrentSession,
    enrollment: web::Data<EnrollmentService>,
    academics: web::Data<AcademicsService>,
    req: actix_web::HttpRequest,
    form: web::Form<DropForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;

    // Resolve the section before the drop so the fragment can render that
    // section's row back to its available state afterward.
    let section_id = enrollment
        .enrollment_section(&actor, form.enrollment_id)
        .await?;
    let outcome = enrollment.drop_self(&actor, form.enrollment_id).await;

    if is_fragment(&req) {
        let section_id = section_id.ok_or(AppError::NotFound)?;
        return row_fragment(
            &actor,
            &current,
            &academics,
            section_id,
            denial_of(outcome)?,
        )
        .await;
    }
    finish_ui_action(
        &actor,
        &current,
        &enrollment,
        &academics,
        outcome.map(|_| "dropped"),
    )
    .await
}

/// True when app.js asked for a single-row fragment rather than a full page.
fn is_fragment(req: &actix_web::HttpRequest) -> bool {
    req.headers().contains_key("X-Fragment")
}

/// Collapse an enroll/drop outcome into an optional denial reason for the
/// row fragment; a non-business error keeps its shape and propagates.
fn denial_of<T>(outcome: Result<T, EnrollError>) -> Result<Option<String>, AppError> {
    match outcome {
        Ok(_) => Ok(None),
        Err(EnrollError::Denied(denial)) => Ok(Some(denial.message().to_owned())),
        Err(EnrollError::App(error)) => Err(error),
    }
}

/// Render one section's row in its committed state (with an optional named
/// denial reason) and return it with an honest status: 409 when the action
/// was refused, 200 when it stuck.
async fn row_fragment(
    actor: &Actor,
    current: &CurrentSession,
    academics: &AcademicsService,
    section_id: Uuid,
    denial: Option<String>,
) -> Result<HttpResponse, AppError> {
    let status = if denial.is_some() {
        StatusCode::CONFLICT
    } else {
        StatusCode::OK
    };
    let body = match academics.registration_row(actor, section_id).await? {
        Some(section) => crate::academics::http::SectionRowFragment {
            row: crate::academics::http::RegistrationRow::new(section, denial),
            csrf_token: current.csrf_token.clone(),
        }
        .render()?,
        None => {
            "<tr><td colspan=\"3\" class=\"c-empty\">This section is no longer available.</td></tr>"
                .to_owned()
        }
    };
    Ok(HttpResponse::build(status)
        .content_type("text/html; charset=utf-8")
        .body(body))
}

/// Success ⇒ 303 back to the page (a refresh never resubmits); a denial ⇒
/// the same page with the reason inline. Anything else keeps its error shape.
async fn finish_ui_action(
    actor: &Actor,
    current: &CurrentSession,
    enrollment: &EnrollmentService,
    academics: &AcademicsService,
    outcome: Result<&'static str, EnrollError>,
) -> Result<HttpResponse, AppError> {
    match outcome {
        Ok(notice) => Ok(HttpResponse::SeeOther()
            .insert_header(("Location", format!("/ui/registration?notice={notice}")))
            .finish()),
        Err(EnrollError::Denied(denial)) => {
            let body = render_registration(
                actor,
                current,
                enrollment,
                academics,
                None,
                Some(denial.message()),
            )
            .await?;
            Ok(HttpResponse::build(StatusCode::CONFLICT)
                .content_type("text/html; charset=utf-8")
                .body(body))
        }
        Err(EnrollError::App(error)) => Err(error),
    }
}

#[derive(Template)]
#[template(path = "pages/registration.html")]
struct RegistrationPage<'a> {
    csrf_token: &'a str,
    term: Option<TermSummary>,
    enrollments: Vec<EnrolledSection>,
    notice: Option<&'a str>,
    error: Option<&'a str>,
}

impl RegistrationPage<'_> {
    pub fn credits_total(&self) -> f64 {
        crate::enrollment::types::credits_total(&self.enrollments)
    }
}

/// Renders the "My registration" page with representative data and no
/// database, for the frontend axe harness.
pub fn sample_registration_html() -> Result<String, askama::Error> {
    use chrono::{Duration, Utc};
    let now = Utc::now();
    RegistrationPage {
        csrf_token: "sample",
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
        enrollments: vec![
            EnrolledSection {
                enrollment_id: Uuid::nil(),
                course_code: "CMPS 2131".into(),
                course_title: "Data structures".into(),
                section_code: "01".into(),
                credit_hours: 3.0,
                meetings: "Mon/Wed 09:00-10:15 · FSB 214".into(),
                instructors: "d.thompson".into(),
            },
            EnrolledSection {
                enrollment_id: Uuid::nil(),
                course_code: "MATH 3201".into(),
                course_title: "Linear algebra".into(),
                section_code: "02".into(),
                credit_hours: 4.0,
                meetings: "Tue/Thu 13:00-14:15".into(),
                instructors: String::new(),
            },
        ],
        notice: Some("You are registered."),
        error: None,
    }
    .render()
}

async fn render_registration(
    actor: &Actor,
    current: &CurrentSession,
    enrollment: &EnrollmentService,
    academics: &AcademicsService,
    notice: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let term = academics.current_term(actor).await?;
    let enrollments = match &term {
        Some(term) => enrollment.list_own_active(actor, term.id).await?,
        None => Vec::new(),
    };
    Ok(RegistrationPage {
        csrf_token: &current.csrf_token,
        term,
        enrollments,
        notice,
        error,
    }
    .render()?)
}

#[derive(Deserialize)]
pub struct PlaceHoldBody {
    term_id: Uuid,
    flag: String,
    reason: String,
}

#[post("/api/v1/students/{student_id}/holds")]
pub async fn place_hold(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    student_id: web::Path<Uuid>,
    body: web::Json<PlaceHoldBody>,
) -> Result<HttpResponse, AppError> {
    service
        .place_hold(
            &actor,
            student_id.into_inner(),
            body.term_id,
            &body.flag,
            &body.reason,
        )
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

#[delete("/api/v1/students/{student_id}/terms/{term_id}/holds/{flag}")]
pub async fn release_hold(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    path: web::Path<(Uuid, Uuid, String)>,
) -> Result<HttpResponse, AppError> {
    let (student_id, term_id, flag) = path.into_inner();
    service
        .release_hold(&actor, student_id, term_id, &flag)
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

#[derive(Deserialize)]
pub struct DropForm {
    enrollment_id: Uuid,
    csrf_token: String,
}

// ---------------------------------------------------------------------------
// Registrar: student lookup, holds, academic status (scanning pages).
// ---------------------------------------------------------------------------

use crate::enrollment::service::StudentHit;

#[derive(Template)]
#[template(path = "pages/registrar_students.html")]
struct RegistrarStudentsPage {
    q: String,
    rows: Vec<StudentHit>,
}

#[derive(Deserialize)]
pub struct StudentSearchQuery {
    q: Option<String>,
}

#[get("/ui/registrar/students")]
async fn registrar_students_page(
    actor: Actor,
    service: web::Data<EnrollmentService>,
    query: web::Query<StudentSearchQuery>,
) -> Result<HttpResponse, AppError> {
    crate::enrollment::policy::require_can_manage_holds(&actor)?;
    let q = query.q.as_deref().unwrap_or("").trim().to_owned();
    let rows = if q.is_empty() {
        Vec::new()
    } else {
        service.search_students(&actor, &q).await?
    };
    let body = RegistrarStudentsPage { q, rows }.render()?;
    Ok(page_html(StatusCode::OK, body))
}

/// One option in the override section select.
pub struct SectionOption {
    pub id: Uuid,
    pub label: String,
}

#[derive(Template)]
#[template(path = "pages/registrar_student_detail.html")]
struct RegistrarStudentPage<'a> {
    csrf_token: &'a str,
    student: StudentHit,
    term: Option<TermSummary>,
    holds: Vec<String>,
    sections: Vec<SectionOption>,
    override_types: &'static [&'static str],
    notice: Option<&'a str>,
    error: Option<&'a str>,
}

impl RegistrarStudentPage<'_> {
    pub fn status_badge(&self) -> &'static str {
        crate::shared::assets::badge_class(&self.student.academic_status)
    }
}

async fn render_student_detail(
    actor: &Actor,
    current: &CurrentSession,
    enrollment: &EnrollmentService,
    academics: &AcademicsService,
    student_id: Uuid,
    notice: Option<&str>,
    error: Option<&str>,
) -> Result<String, AppError> {
    let student = enrollment
        .student_admin(actor, student_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let term = academics.current_term(actor).await?;
    let (holds, sections) = match &term {
        Some(term) => (
            enrollment.student_holds(actor, student_id, term.id).await?,
            academics
                .term_sections_overview(actor, term.id, None)
                .await?
                .into_iter()
                .map(|s| SectionOption {
                    id: s.section_id,
                    label: format!("{} {}", s.course_code, s.section_code),
                })
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    Ok(RegistrarStudentPage {
        csrf_token: &current.csrf_token,
        student,
        term,
        holds,
        sections,
        override_types: &crate::enrollment::service::OVERRIDE_TYPES,
        notice,
        error,
    }
    .render()?)
}

#[derive(Deserialize)]
pub struct StudentNoticeQuery {
    notice: Option<String>,
}

#[get("/ui/registrar/students/{student_id}")]
async fn registrar_student_detail_page(
    actor: Actor,
    current: CurrentSession,
    enrollment: web::Data<EnrollmentService>,
    academics: web::Data<AcademicsService>,
    student_id: web::Path<Uuid>,
    query: web::Query<StudentNoticeQuery>,
) -> Result<HttpResponse, AppError> {
    let notice = match query.notice.as_deref() {
        Some("status") => Some("Academic status updated."),
        Some("hold-placed") => Some("Hold placed."),
        Some("hold-released") => Some("Hold released."),
        Some("override") => Some("Override granted and recorded."),
        _ => None,
    };
    let body = render_student_detail(
        &actor,
        &current,
        &enrollment,
        &academics,
        student_id.into_inner(),
        notice,
        None,
    )
    .await?;
    Ok(page_html(StatusCode::OK, body))
}

#[derive(Deserialize)]
pub struct StatusForm {
    academic_status: String,
    csrf_token: String,
}

#[post("/ui/registrar/students/{student_id}/status")]
async fn set_status_form(
    actor: Actor,
    current: CurrentSession,
    enrollment: web::Data<EnrollmentService>,
    academics: web::Data<AcademicsService>,
    student_id: web::Path<Uuid>,
    form: web::Form<StatusForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    let student_id = student_id.into_inner();
    match enrollment
        .set_academic_status(&actor, student_id, &form.academic_status)
        .await
    {
        Ok(()) => Ok(redirect(&format!(
            "/ui/registrar/students/{student_id}?notice=status"
        ))),
        Err(AppError::Validation(message)) => {
            let body = render_student_detail(
                &actor,
                &current,
                &enrollment,
                &academics,
                student_id,
                None,
                Some(&message),
            )
            .await?;
            Ok(page_html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(other) => Err(other),
    }
}

#[derive(Deserialize)]
pub struct PlaceHoldForm {
    term_id: Uuid,
    flag: String,
    reason: String,
    csrf_token: String,
}

#[post("/ui/registrar/students/{student_id}/holds")]
async fn place_hold_form(
    actor: Actor,
    current: CurrentSession,
    enrollment: web::Data<EnrollmentService>,
    academics: web::Data<AcademicsService>,
    student_id: web::Path<Uuid>,
    form: web::Form<PlaceHoldForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    let student_id = student_id.into_inner();
    match enrollment
        .place_hold(&actor, student_id, form.term_id, &form.flag, &form.reason)
        .await
    {
        Ok(()) => Ok(redirect(&format!(
            "/ui/registrar/students/{student_id}?notice=hold-placed"
        ))),
        Err(AppError::Validation(message)) => {
            let body = render_student_detail(
                &actor,
                &current,
                &enrollment,
                &academics,
                student_id,
                None,
                Some(&message),
            )
            .await?;
            Ok(page_html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(other) => Err(other),
    }
}

#[derive(Deserialize)]
pub struct ReleaseHoldForm {
    term_id: Uuid,
    flag: String,
    csrf_token: String,
}

#[post("/ui/registrar/students/{student_id}/holds/release")]
async fn release_hold_form(
    actor: Actor,
    enrollment: web::Data<EnrollmentService>,
    student_id: web::Path<Uuid>,
    form: web::Form<ReleaseHoldForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    let student_id = student_id.into_inner();
    enrollment
        .release_hold(&actor, student_id, form.term_id, &form.flag)
        .await?;
    Ok(redirect(&format!(
        "/ui/registrar/students/{student_id}?notice=hold-released"
    )))
}

#[derive(Deserialize)]
pub struct GrantOverrideForm {
    term_id: Uuid,
    /// Empty string means the override covers the whole term.
    section_id: String,
    override_type: String,
    reason: String,
    expires_at: Option<String>,
    csrf_token: String,
}

#[post("/ui/registrar/students/{student_id}/overrides")]
async fn grant_override_form(
    actor: Actor,
    current: CurrentSession,
    enrollment: web::Data<EnrollmentService>,
    academics: web::Data<AcademicsService>,
    student_id: web::Path<Uuid>,
    form: web::Form<GrantOverrideForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    let student_id = student_id.into_inner();
    let outcome = async {
        let section_id = match form.section_id.trim() {
            "" => None,
            raw => Some(raw.parse::<Uuid>().map_err(|_| {
                AppError::Validation("the section choice was not recognized".into())
            })?),
        };
        let command = GrantOverrideCommand {
            term_id: form.term_id,
            section_id,
            override_type: form.override_type.clone(),
            reason: form.reason.clone(),
            expires_at: crate::academics::http::parse_optional_utc_local(
                form.expires_at.as_deref(),
                "override expiry",
            )?,
        };
        enrollment.grant_override(&actor, student_id, command).await
    }
    .await;
    match outcome {
        Ok(_) => Ok(redirect(&format!(
            "/ui/registrar/students/{student_id}?notice=override"
        ))),
        Err(AppError::Validation(message)) => {
            let body = render_student_detail(
                &actor,
                &current,
                &enrollment,
                &academics,
                student_id,
                None,
                Some(&message),
            )
            .await?;
            Ok(page_html(StatusCode::UNPROCESSABLE_ENTITY, body))
        }
        Err(other) => Err(other),
    }
}

// ---------------------------------------------------------------------------
// Registrar: override review list.
// ---------------------------------------------------------------------------

use crate::enrollment::service::OverrideRow;

/// One review row with its computed state — the record is the override.
pub struct OverrideView {
    pub row: OverrideRow,
}

impl OverrideView {
    /// consumed > expired > active: a consumed override did its work even
    /// if its expiry has since passed.
    pub fn state(&self) -> &'static str {
        if self.row.consumed_at.is_some() {
            "consumed"
        } else if self
            .row
            .expires_at
            .is_some_and(|at| at <= chrono::Utc::now())
        {
            "expired"
        } else {
            "active"
        }
    }
    pub fn badge_class(&self) -> &'static str {
        crate::shared::assets::badge_class(self.state())
    }
    pub fn granted_on(&self) -> String {
        self.row.created_at.format("%b %-d, %Y %H:%M").to_string()
    }
    pub fn expires(&self) -> String {
        match self.row.expires_at {
            Some(at) => at.format("%b %-d, %Y %H:%M").to_string(),
            None => "—".to_owned(),
        }
    }
    pub fn scope(&self) -> &str {
        if self.row.section_label.is_empty() {
            "whole term"
        } else {
            &self.row.section_label
        }
    }
    pub fn search_key(&self) -> String {
        format!(
            "{} {} {}",
            self.row.student_number, self.row.override_type, self.row.granted_by
        )
        .to_lowercase()
    }
}

#[derive(Template)]
#[template(path = "pages/registrar_overrides.html")]
struct RegistrarOverridesPage {
    term: Option<TermSummary>,
    rows: Vec<OverrideView>,
}

#[get("/ui/registrar/overrides")]
async fn registrar_overrides_page(
    actor: Actor,
    enrollment: web::Data<EnrollmentService>,
    academics: web::Data<AcademicsService>,
) -> Result<HttpResponse, AppError> {
    crate::enrollment::policy::require_can_grant_override(&actor)?;
    let term = academics.current_term(&actor).await?;
    let rows = match &term {
        Some(term) => enrollment
            .list_overrides(&actor, term.id)
            .await?
            .into_iter()
            .map(|row| OverrideView { row })
            .collect(),
        None => Vec::new(),
    };
    let body = RegistrarOverridesPage { term, rows }.render()?;
    Ok(page_html(StatusCode::OK, body))
}

/// Renders the override review list with every state and no database, for
/// the frontend axe harness.
pub fn sample_registrar_overrides_html() -> Result<String, askama::Error> {
    use chrono::{Duration, Utc};
    let now = Utc::now();
    let row = |number: &str,
               kind: &str,
               section: &str,
               note: &str,
               expires: Option<chrono::DateTime<Utc>>,
               consumed: Option<chrono::DateTime<Utc>>| {
        OverrideView {
            row: OverrideRow {
                student_number: number.into(),
                override_type: kind.into(),
                section_label: section.into(),
                granted_by: "m.santos".into(),
                note: note.into(),
                created_at: now - Duration::days(2),
                expires_at: expires,
                consumed_at: consumed,
            },
        }
    };
    RegistrarOverridesPage {
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
        rows: vec![
            row(
                "A-001",
                "capacity",
                "CMPS 2131 01",
                "graduating senior needs this section",
                Some(now + Duration::days(5)),
                Some(now - Duration::days(1)),
            ),
            row(
                "A-002",
                "deadline",
                "",
                "medical leave during add/drop",
                Some(now + Duration::days(3)),
                None,
            ),
            row(
                "A-003",
                "prerequisite",
                "MATH 3201 02",
                "equivalent transfer credit under review",
                Some(now - Duration::days(1)),
                None,
            ),
        ],
    }
    .render()
}

fn page_html(status: StatusCode, body: String) -> HttpResponse {
    HttpResponse::build(status)
        .content_type("text/html; charset=utf-8")
        .body(body)
}

fn redirect(location: &str) -> HttpResponse {
    HttpResponse::SeeOther()
        .insert_header(("Location", location.to_owned()))
        .finish()
}

/// Renders the student lookup + detail pages with representative data and
/// no database, for the frontend axe harness.
pub fn sample_registrar_students_html() -> Result<String, askama::Error> {
    RegistrarStudentsPage {
        q: "A-00".into(),
        rows: vec![
            StudentHit {
                student_id: Uuid::nil(),
                student_number: "A-001".into(),
                program_code: "CS".into(),
                academic_status: "good_standing".into(),
                username: "maria.santos".into(),
            },
            StudentHit {
                student_id: Uuid::nil(),
                student_number: "A-002".into(),
                program_code: "MATH".into(),
                academic_status: "suspended".into(),
                username: "j.reyes".into(),
            },
        ],
    }
    .render()
}

pub fn sample_registrar_student_detail_html() -> Result<String, askama::Error> {
    use chrono::{Duration, Utc};
    let now = Utc::now();
    RegistrarStudentPage {
        csrf_token: "sample",
        student: StudentHit {
            student_id: Uuid::nil(),
            student_number: "A-002".into(),
            program_code: "MATH".into(),
            academic_status: "suspended".into(),
            username: "j.reyes".into(),
        },
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
        holds: vec!["financial".into(), "disciplinary".into()],
        sections: vec![SectionOption {
            id: Uuid::nil(),
            label: "CMPS 2131 01".into(),
        }],
        override_types: &crate::enrollment::service::OVERRIDE_TYPES,
        notice: Some("Hold placed."),
        error: None,
    }
    .render()
}

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(register_json)
        .service(grant_override)
        .service(place_hold)
        .service(release_hold)
        .service(dashboard_page)
        .service(registration_page)
        .service(register_form)
        .service(drop_form)
        .service(registrar_students_page)
        .service(registrar_student_detail_page)
        .service(set_status_form)
        .service(place_hold_form)
        .service(release_hold_form)
        .service(grant_override_form)
        .service(registrar_overrides_page);
}

#[cfg(test)]
mod mini_cal_tests {
    use super::*;
    use chrono::{Datelike, Utc};

    #[test]
    fn mini_cal_covers_the_current_month_and_marks_event_spans() {
        let today = Utc::now().date_naive();
        let event = InstitutionEvent {
            id: Uuid::nil(),
            title: "x".into(),
            event_type: "holiday".into(),
            starts_on: today,
            ends_on: today,
        };
        let cal = mini_cal(&[event]);
        assert!(cal.lead < 7);
        assert!((28..=31).contains(&cal.days.len()));
        assert_eq!(cal.days.iter().filter(|d| d.today).count(), 1);
        let today_cell = cal.days.iter().find(|d| d.today).unwrap();
        assert!(today_cell.has_event);
        assert_eq!(today_cell.iso, today.format("%Y-%m-%d").to_string());
        assert_eq!(cal.days[0].day, 1);
        assert_eq!(cal.days.last().unwrap().day, cal.days.len() as u32);
        assert!(cal.label.contains(&today.year().to_string()));
    }

    #[test]
    fn mini_cal_jumps_to_the_first_event_month_when_this_month_is_empty() {
        let today = Utc::now().date_naive();
        let future = today
            .checked_add_months(chrono::Months::new(2))
            .unwrap()
            .with_day(15)
            .unwrap();
        let event = InstitutionEvent {
            id: Uuid::nil(),
            title: "x".into(),
            event_type: "holiday".into(),
            starts_on: future,
            ends_on: future,
        };
        let cal = mini_cal(&[event]);
        assert!(cal.days.iter().all(|d| !d.today));
        assert!(cal.days.iter().any(|d| d.has_event && d.day == 15));
        // No events at all: fall back to the current month.
        assert_eq!(mini_cal(&[]).label, today.format("%B %Y").to_string());
    }
}
