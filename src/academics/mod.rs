pub(crate) mod http;
mod policy;
mod service;

pub use service::AcademicsService;

#[cfg(test)]
mod tests;
