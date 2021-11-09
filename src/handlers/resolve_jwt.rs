use std::sync::Arc;

use futures::lock::Mutex;

use crate::{
    credential_service::{CredentialService, CredentialServiceError},
    models::UserId,
};

use warp::{self, Filter, Rejection, http::{HeaderValue, header::AUTHORIZATION}, reject};

/// Used for parsing the Authorization header
pub fn resolve_jwt(
    credential_service: Arc<Mutex<CredentialService>>,
) -> impl Filter<Extract = (UserId,), Error = Rejection> + Clone {
    let credential_service_filter = warp::any().map(move || credential_service.clone());

    warp::any()
        .and(credential_service_filter)
        .and(warp::header::value(AUTHORIZATION.as_str()))
        .and_then(|credential_service: Arc<Mutex<CredentialService>>, auth_header: HeaderValue| async move {
            match auth_header.to_str() {
                Ok(auth_header) => {
                    match auth_header
                        .split_once("Bearer ") {
                        Some((before, jwt)) => {
                            if !before.is_empty() {
                                // Malformed Bearer header
                                return Err(reject::custom(CredentialServiceError::NoCredentialFound));
                            }
                            let credential_service = credential_service.lock().await;
                            match credential_service.try_decode(jwt) {
                                Ok(user_id) => Ok(user_id),
                                // Error in decoding JWT
                                Err(err) => Err(reject::custom(err)),
                            }
                        }
                        // "Bearer: " doesn't appear in the header
                        None => Err(reject::custom(CredentialServiceError::NoCredentialFound))
                    }
                }
                // no Authorization header is provided
                Err(_) => Err(reject::custom(CredentialServiceError::NoCredentialFound))
            }
        })
}
