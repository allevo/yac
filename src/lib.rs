use std::sync::Arc;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::lock::Mutex;
use futures::{channel::mpsc::unbounded, future, FutureExt};
use futures::{SinkExt, StreamExt};
use warp::Filter;
use ws_pool::{BusinessEvent, WsContext, WsPool};

use crate::chat_service::ChatService;
use crate::web_service::get_router;
use crate::ws_pool::Item;

#[macro_use]
extern crate log;

mod chat_service;
mod credential_service;
mod web_service;
mod ws_pool;

async fn process_chat(
    chat_service: Arc<Mutex<ChatService>>,
    mut internal_receiver: UnboundedReceiver<(Arc<WsContext>, BusinessEvent)>,
) {
    loop {
        let business_event = internal_receiver.next().await;

        info!("process business event {:?}", business_event);

        let res = match business_event {
            None => {
                warn!("NONE!");
                None
            }
            Some((context, BusinessEvent::AddToChat(atc))) => {
                let mut chat_service = chat_service.lock().await;
                Some(chat_service.join_chat(context, atc))
            }
            Some((context, BusinessEvent::RemoveDevice)) => {
                let mut chat_service = chat_service.lock().await;
                Some(chat_service.remove_device(context))
            }
            Some((context, BusinessEvent::RemoveFromChat(rfc))) => {
                let mut chat_service = chat_service.lock().await;
                Some(chat_service.disjoin_chat(context, rfc))
            }
            Some((context, BusinessEvent::SendMessageInChat(smic))) => {
                let mut chat_service = chat_service.lock().await;
                Some(chat_service.send_message(context, smic).await)
            }
            Some((_, BusinessEvent::Stop)) => break,
        };

        if let Some(Err(e)) = res {
            // Maybe here we need to send a error message to client...
            warn!("process business event error {:?}", e);
        }
    }

    info!("end process_chat");
}

pub async fn start() {
    let (router, mut ws_pool, chat_service, mut item_sender, from_pool_to_chat_receiver) = init();

    let ws_process = ws_pool.process();
    let chat_process = process_chat(chat_service.clone(), from_pool_to_chat_receiver);

    let server = warp::serve(router).run(([127, 0, 0, 1], 3030));

    let all_futures_to_wait = vec![ws_process.boxed(), chat_process.boxed(), server.boxed()];
    let _ = future::select_all(all_futures_to_wait).await;

    info!("Sending stop signal");

    // create a fake context...
    let ws_context = Arc::new(WsContext {
        user_id: "".into(),
        device_id: "".into(),
    });
    item_sender
        .send(Item::BusinessEvent(ws_context, BusinessEvent::Stop))
        .await
        .expect("Unable to send STOP signal");

    info!("Ended");
}

fn init() -> (
    impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone,
    WsPool,
    Arc<Mutex<ChatService>>,
    UnboundedSender<Item>,
    UnboundedReceiver<(Arc<WsContext>, BusinessEvent)>,
) {
    let (item_sender, item_receiver) = unbounded();
    let (from_pool_to_chat_sender, from_pool_to_chat_receiver) = unbounded();

    let ws_pool = WsPool::new(item_receiver, from_pool_to_chat_sender);

    let chat_service = ChatService::new(item_sender.clone());
    let chat_service = Arc::new(Mutex::new(chat_service));

    let router = get_router(chat_service.clone());

    (
        router,
        ws_pool,
        chat_service,
        item_sender,
        from_pool_to_chat_receiver,
    )
}

#[cfg(test)]
mod tests {
    use warp::{
        http::StatusCode,
        test::{request, ws},
        ws::Message,
    };

    use crate::{
        chat_service::Chat,
        web_service::handlers::{CreateChatRequest, LoginRequest, LoginResponse},
        ws_pool::{AddToChat, PublishedMessage, RemoveFromChat, SendMessageInChat},
    };

    use super::*;
    use helper::*;

    #[tokio::test]
    async fn test_post() {
        pretty_env_logger::try_init().ok();

        let (router, ws_pool, chat_service, mut item_sender, from_pool_to_chat_receiver) = init();

        let ws_pool = Box::leak(Box::new(ws_pool));
        tokio::spawn(ws_pool.process());

        let chat_process = process_chat(chat_service.clone(), from_pool_to_chat_receiver);
        tokio::spawn(chat_process);

        let jwt = perform_login!(router, "pippo", "pippo");
        let chat_id = perform_create_chat!(router, jwt, "MyChatName");

        let mut ws_client = perform_create_ws_client!(router, jwt);
        perform_add_to_chat!(ws_client, chat_id);
        perform_send_message!(ws_client, chat_id, "text");

        let msg = perform_recv_message!(ws_client);
        assert_msg!(msg, "pippo", chat_id, "text");

        let jwt2 = perform_login!(router, "pluto", "pluto");
        let mut ws_client2 = perform_create_ws_client!(router, jwt2);
        perform_add_to_chat!(ws_client2, chat_id);
        perform_send_message!(ws_client2, chat_id, "text2");

        // user1 should receive the message
        let msg = perform_recv_message!(ws_client);
        assert_msg!(msg, "pluto", chat_id, "text2");

        // user2 should receive the message
        let msg = perform_recv_message!(ws_client2);
        assert_msg!(msg, "pluto", chat_id, "text2");

        perform_remove_from_chat!(ws_client2, chat_id);
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

        // Close server
        let ws_context = Arc::new(WsContext {
            user_id: "".into(),
            device_id: "".into(),
        });
        item_sender
            .send(Item::BusinessEvent(ws_context, BusinessEvent::Stop))
            .await
            .expect("Unable to send STOP signal");
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
                resp.jwt
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
                ws().path(&format!("/ws?jwt={}", $jwt))
                    .handshake($router.clone())
                    .await
                    .unwrap()
            }};
        }

        macro_rules! perform_add_to_chat {
            ($ws_client: ident, $chat_id: ident) => {
                let event = BusinessEvent::AddToChat(AddToChat {
                    chat_id: $chat_id.clone(),
                });
                let text = serde_json::to_string(&event).unwrap();
                $ws_client.send_text(text).await;
            };
        }

        macro_rules! perform_send_message {
            ($ws_client: ident, $chat_id: ident, $text: literal) => {
                let event = BusinessEvent::SendMessageInChat(SendMessageInChat {
                    chat_id: $chat_id.clone(),
                    text: $text.into(),
                });
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

        macro_rules! perform_remove_from_chat {
            ($ws_client: ident, $chat_id: ident) => {
                let event = BusinessEvent::RemoveFromChat(RemoveFromChat {
                    chat_id: $chat_id.clone(),
                });
                let text = serde_json::to_string(&event).unwrap();
                $ws_client.send_text(text).await;
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
