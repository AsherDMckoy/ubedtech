pub(crate) mod http;
mod policy;
pub(crate) mod service;

pub use service::AcademicsService;

#[cfg(test)]
mod tests;
