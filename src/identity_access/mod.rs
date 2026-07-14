pub(crate) mod extractor; // Actix FromRequest implementation
pub(crate) mod http;
pub(crate) mod middleware;
pub(crate) mod password;
pub(crate) mod service;
pub(crate) mod sessions;

#[cfg(test)]
mod tests;
