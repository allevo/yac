use std::sync::Arc;

use futures::lock::Mutex;

use crate::{
    credential_service::{CredentialService, CredentialServiceError},
    models::UserId,
};

use warp::{self, http::HeaderValue, reject, Filter, Rejection};

pub fn resolve_jwt(
    credential_service: Arc<Mutex<CredentialService>>,
) -> impl Filter<Extract = (UserId,), Error = Rejection> + Clone {
    let credential_service_filter = warp::any().map(move || credential_service.clone());

    warp::any()
        .and(credential_service_filter)
        .and(warp::header::value("Authorization"))
        .and_then(|credential_service: Arc<Mutex<CredentialService>>, auth_header: HeaderValue| async move {
            match auth_header.to_str() {
                Ok(auth_header) => {
                    match auth_header
                        .split_once("Bearer ") {
                        None => Err(reject::custom(CredentialServiceError::NoCredentialFound)),
                        Some((_, jwt)) => {
                            let credential_service = credential_service.lock().await;
                            match credential_service.try_decode(jwt) {
                                Ok(user_id) => Ok(user_id),
                                Err(err) => Err(reject::custom(err)),
                            }
                        }
                    }
                },
                Err(_) => Err(reject::custom(CredentialServiceError::NoCredentialFound)),
            }
        })
}
