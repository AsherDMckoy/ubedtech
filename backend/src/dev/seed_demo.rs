//! `seed-demo`: a deterministic, large demo dataset layered on top of
//! `src/dev/seed.sql` (which stays the minimal bootstrap). Development only.
//!
//! The rule that keeps the dataset honest (CLAUDE.md §3): anything with an
//! invariant or an audit requirement — enrollment, grades, holds, overrides,
//! document requests/decisions — goes through the REAL service functions.
//! Only inert reference data (accounts, courses, sections, meetings, terms,
//! events-of-record like capacities) is bulk-inserted. A verification pass
//! at the end fails loudly if counters, capacity rows, or audit coverage
//! drifted.
//!
//! Deterministic: fixed RNG seed and counter-derived UUIDs, so every run
//! produces the same people, courses, and relationships. (Wall-clock
//! columns like `occurred_at`/`registered_at` move with the clock; the
//! shape of the data does not.)
//!
//! Idempotent by detection: if the generated second institution already
//! exists the command reports and exits 0. Rebuild = drop/recreate the
//! database, re-apply seed.sql, re-run.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::{NaiveDate, NaiveTime};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::AuditWriter;
use crate::config::AppConfig;
use crate::documents::{DocumentService, DocumentWorker, RequestDocumentCommand};
use crate::enrollment::{EnrollmentService, GrantOverrideCommand, RegisterCommand};
use crate::institution::InstitutionService;
use crate::records::grades::{CorrectGradeCommand, SaveGradeCommand};
use crate::records::{GradeService, TranscriptSnapshotService};
use crate::shared::actor::{Actor, Role};
use crate::shared::error::AppError;

/// Dataset scale. FULL is the developer dataset; SMOKE keeps the invariant
/// test fast while exercising every code path.
pub struct Scale {
    pub students: usize,
    pub instructors: usize,
    pub past_sections_per_term: usize,
    pub current_sections: usize,
    pub max_load: usize,
    pub documents: usize,
    pub stc_students: usize,
}

pub const FULL: Scale = Scale {
    students: 900,
    instructors: 45,
    past_sections_per_term: 200,
    current_sections: 276, // + the 4 demo sections = ~280
    max_load: 6,
    documents: 47,
    stc_students: 20,
};

#[allow(dead_code)] // exercised by the cfg(test) acceptance test below
pub const SMOKE: Scale = Scale {
    students: 48,
    instructors: 8,
    past_sections_per_term: 10,
    current_sections: 14,
    max_load: 3,
    documents: 12,
    stc_students: 6,
};

// ---------------------------------------------------------------------------
// Fixed identities from seed.sql (never regenerated here).
// ---------------------------------------------------------------------------

const UB: Uuid = Uuid::from_u128(0x01);
const DEMO_REGISTRAR: Uuid = Uuid::from_u128(0x16);
const DEMO_ADMIN: Uuid = Uuid::from_u128(0x17);
const FALL_2026: Uuid = Uuid::from_u128(0x20);
const SEC_MATH_3201: Uuid = Uuid::from_u128(0x33);
const SEC_PHYS_2101: Uuid = Uuid::from_u128(0x34);
const COURSE_MATH_2110: Uuid = Uuid::from_u128(0x25);
/// seed.sql's core rows that predate this command and carry no audit events;
/// backfilled below so audit coverage holds for the WHOLE database.
const CORE_ENROLLMENTS: [Uuid; 4] = [
    Uuid::from_u128(0x51),
    Uuid::from_u128(0x52),
    Uuid::from_u128(0x53),
    Uuid::from_u128(0x54),
];
const CORE_GRADES: [Uuid; 2] = [Uuid::from_u128(0x71), Uuid::from_u128(0x72)];
const CORE_DOC_REQUEST: Uuid = Uuid::from_u128(0x75);

/// Deterministic UUIDs for generated rows, far from seed.sql's low range.
fn id(n: u64) -> Uuid {
    Uuid::from_u128(0xDE30_0000_0000_0000_0000_0000_0000_0000 + u128::from(n))
}

// ---------------------------------------------------------------------------
// Belizean name pools. There is no human-name column in the schema — the
// username IS the rendered identity — so the layout-stressing names go
// exactly where the UI will actually display them.
// ---------------------------------------------------------------------------

const FIRST_NAMES: &[&str] = &[
    // Mestizo
    "carlos",
    "maría",
    "josé",
    "ana",
    "luis",
    "carmen",
    "jorge",
    "guadalupe",
    "miguel",
    "rosa",
    "alejandro",
    "beatriz",
    "hernán",
    "lucía",
    "ramón",
    "silvia", // Kriol
    "marlon",
    "shanice",
    "dwayne",
    "keisha",
    "andre",
    "tanisha",
    "jamal",
    "latoya",
    "clive",
    "denise",
    "winston",
    "arlene", // Maya (Q'eqchi'/Mopan)
    "ernesto",
    "florentina",
    "ix'chel",
    "b'alam",
    "domingo",
    "petrona",
    "santiago",
    "juana",
    // Garifuna
    "delvin",
    "marisol",
    "egbert",
    "isani",
    "seferino",
    "gwen", // Chinese
    "kevin",
    "amy",
    "wei",
    "mei",
    "xi", // East Indian
    "anil",
    "priya",
    "devi",
    "sunil",
    "kamla",
];

const SURNAMES: &[&str] = &[
    // Mestizo
    "hernández",
    "garcía",
    "cárdenas",
    "ramírez",
    "lópez",
    "novelo",
    "ayala",
    "marín",
    "vásquez",
    "requeña",
    "aragón",
    "urbina", // Kriol
    "flowers",
    "gillett",
    "hyde",
    "usher",
    "fairweather",
    "lightburn",
    "staine",
    "gentle",
    "broaster",
    "neal",
    "young",
    "mckoy", // Maya
    "coc",
    "choc",
    "cal",
    "chun",
    "cucul",
    "caal",
    "tzul",
    "tzib",
    "xol",
    "pop",
    "ical",
    "bol",
    "mes",
    "ack",
    "pech",
    "cocom", // Garifuna
    "palacio",
    "cayetano",
    "lambey",
    "arana",
    "enriquez",
    "zuniga",
    "castillo", // Chinese
    "chen",
    "wong",
    "quan",
    "ho",
    "cho",
    "yang", // East Indian
    "singh",
    "persaud",
    "ramnarace",
    "sooknandan",
];

/// Deliberate layout stressors (dataset brief): a name long enough to
/// threaten a table column, and a single-word short one.
const LONG_INSTRUCTOR_USERNAME: &str = "guadalupe.hernández-castellanos-de-la-fuente";
const SHORT_USERNAME: &str = "xi";

// ---------------------------------------------------------------------------
// Course catalog: five faculties. FEA/FMSS/FST/FHS are the University of
// Belize's real faculties (ub.edu.bz/academic-faculties); Agriculture is a
// plausible fifth for the Central Farm campus — assumption recorded in the
// session report.
// ---------------------------------------------------------------------------

const COURSES: &[(&str, &str, f64)] = &[
    // FST — Science & Technology
    ("CMPS-1131", "Fundamentals of computing", 3.0),
    ("CMPS-1150", "Programming I", 3.0),
    ("CMPS-2111", "Programming II", 3.0),
    ("CMPS-2240", "Computer organization", 3.0),
    ("CMPS-3111", "Database systems", 3.0),
    ("CMPS-3211", "Operating systems", 3.0),
    ("CMPS-3320", "Computer networks", 3.0),
    ("CMPS-4131", "Algorithms", 3.0),
    ("CMPS-4222", "Information security", 3.0),
    ("MATH-1101", "College algebra", 3.0),
    ("MATH-1201", "Precalculus", 3.0),
    ("MATH-2101", "Calculus I", 4.0),
    ("MATH-2251", "Discrete mathematics", 3.0),
    ("MATH-3301", "Probability and statistics", 3.0),
    ("MATH-3402", "Differential equations", 3.0),
    ("MATH-4110", "Real analysis", 3.0),
    ("PHYS-1101", "Introductory physics", 4.0),
    ("PHYS-2202", "Electricity and magnetism", 4.0),
    ("PHYS-3101", "Thermodynamics", 3.0),
    ("PHYS-3240", "Optics", 3.0),
    ("BIOL-1101", "General biology I", 4.0),
    ("BIOL-1102", "General biology II", 4.0),
    ("BIOL-2210", "Cell biology", 3.0),
    ("BIOL-3105", "Genetics", 3.0),
    (
        "BIOL-3310",
        "Marine ecology of the Belize barrier reef",
        4.0,
    ),
    ("BIOL-4201", "Tropical field biology", 4.0),
    ("CHEM-1101", "General chemistry I", 4.0),
    ("CHEM-1102", "General chemistry II", 4.0),
    ("CHEM-2201", "Organic chemistry I", 4.0),
    ("CHEM-3110", "Analytical chemistry", 3.0),
    ("ENVS-2101", "Introduction to environmental science", 3.0),
    ("ENVS-3220", "Watershed and coastal zone management", 3.0),
    ("ENVS-4110", "Environmental impact assessment", 3.0),
    // FEA — Education & Arts
    ("ENGL-1011", "English composition I", 3.0),
    ("ENGL-1012", "English composition II", 3.0),
    ("ENGL-2201", "Caribbean literature", 3.0),
    ("ENGL-3110", "Technical and professional writing", 3.0),
    ("ENGL-3305", "Kriol and creole linguistics", 3.0),
    ("EDUC-1101", "Foundations of education", 3.0),
    ("EDUC-2210", "Educational psychology", 3.0),
    ("EDUC-3110", "Curriculum and assessment", 3.0),
    ("EDUC-3320", "Classroom management practicum", 3.0),
    ("EDUC-4201", "Teaching internship", 6.0),
    ("HIST-1101", "History of Belize", 3.0),
    ("HIST-2201", "The Maya world before 1500", 3.0),
    ("HIST-3110", "Central American history", 3.0),
    ("SPAN-1101", "Spanish I", 3.0),
    ("SPAN-1102", "Spanish II", 3.0),
    ("SPAN-2201", "Spanish for the professions", 3.0),
    ("ARTS-1110", "Introduction to visual arts", 3.0),
    ("ARTS-2205", "Garifuna music and dance traditions", 3.0),
    // FMSS — Management & Social Sciences
    ("BUSI-1101", "Introduction to business", 3.0),
    ("BUSI-2201", "Principles of management", 3.0),
    ("BUSI-3110", "Business law", 3.0),
    ("BUSI-3315", "Entrepreneurship and small business", 3.0),
    ("BUSI-4201", "Strategic management", 3.0),
    ("ACCT-1101", "Principles of accounting I", 3.0),
    ("ACCT-1102", "Principles of accounting II", 3.0),
    ("ACCT-3110", "Intermediate accounting", 3.0),
    ("ACCT-3220", "Auditing", 3.0),
    ("ECON-1101", "Principles of microeconomics", 3.0),
    ("ECON-1102", "Principles of macroeconomics", 3.0),
    ("ECON-3201", "Development economics", 3.0),
    ("PSYC-1101", "Introduction to psychology", 3.0),
    ("PSYC-2201", "Developmental psychology", 3.0),
    ("PSYC-3310", "Abnormal psychology", 3.0),
    ("SOCI-1101", "Introduction to sociology", 3.0),
    ("SOCI-2210", "Belizean society and culture", 3.0),
    ("TOUR-1101", "Introduction to tourism", 3.0),
    ("TOUR-2201", "Ecotourism and protected areas", 3.0),
    ("TOUR-3110", "Hospitality operations", 3.0),
    // FHS — Health Sciences
    ("NURS-1101", "Foundations of nursing practice", 4.0),
    ("NURS-2201", "Adult health nursing I", 4.0),
    ("NURS-2202", "Adult health nursing II", 4.0),
    ("NURS-3110", "Community health nursing", 3.0),
    ("NURS-4201", "Clinical practicum", 6.0),
    ("HLTH-1101", "Personal and community health", 3.0),
    ("HLTH-2210", "Epidemiology", 3.0),
    ("HLTH-3301", "Health policy in small states", 3.0),
    ("SOWK-1101", "Introduction to social work", 3.0),
    (
        "SOWK-2201",
        "Human behavior and the social environment",
        3.0,
    ),
    ("SOWK-3310", "Field practice I", 4.0),
    // Agriculture (Central Farm)
    ("AGRI-1101", "Introduction to agriculture", 3.0),
    ("AGRI-2201", "Soil science", 3.0),
    ("AGRI-2210", "Tropical crop production", 3.0),
    // Deliberate two-line course title (dataset brief).
    (
        "AGRI-3405",
        "Sustainable agricultural systems and rural development in the Caribbean coastal \
         lowlands of Central America",
        3.0,
    ),
    ("AGRI-4110", "Agribusiness management", 3.0),
    ("FSCI-2101", "Food science and safety", 3.0),
    ("FSCI-3201", "Post-harvest technology", 3.0),
];

/// Weekly meeting patterns (day 1 = Monday, ISO). Index into this pool is
/// deterministic per section. Special shapes required by the brief are
/// pinned to fixed pattern slots: evening (5), Saturday (6), 3-hour lab
/// (7), online/no-meetings (8), and the deliberate conflict pair both use
/// slot 2.
const PATTERNS: &[&[(i16, &str, &str)]] = &[
    &[(1, "09:00", "10:15"), (3, "09:00", "10:15")],
    &[(1, "10:30", "11:45"), (3, "10:30", "11:45")], // back-to-back with slot 0
    &[(2, "13:00", "14:15"), (4, "13:00", "14:15")],
    &[(2, "08:00", "09:15"), (4, "08:00", "09:15")],
    &[(5, "09:00", "11:45")],
    &[(1, "18:00", "20:45")], // evening
    &[(6, "09:00", "12:00")], // Saturday
    &[(4, "13:00", "16:00")], // 3-hour lab block
    &[],                      // online: no meetings
    &[(2, "10:30", "11:45"), (4, "10:30", "11:45")],
    &[(1, "13:00", "14:15"), (3, "13:00", "14:15")],
    &[(5, "13:00", "15:45")],
];

const HOLD_FLAGS: [&str; 3] = ["advising", "financial", "library"];

/// FALL-2026 calendar: real Belize public holidays in the term window plus
/// ordinary campus events.
const EVENTS: &[(&str, &str, &str, &str)] = &[
    (
        "St. George's Caye Day",
        "holiday",
        "2026-09-10",
        "2026-09-10",
    ),
    ("Independence Day", "holiday", "2026-09-21", "2026-09-21"),
    ("Pan American Day", "holiday", "2026-10-12", "2026-10-12"),
    (
        "Garifuna Settlement Day",
        "holiday",
        "2026-11-19",
        "2026-11-19",
    ),
    ("Christmas Day", "holiday", "2026-12-25", "2026-12-25"),
    ("Boxing Day", "holiday", "2026-12-26", "2026-12-26"),
    ("Orientation week", "event", "2026-08-24", "2026-08-28"),
    (
        "Late registration closes",
        "event",
        "2026-09-04",
        "2026-09-04",
    ),
    ("Club and society fair", "event", "2026-09-11", "2026-09-11"),
    ("Career fair", "event", "2026-09-30", "2026-09-30"),
    ("Midterm examinations", "event", "2026-10-12", "2026-10-16"),
    ("Research symposium", "event", "2026-10-23", "2026-10-23"),
    ("Sports day", "event", "2026-10-30", "2026-10-30"),
    ("Cultural day", "event", "2026-11-06", "2026-11-06"),
    ("Reading week", "event", "2026-11-23", "2026-11-27"),
    ("Final examinations", "event", "2026-12-01", "2026-12-11"),
    ("Grades due", "event", "2026-12-18", "2026-12-18"),
    ("Graduation rehearsal", "event", "2026-12-19", "2026-12-19"),
];

/// Published-grade distribution for the completed terms. `None` points =
/// no prerequisite credit (per A29: I and W never satisfy a prerequisite).
const GRADES: &[(&str, Option<f64>, u32)] = &[
    ("A", Some(4.0), 18),
    ("A-", Some(3.7), 10),
    ("B+", Some(3.3), 12),
    ("B", Some(3.0), 15),
    ("B-", Some(2.7), 8),
    ("C+", Some(2.3), 8),
    ("C", Some(2.0), 10),
    ("D", Some(1.0), 7),
    ("F", Some(0.0), 5),
    ("I", None, 3),
    ("W", None, 4),
];

pub struct Stats {
    pub students: usize,
    pub instructors: usize,
    pub courses: usize,
    pub sections: usize,
    pub current_enrollments: usize,
    pub past_enrollments: usize,
    pub grades: usize,
    pub documents: usize,
    pub holds: usize,
    pub events: usize,
}

/// CLI entry: guards, then seed, then verify; exit code for main.rs.
pub async fn run(pool: &PgPool, config: &AppConfig) -> i32 {
    if config.environment.is_production() {
        eprintln!("seed-demo refuses to run with APP_ENV=production");
        return 1;
    }
    let started = Instant::now();
    // Seeding-only pool: synchronous_commit off. A dev seed has no
    // durability requirement, and ~20k service transactions at this
    // workstation's ~70 ms fsync would take twenty minutes for nothing.
    // The application pool and production path are untouched.
    let seed_pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::Connection::ping(conn).await?; // warm + typed as PgConnection
                sqlx::query("SET synchronous_commit TO off")
                    .execute(conn)
                    .await
                    .map(|_| ())
            })
        })
        .connect(&config.database_url)
        .await
    {
        Ok(seed_pool) => seed_pool,
        Err(error) => {
            eprintln!("seed-demo failed to open its pool: {error}");
            return 1;
        }
    };
    let _ = pool;
    match seed(&seed_pool, config, &FULL).await {
        Ok(Some(stats)) => {
            println!(
                "seed-demo complete in {:.1?}: {} students, {} instructors, {} courses, \
                 {} sections, {} current enrollments, {} past enrollments, {} grades, \
                 {} document requests, {} holds, {} events",
                started.elapsed(),
                stats.students,
                stats.instructors,
                stats.courses,
                stats.sections,
                stats.current_enrollments,
                stats.past_enrollments,
                stats.grades,
                stats.documents,
                stats.holds,
                stats.events,
            );
            println!("invariant verification: OK (counters, capacity rows, audit coverage)");
            0
        }
        Ok(None) => {
            println!(
                "seed-demo: dataset already present (institution STC exists) — nothing done. \
                 Rebuild: drop and recreate the database, re-apply src/dev/seed.sql, re-run."
            );
            0
        }
        Err(error) => {
            eprintln!("seed-demo failed: {error}");
            1
        }
    }
}

/// Seed everything. Returns None when the dataset already exists.
/// Requires seed.sql's core rows (UB institution, demo accounts, FALL-2026).
pub async fn seed(
    pool: &PgPool,
    config: &AppConfig,
    scale: &Scale,
) -> Result<Option<Stats>, AppError> {
    let already: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM institution WHERE code = 'STC' LIMIT 1")
            .fetch_optional(pool)
            .await?;
    if already.is_some() {
        return Ok(None);
    }
    let demo_core: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM user_account WHERE username = 'demo.student' LIMIT 1")
            .fetch_optional(pool)
            .await?;
    if demo_core.is_none() {
        return Err(AppError::Validation(
            "seed.sql has not been applied (demo.student missing); \
             run `psql -f src/dev/seed.sql` first"
                .into(),
        ));
    }

    let mut rng = StdRng::seed_from_u64(2026);
    let mut next_id: u64 = 1;
    let mut fresh = move || {
        let n = next_id;
        next_id += 1;
        id(n)
    };

    let audit = AuditWriter;
    let enrollment = EnrollmentService::new(pool.clone(), audit.clone());
    let grades = GradeService::new(pool.clone(), audit.clone());
    let documents = DocumentService::new(pool.clone(), audit.clone(), TranscriptSnapshotService);
    let institution = InstitutionService::new(pool.clone(), audit.clone());

    let registrar = staff_actor(DEMO_REGISTRAR, UB, &[Role::Registrar, Role::RecordsOfficer]);
    let officer = staff_actor(
        DEMO_REGISTRAR,
        UB,
        &[Role::RecordsOfficer, Role::DocumentOfficer],
    );
    let admin = staff_actor(DEMO_ADMIN, UB, &[Role::InstitutionAdmin]);

    // One Argon2 hash shared by every generated account: same password,
    // hashed once — seeding time goes to data, not to KDF work.
    let passwords = crate::identity_access::password::PasswordService::from_config(config)
        .map_err(|_| AppError::Internal)?;
    let password_hash = passwords
        .hash("ub-demo-password")
        .map_err(|_| AppError::Internal)?;

    // ---- People (bulk: inert reference data) -----------------------------
    let mut usernames = HashSet::new();
    let mut students: Vec<(Uuid, Uuid)> = Vec::new(); // (user_id, profile_id)
    let mut instructors: Vec<Uuid> = Vec::new();

    let mut accounts: Vec<(Uuid, String, String)> = Vec::new(); // user, username, role
    for i in 0..scale.students {
        let username = if i == 0 {
            SHORT_USERNAME.to_owned() // deliberate single-word name
        } else {
            unique_username(&mut rng, &mut usernames)
        };
        accounts.push((fresh(), username, "student".into()));
    }
    for i in 0..scale.instructors {
        let username = if i == 0 {
            LONG_INSTRUCTOR_USERNAME.to_owned() // deliberate column-stressing name
        } else {
            unique_username(&mut rng, &mut usernames)
        };
        accounts.push((fresh(), username, "instructor".into()));
    }
    // Named staff beyond the demo accounts, including one person who holds
    // two roles at once (the brief's multi-role requirement is also met by
    // demo.registrar; this one is a generated person).
    let extra_registrar = fresh();
    accounts.push((extra_registrar, "esther.tzul".into(), "registrar".into()));
    accounts.push((
        extra_registrar,
        "esther.tzul".into(),
        "records_officer".into(),
    ));

    insert_accounts(pool, UB, &accounts, &password_hash).await?;

    let student_accounts: Vec<(Uuid, String)> = accounts
        .iter()
        .filter(|(_, _, role)| role == "student")
        .map(|(user, name, _)| (*user, name.clone()))
        .collect();
    let mut profiles: Vec<(Uuid, Uuid, String)> = Vec::new();
    for (i, (user_id, _)) in student_accounts.iter().enumerate() {
        let profile_id = fresh();
        profiles.push((profile_id, *user_id, format!("2026{:05}", 10_000 + i)));
        students.push((*user_id, profile_id));
    }
    insert_profiles(pool, UB, &profiles, &mut rng).await?;
    for (user, _, role) in &accounts {
        if role == "instructor" {
            instructors.push(*user);
        }
    }

    // ---- Terms -----------------------------------------------------------
    // Past terms are created with windows OPEN NOW so the real services
    // accept registrations and grades; their windows move to the true past
    // dates after seeding (the term row is inert reference data).
    let fall_2025 = fresh();
    let spring_2026 = fresh();
    let spring_2027 = fresh();
    insert_terms(pool, fall_2025, spring_2026, spring_2027).await?;

    // ---- Courses, sections, meetings, capacities, assignments ------------
    let mut course_ids: HashMap<&'static str, Uuid> = HashMap::new();
    let mut course_rows: Vec<(Uuid, &str, &str, f64)> = Vec::new();
    for (code, title, credits) in COURSES {
        let cid = fresh();
        course_ids.insert(code, cid);
        course_rows.push((cid, code, title, *credits));
    }
    insert_courses(pool, UB, &course_rows).await?;
    insert_prerequisites(pool, &course_ids).await?;

    // Section plan: (section_id, term, course, pattern index, capacity).
    let mut sections: Vec<(Uuid, Uuid, Uuid, usize, i32)> = Vec::new();
    let plan = |term: Uuid, count: usize, rng: &mut StdRng, fresh: &mut dyn FnMut() -> Uuid| {
        let mut planned = Vec::new();
        for i in 0..count {
            let course = course_rows[i % course_rows.len()].0;
            // Fixed slots keep the special shapes present at any scale:
            // evening, Saturday, lab, online, and the conflict pair (two
            // consecutive sections sharing pattern 2).
            let pattern = match i {
                0 => 5,
                1 => 6,
                2 => 7,
                3 => 8,
                4 | 5 => 2,
                _ => rng.random_range(0..PATTERNS.len()),
            };
            let capacity = rng.random_range(18..=45);
            let sid = fresh();
            planned.push((sid, term, course, pattern, capacity));
        }
        planned
    };
    sections.extend(plan(
        fall_2025,
        scale.past_sections_per_term,
        &mut rng,
        &mut fresh,
    ));
    sections.extend(plan(
        spring_2026,
        scale.past_sections_per_term,
        &mut rng,
        &mut fresh,
    ));
    sections.extend(plan(
        FALL_2026,
        scale.current_sections,
        &mut rng,
        &mut fresh,
    ));
    // A handful of future-term sections; the closed registration window is
    // what keeps them un-registerable.
    sections.extend(plan(
        spring_2027,
        8.min(scale.current_sections),
        &mut rng,
        &mut fresh,
    ));

    insert_sections(pool, UB, &sections, &mut fresh).await?;

    // Badge variety: two ordinary current-term sections closed/cancelled
    // (skipping the pinned special-shape slots).
    let ordinary: Vec<Uuid> = sections
        .iter()
        .filter(|s| s.1 == FALL_2026)
        .skip(6)
        .map(|s| s.0)
        .collect();
    if let [closed, cancelled, ..] = ordinary[..] {
        sqlx::query("UPDATE section SET status = 'closed' WHERE id = $1")
            .bind(closed)
            .execute(pool)
            .await?;
        sqlx::query("UPDATE section SET status = 'cancelled' WHERE id = $1")
            .bind(cancelled)
            .execute(pool)
            .await?;
    }

    // Instructor load skew: the first quarter carry 4+ sections, the middle
    // carries 2–3, the tail 1 — and the LAST instructor is deliberately
    // assigned nothing (the brief's empty-state instructor).
    let assignable = &instructors[..instructors.len().saturating_sub(1)];
    let mut assignments: Vec<(Uuid, Uuid)> = Vec::new();
    for (i, (sid, _, _, _, _)) in sections.iter().enumerate() {
        let weighted = i % (assignable.len() * 2);
        let idx = if weighted < assignable.len() / 2 {
            weighted // first quarter appears twice as often
        } else {
            weighted % assignable.len()
        };
        assignments.push((*sid, assignable[idx]));
    }
    insert_assignments(pool, &assignments).await?;

    // ---- Past terms: enroll + grade through the real services ------------
    let mut past_enrollments = 0usize;
    let mut grade_count = 0usize;
    for term in [fall_2025, spring_2026] {
        let mut loads: HashMap<Uuid, usize> = HashMap::new();
        let term_sections: Vec<_> = sections.iter().filter(|s| s.1 == term).collect();
        for (sid, _, _, _, capacity) in &term_sections {
            let target = (*capacity as usize * rng.random_range(55..=90) / 100).max(3);
            let enrolled = fill_section(
                &enrollment,
                UB,
                &students,
                *sid,
                target,
                scale.max_load,
                &mut loads,
                &mut rng,
            )
            .await?;
            past_enrollments += enrolled.len();
            for enrollment_id in &enrolled {
                let (code, points) = pick_grade(&mut rng);
                grades
                    .save_draft(
                        &officer,
                        SaveGradeCommand {
                            enrollment_id: *enrollment_id,
                            grade_code: code.to_owned(),
                            grade_points: points,
                            numeric_value: None,
                            expected_version: 0,
                        },
                    )
                    .await?;
                grade_count += 1;
            }
            grades.publish_section(&officer, *sid).await?;
        }
    }
    // Guarantee the PHYS-2101 refill below can find enough prerequisite
    // holders: give the first 60 students a published MATH-2110 pass in
    // FALL-2025 (through the same services).
    let math_2110_past = fresh();
    sqlx::query(
        "INSERT INTO section (id, institution_id, term_id, course_id, section_code, status) \
         VALUES ($1, $2, $3, $4, '90', 'open')",
    )
    .bind(math_2110_past)
    .bind(UB)
    .bind(fall_2025)
    .bind(COURSE_MATH_2110)
    .execute(pool)
    .await?;
    sqlx::query("UPDATE section_capacity SET capacity = 80 WHERE section_id = $1")
        .bind(math_2110_past)
        .execute(pool)
        .await?;
    let prereq_holders = 60.min(students.len());
    let mut math_graded = 0usize;
    for (user_id, profile_id) in students.iter().take(prereq_holders) {
        let receipt = enrollment
            .register_for(
                &actor_in(UB, *user_id, *profile_id),
                *profile_id,
                RegisterCommand {
                    section_id: math_2110_past,
                    idempotency_key: fresh(),
                },
            )
            .await;
        if let Ok(receipt) = receipt {
            past_enrollments += 1;
            grades
                .save_draft(
                    &officer,
                    SaveGradeCommand {
                        enrollment_id: receipt.enrollment_id,
                        grade_code: "B".into(),
                        grade_points: Some(3.0),
                        numeric_value: None,
                        expected_version: 0,
                    },
                )
                .await?;
            math_graded += 1;
        }
    }
    grades.publish_section(&officer, math_2110_past).await?;
    grade_count += math_graded;

    // Move the completed terms into the actual past, and their transaction
    // timestamps with them (cosmetic; the rows are otherwise service-made).
    close_past_term(pool, fall_2025, "2025-08-25", "2025-12-12").await?;
    close_past_term(pool, spring_2026, "2026-01-12", "2026-05-08").await?;

    // ---- Current term ----------------------------------------------------
    // seed.sql ships MATH-3201 (22/25) and PHYS-2101 (40/40) as counters
    // with no enrollment rows. Reconcile: reset the fake counters and refill
    // through the real service so the VISIBLE demo state is identical and
    // the counter invariant is true for every section in the database.
    sqlx::query(
        "UPDATE section_capacity SET enrolled_count = 0 \
         WHERE section_id IN ($1, $2) AND enrolled_count > \
               (SELECT count(*) FROM enrollment e \
                 WHERE e.section_id = section_capacity.section_id AND e.status = 'enrolled')",
    )
    .bind(SEC_MATH_3201)
    .bind(SEC_PHYS_2101)
    .execute(pool)
    .await?;
    let mut current_enrollments = 0usize;
    let mut loads: HashMap<Uuid, usize> = HashMap::new();
    current_enrollments += fill_section(
        &enrollment,
        UB,
        &students,
        SEC_MATH_3201,
        22,
        usize::MAX,
        &mut loads,
        &mut rng,
    )
    .await?
    .len();
    let prereq_students: Vec<(Uuid, Uuid)> = students[..prereq_holders].to_vec();
    current_enrollments += fill_section(
        &enrollment,
        UB,
        &prereq_students,
        SEC_PHYS_2101,
        40,
        usize::MAX,
        &mut loads,
        &mut rng,
    )
    .await?
    .len();

    // The generated current-term sections: ~15% full, ~20% nearly full
    // (1–3 seats left), the rest 40–85%.
    // The empty-state student is students[1] ("first" generated after xi);
    // students[2] is reserved conflict-free for the override showcase below.
    // Both are excluded from every fill.
    let enrollable: Vec<(Uuid, Uuid)> = students
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 1 && *i != 2)
        .map(|(_, s)| *s)
        .collect();
    let mut showcase_full: Option<Uuid> = None;
    for (sid, term, _, pattern, capacity) in &sections {
        if *term != FALL_2026 {
            continue;
        }
        let roll = rng.random_range(0..100);
        let target = if roll < 15 {
            *capacity as usize
        } else if roll < 35 {
            (*capacity as usize).saturating_sub(rng.random_range(1..=3))
        } else {
            *capacity as usize * rng.random_range(40..=85) / 100
        };
        let filled = fill_section(
            &enrollment,
            UB,
            &enrollable,
            *sid,
            target,
            scale.max_load,
            &mut loads,
            &mut rng,
        )
        .await?
        .len();
        current_enrollments += filled;
        // Remember one genuinely full section with real meetings for the
        // override + amended-grade showcase.
        if showcase_full.is_none() && filled == *capacity as usize && !PATTERNS[*pattern].is_empty()
        {
            showcase_full = Some(*sid);
        }
    }

    // NOTE (brief asked for an over-enrolled section): the schema forbids it
    // outright — section_capacity CHECK (enrolled_count <= capacity), and
    // set_section_capacity refuses to shrink below enrollment. That state is
    // unrepresentable by design; recorded in the session report instead of
    // weakening a correctness constraint for display purposes.

    // ---- Override + amended-grade showcase (NOT on CMPS-2131) ------------
    // The CMPS-2131 roster the demo script narrates stays byte-for-byte as
    // seed.sql made it (publish_section is all-or-nothing and would flip
    // its seeded draft). Instead: one generated FULL section gets a real
    // registrar capacity override consumed by a reserved conflict-free
    // student, whose grade is then published and corrected — giving the UI
    // a consumed override record and an 'amended' badge without touching
    // any scripted scenario.
    if let Some(section_id) = showcase_full {
        let (user, profile) = students[2];
        enrollment
            .grant_override(
                &registrar,
                profile,
                GrantOverrideCommand {
                    term_id: FALL_2026,
                    section_id: Some(section_id),
                    override_type: "capacity".into(),
                    reason: "final graduation requirement; section is the only offering".into(),
                    expires_at: None,
                },
            )
            .await?;
        let receipt = enrollment
            .register_for(
                &actor_in(UB, user, profile),
                profile,
                RegisterCommand {
                    section_id,
                    idempotency_key: Uuid::new_v4(),
                },
            )
            .await
            .map_err(|error| AppError::Validation(format!("override showcase: {error}")))?;
        current_enrollments += 1;
        grades
            .save_draft(
                &officer,
                SaveGradeCommand {
                    enrollment_id: receipt.enrollment_id,
                    grade_code: "C".into(),
                    grade_points: Some(2.0),
                    numeric_value: None,
                    expected_version: 0,
                },
            )
            .await?;
        grades.publish_section(&officer, section_id).await?;
        let version: i64 =
            sqlx::query_scalar("SELECT version FROM grade_record WHERE enrollment_id = $1")
                .bind(receipt.enrollment_id)
                .fetch_one(pool)
                .await?;
        grades
            .correct_grade(
                &officer,
                CorrectGradeCommand {
                    enrollment_id: receipt.enrollment_id,
                    grade_code: "B-".into(),
                    grade_points: Some(2.7),
                    numeric_value: None,
                    reason: "transcription error on the exam total".into(),
                    expected_version: version,
                },
            )
            .await?;
    }

    // ---- Holds (~8% of students, one carrying two at once) --------------
    let mut holds = 0usize;
    let hold_count = (students.len() * 8 / 100).max(2);
    for i in 0..hold_count {
        let (_, profile_id) = students[3 + (i * 7) % (students.len() - 3)];
        let flag = HOLD_FLAGS[i % HOLD_FLAGS.len()];
        enrollment
            .place_hold(
                &registrar,
                profile_id,
                FALL_2026,
                flag,
                seed_hold_reason(flag),
            )
            .await?;
        holds += 1;
        if i == 0 {
            enrollment
                .place_hold(
                    &registrar,
                    profile_id,
                    FALL_2026,
                    "financial",
                    "outstanding balance from Spring 2026",
                )
                .await?;
            holds += 1;
        }
    }

    // ---- Documents: every status visible --------------------------------
    let worker = DocumentWorker::new(
        pool.clone(),
        "seed-demo-worker".into(),
        config.document_storage_path.clone(),
        config.job_stale_secs,
    );
    let doc_stats = seed_documents(
        pool,
        &documents,
        &worker,
        &officer,
        &students,
        scale.documents,
        &mut rng,
        &mut fresh,
    )
    .await?;

    // ---- Calendar (admin service; SPRING-2027 keeps zero events) ---------
    for (title, kind, from, to) in EVENTS {
        institution
            .create_event(
                &admin,
                crate::institution::service::EventCommand {
                    title: (*title).to_owned(),
                    event_type: (*kind).to_owned(),
                    starts_on: date(from),
                    ends_on: date(to),
                },
            )
            .await?;
    }

    // ---- The small second institution (cross-institution scoping) --------
    seed_stc(pool, config, scale, &password_hash, &mut rng, &mut fresh).await?;

    // ---- Audit backfill for seed.sql's pre-service core rows -------------
    backfill_core_audits(pool).await?;

    // ---- Verify or die ---------------------------------------------------
    verify(pool).await?;

    Ok(Some(Stats {
        students: students.len(),
        instructors: instructors.len(),
        courses: course_rows.len(),
        sections: sections.len() + 1, // + the past MATH-2110 section
        current_enrollments,
        past_enrollments,
        grades: grade_count + 2, // + the fifth-student entry and correction
        documents: doc_stats,
        holds,
        events: EVENTS.len(),
    }))
}

/// Register students into one section through the real service until the
/// target headcount is reached. Refusals (conflicts, holds, duplicates,
/// full) skip to the next candidate; real errors propagate.
#[allow(clippy::too_many_arguments)] // internal seeding helper, not an API
async fn fill_section(
    enrollment: &EnrollmentService,
    institution: Uuid,
    students: &[(Uuid, Uuid)],
    section_id: Uuid,
    target: usize,
    max_load: usize,
    loads: &mut HashMap<Uuid, usize>,
    rng: &mut StdRng,
) -> Result<Vec<Uuid>, AppError> {
    use crate::enrollment::types::EnrollError;
    let mut enrolled = Vec::new();
    if target == 0 || students.is_empty() {
        return Ok(enrolled);
    }
    let start = rng.random_range(0..students.len());
    for step in 0..students.len() {
        if enrolled.len() >= target {
            break;
        }
        let (user_id, profile_id) = students[(start + step) % students.len()];
        if *loads.get(&profile_id).unwrap_or(&0) >= max_load {
            continue;
        }
        match enrollment
            .register_for(
                &actor_in(institution, user_id, profile_id),
                profile_id,
                RegisterCommand {
                    section_id,
                    idempotency_key: Uuid::new_v4(),
                },
            )
            .await
        {
            Ok(receipt) => {
                *loads.entry(profile_id).or_insert(0) += 1;
                enrolled.push(receipt.enrollment_id);
            }
            Err(EnrollError::Denied(_)) => continue,
            Err(EnrollError::App(error)) => return Err(error),
        }
    }
    Ok(enrolled)
}

#[allow(clippy::too_many_arguments)] // seeding orchestration, not an API
async fn seed_documents(
    pool: &PgPool,
    documents: &DocumentService,
    worker: &DocumentWorker,
    officer: &Actor,
    students: &[(Uuid, Uuid)],
    total: usize,
    rng: &mut StdRng,
    fresh: &mut dyn FnMut() -> Uuid,
) -> Result<usize, AppError> {
    let kinds = ["official_transcript", "enrollment_letter"];
    let mut request_ids = Vec::new();
    for i in 0..total {
        let (user_id, profile_id) = students[(5 + i * 11) % students.len()];
        let receipt = documents
            .request_for_self(
                &actor_in(UB, user_id, profile_id),
                RequestDocumentCommand {
                    document_type: kinds[i % kinds.len()].to_owned(),
                    purpose: Some(seed_purpose(rng).to_owned()),
                    delivery_method: "download".into(),
                    idempotency_key: fresh(),
                },
            )
            .await?;
        request_ids.push(receipt.request_id);
    }

    // Officer decisions through the service: reject some, approve the rest
    // of the non-pending share (approval queues the render job).
    let pending = total * 25 / 100;
    let rejected = total * 17 / 100;
    let decided = &request_ids[pending..];
    for request_id in decided.iter().take(rejected) {
        documents
            .reject(officer, *request_id, "insufficient supporting information")
            .await?;
    }
    let approved: Vec<Uuid> = decided.iter().skip(rejected).copied().collect();
    for request_id in &approved {
        documents
            .approve(officer, *request_id, "identity verified against records")
            .await?;
    }

    // Render a batch to 'ready' with the real worker (real PDFs on disk).
    let ready = approved.len() / 2;
    for _ in 0..ready {
        worker.run_once().await?;
    }

    // Freeze the two remaining lifecycle states exactly as the real code
    // paths leave them. 'generating': the claim state of a worker that died
    // mid-render (what reap_stale exists to recover — worker.rs run_once /
    // fail are the mirrored statements). 'failed': a job that exhausted its
    // 3 attempts.
    // OFFSET 1 keeps at least one job honestly queued, so the 'approved'
    // state stays visible at every scale (SMOKE included).
    let frozen: Vec<Uuid> = sqlx::query_scalar(
        "SELECT request_id FROM document_job WHERE status = 'queued' \
         ORDER BY created_at LIMIT 5 OFFSET 1",
    )
    .fetch_all(pool)
    .await?;
    for (i, request_id) in frozen.iter().enumerate() {
        if i < 2 {
            sqlx::query(
                "UPDATE document_job SET status = 'running', attempts = attempts + 1, \
                 locked_at = now(), locked_by = 'seed-demo-crashed' WHERE request_id = $1",
            )
            .bind(request_id)
            .execute(pool)
            .await?;
            sqlx::query(
                "UPDATE document_request SET status = 'generating', updated_at = now() \
                 WHERE id = $1",
            )
            .bind(request_id)
            .execute(pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE document_job SET status = 'failed', attempts = 3, \
                 last_error = 'render failed: transcript snapshot unreadable (seeded failure)' \
                 WHERE request_id = $1",
            )
            .bind(request_id)
            .execute(pool)
            .await?;
            sqlx::query(
                "UPDATE document_request SET status = 'failed', updated_at = now() WHERE id = $1",
            )
            .bind(request_id)
            .execute(pool)
            .await?;
        }
    }
    Ok(total)
}

/// The tiny second institution: enough to check cross-institution scoping
/// by eye. No license row — a deployment is single-tenant, and
/// load_initial_license takes the latest-updated row (main.rs), which must
/// remain UB's.
async fn seed_stc(
    pool: &PgPool,
    config: &AppConfig,
    scale: &Scale,
    password_hash: &str,
    rng: &mut StdRng,
    fresh: &mut dyn FnMut() -> Uuid,
) -> Result<(), AppError> {
    let stc = fresh();
    sqlx::query(
        "INSERT INTO institution (id, code, name, timezone, status) \
         VALUES ($1, 'STC', 'Stann Creek Technical College', 'America/Belize', 'active')",
    )
    .bind(stc)
    .execute(pool)
    .await?;

    let mut usernames = HashSet::new();
    let mut accounts: Vec<(Uuid, String, String)> = Vec::new();
    for _ in 0..scale.stc_students {
        accounts.push((
            fresh(),
            unique_username(rng, &mut usernames),
            "student".into(),
        ));
    }
    let stc_instructor = fresh();
    accounts.push((stc_instructor, "ines.palacio".into(), "instructor".into()));
    let stc_registrar = fresh();
    accounts.push((stc_registrar, "colin.flowers".into(), "registrar".into()));
    insert_accounts(pool, stc, &accounts, password_hash).await?;

    let mut profiles: Vec<(Uuid, Uuid, String)> = Vec::new();
    let mut students: Vec<(Uuid, Uuid)> = Vec::new();
    for (i, (user, _, role)) in accounts.iter().enumerate() {
        if role == "student" {
            let profile = fresh();
            profiles.push((profile, *user, format!("STC26{:04}", 100 + i)));
            students.push((*user, profile));
        }
    }
    insert_profiles(pool, stc, &profiles, rng).await?;

    let term = fresh();
    sqlx::query(
        "INSERT INTO academic_term (id, institution_id, code, name, starts_on, ends_on, \
         registration_opens_at, add_drop_closes_at, grade_entry_closes_at) \
         VALUES ($1, $2, 'FALL-2026', 'Fall 2026', \
                 LEAST(CURRENT_DATE, DATE '2026-08-24'), DATE '2026-12-11', \
                 now() - interval '7 days', now() + interval '35 days', \
                 now() + interval '150 days')",
    )
    .bind(term)
    .bind(stc)
    .execute(pool)
    .await?;

    let courses = [
        ("AGRI-1101", "Introduction to agriculture"),
        ("FSCI-2101", "Food science and safety"),
        ("BUSI-1101", "Introduction to business"),
        ("ENGL-1011", "English composition I"),
        ("MATH-1101", "College algebra"),
        ("TOUR-1101", "Introduction to tourism"),
    ];
    let mut section_ids = Vec::new();
    for (i, (code, title)) in courses.iter().enumerate() {
        let course = fresh();
        sqlx::query(
            "INSERT INTO course (id, institution_id, code, title, credit_hours) \
             VALUES ($1, $2, $3, $4, 3.0)",
        )
        .bind(course)
        .bind(stc)
        .bind(code)
        .bind(title)
        .execute(pool)
        .await?;
        let section = fresh();
        sqlx::query(
            "INSERT INTO section (id, institution_id, term_id, course_id, section_code, status) \
             VALUES ($1, $2, $3, $4, '01', 'open')",
        )
        .bind(section)
        .bind(stc)
        .bind(term)
        .bind(course)
        .execute(pool)
        .await?;
        sqlx::query("UPDATE section_capacity SET capacity = 20 WHERE section_id = $1")
            .bind(section)
            .execute(pool)
            .await?;
        let pattern = PATTERNS[i % 4];
        for (day, from, to) in pattern {
            sqlx::query(
                "INSERT INTO section_meeting (id, section_id, day_of_week, starts_at, ends_at) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(fresh())
            .bind(section)
            .bind(day)
            .bind(time(from))
            .bind(time(to))
            .execute(pool)
            .await?;
        }
        sqlx::query(
            "INSERT INTO instructor_assignment (section_id, instructor_user_id) \
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(section)
        .bind(stc_instructor)
        .execute(pool)
        .await?;
        section_ids.push(section);
    }

    let enrollment = EnrollmentService::new(pool.clone(), AuditWriter);
    let mut loads: HashMap<Uuid, usize> = HashMap::new();
    for section in &section_ids {
        fill_section(
            &enrollment,
            stc,
            &students,
            *section,
            10,
            3,
            &mut loads,
            rng,
        )
        .await?;
    }
    let _ = config;
    Ok(())
}

/// Verification pass (the brief's "fail loudly"): counter reconciliation,
/// capacity-row presence, and audit coverage for every auditable row in the
/// database — including seed.sql's backfilled core rows.
pub async fn verify(pool: &PgPool) -> Result<(), AppError> {
    let drifted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM section_capacity c \
          WHERE c.enrolled_count <> (SELECT count(*) FROM enrollment e \
                 WHERE e.section_id = c.section_id AND e.status = 'enrolled')",
    )
    .fetch_one(pool)
    .await?;
    let capacity_missing: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM section s \
          WHERE NOT EXISTS (SELECT 1 FROM section_capacity c WHERE c.section_id = s.id)",
    )
    .fetch_one(pool)
    .await?;
    let unaudited_enrollments: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM enrollment e \
          WHERE NOT EXISTS (SELECT 1 FROM audit_event a \
                 WHERE a.action = 'enrollment.registered' AND a.resource_id = e.id)",
    )
    .fetch_one(pool)
    .await?;
    let unaudited_requests: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM document_request r \
          WHERE NOT EXISTS (SELECT 1 FROM audit_event a \
                 WHERE a.action = 'document.requested' AND a.resource_id = r.id)",
    )
    .fetch_one(pool)
    .await?;
    let unaudited_grades: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM grade_record g \
          WHERE NOT EXISTS (SELECT 1 FROM audit_event a \
                 WHERE a.action IN ('grade.draft_created', 'grade.corrected') \
                   AND a.resource_id = g.id)",
    )
    .fetch_one(pool)
    .await?;

    if drifted + capacity_missing + unaudited_enrollments + unaudited_requests + unaudited_grades
        > 0
    {
        return Err(AppError::Validation(format!(
            "seeded dataset violates invariants: {drifted} drifted seat counters, \
             {capacity_missing} sections without capacity rows, \
             {unaudited_enrollments} unaudited enrollments, \
             {unaudited_requests} unaudited document requests, \
             {unaudited_grades} unaudited grade records"
        )));
    }
    Ok(())
}

/// seed.sql bulk-inserts 4 enrollments, 2 grade rows, and 1 document
/// request with no audit trail (they predate the services in this path).
/// Backfill clearly-labeled audit events so coverage holds database-wide.
async fn backfill_core_audits(pool: &PgPool) -> Result<(), AppError> {
    let detail = serde_json::json!({"backfilled_by": "seed-demo",
        "reason": "seed.sql core row created before service involvement"});
    for (action, resource_type, ids) in [
        (
            "enrollment.registered",
            "enrollment",
            CORE_ENROLLMENTS.to_vec(),
        ),
        ("grade.draft_created", "grade_record", CORE_GRADES.to_vec()),
        (
            "document.requested",
            "document_request",
            vec![CORE_DOC_REQUEST],
        ),
    ] {
        for resource in ids {
            sqlx::query(
                "INSERT INTO audit_event (id, institution_id, actor_user_id, action, \
                 resource_type, resource_id, detail, occurred_at) \
                 SELECT gen_random_uuid(), $1, $2, $3, $4, $5, $6, now() \
                 WHERE NOT EXISTS (SELECT 1 FROM audit_event \
                        WHERE action = $3 AND resource_id = $5)",
            )
            .bind(UB)
            .bind(DEMO_REGISTRAR)
            .bind(action)
            .bind(resource_type)
            .bind(resource)
            .bind(&detail)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Bulk-insert helpers (inert reference data only).
// ---------------------------------------------------------------------------

async fn insert_accounts(
    pool: &PgPool,
    institution: Uuid,
    accounts: &[(Uuid, String, String)],
    password_hash: &str,
) -> Result<(), AppError> {
    let mut seen = HashSet::new();
    let unique: Vec<&(Uuid, String, String)> = accounts
        .iter()
        .filter(|(id, _, _)| seen.insert(*id))
        .collect();
    let ids: Vec<Uuid> = unique.iter().map(|(id, _, _)| *id).collect();
    let names: Vec<String> = unique.iter().map(|(_, name, _)| name.clone()).collect();
    let emails: Vec<String> = unique
        .iter()
        .map(|(_, name, _)| format!("{}@ub-demo.example.test", name.replace('\'', "")))
        .collect();
    sqlx::query(
        "INSERT INTO user_account (id, institution_id, username, email, status) \
         SELECT u.id, $1, u.username, u.email, 'active' \
           FROM UNNEST($2::uuid[], $3::text[], $4::text[]) AS u(id, username, email)",
    )
    .bind(institution)
    .bind(&ids)
    .bind(&names)
    .bind(&emails)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO password_credential (user_id, password_hash) \
         SELECT id, $2 FROM UNNEST($1::uuid[]) AS u(id)",
    )
    .bind(&ids)
    .bind(password_hash)
    .execute(pool)
    .await?;
    let role_users: Vec<Uuid> = accounts.iter().map(|(id, _, _)| *id).collect();
    let role_codes: Vec<String> = accounts.iter().map(|(_, _, role)| role.clone()).collect();
    sqlx::query(
        "INSERT INTO user_role (institution_id, user_id, role_id) \
         SELECT $1, u.user_id, r.id \
           FROM UNNEST($2::uuid[], $3::text[]) AS u(user_id, code) \
           JOIN role r ON r.code = u.code \
         ON CONFLICT DO NOTHING",
    )
    .bind(institution)
    .bind(&role_users)
    .bind(&role_codes)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_profiles(
    pool: &PgPool,
    institution: Uuid,
    profiles: &[(Uuid, Uuid, String)],
    rng: &mut StdRng,
) -> Result<(), AppError> {
    const PROGRAMS: [&str; 8] = ["CS", "BIOL", "NURS", "BUSI", "EDUC", "TOUR", "AGRI", "ENGL"];
    let ids: Vec<Uuid> = profiles.iter().map(|(id, _, _)| *id).collect();
    let users: Vec<Uuid> = profiles.iter().map(|(_, user, _)| *user).collect();
    let numbers: Vec<String> = profiles.iter().map(|(_, _, n)| n.clone()).collect();
    let programs: Vec<String> = profiles
        .iter()
        .map(|_| PROGRAMS[rng.random_range(0..PROGRAMS.len())].to_owned())
        .collect();
    sqlx::query(
        "INSERT INTO student_profile (id, institution_id, user_id, student_number, program_code) \
         SELECT p.id, $1, p.user_id, p.number, p.program \
           FROM UNNEST($2::uuid[], $3::uuid[], $4::text[], $5::text[]) \
                AS p(id, user_id, number, program)",
    )
    .bind(institution)
    .bind(&ids)
    .bind(&users)
    .bind(&numbers)
    .bind(&programs)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_terms(
    pool: &PgPool,
    fall_2025: Uuid,
    spring_2026: Uuid,
    spring_2027: Uuid,
) -> Result<(), AppError> {
    // Past terms open NOW (see seed()); the future term's registration
    // window opens next July, which is what "not yet open" means to the
    // service.
    sqlx::query(
        "INSERT INTO academic_term (id, institution_id, code, name, starts_on, ends_on, \
         registration_opens_at, add_drop_closes_at, grade_entry_closes_at) VALUES \
         ($1, $4, 'FALL-2025', 'Fall 2025', DATE '2025-08-25', DATE '2025-12-12', \
          now() - interval '1 day', now() + interval '30 days', now() + interval '60 days'), \
         ($2, $4, 'SPRING-2026', 'Spring 2026', DATE '2026-01-12', DATE '2026-05-08', \
          now() - interval '1 day', now() + interval '30 days', now() + interval '60 days'), \
         ($3, $4, 'SPRING-2027', 'Spring 2027', DATE '2027-01-11', DATE '2027-05-07', \
          TIMESTAMPTZ '2026-11-02 14:00+00', TIMESTAMPTZ '2027-01-22 23:59+00', \
          TIMESTAMPTZ '2027-05-21 23:59+00')",
    )
    .bind(fall_2025)
    .bind(spring_2026)
    .bind(spring_2027)
    .bind(UB)
    .execute(pool)
    .await?;
    Ok(())
}

/// Rewrite a completed term's windows to their true past dates, and move
/// the service-written wall-clock stamps of its enrollments/grades into the
/// term window (cosmetic realism; relationships and audits are untouched).
async fn close_past_term(
    pool: &PgPool,
    term: Uuid,
    starts: &str,
    ends: &str,
) -> Result<(), AppError> {
    let starts = date(starts);
    let ends = date(ends);
    sqlx::query(
        "UPDATE academic_term SET \
         registration_opens_at = ($1::date - 14) :: timestamptz, \
         add_drop_closes_at = ($1::date + 14) :: timestamptz, \
         grade_entry_closes_at = ($2::date + 14) :: timestamptz \
         WHERE id = $3",
    )
    .bind(starts)
    .bind(ends)
    .bind(term)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE enrollment SET registered_at = ($2::date + 2) :: timestamptz \
          WHERE section_id IN (SELECT id FROM section WHERE term_id = $1)",
    )
    .bind(term)
    .bind(starts)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE grade_record SET published_at = ($2::date + 10) :: timestamptz \
          WHERE published_at IS NOT NULL AND enrollment_id IN \
                (SELECT e.id FROM enrollment e JOIN section s ON s.id = e.section_id \
                  WHERE s.term_id = $1)",
    )
    .bind(term)
    .bind(ends)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_courses(
    pool: &PgPool,
    institution: Uuid,
    rows: &[(Uuid, &str, &str, f64)],
) -> Result<(), AppError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let codes: Vec<String> = rows.iter().map(|r| r.1.to_owned()).collect();
    let titles: Vec<String> = rows.iter().map(|r| r.2.to_owned()).collect();
    let credits: Vec<f64> = rows.iter().map(|r| r.3).collect();
    sqlx::query(
        "INSERT INTO course (id, institution_id, code, title, credit_hours) \
         SELECT c.id, $1, c.code, c.title, c.credits \
           FROM UNNEST($2::uuid[], $3::text[], $4::text[], $5::float8[]) \
                AS c(id, code, title, credits)",
    )
    .bind(institution)
    .bind(&ids)
    .bind(&codes)
    .bind(&titles)
    .bind(&credits)
    .execute(pool)
    .await?;
    Ok(())
}

/// A few realistic chains beyond seed.sql's PHYS-2101 → MATH-2110.
async fn insert_prerequisites(
    pool: &PgPool,
    courses: &HashMap<&'static str, Uuid>,
) -> Result<(), AppError> {
    const CHAINS: &[(&str, &str)] = &[
        ("CMPS-2111", "CMPS-1150"),
        ("CMPS-3111", "CMPS-2111"),
        ("CMPS-4131", "CMPS-2111"),
        ("MATH-2101", "MATH-1201"),
        ("MATH-3402", "MATH-2101"),
        ("CHEM-2201", "CHEM-1102"),
        ("BIOL-2210", "BIOL-1102"),
        ("NURS-2201", "NURS-1101"),
        ("ACCT-1102", "ACCT-1101"),
        ("SPAN-1102", "SPAN-1101"),
    ];
    for (course, prerequisite) in CHAINS {
        let (Some(course_id), Some(prereq_id)) = (courses.get(course), courses.get(prerequisite))
        else {
            continue;
        };
        sqlx::query(
            "INSERT INTO course_prerequisite (course_id, prerequisite_course_id, \
             minimum_grade_points) VALUES ($1, $2, 1.0) ON CONFLICT DO NOTHING",
        )
        .bind(course_id)
        .bind(prereq_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn insert_sections(
    pool: &PgPool,
    institution: Uuid,
    sections: &[(Uuid, Uuid, Uuid, usize, i32)],
    fresh: &mut dyn FnMut() -> Uuid,
) -> Result<(), AppError> {
    let ids: Vec<Uuid> = sections.iter().map(|s| s.0).collect();
    let terms: Vec<Uuid> = sections.iter().map(|s| s.1).collect();
    let courses: Vec<Uuid> = sections.iter().map(|s| s.2).collect();
    // Section codes count up per (term, course); "05"-style zero-padded.
    let mut counters: HashMap<(Uuid, Uuid), u32> = HashMap::new();
    let codes: Vec<String> = sections
        .iter()
        .map(|s| {
            let n = counters.entry((s.1, s.2)).or_insert(0);
            *n += 1;
            format!("{:02}", *n + 1) // seed.sql owns "01" for its courses
        })
        .collect();
    sqlx::query(
        "INSERT INTO section (id, institution_id, term_id, course_id, section_code, status) \
         SELECT s.id, $1, s.term, s.course, s.code, 'open' \
           FROM UNNEST($2::uuid[], $3::uuid[], $4::uuid[], $5::text[]) \
                AS s(id, term, course, code)",
    )
    .bind(institution)
    .bind(&ids)
    .bind(&terms)
    .bind(&courses)
    .bind(&codes)
    .execute(pool)
    .await?;
    // Capacities (trigger created the rows at 0 — set the planned values).
    let capacities: Vec<i32> = sections.iter().map(|s| s.4).collect();
    sqlx::query(
        "UPDATE section_capacity c SET capacity = v.capacity \
           FROM UNNEST($1::uuid[], $2::int4[]) AS v(section_id, capacity) \
          WHERE c.section_id = v.section_id",
    )
    .bind(&ids)
    .bind(&capacities)
    .execute(pool)
    .await?;

    // Meetings from the pattern pool.
    let mut m_ids = Vec::new();
    let mut m_sections = Vec::new();
    let mut m_days: Vec<i16> = Vec::new();
    let mut m_starts: Vec<NaiveTime> = Vec::new();
    let mut m_ends: Vec<NaiveTime> = Vec::new();
    for (sid, _, _, pattern, _) in sections {
        for (day, from, to) in PATTERNS[*pattern] {
            m_ids.push(fresh());
            m_sections.push(*sid);
            m_days.push(*day);
            m_starts.push(time(from));
            m_ends.push(time(to));
        }
    }
    sqlx::query(
        "INSERT INTO section_meeting (id, section_id, day_of_week, starts_at, ends_at) \
         SELECT m.id, m.section_id, m.day, m.starts, m.ends \
           FROM UNNEST($1::uuid[], $2::uuid[], $3::int2[], $4::time[], $5::time[]) \
                AS m(id, section_id, day, starts, ends)",
    )
    .bind(&m_ids)
    .bind(&m_sections)
    .bind(&m_days)
    .bind(&m_starts)
    .bind(&m_ends)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_assignments(pool: &PgPool, rows: &[(Uuid, Uuid)]) -> Result<(), AppError> {
    let sections: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let instructors: Vec<Uuid> = rows.iter().map(|r| r.1).collect();
    sqlx::query(
        "INSERT INTO instructor_assignment (section_id, instructor_user_id) \
         SELECT a.section_id, a.instructor FROM UNNEST($1::uuid[], $2::uuid[]) \
              AS a(section_id, instructor) ON CONFLICT DO NOTHING",
    )
    .bind(&sections)
    .bind(&instructors)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Small pure helpers.
// ---------------------------------------------------------------------------

fn actor_in(institution_id: Uuid, user_id: Uuid, profile_id: Uuid) -> Actor {
    Actor {
        user_id,
        institution_id,
        student_id: Some(profile_id),
        roles: HashSet::from([Role::Student]),
    }
}

fn staff_actor(user_id: Uuid, institution_id: Uuid, roles: &[Role]) -> Actor {
    Actor {
        user_id,
        institution_id,
        student_id: None,
        roles: roles.iter().copied().collect(),
    }
}

fn unique_username(rng: &mut StdRng, taken: &mut HashSet<String>) -> String {
    loop {
        let first = FIRST_NAMES[rng.random_range(0..FIRST_NAMES.len())];
        let last = SURNAMES[rng.random_range(0..SURNAMES.len())];
        let base = format!("{first}.{last}");
        let candidate = if taken.contains(&base) {
            format!("{base}{}", rng.random_range(2..99))
        } else {
            base
        };
        if taken.insert(candidate.clone()) {
            return candidate;
        }
    }
}

fn pick_grade(rng: &mut StdRng) -> (&'static str, Option<f64>) {
    let total: u32 = GRADES.iter().map(|g| g.2).sum();
    let mut roll = rng.random_range(0..total);
    for (code, points, weight) in GRADES {
        if roll < *weight {
            return (code, *points);
        }
        roll -= weight;
    }
    ("B", Some(3.0))
}

fn seed_hold_reason(flag: &str) -> &'static str {
    match flag {
        "advising" => "advising appointment required before registration",
        "financial" => "outstanding balance on student account",
        _ => "unreturned library materials",
    }
}

fn seed_purpose(rng: &mut StdRng) -> &'static str {
    const PURPOSES: [&str; 5] = [
        "Graduate school application",
        "Employment verification",
        "Scholarship application",
        "Visa application",
        "Professional licensing board",
    ];
    PURPOSES[rng.random_range(0..PURPOSES.len())]
}

fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("seed date literal")
}

fn time(s: &str) -> NaiveTime {
    NaiveTime::parse_from_str(s, "%H:%M").expect("seed time literal")
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO_STUDENT_USER: Uuid = Uuid::from_u128(0x11);
    const DEMO_STUDENT_PROFILE: Uuid = Uuid::from_u128(0x12);
    const DEMO_INSTRUCTOR: Uuid = Uuid::from_u128(0x15);
    const SEC_CMPS_2131: Uuid = Uuid::from_u128(0x31);

    /// The dataset acceptance test (SMOKE scale): every stage runs against
    /// a real database, the invariant verification inside seed() must hold
    /// (drifted counters / missing capacity rows / unaudited rows fail it),
    /// the demo-script core scenarios survive untouched, and a second run
    /// is a no-op.
    #[sqlx::test(migrations = "./migrations")]
    async fn seeded_dataset_satisfies_invariants_and_preserves_the_demo_cores(
        pool_opts: sqlx::pool::PoolOptions<sqlx::Postgres>,
        connect_opts: sqlx::postgres::PgConnectOptions,
    ) {
        // Own pool instead of the harness one: the full suite saturates
        // Postgres (24-way parallelism vs max_connections=100) and the
        // default 30 s acquire timeout can expire before this deliberately
        // heavy test gets its first connection. synchronous_commit off for
        // the same reason as the CLI path — thousands of seed transactions
        // with no durability requirement.
        let pool = pool_opts
            .max_connections(4)
            .acquire_timeout(std::time::Duration::from_secs(180))
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET synchronous_commit TO off")
                        .execute(conn)
                        .await
                        .map(|_| ())
                })
            })
            .connect_with(connect_opts)
            .await
            .expect("seed test pool connects");
        sqlx::raw_sql(include_str!("seed.sql"))
            .execute(&pool)
            .await
            .expect("seed.sql applies");

        let mut config = crate::config::AppConfig::from_env().expect("test env config");
        config.document_storage_path = std::env::temp_dir().join("seed-demo-test-docs");

        let stats = seed(&pool, &config, &SMOKE)
            .await
            .expect("seed-demo succeeds (verification included)")
            .expect("fresh database seeds");
        assert!(stats.current_enrollments > 0);
        assert!(stats.past_enrollments > 0);
        assert!(stats.documents > 0);

        // Every document lifecycle state is on the board.
        let states: Vec<String> =
            sqlx::query_scalar("SELECT DISTINCT status FROM document_request ORDER BY status")
                .fetch_all(&pool)
                .await
                .unwrap();
        for state in [
            "approved",
            "failed",
            "generating",
            "pending",
            "ready",
            "rejected",
        ] {
            assert!(
                states.iter().any(|s| s == state),
                "missing doc state {state}"
            );
        }

        // The rehearsed demo cores are untouched (DEMO_SCRIPT.md hard
        // constraint): MATH-3201 shows exactly 3 seats, PHYS-2101 is full,
        // CMPS-2131 is 4/4 with its published/draft/blank roster, and
        // demo.student's transcript request is still pending.
        let (math_free, phys_free, cmps): (i64, i64, i64) = sqlx::query_as(
            "SELECT \
             (SELECT (capacity - enrolled_count)::int8 FROM section_capacity WHERE section_id = $1), \
             (SELECT (capacity - enrolled_count)::int8 FROM section_capacity WHERE section_id = $2), \
             (SELECT enrolled_count::int8 FROM section_capacity WHERE section_id = $3)",
        )
        .bind(SEC_MATH_3201)
        .bind(SEC_PHYS_2101)
        .bind(SEC_CMPS_2131)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((math_free, phys_free, cmps), (3, 0, 4));
        let roster_states: Vec<String> = sqlx::query_scalar(
            "SELECT state FROM grade_record WHERE enrollment_id IN \
             (SELECT id FROM enrollment WHERE section_id = $1) ORDER BY state",
        )
        .bind(SEC_CMPS_2131)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(roster_states, ["draft", "published"]);
        let pending: String =
            sqlx::query_scalar("SELECT status FROM document_request WHERE student_id = $1 \
                                AND document_type = 'official_transcript' ORDER BY requested_at LIMIT 1")
                .bind(DEMO_STUDENT_PROFILE)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pending, "pending");
        // demo.student can still register for CMPS-3141 (the scripted live
        // registration): their journey account exists untouched.
        let demo_user: Uuid =
            sqlx::query_scalar("SELECT id FROM user_account WHERE username = 'demo.student'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(demo_user, DEMO_STUDENT_USER);
        let instructor_sections: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM instructor_assignment WHERE instructor_user_id = $1",
        )
        .bind(DEMO_INSTRUCTOR)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            instructor_sections, 2,
            "demo.instructor keeps exactly CMPS-2131 + CMPS-3141"
        );

        // The demo term must be the current term: current_term resolves by
        // date span, and the sectionless DEV-2026 bootstrap term once
        // shadowed FALL-2026, blanking every student screen. FALL-2026
        // starts no later than today (LEAST in seed.sql) and DEV-2026 ends
        // before it starts.
        let (fall_started, dev_clear): (bool, bool) = sqlx::query_as(
            "SELECT (SELECT starts_on <= CURRENT_DATE FROM academic_term WHERE id = $1), \
                    (SELECT d.ends_on < f.starts_on FROM academic_term d, academic_term f \
                      WHERE d.code = 'DEV-2026' AND f.id = $1)",
        )
        .bind(Uuid::from_u128(0x20))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            fall_started && dev_clear,
            "FALL-2026 must span today and DEV-2026 must not shadow it"
        );

        // Idempotence: the second run detects the dataset and does nothing.
        let rerun = seed(&pool, &config, &SMOKE).await.expect("re-run is clean");
        assert!(rerun.is_none(), "second run must skip");
    }
}
