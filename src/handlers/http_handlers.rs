use std::sync::Arc;

use futures::lock::Mutex;
use warp::{http::StatusCode, Rejection, Reply};

use crate::{
    chat_service::{Chat, ChatService, ChatServiceError, Chats},
    credential_service::{CredentialService, CredentialServiceError},
    models::{AddToChat, ChatId, RemoveFromChat, UserId},
};

use serde::{Deserialize, Serialize};

/// Login handler.
pub async fn login(
    request_body: LoginRequest,
    credential_service: Arc<Mutex<CredentialService>>,
) -> Result<impl warp::Reply, Rejection> {
    let credential_service = credential_service.lock().await;
    let (user_id, jwt) = credential_service.login(request_body.username, request_body.password)?;

    Ok(LoginResponse { user_id, jwt })
}

/// Returns the list of the chats
pub async fn get_chats(
    _: UserId,
    chat_service: Arc<Mutex<ChatService>>,
) -> Result<impl warp::Reply, Rejection> {
    let chats = {
        let chat_service = chat_service.lock().await;
        chat_service.list_chats().await?
    };

    Ok(chats.into_response())
}

/// Create a chat
pub async fn create_chat(
    request_body: CreateChatRequest,
    user_id: UserId,
    chat_service: Arc<Mutex<ChatService>>,
) -> Result<impl warp::Reply, Rejection> {
    let chats = {
        let mut chat_service = chat_service.lock().await;
        chat_service.create_chat(user_id, request_body.name).await?
    };

    Ok(chats.into_response())
}

/// Join the chat
pub async fn join_chat(
    user_id: UserId,
    chat_id: ChatId,
    auth_user_id: UserId,
    chat_service: Arc<Mutex<ChatService>>,
) -> Result<impl warp::Reply, Rejection> {
    let mut chat_service = chat_service.lock().await;
    let add_to_chat = AddToChat { chat_id };
    chat_service
        .join_chat(auth_user_id, user_id, add_to_chat)
        .await?;

    Ok(warp::reply::with_status("", StatusCode::NO_CONTENT))
}

/// Disjoin the chat
pub async fn disjoin_chat(
    user_id: UserId,
    chat_id: ChatId,
    auth_user_id: UserId,
    chat_service: Arc<Mutex<ChatService>>,
) -> Result<impl warp::Reply, Rejection> {
    let mut chat_service = chat_service.lock().await;
    let remove_from_chat = RemoveFromChat { chat_id };
    chat_service
        .disjoin_chat(auth_user_id, user_id, remove_from_chat)
        .await?;

    Ok(warp::reply::with_status("", StatusCode::NO_CONTENT))
}

/// This allows to use "?" in the handler
impl warp::reject::Reject for CredentialServiceError {}
impl warp::reject::Reject for ChatServiceError {}

#[derive(Serialize)]
struct ErrorMessage {
    code: u16,
    message: String,
}

/// Error handler. This could be written better for handling other errors,
/// like "405: Method not allowed", or malformed JSON etc....
pub async fn handle_rejection(err: Rejection) -> Result<impl Reply, Rejection> {
    let code;
    let message;

    if err.is_not_found() {
        code = StatusCode::NOT_FOUND;
        message = "NOT_FOUND".to_owned();
    } else if let Some(err) = err.find::<CredentialServiceError>() {
        code = StatusCode::BAD_REQUEST;
        message = format!("{:?}", err);
    } else if let Some(err) = err.find::<ChatServiceError>() {
        code = StatusCode::BAD_REQUEST;
        message = format!("{:?}", err);
    } else {
        // We should have expected this... Just log and say its a 500
        eprintln!("unhandled rejection: {:?}", err);
        code = StatusCode::INTERNAL_SERVER_ERROR;
        message = "UNHANDLED_REJECTION".to_owned();
    }

    let json = warp::reply::json(&ErrorMessage {
        code: code.as_u16(),
        message,
    });

    Ok(warp::reply::with_status(json, code))
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[cfg_attr(test, derive(Deserialize))]
#[derive(Serialize)]
pub struct LoginResponse {
    pub user_id: UserId,
    pub jwt: String,
}

/// Needed for returning the structures directly from the handlers
impl warp::Reply for LoginResponse {
    fn into_response(self) -> warp::reply::Response {
        warp::reply::json(&self).into_response()
    }
}

impl warp::Reply for Chats {
    fn into_response(self) -> warp::reply::Response {
        warp::reply::json(&self.0).into_response()
    }
}

impl warp::Reply for Chat {
    fn into_response(self) -> warp::reply::Response {
        warp::reply::json(&self).into_response()
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Deserialize)]
pub struct CreateChatRequest {
    pub name: String,
}
