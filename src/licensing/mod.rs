mod gate;
pub(crate) mod http;
pub(crate) mod middleware;
mod service;
mod signed_license;

#[cfg(test)]
mod tests;

pub use gate::{LicenseGate, LicenseSnapshot, LicenseStatus};
pub use service::LicenseService;
