use std::sync::Arc;

use futures::{channel::mpsc::UnboundedSender, lock::Mutex, SinkExt, StreamExt};
use hyperid::HyperId;

use crate::{
    chat_service::ChatService,
    credential_service::{CredentialService, CredentialServiceError},
    ws_pool::{
        handle_event, AddDevice, ChatId, DeviceId, Item, MessageSender, PublishedMessage, UserId,
        WsContext,
    },
};
use async_trait::async_trait;
use warp::{
    self,
    http::HeaderValue,
    reject,
    ws::{Message, WebSocket},
    Filter, Rejection, Reply,
};

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
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(chat_service.clone())
        .and(item_sender)
        .and(credential_service_filter.clone())
        .and(device_generator)
        .and(warp::query::<WsQueryParameter>())
        .and_then(ws);

    let login = warp::path("login")
        .and(warp::post())
        .and(warp::body::json())
        .and(credential_service_filter)
        .and_then(handlers::login);

    let get_chats = warp::path("chat")
        .and(warp::get())
        .and(resolve_jwt.clone())
        .and(chat_service.clone())
        .and_then(handlers::get_chats);

    let create_chat = warp::path("chat")
        .and(warp::post())
        .and(warp::body::json())
        .and(resolve_jwt.clone())
        .and(chat_service.clone())
        .and_then(handlers::create_chat);

    let join_chat = warp::path!("user" / UserId / "joined-chat" / ChatId)
        .and(warp::post())
        .and(resolve_jwt.clone())
        .and(chat_service.clone())
        .and_then(handlers::join_chat);

    let disjoin_chat = warp::path!("user" / UserId / "joined-chat" / ChatId)
        .and(warp::delete())
        .and(resolve_jwt)
        .and(chat_service)
        .and_then(handlers::disjoin_chat);

    login
        .or(get_chats)
        .or(create_chat)
        .or(join_chat)
        .or(disjoin_chat)
        .or(chat)
        .or(warp::fs::dir("static"))
        .recover(handlers::handle_rejection)
}

fn resolve_jwt(
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

pub mod handlers {
    use std::sync::Arc;

    use futures::lock::Mutex;
    use warp::{http::StatusCode, Rejection, Reply};

    use crate::{
        chat_service::{Chat, ChatService, ChatServiceError, Chats},
        credential_service::{CredentialService, CredentialServiceError},
        ws_pool::{AddToChat, ChatId, RemoveFromChat, UserId},
    };

    #[derive(Serialize)]
    struct ErrorMessage {
        code: u16,
        message: String,
    }
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

    pub async fn login(
        request_body: LoginRequest,
        credential_service: Arc<Mutex<CredentialService>>,
    ) -> Result<impl warp::Reply, Rejection> {
        let credential_service = credential_service.lock().await;
        let (user_id, jwt) =
            credential_service.login(request_body.username, request_body.password)?;

        Ok(LoginResponse { user_id, jwt })
    }

    impl warp::reject::Reject for CredentialServiceError {}

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

    pub async fn join_chat(
        user_id: UserId,
        chat_id: ChatId,
        _auth_user_id: UserId,
        chat_service: Arc<Mutex<ChatService>>,
    ) -> Result<impl warp::Reply, Rejection> {
        let mut chat_service = chat_service.lock().await;
        let add_to_chat = AddToChat { chat_id };
        chat_service.join_chat(user_id, add_to_chat).await?;

        Ok(warp::reply::with_status("", StatusCode::NO_CONTENT))
    }

    impl warp::reject::Reject for ChatServiceError {}

    pub async fn disjoin_chat(
        user_id: UserId,
        chat_id: ChatId,
        _auth_user_id: UserId,
        chat_service: Arc<Mutex<ChatService>>,
    ) -> Result<impl warp::Reply, Rejection> {
        let mut chat_service = chat_service.lock().await;
        let remove_from_chat = RemoveFromChat { chat_id };
        chat_service.disjoin_chat(user_id, remove_from_chat).await?;

        Ok(warp::reply::with_status("", StatusCode::NO_CONTENT))
    }

    use serde::{Deserialize, Serialize};

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
}

#[async_trait]
impl MessageSender for futures::stream::SplitSink<WebSocket, Message> {
    async fn send_message(&mut self, msg: PublishedMessage) {
        let text = serde_json::to_string(&msg).expect("");
        match self.send(Message::text(text)).await {
            Ok(()) => info!("Message sent!"),
            Err(e) => error!("Message not delivered: {}", e),
        };
    }
}

use serde::Deserialize;

#[derive(Deserialize)]
struct WsQueryParameter {
    jwt: String,
}

async fn ws(
    ws: warp::ws::Ws,
    chat_service: Arc<Mutex<ChatService>>,
    item_sender: UnboundedSender<Item>,
    credential_service: Arc<Mutex<CredentialService>>,
    device_generator: Arc<Mutex<HyperId>>,
    params: WsQueryParameter,
) -> Result<impl warp::Reply, Rejection> {
    info!("ws");
    let user_id = {
        let credential_service = credential_service.lock().await;
        credential_service.try_decode(&params.jwt)?
    };
    info!("ws {:?}", user_id);

    let device_id = {
        let mut device_generator = device_generator.lock().await;
        device_generator.generate().to_url_safe()
    };
    let device_id: DeviceId = device_id.into();
    info!("ws {:?} {:?}", user_id, device_id);

    Ok(ws
        .on_upgrade(move |socket| {
            user_connected(socket, chat_service, item_sender, user_id, device_id)
        })
        .into_response())
}

async fn user_connected(
    ws: WebSocket,
    chat_service: Arc<Mutex<ChatService>>,
    mut item_sender: UnboundedSender<Item>,
    user_id: UserId,
    device_id: DeviceId,
) {
    let (write, read) = ws.split();

    let ws_context = Arc::new(WsContext::new(user_id, device_id.clone()));

    let socket_context = ws_context.clone();
    let socket_chat_service = chat_service.clone();
    let read = read.filter_map(move |message| {
        let socket_context = socket_context.clone();
        let socket_chat_service = socket_chat_service.clone();

        handle_event(socket_context, socket_chat_service, message)
    });

    info!("user_connected {:?}", ws_context);
    let mut chat_service = chat_service.lock().await;
    chat_service.add_device(ws_context.clone()).await;

    let event = Item::AddDevice(
        ws_context.clone(),
        AddDevice {
            sender: Box::new(write),
            receiver: Box::pin(read),
        },
    );

    info!("item_sender sending...");
    match item_sender.send(event).await {
        Err(e) => error!("error on sending event {}", e),
        Ok(_) => info!("item_sender sent"),
    };
}
