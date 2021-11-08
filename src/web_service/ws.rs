use std::sync::Arc;

use futures::{channel::mpsc::UnboundedSender, lock::Mutex, SinkExt, StreamExt};
use hyperid::HyperId;

use crate::{
    chat_service::ChatService,
    credential_service::CredentialService,
    models::{
        AddDevice, DeviceId, Item, MessageSender, PublishedMessage, ReceiverStream, UserId,
        WsContext,
    },
    ws_pool::handle_event,
};
use async_trait::async_trait;
use warp::{
    self,
    ws::{Message, WebSocket},
    Rejection, Reply,
};

use serde::Deserialize;

#[derive(Deserialize)]
pub struct WsQueryParameter {
    jwt: String,
}

pub async fn ws(
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
            receiver: ReceiverStream::new(ws_context.device_id.clone(), Box::pin(read)),
        },
    );

    info!("item_sender sending...");
    match item_sender.send(event).await {
        Err(e) => error!("error on sending event {}", e),
        Ok(_) => info!("item_sender sent"),
    };
}
