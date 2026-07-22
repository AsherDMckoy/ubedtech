//! Development/CI tooling: the design-system gallery page and the
//! `render-pages` subcommand that feeds the frontend accessibility harness
//! (`frontend/test/a11y.mjs` runs axe over the rendered files). Needs no
//! configuration or database — it must work on a bare checkout and in CI.

use askama::Template;

/// Every design-system primitive rendered once (FRONTEND.md §9). Not
/// routed in the application; the harness and humans read it.
#[derive(Template)]
#[template(path = "pages/gallery.html")]
struct GalleryPage;

/// Writes the critical pages for the axe pass. Later sessions append
/// screens here as they are built.
pub fn render_pages(out_dir: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let render = |template: &str, result: Result<String, askama::Error>| {
        result.unwrap_or_else(|error| panic!("rendering {template} failed: {error}"))
    };
    let pages = [
        (
            "signin.html",
            render("signin", crate::identity_access::http::signin_html(None)),
        ),
        ("gallery.html", render("gallery", GalleryPage.render())),
        (
            "dashboard.html",
            render(
                "dashboard",
                crate::enrollment::http::sample_dashboard_html(),
            ),
        ),
        (
            "registration.html",
            render(
                "registration",
                crate::academics::http::sample_catalog_html(),
            ),
        ),
        (
            "schedule.html",
            render("schedule", crate::records::http::sample_schedule_html()),
        ),
        (
            "grades.html",
            render("grades", crate::records::http::sample_grades_html()),
        ),
        (
            "history.html",
            render("history", crate::records::http::sample_history_html()),
        ),
        (
            "transcript.html",
            render("transcript", crate::records::http::sample_transcript_html()),
        ),
        (
            "proof_of_enrollment.html",
            render(
                "proof_of_enrollment",
                crate::records::http::sample_proof_html(),
            ),
        ),
        (
            "instructor_sections.html",
            render(
                "instructor_sections",
                crate::records::http::sample_instructor_sections_html(),
            ),
        ),
        (
            "roster.html",
            render("roster", crate::records::http::sample_roster_html()),
        ),
        (
            "grade_history.html",
            render(
                "grade_history",
                crate::records::http::sample_grade_history_html(),
            ),
        ),
        (
            "documents.html",
            render("documents", crate::documents::http::sample_documents_html()),
        ),
        (
            "document_queue.html",
            render(
                "document_queue",
                crate::documents::http::sample_queue_html(),
            ),
        ),
        (
            "registrar_overview.html",
            render(
                "registrar_overview",
                crate::academics::http::sample_registrar_overview_html(),
            ),
        ),
        (
            "registrar_terms.html",
            render(
                "registrar_terms",
                crate::academics::http::sample_registrar_terms_html(),
            ),
        ),
        (
            "registrar_sections.html",
            render(
                "registrar_sections",
                crate::academics::http::sample_registrar_sections_html(),
            ),
        ),
        (
            "registrar_section_detail.html",
            render(
                "registrar_section_detail",
                crate::academics::http::sample_registrar_section_detail_html(),
            ),
        ),
        (
            "registrar_courses.html",
            render(
                "registrar_courses",
                crate::academics::http::sample_registrar_courses_html(),
            ),
        ),
        (
            "registrar_students.html",
            render(
                "registrar_students",
                crate::enrollment::http::sample_registrar_students_html(),
            ),
        ),
        (
            "registrar_student_detail.html",
            render(
                "registrar_student_detail",
                crate::enrollment::http::sample_registrar_student_detail_html(),
            ),
        ),
        (
            "registrar_overrides.html",
            render(
                "registrar_overrides",
                crate::enrollment::http::sample_registrar_overrides_html(),
            ),
        ),
        (
            "calendar_admin.html",
            render(
                "calendar_admin",
                crate::institution::http::sample_calendar_admin_html(),
            ),
        ),
        (
            "institution_settings.html",
            render(
                "institution_settings",
                crate::institution::http::sample_settings_html(),
            ),
        ),
    ];
    for (name, html) in pages {
        let path = format!("{out_dir}/{name}");
        std::fs::write(&path, html)?;
        println!("{path}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gallery is the one page that exercises every primitive; the
    /// structural audit must hold for it like any critical page.
    #[test]
    fn the_gallery_and_signin_pages_pass_the_structural_audit() {
        crate::shared::assets::assert_page_a11y(&GalleryPage.render().unwrap());
        crate::shared::assets::assert_page_a11y(
            &crate::identity_access::http::signin_html(Some("Invalid username or password."))
                .unwrap(),
        );
    }
}
