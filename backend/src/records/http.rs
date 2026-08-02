use crate::academics::AcademicsService;
use crate::academics::service::TermSummary;
use crate::identity_access::sessions::CurrentSession;
use crate::shared::actor::Role;
use crate::shared::{actor::Actor, error::AppError};
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, get, post, web};
use askama::Template;
use serde::Deserialize;
use uuid::Uuid;

use crate::records::grades::{
    CorrectGradeCommand, HistoryRow, InstructorSection, RosterView, SaveGradeCommand,
    StudentGradeRow,
};
use crate::records::transcript::{SnapshotSummary, StudentIdentity};
use crate::records::{GradeService, ScheduleQuery, TranscriptSnapshotService};
use sqlx::PgPool;

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
    Ok(HttpResponse::Ok().json(service.student_grades(&actor, query.term_id).await?))
}

#[get("/api/v1/me/schedule")]
pub async fn schedule_json(
    actor: Actor,
    query_service: web::Data<ScheduleQuery>,
    query: web::Query<TermQuery>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(query_service.for_student(&actor, query.term_id).await?))
}

/// Draft entry: assigned instructor (inside the entry window) or records
/// officer. Returns the new optimistic version.
#[post("/api/v1/grades/draft")]
pub async fn save_draft_json(
    actor: Actor,
    service: web::Data<GradeService>,
    body: web::Json<SaveGradeCommand>,
) -> Result<HttpResponse, AppError> {
    let version = service.save_draft(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "version": version })))
}

/// Records-office correction of a published grade (reason required).
#[post("/api/v1/grades/correct")]
pub async fn correct_grade_json(
    actor: Actor,
    service: web::Data<GradeService>,
    body: web::Json<CorrectGradeCommand>,
) -> Result<HttpResponse, AppError> {
    let version = service.correct_grade(&actor, body.into_inner()).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "version": version })))
}

/// Publish every draft grade of a section (records officer).
#[post("/api/v1/sections/{section_id}/grades/publish")]
pub async fn publish_section_json(
    actor: Actor,
    service: web::Data<GradeService>,
    section_id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let published = service
        .publish_section(&actor, section_id.into_inner())
        .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "published": published })))
}

/// The instructor's own assigned sections.
#[get("/api/v1/instructor/sections")]
pub async fn instructor_sections_json(
    actor: Actor,
    service: web::Data<GradeService>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(service.instructor_sections(&actor).await?))
}

/// Roster with grade states: assigned instructor or records officer.
#[get("/api/v1/sections/{section_id}/roster")]
pub async fn roster_json(
    actor: Actor,
    service: web::Data<GradeService>,
    section_id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok().json(service.roster(&actor, section_id.into_inner()).await?))
}

/// Records officer freezes the student's published record into an immutable
/// snapshot (new monotonic version).
#[post("/api/v1/students/{student_id}/transcript-snapshots")]
pub async fn generate_snapshot_json(
    actor: Actor,
    service: web::Data<TranscriptSnapshotService>,
    pool: web::Data<PgPool>,
    student_id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let snapshot_id = service
        .generate(
            &pool,
            &crate::audit::AuditWriter,
            &actor,
            student_id.into_inner(),
        )
        .await?;
    Ok(HttpResponse::Created().json(serde_json::json!({ "snapshot_id": snapshot_id })))
}

/// The student's own academic history and snapshot list.
#[get("/api/v1/me/history")]
pub async fn history_json(
    actor: Actor,
    grades: web::Data<GradeService>,
    snapshots: web::Data<TranscriptSnapshotService>,
    pool: web::Data<PgPool>,
) -> Result<HttpResponse, AppError> {
    let courses = grades.academic_history(&actor).await?;
    let snapshots = snapshots.own_snapshots(&pool, &actor).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "courses": courses,
        "snapshots": snapshots,
    })))
}

// ---------------------------------------------------------------------------
// Server-rendered pages (plain forms, no JavaScript required).
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "pages/instructor_sections.html")]
struct InstructorSectionsPage {
    sections: Vec<InstructorSection>,
}

#[get("/ui/instructor")]
pub async fn instructor_sections_page(
    actor: Actor,
    service: web::Data<GradeService>,
) -> Result<HttpResponse, AppError> {
    let page = InstructorSectionsPage {
        sections: service.instructor_sections(&actor).await?,
    };
    Ok(html(StatusCode::OK, page.render()?))
}

/// Renders the instructor landing with representative data and no
/// database, for the frontend axe harness (`render-pages`).
pub fn sample_instructor_sections_html() -> Result<String, askama::Error> {
    InstructorSectionsPage {
        sections: vec![InstructorSection {
            section_id: Uuid::nil(),
            course_code: "CMPS 2131".into(),
            course_title: "Data structures".into(),
            section_code: "01".into(),
            term_name: "Fall 2026".into(),
            enrolled_count: 40,
        }],
    }
    .render()
}

#[derive(Template)]
#[template(path = "pages/roster.html")]
struct RosterPage<'a> {
    csrf_token: &'a str,
    view: RosterView,
    can_publish: bool,
    notice: Option<&'a str>,
    error: Option<&'a str>,
    /// The row a denial belongs to — its message renders inline under that
    /// row's select instead of as a page banner.
    error_enrollment: Option<Uuid>,
    /// False outside the grade-entry window (for non-officers): selects
    /// render read-only with the window banner explaining why.
    entry_enabled: bool,
    window_kind: &'static str,
    window_note: String,
    /// The section-switcher menu: the viewing instructor's own assignments
    /// (empty for a records officer — no menu renders).
    switcher: Vec<InstructorSection>,
}

impl RosterPage<'_> {
    /// The denial message for one row, when the failed save was that row's.
    fn row_error(&self, row: &crate::records::grades::RosterRow) -> Option<&str> {
        (self.error_enrollment == Some(row.enrollment_id))
            .then_some(self.error)
            .flatten()
    }
}

/// Renders the grade-entry roster with every row state (not entered, draft,
/// published, inline error) and no database, for the frontend axe harness.
pub fn sample_roster_html() -> Result<String, askama::Error> {
    use crate::records::grades::{RosterRow, SectionHeader};
    let row = |number: &str, name: &str, grade: Option<&str>, state: Option<&str>| RosterRow {
        enrollment_id: Uuid::new_v4(),
        student_number: number.to_owned(),
        student_name: name.to_owned(),
        grade_code: grade.map(str::to_owned),
        state: state.map(str::to_owned),
        version: state.map(|_| 1),
    };
    let rows = vec![
        row("2024-00871", "Layla Ahmad", None, None),
        row("2023-01144", "Marcus Bennett", Some("B+"), Some("draft")),
        row("2024-00233", "Wei Chen", Some("A"), Some("published")),
        row("2024-00590", "Kwame Osei", Some("I"), Some("draft")),
    ];
    let error_enrollment = Some(rows[3].enrollment_id);
    RosterPage {
        csrf_token: "sample",
        view: RosterView {
            section: SectionHeader {
                section_id: Uuid::nil(),
                course_code: "CMPS 2131".into(),
                course_title: "Data structures".into(),
                section_code: "01".into(),
                term_name: "Fall 2026".into(),
                grade_entry_closes_at: Some(chrono::Utc::now() + chrono::Duration::days(14)),
            },
            rows,
        },
        can_publish: true,
        notice: None,
        error: Some("Incomplete needs a filed extension on record before it can be published."),
        error_enrollment,
        entry_enabled: true,
        window_kind: "success",
        window_note:
            "Grade entry open · closes Oct 21, 2026 17:00. Published grades are visible to students immediately."
                .to_owned(),
        switcher: vec![
            InstructorSection {
                section_id: Uuid::nil(),
                course_code: "CMPS 2131".into(),
                course_title: "Data structures".into(),
                section_code: "01".into(),
                term_name: "Fall 2026".into(),
                enrolled_count: 40,
            },
            InstructorSection {
                section_id: Uuid::new_v4(),
                course_code: "CMPS 3141".into(),
                course_title: "Software engineering".into(),
                section_code: "01".into(),
                term_name: "Fall 2026".into(),
                enrolled_count: 28,
            },
        ],
    }
    .render()
}

#[derive(Deserialize)]
pub struct RosterNoticeQuery {
    notice: Option<String>,
}

#[get("/ui/instructor/sections/{section_id}")]
pub async fn roster_page(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<GradeService>,
    section_id: web::Path<Uuid>,
    query: web::Query<RosterNoticeQuery>,
) -> Result<HttpResponse, AppError> {
    let notice = match query.notice.as_deref() {
        Some("saved") => Some("Draft grade saved."),
        Some("published") => Some("Draft grades published."),
        _ => None,
    };
    let body = render_roster(
        &actor,
        &current,
        &service,
        section_id.into_inner(),
        notice,
        None,
        None,
    )
    .await?;
    Ok(html(StatusCode::OK, body))
}

#[derive(Template)]
#[template(path = "pages/grade_history.html")]
struct GradeHistoryPage {
    view: crate::records::grades::GradeHistoryView,
}

/// Per-student grade revision history: the current record plus every prior
/// value the database trigger captured, attributed. Roster visibility rules
/// apply (assigned instructor or records officer).
#[get("/ui/instructor/enrollments/{enrollment_id}/grade-history")]
pub async fn grade_history_page(
    actor: Actor,
    service: web::Data<GradeService>,
    enrollment_id: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let page = GradeHistoryPage {
        view: service
            .grade_history(&actor, enrollment_id.into_inner())
            .await?,
    };
    Ok(html(StatusCode::OK, page.render()?))
}

/// Renders the grade-history page with representative data and no
/// database, for the frontend axe harness (`render-pages`).
pub fn sample_grade_history_html() -> Result<String, askama::Error> {
    use crate::records::grades::{GradeHistoryHead, GradeHistoryView, RevisionRow};
    let entry = |version, code: &str, state: &str, by: &str| RevisionRow {
        grade_code: code.to_owned(),
        state: state.to_owned(),
        version,
        entered_by: by.to_owned(),
        recorded_at: chrono::Utc::now(),
    };
    GradeHistoryPage {
        view: GradeHistoryView {
            head: GradeHistoryHead {
                section_id: Uuid::nil(),
                student_number: "2023-01144".into(),
                student_name: "Marcus Bennett".into(),
                course_code: "CMPS 2131".into(),
                course_title: "Data structures".into(),
                section_code: "01".into(),
            },
            entries: vec![
                entry(3, "A-", "amended", "records.officer"),
                entry(2, "B+", "published", "records.officer"),
                entry(1, "B+", "draft", "prof.alvarez"),
            ],
        },
    }
    .render()
}

#[derive(Deserialize)]
pub struct GradeEntryForm {
    section_id: Uuid,
    enrollment_id: Uuid,
    grade_code: String,
    /// Free text so an empty field is representable; parsed below.
    grade_points: Option<String>,
    expected_version: i64,
    csrf_token: String,
}

/// Draft entry from the roster form. Success redirects back (PRG); a denial
/// re-renders the roster with the reason inline.
#[post("/ui/instructor/grades")]
pub async fn save_grade_form(
    actor: Actor,
    current: CurrentSession,
    service: web::Data<GradeService>,
    form: web::Form<GradeEntryForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token; // validated by the CSRF middleware

    // The roster select supplies only a code; points come from the standard
    // scale (assumption A29). An explicit points field still wins so API
    // and legacy callers keep their exact values.
    let points_text = form.grade_points.as_deref().unwrap_or("").trim();
    let grade_points = if points_text.is_empty() {
        Ok(crate::records::grades::standard_grade_points(
            &form.grade_code,
        ))
    } else {
        points_text
            .parse::<f64>()
            .map(Some)
            .map_err(|_| AppError::Validation("grade points must be a number".into()))
    };

    let outcome = match grade_points {
        Ok(grade_points) => service
            .save_draft(
                &actor,
                SaveGradeCommand {
                    enrollment_id: form.enrollment_id,
                    grade_code: form.grade_code.clone(),
                    grade_points,
                    numeric_value: None,
                    expected_version: form.expected_version,
                },
            )
            .await
            .map(|_| ()),
        Err(error) => Err(error),
    };

    match outcome {
        Ok(()) => Ok(see_other(&format!(
            "/ui/instructor/sections/{}?notice=saved",
            form.section_id
        ))),
        // Business denials render inline with their honest status; anything
        // else keeps its error shape.
        Err(error @ (AppError::Conflict(_) | AppError::Validation(_))) => {
            let status = match &error {
                AppError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
                _ => StatusCode::CONFLICT,
            };
            let message = error.to_string();
            let body = render_roster(
                &actor,
                &current,
                &service,
                form.section_id,
                None,
                Some(&message),
                Some(form.enrollment_id),
            )
            .await?;
            Ok(html(status, body))
        }
        Err(other) => Err(other),
    }
}

#[derive(Deserialize)]
pub struct PublishForm {
    csrf_token: String,
}

/// Publish every draft grade of the section (records officer).
#[post("/ui/instructor/sections/{section_id}/publish")]
pub async fn publish_form(
    actor: Actor,
    service: web::Data<GradeService>,
    section_id: web::Path<Uuid>,
    form: web::Form<PublishForm>,
) -> Result<HttpResponse, AppError> {
    let _ = &form.csrf_token;
    let section_id = section_id.into_inner();
    service.publish_section(&actor, section_id).await?;
    Ok(see_other(&format!(
        "/ui/instructor/sections/{section_id}?notice=published"
    )))
}

#[derive(Template)]
#[template(path = "pages/schedule.html")]
struct SchedulePage {
    term: Option<TermSummary>,
    days: Vec<DayColumn>,
    empty: bool,
    weekend: bool,
    events: Vec<crate::institution::service::InstitutionEvent>,
    grid: Option<MonthGrid>,
    selected: Option<SelectedDay>,
}

/// One entry on a month-calendar day — only real, queryable things: a
/// class meeting occurrence, a campus event, or a modeled deadline.
/// There is no coursework/assignment data in this system (CURRENT_STATE
/// scope decision), so nothing of the kind is invented here.
#[derive(Clone)]
pub struct DayItem {
    pub time: Option<String>,
    pub label: String,
    /// "class" | "event" | "deadline" — carried as a word next to the
    /// tint, never color alone.
    pub kind: &'static str,
}

#[derive(Clone)]
pub struct MonthDay {
    pub day: u32,
    pub iso: String,
    pub today: bool,
    pub selected: bool,
    pub items: Vec<DayItem>,
}

impl MonthDay {
    /// Campus events (and the add/drop deadline) earn the cell's bar
    /// marker; class meetings don't — classes are a given, the weekly
    /// grid and hover highlight carry them.
    pub fn has_event(&self) -> bool {
        self.items.iter().any(|item| item.kind != "class")
    }
}

/// One month as Monday-first week rows (None pads the edges) plus real
/// prev/next links ("YYYY-MM").
pub struct MonthGrid {
    pub label: String,
    /// "YYYY-MM" — this month, for the day links' month parameter.
    pub month: String,
    pub prev: String,
    pub next: String,
    pub weeks: Vec<Vec<Option<MonthDay>>>,
}

/// The server-rendered day detail for a clicked/tapped day cell — an
/// inline list, not a new surface (JS-off: the cell is a real link).
pub struct SelectedDay {
    pub label: String,
    pub items: Vec<DayItem>,
}

/// Everything scheduled on one real date: weekly meetings expanded onto
/// the calendar (bounded by the term — the enrollment query already
/// excludes dropped sections), events spanning the date, and the
/// add/drop deadline.
fn day_items(
    date: chrono::NaiveDate,
    term: &TermSummary,
    meetings: &[crate::records::schedule::ScheduleMeeting],
    events: &[crate::institution::service::InstitutionEvent],
) -> Vec<DayItem> {
    use chrono::Datelike;
    let mut items = Vec::new();
    if term.starts_on <= date && date <= term.ends_on {
        for meeting in meetings {
            if meeting.day_of_week == date.weekday().number_from_monday() as i16 {
                items.push(DayItem {
                    time: Some(meeting.starts_at.format("%H:%M").to_string()),
                    label: format!("{} · {}", meeting.course_code, meeting.course_title),
                    kind: "class",
                });
            }
        }
    }
    for event in events {
        if event.starts_on <= date && date <= event.ends_on {
            items.push(DayItem {
                time: None,
                label: event.title.clone(),
                kind: "event",
            });
        }
    }
    // ponytail: deadline day is the UTC date, same known lag as the
    // dashboard mini-calendar's "today" marker.
    if term.add_drop_closes_at.date_naive() == date {
        items.push(DayItem {
            time: None,
            label: "Add/drop closes".to_owned(),
            kind: "deadline",
        });
    }
    items
}

fn month_grid(
    first: chrono::NaiveDate,
    today: chrono::NaiveDate,
    selected: Option<chrono::NaiveDate>,
    term: &TermSummary,
    meetings: &[crate::records::schedule::ScheduleMeeting],
    events: &[crate::institution::service::InstitutionEvent],
) -> MonthGrid {
    use chrono::Datelike;
    let next = first
        .checked_add_months(chrono::Months::new(1))
        .expect("next month exists");
    let prev = first
        .checked_sub_months(chrono::Months::new(1))
        .expect("previous month exists");
    let count = (next - first).num_days() as u32;
    let lead = first.weekday().num_days_from_monday() as usize;
    let mut cells: Vec<Option<MonthDay>> = (0..lead).map(|_| None).collect();
    for n in 1..=count {
        let date = first.with_day(n).expect("n is within the month");
        cells.push(Some(MonthDay {
            day: n,
            iso: date.format("%Y-%m-%d").to_string(),
            today: date == today,
            selected: selected == Some(date),
            items: day_items(date, term, meetings, events),
        }));
    }
    while !cells.len().is_multiple_of(7) {
        cells.push(None);
    }
    MonthGrid {
        label: first.format("%B %Y").to_string(),
        month: first.format("%Y-%m").to_string(),
        prev: prev.format("%Y-%m").to_string(),
        next: next.format("%Y-%m").to_string(),
        weeks: cells.chunks(7).map(<[_]>::to_vec).collect(),
    }
}

/// One weekday's meetings, already time-sorted by the schedule query.
struct DayColumn {
    name: &'static str,
    meetings: Vec<crate::records::schedule::ScheduleMeeting>,
    /// Comma-joined ISO dates this weekday still occurs, today through
    /// term end — the month-calendar hover highlight (every meeting in a
    /// column shares its weekday, so one list serves the whole column).
    dates: String,
}

const DAY_NAMES: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// Group meetings into Monday-first day columns. Weekend columns render
/// only when a weekend meeting exists.
fn day_columns(
    meetings: Vec<crate::records::schedule::ScheduleMeeting>,
    term: Option<&TermSummary>,
) -> (Vec<DayColumn>, bool) {
    use chrono::Datelike;
    let weekend = meetings.iter().any(|meeting| meeting.day_of_week > 5);
    let today = chrono::Utc::now().date_naive();
    let remaining = |weekday: u32| -> String {
        let Some(term) = term else {
            return String::new();
        };
        let mut date = today.max(term.starts_on);
        let mut out = Vec::new();
        while date <= term.ends_on {
            if date.weekday().number_from_monday() == weekday {
                out.push(date.format("%Y-%m-%d").to_string());
            }
            date = date.succ_opt().expect("date within term range");
        }
        out.join(",")
    };
    let mut days: Vec<DayColumn> = DAY_NAMES[..if weekend { 7 } else { 5 }]
        .iter()
        .enumerate()
        .map(|(index, name)| DayColumn {
            name,
            meetings: Vec::new(),
            dates: remaining(index as u32 + 1),
        })
        .collect();
    for meeting in meetings {
        // ISO day 1-7; an out-of-range row is broken data — clamp rather
        // than panic on a student's schedule.
        let index = usize::from(meeting.day_of_week.clamp(1, 7) as u16) - 1;
        if let Some(day) = days.get_mut(index) {
            day.meetings.push(meeting);
        }
    }
    (days, weekend)
}

#[derive(serde::Deserialize)]
pub struct ScheduleView {
    /// "YYYY-MM" — which month the calendar shows; default: this month.
    month: Option<String>,
    /// "YYYY-MM-DD" — a day whose detail list renders inline.
    day: Option<String>,
}

/// The student's weekly schedule (grid at the top) plus the month
/// calendar of real occurrences and the campus-events aside.
#[get("/ui/schedule")]
pub async fn schedule_page(
    actor: Actor,
    query_service: web::Data<ScheduleQuery>,
    academics: web::Data<AcademicsService>,
    institution: web::Data<crate::institution::InstitutionService>,
    view: web::Query<ScheduleView>,
) -> Result<HttpResponse, AppError> {
    use chrono::Datelike;
    let term = academics.current_term(&actor).await?;
    let meetings = match &term {
        Some(term) => query_service.for_student(&actor, term.id).await?,
        None => Vec::new(),
    };

    let mut grid = None;
    let mut selected = None;
    if let Some(term) = &term {
        let today = chrono::Utc::now().date_naive();
        let first = view
            .month
            .as_deref()
            .and_then(|month| {
                chrono::NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d").ok()
            })
            .unwrap_or_else(|| today.with_day(1).expect("day 1 exists"));
        let last = first
            .checked_add_months(chrono::Months::new(1))
            .expect("next month exists")
            .pred_opt()
            .expect("month has a last day");
        let month_events = institution.events_between(&actor, first, last).await?;
        let day = view
            .day
            .as_deref()
            .and_then(|day| chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
            .filter(|day| (first..=last).contains(day));
        if let Some(day) = day {
            selected = Some(SelectedDay {
                label: day.format("%A, %B %-d").to_string(),
                items: day_items(day, term, &meetings, &month_events),
            });
        }
        grid = Some(month_grid(
            first,
            today,
            day,
            term,
            &meetings,
            &month_events,
        ));
    }

    let empty = meetings.is_empty();
    let (days, weekend) = day_columns(meetings, term.as_ref());
    let page = SchedulePage {
        term,
        days,
        empty,
        weekend,
        events: institution.upcoming_events(&actor).await?,
        grid,
        selected,
    };
    Ok(html(StatusCode::OK, page.render()?))
}

/// Renders the schedule with representative data and no database, for the
/// frontend axe harness (`render-pages`).
pub fn sample_schedule_html() -> Result<String, askama::Error> {
    use crate::records::schedule::ScheduleMeeting;
    use chrono::{Duration, NaiveTime, Utc};
    let now = Utc::now();
    let meeting =
        |day, course: &str, title: &str, from: (u32, u32), to: (u32, u32)| ScheduleMeeting {
            course_code: course.to_owned(),
            course_title: title.to_owned(),
            section_code: "01".to_owned(),
            day_of_week: day,
            starts_at: NaiveTime::from_hms_opt(from.0, from.1, 0).unwrap(),
            ends_at: NaiveTime::from_hms_opt(to.0, to.1, 0).unwrap(),
            campus_code: None,
            room_code: Some("214".to_owned()),
        };
    let meetings = vec![
        meeting(1, "CMPS 2131", "Data structures", (9, 0), (10, 15)),
        meeting(3, "CMPS 2131", "Data structures", (9, 0), (10, 15)),
        meeting(2, "MATH 3201", "Linear algebra", (13, 0), (14, 15)),
    ];
    let term = TermSummary {
        id: Uuid::nil(),
        code: "FA26".into(),
        name: "Fall 2026".into(),
        starts_on: now.date_naive(),
        ends_on: (now + Duration::days(100)).date_naive(),
        registration_opens_at: now - Duration::days(20),
        add_drop_closes_at: now + Duration::days(14),
        grade_entry_closes_at: None,
    };
    let events = vec![crate::institution::service::InstitutionEvent {
        id: Uuid::nil(),
        title: "Independence Day".into(),
        event_type: "holiday".into(),
        starts_on: (now + Duration::days(30)).date_naive(),
        ends_on: (now + Duration::days(30)).date_naive(),
    }];
    use chrono::Datelike;
    let today = now.date_naive();
    let first = today.with_day(1).expect("day 1 exists");
    let grid = Some(month_grid(
        first,
        today,
        Some(today),
        &term,
        &meetings,
        &events,
    ));
    let selected = Some(SelectedDay {
        label: today.format("%A, %B %-d").to_string(),
        items: day_items(today, &term, &meetings, &events),
    });
    let (days, weekend) = day_columns(meetings, Some(&term));
    SchedulePage {
        term: Some(term),
        days,
        empty: false,
        weekend,
        events,
        grid,
        selected,
    }
    .render()
}

#[derive(Template)]
#[template(path = "pages/grades.html")]
struct StudentGradesPage {
    term: Option<TermSummary>,
    grades: Vec<StudentGradeRow>,
}

/// Sample term used by the no-database axe renders below.
fn sample_term() -> TermSummary {
    use chrono::{Duration, Utc};
    let now = Utc::now();
    TermSummary {
        id: Uuid::nil(),
        code: "FA26".into(),
        name: "Fall 2026".into(),
        starts_on: now.date_naive(),
        ends_on: (now + Duration::days(100)).date_naive(),
        registration_opens_at: now - Duration::days(20),
        add_drop_closes_at: now + Duration::days(14),
        grade_entry_closes_at: None,
    }
}

/// Renders the grades page with representative data and no database, for
/// the frontend axe harness (`render-pages`).
pub fn sample_grades_html() -> Result<String, askama::Error> {
    StudentGradesPage {
        term: Some(sample_term()),
        grades: vec![StudentGradeRow {
            course_code: "CMPS 2131".into(),
            course_title: "Data structures".into(),
            section_code: "01".into(),
            grade_code: "B+".into(),
            published_at: Some(chrono::Utc::now()),
        }],
    }
    .render()
}

/// Renders the history page with representative data and no database, for
/// the frontend axe harness (`render-pages`).
pub fn sample_history_html() -> Result<String, askama::Error> {
    HistoryPage {
        courses: vec![HistoryRow {
            term_code: "FA26".into(),
            term_name: "Fall 2026".into(),
            course_code: "CMPS 2131".into(),
            course_title: "Data structures".into(),
            credit_hours: 3.0,
            grade_code: "B+".into(),
            grade_points: Some(3.3),
            state: "published".into(),
        }],
        snapshots: Vec::new(),
    }
    .render()
}

/// The student's published grades for the current term. Drafts are excluded
/// by the underlying query, not by this page.
#[get("/ui/grades")]
pub async fn student_grades_page(
    actor: Actor,
    grades: web::Data<GradeService>,
    academics: web::Data<AcademicsService>,
) -> Result<HttpResponse, AppError> {
    let term = academics.current_term(&actor).await?;
    let rows = match &term {
        Some(term) => grades.student_grades(&actor, term.id).await?,
        None => Vec::new(),
    };
    let page = StudentGradesPage { term, grades: rows };
    Ok(html(StatusCode::OK, page.render()?))
}

#[derive(Template)]
#[template(path = "pages/history.html")]
struct HistoryPage {
    courses: Vec<HistoryRow>,
    snapshots: Vec<SnapshotSummary>,
}

#[get("/ui/history")]
pub async fn history_page(
    actor: Actor,
    grades: web::Data<GradeService>,
    snapshots: web::Data<TranscriptSnapshotService>,
    pool: web::Data<PgPool>,
) -> Result<HttpResponse, AppError> {
    let page = HistoryPage {
        courses: grades.academic_history(&actor).await?,
        snapshots: snapshots.own_snapshots(&pool, &actor).await?,
    };
    Ok(html(StatusCode::OK, page.render()?))
}

#[derive(Template)]
#[template(path = "pages/transcript.html")]
struct TranscriptPage {
    identity: StudentIdentity,
    courses: Vec<HistoryRow>,
    generated_on: String,
}

/// The unofficial transcript: the student's published record on screen,
/// printable via the print stylesheet, clearly marked unofficial. No PDF
/// pipeline — official documents keep theirs.
#[get("/ui/transcript")]
pub async fn transcript_page(
    actor: Actor,
    grades: web::Data<GradeService>,
    snapshots: web::Data<TranscriptSnapshotService>,
    pool: web::Data<PgPool>,
) -> Result<HttpResponse, AppError> {
    let page = TranscriptPage {
        identity: snapshots.own_identity(&pool, &actor).await?,
        courses: grades.academic_history(&actor).await?,
        generated_on: generated_on(),
    };
    Ok(html(StatusCode::OK, page.render()?))
}

#[derive(Template)]
#[template(path = "pages/proof_of_enrollment.html")]
struct ProofOfEnrollmentPage {
    identity: StudentIdentity,
    term: Option<TermSummary>,
    enrollments: Vec<crate::enrollment::types::EnrolledSection>,
    generated_on: String,
}

/// Unofficial proof of enrollment: identity plus the current term's active
/// enrollments, printable, clearly marked unofficial.
#[get("/ui/proof-of-enrollment")]
pub async fn proof_of_enrollment_page(
    actor: Actor,
    enrollment: web::Data<crate::enrollment::EnrollmentService>,
    academics: web::Data<AcademicsService>,
    snapshots: web::Data<TranscriptSnapshotService>,
    pool: web::Data<PgPool>,
) -> Result<HttpResponse, AppError> {
    let term = academics.current_term(&actor).await?;
    let enrollments = match &term {
        Some(term) => enrollment.list_own_active(&actor, term.id).await?,
        None => Vec::new(),
    };
    let page = ProofOfEnrollmentPage {
        identity: snapshots.own_identity(&pool, &actor).await?,
        term,
        enrollments,
        generated_on: generated_on(),
    };
    Ok(html(StatusCode::OK, page.render()?))
}

fn generated_on() -> String {
    chrono::Utc::now().format("%b %-d, %Y").to_string()
}

fn sample_identity() -> StudentIdentity {
    StudentIdentity {
        student_number: "2026-0042".into(),
        student_name: "d.reyes".into(),
        program_code: "CS".into(),
        institution_name: "University of Belize".into(),
    }
}

/// Renders the unofficial transcript with representative data and no
/// database, for the frontend axe harness (`render-pages`).
pub fn sample_transcript_html() -> Result<String, askama::Error> {
    TranscriptPage {
        identity: sample_identity(),
        courses: vec![HistoryRow {
            term_code: "FA26".into(),
            term_name: "Fall 2026".into(),
            course_code: "CMPS 2131".into(),
            course_title: "Data structures".into(),
            credit_hours: 3.0,
            grade_code: "B+".into(),
            grade_points: Some(3.3),
            state: "published".into(),
        }],
        generated_on: generated_on(),
    }
    .render()
}

/// Renders the proof of enrollment with representative data and no
/// database, for the frontend axe harness (`render-pages`).
pub fn sample_proof_html() -> Result<String, askama::Error> {
    ProofOfEnrollmentPage {
        identity: sample_identity(),
        term: Some(sample_term()),
        enrollments: vec![crate::enrollment::types::EnrolledSection {
            enrollment_id: Uuid::nil(),
            course_code: "CMPS 2131".into(),
            course_title: "Data structures".into(),
            section_code: "01".into(),
            credit_hours: 3.0,
            meetings: "Mon/Wed 09:00-10:15".into(),
            instructors: "d.thompson".into(),
        }],
        generated_on: generated_on(),
    }
    .render()
}

#[allow(clippy::too_many_arguments)] // one page, one call site per outcome
async fn render_roster(
    actor: &Actor,
    current: &CurrentSession,
    service: &GradeService,
    section_id: Uuid,
    notice: Option<&str>,
    error: Option<&str>,
    error_enrollment: Option<Uuid>,
) -> Result<String, AppError> {
    let view = service.roster(actor, section_id).await?;
    let is_officer = actor.has_role(Role::RecordsOfficer);

    // The entry window binds instructors; the officer is the late-entry
    // escape hatch (assumption A17) — the banner explains either way.
    let closes_at = view.section.grade_entry_closes_at;
    let window_open = closes_at.is_none_or(|at| chrono::Utc::now() < at);
    let (window_kind, mut window_note) = match (window_open, closes_at) {
        (true, Some(at)) => (
            "success",
            format!(
                "Grade entry open · closes {}. Published grades are visible to students immediately.",
                at.format("%b %-d, %Y %H:%M")
            ),
        ),
        (true, None) => (
            "success",
            "Grade entry open. Published grades are visible to students immediately.".to_owned(),
        ),
        (false, at) => (
            "warning",
            format!(
                "Grade entry closed{}. Entries are read-only — the records office handles late entry.",
                at.map(|at| format!(" {}", at.format("%b %-d, %Y %H:%M")))
                    .unwrap_or_default()
            ),
        ),
    };
    if is_officer && !window_open {
        window_note.push_str(" As records office you may still enter grades.");
    }

    let switcher = if actor.has_role(Role::Instructor) {
        service.instructor_sections(actor).await?
    } else {
        Vec::new()
    };

    Ok(RosterPage {
        csrf_token: &current.csrf_token,
        view,
        can_publish: is_officer,
        notice,
        error,
        error_enrollment,
        entry_enabled: window_open || is_officer,
        window_kind,
        window_note,
        switcher,
    }
    .render()?)
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

pub fn routes(cfg: &mut web::ServiceConfig) {
    cfg.service(grades_json)
        .service(schedule_json)
        .service(save_draft_json)
        .service(correct_grade_json)
        .service(publish_section_json)
        .service(instructor_sections_json)
        .service(roster_json)
        .service(generate_snapshot_json)
        .service(history_json)
        .service(instructor_sections_page)
        .service(roster_page)
        .service(save_grade_form)
        .service(publish_form)
        .service(grade_history_page)
        .service(schedule_page)
        .service(student_grades_page)
        .service(history_page)
        .service(transcript_page)
        .service(proof_of_enrollment_page);
}

#[cfg(test)]
mod month_grid_tests {
    use super::*;
    use chrono::NaiveDate;

    fn term(starts: NaiveDate, ends: NaiveDate, closes: NaiveDate) -> TermSummary {
        TermSummary {
            id: Uuid::nil(),
            code: "T".into(),
            name: "Term".into(),
            starts_on: starts,
            ends_on: ends,
            registration_opens_at: chrono::Utc::now(),
            add_drop_closes_at: closes.and_hms_opt(12, 0, 0).unwrap().and_utc(),
            grade_entry_closes_at: None,
        }
    }

    fn monday_meeting() -> crate::records::schedule::ScheduleMeeting {
        crate::records::schedule::ScheduleMeeting {
            course_code: "CMPS-2131".into(),
            course_title: "Data structures".into(),
            section_code: "01".into(),
            day_of_week: 1,
            starts_at: chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            ends_at: chrono::NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
            campus_code: None,
            room_code: None,
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// The honest-data core: a weekly meeting lands ONLY on matching
    /// weekdays inside the term window — nothing before the term starts,
    /// nothing after it ends, nothing invented.
    #[test]
    fn meeting_occurrences_stay_inside_the_term_and_weekday() {
        // September 2026: Mondays are the 7th, 14th, 21st, 28th.
        // Term runs Sep 10 – Sep 22 → only the 14th and 21st qualify.
        let term = term(date(2026, 9, 10), date(2026, 9, 22), date(2026, 9, 15));
        let meetings = [monday_meeting()];
        let grid = month_grid(
            date(2026, 9, 1),
            date(2026, 9, 14),
            None,
            &term,
            &meetings,
            &[],
        );
        let class_days: Vec<u32> = grid
            .weeks
            .iter()
            .flatten()
            .flatten()
            .filter(|day| day.items.iter().any(|item| item.kind == "class"))
            .map(|day| day.day)
            .collect();
        assert_eq!(class_days, vec![14, 21], "only in-term Mondays");
        let today_marked: Vec<u32> = grid
            .weeks
            .iter()
            .flatten()
            .flatten()
            .filter(|day| day.today)
            .map(|day| day.day)
            .collect();
        assert_eq!(today_marked, vec![14]);
    }

    /// Events span their full date range and the add/drop deadline lands
    /// on its (UTC) date, each carrying its kind word.
    #[test]
    fn events_and_the_deadline_mark_their_days() {
        let term = term(date(2026, 9, 1), date(2026, 12, 15), date(2026, 9, 18));
        let events = [crate::institution::service::InstitutionEvent {
            id: Uuid::nil(),
            title: "Orientation".into(),
            event_type: "academic".into(),
            starts_on: date(2026, 9, 2),
            ends_on: date(2026, 9, 3),
        }];
        let grid = month_grid(
            date(2026, 9, 1),
            date(2026, 9, 1),
            None,
            &term,
            &[],
            &events,
        );
        let kinds = |day_number: u32| -> Vec<&'static str> {
            grid.weeks
                .iter()
                .flatten()
                .flatten()
                .find(|day| day.day == day_number)
                .map(|day| day.items.iter().map(|item| item.kind).collect())
                .unwrap_or_default()
        };
        assert_eq!(kinds(2), vec!["event"]);
        assert_eq!(kinds(3), vec!["event"]);
        assert_eq!(kinds(4), Vec::<&str>::new());
        assert_eq!(kinds(18), vec!["deadline"]);
        assert_eq!(grid.prev, "2026-08");
        assert_eq!(grid.next, "2026-10");
    }
}
