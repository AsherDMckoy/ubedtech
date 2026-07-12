use actix_web::{FromRequest, HttpMessage, HttpRequest, dev::Payload};
use std::future::{Ready, ready};

use crate::shared::actor::Actor;

impl FromRequest for Actor {
    type Error = crate::shared::error::AppError;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        ready(
            req.extensions()
                .get::<Actor>()
                .cloned()
                .ok_or(crate::shared::error::AppError::Unauthenticated),
        )
    }
}
