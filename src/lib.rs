use std::sync::Arc;
use std::thread;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::lock::Mutex;
use futures::{channel::mpsc::unbounded, future, FutureExt};
use futures::{SinkExt, StreamExt};
use tokio::runtime::Runtime;
use warp::Filter;
use ws_pool::{WsContext, WsPool};

use crate::chat_service::{process_chat, ChatService};
use crate::web_service::get_router;
use crate::ws_pool::{process_ws_pool, Item, SendMessageInChat};

#[macro_use]
extern crate log;

mod chat_service;
mod credential_service;
mod web_service;
mod ws_pool;

fn from_redis(mut from_redis_sender: UnboundedSender<(Arc<WsContext>, SendMessageInChat)>) {
    info!("from_redis 1!!");
    info!("from_redis 2!!");

    let client = redis::Client::open("redis://127.0.0.1/").unwrap();
    let mut con = client.get_connection().unwrap();
    let mut pubsub = con.as_pubsub();
    pubsub.subscribe("chats").unwrap();

    let runtime = Runtime::new().unwrap();
    info!("starting subscribing");
    loop {
        let msg = pubsub.get_message().unwrap();
        let payload: String = msg.get_payload().unwrap();
        info!("channel '{}': {}", msg.get_channel_name(), payload);

        let (ws_context, smic): (WsContext, SendMessageInChat) =
            serde_json::from_str(&payload).unwrap();
        let ws_context = Arc::new(ws_context);

        runtime
            .block_on(from_redis_sender.send((ws_context, smic)))
            .unwrap();
    }
}

async fn to_redis(mut to_redis_receiver: UnboundedReceiver<(Arc<WsContext>, SendMessageInChat)>) {
    let client = redis::Client::open("redis://127.0.0.1/").unwrap();
    let mut con = client.get_connection().unwrap();

    info!("starting to_redis");
    loop {
        let m = to_redis_receiver.next();

        match m.await {
            None => {
                info!("to_redis: none");
            }
            Some((ws_context, smic)) => {
                info!("to_redis: {:?}, {:?}", ws_context, smic);

                match serde_json::to_string(&(ws_context, smic)) {
                    Err(e) => {
                        error!("to_redis: {}", e);
                    }
                    Ok(s) => {
                        info!("sending to_redis: {}", s);
                        match redis::cmd("PUBLISH")
                            .arg("chats")
                            .arg(s)
                            .query::<i32>(&mut con)
                        {
                            Err(r) => {
                                error!("error to_redis: {}", r);
                            }
                            Ok(r) => {
                                info!("sent to_redis: {}", r);
                            }
                        }
                    }
                }
            }
        }
    }
}

pub async fn start() {
    let (router, ws_pool, chat_service) = init();

    let (to_redis_sender, to_redis_receiver) = unbounded::<(Arc<WsContext>, SendMessageInChat)>();
    let (from_redis_sender, from_redis_receiver) =
        unbounded::<(Arc<WsContext>, SendMessageInChat)>();

    let ws_process = process_ws_pool(ws_pool, to_redis_sender);
    let chat_process = process_chat(chat_service.clone(), from_redis_receiver);

    thread::spawn(|| {
        info!("from_redis thread spawn");
        from_redis(from_redis_sender);
    });
    let to_redis_process = to_redis(to_redis_receiver);

    let server = warp::serve(router).run(([127, 0, 0, 1], 3030));

    let all_futures_to_wait = vec![
        ws_process.boxed(),
        chat_process.boxed(),
        server.boxed(),
        to_redis_process.boxed(),
    ];
    let _ = future::select_all(all_futures_to_wait).await;

    info!("Ended");
}

fn init() -> (
    impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone,
    WsPool,
    Arc<Mutex<ChatService>>,
) {
    let (ws_item_sender, ws_item_receiver) = unbounded::<Item>();

    let chat_service = ChatService::new(ws_item_sender.clone());
    let ws_pool = WsPool::new(ws_item_receiver);

    let chat_service = Arc::new(Mutex::new(chat_service));

    let router = get_router(chat_service.clone(), ws_item_sender);

    (router, ws_pool, chat_service)
}

#[cfg(test)]
mod tests {
    use std::thread;

    use warp::{
        http::StatusCode,
        test::{request, ws},
        ws::Message,
    };

    use crate::{
        chat_service::Chat,
        web_service::handlers::{CreateChatRequest, LoginRequest, LoginResponse},
        ws_pool::{PublishedMessage, SendMessageInChat},
    };

    use super::*;
    use helper::*;

    #[tokio::test]
    async fn test_post() {
        pretty_env_logger::try_init().ok();

        let (to_redis_sender, to_redis_receiver) =
            unbounded::<(Arc<WsContext>, SendMessageInChat)>();
        let (from_redis_sender, from_redis_receiver) =
            unbounded::<(Arc<WsContext>, SendMessageInChat)>();

        let (router, ws_pool, chat_service) = init();

        let ws_process = process_ws_pool(ws_pool, to_redis_sender);
        let chat_process = process_chat(chat_service.clone(), from_redis_receiver);
        let to_redis_process = to_redis(to_redis_receiver);

        tokio::spawn(ws_process);
        tokio::spawn(chat_process);
        tokio::spawn(to_redis_process);

        thread::spawn(|| {
            info!("from_redis thread spawn");
            from_redis(from_redis_sender);
        });

        let (user_id, jwt) = perform_login!(router, "pippo", "pippo");
        let chat_id = perform_create_chat!(router, jwt, "MyChatName");

        perform_add_to_chat!(router, jwt, user_id, chat_id);

        let mut ws_client = perform_create_ws_client!(router, jwt);
        perform_send_message!(ws_client, chat_id, "text");

        let msg = perform_recv_message!(ws_client);
        assert_msg!(msg, "pippo", chat_id, "text");

        let (user_id2, jwt2) = perform_login!(router, "pluto", "pluto");
        perform_add_to_chat!(router, jwt2, user_id2, chat_id);

        let mut ws_client2 = perform_create_ws_client!(router, jwt2);

        perform_send_message!(ws_client2, chat_id, "text2");

        // user1 should receive the message
        let msg = perform_recv_message!(ws_client);
        assert_msg!(msg, "pluto", chat_id, "text2");

        // user2 should receive the message
        let msg = perform_recv_message!(ws_client2);
        assert_msg!(msg, "pluto", chat_id, "text2");

        perform_remove_from_chat!(router, jwt2, user_id2, chat_id);
        perform_send_message!(
            ws_client2,
            chat_id,
            "this-message-should-never-be-received!"
        );

        perform_send_message!(ws_client, chat_id, "text3");
        let msg = perform_recv_message!(ws_client);
        assert_msg!(msg, "pippo", chat_id, "text3");

        perform_close_ws!(ws_client2);
        perform_close_ws!(ws_client);
    }

    mod helper {
        macro_rules! perform_login {
            ($router: ident, $username: literal , $password: literal ) => {{
                let resp = request()
                    .method("POST")
                    .path("/login")
                    .json(&LoginRequest {
                        username: $username.to_owned(),
                        password: $password.to_owned(),
                    })
                    .reply(&$router.clone())
                    .await;
                assert_eq!(resp.status(), StatusCode::OK);
                let resp: LoginResponse = serde_json::from_slice(&*resp.body()).unwrap();
                (resp.user_id, resp.jwt)
            }};
        }

        macro_rules! perform_create_chat {
            ($router: ident, $jwt: ident, $name: literal) => {{
                let resp = request()
                    .method("POST")
                    .path("/chat")
                    .header("Authorization", format!("Bearer {}", $jwt))
                    .json(&CreateChatRequest {
                        name: $name.to_owned(),
                    })
                    .reply(&$router.clone())
                    .await;
                assert_eq!(resp.status(), StatusCode::OK);
                let resp: Chat = serde_json::from_slice(&*resp.body()).unwrap();
                resp.id
            }};
        }

        macro_rules! perform_create_ws_client {
            ($router: ident, $jwt: ident) => {{
                info!("perform_create_ws_client {}", $jwt);
                ws().path(&format!("/ws?jwt={}", $jwt))
                    .handshake($router.clone())
                    .await
                    .unwrap()
            }};
        }

        macro_rules! perform_add_to_chat {
            ($router: ident, $jwt: ident, $user_id: ident, $chat_id: ident) => {{
                info!("perform_add_to_chat {:?}", $user_id);
                let resp = request()
                    .method("POST")
                    .path(&format!("/user/{}/joined-chat/{}", $user_id.0, $chat_id.0))
                    .header("Authorization", format!("Bearer {}", $jwt))
                    .body("")
                    .reply(&$router.clone())
                    .await;
                assert_eq!(resp.status(), StatusCode::NO_CONTENT);
            }};
        }

        macro_rules! perform_remove_from_chat {
            ($router: ident, $jwt: ident, $user_id: ident, $chat_id: ident) => {{
                info!("perform_remove_from_chat {:?}", $user_id);
                let resp = request()
                    .method("DELETE")
                    .path(&format!("/user/{}/joined-chat/{}", $user_id.0, $chat_id.0))
                    .header("Authorization", format!("Bearer {}", $jwt))
                    .body("")
                    .reply(&$router.clone())
                    .await;
                assert_eq!(resp.status(), StatusCode::NO_CONTENT);
            }};
        }

        macro_rules! perform_send_message {
            ($ws_client: ident, $chat_id: ident, $text: literal) => {
                info!("perform_send_message {:?}", $text);
                let event = SendMessageInChat {
                    chat_id: $chat_id.clone(),
                    text: $text.into(),
                };
                let text = serde_json::to_string(&event).unwrap();
                $ws_client.send_text(text).await;
            };
        }

        macro_rules! perform_recv_message {
            ($ws_client: ident) => {{
                let msg = $ws_client.recv().await;
                let msg = msg.unwrap();
                let s = msg.to_str().unwrap();
                let resp: PublishedMessage = serde_json::from_str(&s).unwrap();

                resp
            }};
        }

        macro_rules! assert_msg {
            ($msg: ident, $writer: literal, $chat_id: ident, $text: literal) => {
                assert_eq!($msg.writer.0, $writer);
                assert_eq!($msg.chat_id, $chat_id);
                assert_eq!($msg.text, $text);
            };
        }

        macro_rules! perform_close_ws {
            ($ws_client: ident) => {
                $ws_client.send(Message::close()).await;
                drop($ws_client);
            };
        }

        pub(crate) use assert_msg;
        pub(crate) use perform_add_to_chat;
        pub(crate) use perform_close_ws;
        pub(crate) use perform_create_chat;
        pub(crate) use perform_create_ws_client;
        pub(crate) use perform_login;
        pub(crate) use perform_recv_message;
        pub(crate) use perform_remove_from_chat;
        pub(crate) use perform_send_message;
    }
}
