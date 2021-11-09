use std::sync::Arc;

use futures::{channel::mpsc::UnboundedSender, lock::Mutex};
use hyperid::HyperId;

use crate::{
    chat_service::ChatService,
    credential_service::CredentialService,
    handlers::resolve_jwt::resolve_jwt,
    models::{ChatId, Item, UserId},
};

use warp::{self, Filter};

pub(crate) mod http_handlers;
mod resolve_jwt;
mod ws;

pub fn get_router(
    chat_service: Arc<Mutex<ChatService>>,
    item_sender: UnboundedSender<Item>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let chat_service = warp::any().map(move || chat_service.clone());
    let item_sender = warp::any().map(move || item_sender.clone());
    let device_generator = Arc::new(Mutex::new(HyperId::new()));
    let device_generator = warp::any().map(move || device_generator.clone());
    let credential_service = Arc::new(Mutex::new(CredentialService::new()));

    let resolve_jwt = resolve_jwt(credential_service.clone());
    let credential_service_filter = warp::any().map(move || credential_service.clone());

    let chat = warp::path("ws")
        .and(warp::ws())
        .and(chat_service.clone())
        .and(item_sender)
        .and(credential_service_filter.clone())
        .and(device_generator)
        .and(warp::query::<ws::WsQueryParameter>())
        .and_then(ws::ws);

    let login = warp::path("login")
        .and(warp::post())
        .and(warp::body::json())
        .and(credential_service_filter)
        .and_then(http_handlers::login);

    let get_chats = warp::path("chat")
        .and(warp::get())
        .and(resolve_jwt.clone())
        .and(chat_service.clone())
        .and_then(http_handlers::get_chats);

    let create_chat = warp::path("chat")
        .and(warp::post())
        .and(warp::body::json())
        .and(resolve_jwt.clone())
        .and(chat_service.clone())
        .and_then(http_handlers::create_chat);

    let join_chat = warp::path!("user" / UserId / "joined-chat" / ChatId)
        .and(warp::post())
        .and(resolve_jwt.clone())
        .and(chat_service.clone())
        .and_then(http_handlers::join_chat);

    let disjoin_chat = warp::path!("user" / UserId / "joined-chat" / ChatId)
        .and(warp::delete())
        .and(resolve_jwt)
        .and(chat_service)
        .and_then(http_handlers::disjoin_chat);

    // Combine whole apis togheter specifing the orders
    login
        .or(get_chats)
        .or(create_chat)
        .or(join_chat)
        .or(disjoin_chat)
        .or(chat)
        .or(warp::fs::dir("static"))
        .recover(http_handlers::handle_rejection)
}
