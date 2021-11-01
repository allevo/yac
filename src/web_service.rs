use std::{convert::Infallible, sync::Arc};

use futures::{lock::Mutex, SinkExt, StreamExt};
use hyperid::HyperId;

use crate::{
    chat_service::ChatService,
    credential_service::{CredentialService, CredentialServiceError},
    ws_pool::{
        AddDevice, BusinessEvent, DeviceId, Item, MessageSender, PublishedMessage, SocketEvent,
        UserId, WsContext,
    },
};
use async_trait::async_trait;
use warp::{
    self,
    http::HeaderValue,
    reject,
    ws::{Message, WebSocket},
    Error, Filter, Rejection, Reply,
};

pub fn get_router(
    chat_service: Arc<Mutex<ChatService>>,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let chat_service = warp::any().map(move || chat_service.clone());
    let device_generator = Arc::new(Mutex::new(HyperId::new()));
    let device_generator = warp::any().map(move || device_generator.clone());
    let credential_service = Arc::new(Mutex::new(CredentialService::new()));

    let resolve_jwt = resolve_jwt(credential_service.clone());
    let credential_service_filter = warp::any().map(move || credential_service.clone());

    let chat = warp::path("ws")
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(chat_service.clone())
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
        .and(resolve_jwt)
        .and(chat_service)
        .and_then(handlers::create_chat);

    login
        .or(get_chats)
        .or(create_chat)
        .or(chat)
        .or(warp::fs::dir("static"))
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

impl warp::reject::Reject for CredentialServiceError {}

pub mod handlers {
    use std::{convert::Infallible, sync::Arc};

    use futures::lock::Mutex;
    use warp::Reply;

    use crate::{
        chat_service::{Chat, ChatService, Chats},
        credential_service::{CredentialService, CredentialServiceError},
        ws_pool::UserId,
    };

    pub async fn login(
        request_body: LoginRequest,
        credential_service: Arc<Mutex<CredentialService>>,
    ) -> Result<impl warp::Reply, Infallible> {
        let credential_service = credential_service.lock().await;
        let ret = credential_service
            .login(request_body.username, request_body.password)
            .map(|jwt| LoginResponse { jwt })
            .map(warp::Reply::into_response)
            .unwrap_or_else(warp::Reply::into_response);

        Ok(ret)
    }

    pub async fn get_chats(
        _: UserId,
        chat_service: Arc<Mutex<ChatService>>,
    ) -> Result<impl warp::Reply, Infallible> {
        let chats = {
            let chat_service = chat_service.lock().await;
            chat_service.list_chats().await
        };

        Ok(chats.into_response())
    }

    pub async fn create_chat(
        request_body: CreateChatRequest,
        user_id: UserId,
        chat_service: Arc<Mutex<ChatService>>,
    ) -> Result<impl warp::Reply, Infallible> {
        let chats = {
            let mut chat_service = chat_service.lock().await;
            chat_service.create_chat(user_id, request_body.name).await
        };

        Ok(chats.into_response())
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
        pub jwt: String,
    }

    impl warp::Reply for CredentialServiceError {
        fn into_response(self) -> warp::reply::Response {
            warp::http::Response::builder()
                .status(400)
                .body(format!("{}", self).into())
                .unwrap()
        }
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
    credential_service: Arc<Mutex<CredentialService>>,
    device_generator: Arc<Mutex<HyperId>>,
    params: WsQueryParameter,
) -> Result<impl warp::Reply, Infallible> {
    let user_id = match {
        let credential_service = credential_service.lock().await;
        credential_service.try_decode(&params.jwt)
    } {
        Err(e) => return Ok(e.into_response()),
        Ok(user_id) => user_id,
    };

    let device_id = {
        let mut device_generator = device_generator.lock().await;
        device_generator.generate().to_url_safe()
    };
    let device_id: DeviceId = device_id.into();

    Ok(ws
        .on_upgrade(move |socket| user_connected(socket, chat_service, user_id, device_id))
        .into_response())
}

async fn user_connected(
    ws: WebSocket,
    chat_service: Arc<Mutex<ChatService>>,
    user_id: UserId,
    device_id: DeviceId,
) {
    let (write, read) = ws.split();

    let ws_context = Arc::new(WsContext::new(user_id, device_id));

    let socket_context = ws_context.clone();
    let read = read.filter_map(move |message| {
        let socket_context = socket_context.clone();
        handle_event(socket_context, message)
    });

    let mut chat_service = chat_service.lock().await;
    chat_service
        .add_device(
            ws_context.clone(),
            AddDevice {
                sender: Box::new(write),
                receiver: Box::pin(read),
            },
        )
        .await;
}

async fn handle_event(
    socket_context: Arc<WsContext>,
    message: Result<Message, Error>,
) -> Option<Item> {
    match message {
        Ok(message) => {
            match (message.is_text(), message.is_close()) {
                // Text
                (true, _) => {
                    let str = message.to_str().expect("Expect text message");
                    let business_event: BusinessEvent = match serde_json::from_str(str) {
                        Ok(be) => be,
                        // Ignore all invalid json
                        Err(_) => return None,
                    };

                    Some(Item::BusinessEvent(socket_context, business_event))
                }
                // Close
                (_, true) => Some(Item::SocketEvent(SocketEvent::RemoveDevice(socket_context))),
                // Ignore others event...
                _ => None,
            }
        }
        Err(err) => {
            warn!(
                "Error on handling event from client: {:?} {}",
                socket_context, err
            );
            Some(Item::SocketEvent(SocketEvent::RemoveDevice(socket_context)))
        }
    }
}
