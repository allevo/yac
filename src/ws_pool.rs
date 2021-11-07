use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    pin::Pin,
    str::FromStr,
    sync::Arc,
};

use async_trait::async_trait;
use futures::{
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
    lock::Mutex,
    stream::{select_all, SelectAll},
    SinkExt, Stream, StreamExt,
};

pub type SenderStream = Box<dyn MessageSender + Send + 'static>;
pub type ReceiverStream = Pin<Box<dyn Stream<Item = Item> + Send + 'static>>;

use serde::{Deserialize, Serialize};
use warp::{ws::Message, Error};

use crate::chat_service::ChatService;

#[async_trait]
pub trait MessageSender {
    async fn send_message(&mut self, msg: PublishedMessage);
}

#[derive(Debug)]
pub enum Item {
    AddDevice(Arc<WsContext>, AddDevice),
    RemoveDevice(DeviceId),
    PublishMessage(HashSet<DeviceId>, PublishedMessage),
    SendMessageInChat(Arc<WsContext>, SendMessageInChat),
}

pub struct AddDevice {
    pub sender: SenderStream,
    pub receiver: ReceiverStream,
}

impl Debug for AddDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddDevice").finish()
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug, Deserialize)]
pub struct AddToChat {
    pub chat_id: ChatId,
}
#[cfg_attr(test, derive(Serialize))]
#[derive(Debug, Deserialize)]
pub struct RemoveFromChat {
    pub chat_id: ChatId,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct SendMessageInChat {
    pub chat_id: ChatId,
    pub text: String,
}

#[cfg_attr(test, derive(Deserialize))]
#[derive(Debug, Clone, Serialize)]
pub struct PublishedMessage {
    pub writer: UserId,
    pub chat_id: ChatId,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WsContext {
    pub user_id: UserId,
    pub device_id: DeviceId,
}
impl WsContext {
    pub fn new(user_id: UserId, device_id: DeviceId) -> Self {
        Self { user_id, device_id }
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug, Deserialize, Serialize)]
pub struct UserId(pub String);
impl<S: Into<String>> From<S> for UserId {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}
impl FromStr for UserId {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug, Deserialize, Serialize)]
pub struct DeviceId(String);
impl<S: Into<String>> From<S> for DeviceId {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}
#[derive(Hash, PartialEq, Eq, Clone, Debug, Deserialize, Serialize)]
pub struct ChatId(pub String);
impl<S: Into<String>> From<S> for ChatId {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}

impl FromStr for ChatId {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

pub struct WsPool {
    incoming_streams: SelectAll<ReceiverStream>,
    devices: HashMap<DeviceId, SenderStream>,
}

impl WsPool {
    pub fn new(item_receiver: UnboundedReceiver<Item>) -> Self {
        let recv: Vec<ReceiverStream> = vec![Box::pin(item_receiver)];

        let select_all = select_all(recv);

        Self {
            incoming_streams: select_all,
            devices: Default::default(),
        }
    }

    // TODO: move into a trait?
    pub async fn send_message_to(&mut self, device_ids: HashSet<DeviceId>, msg: PublishedMessage) {
        // TODO: use send_all for better performance
        for device_id in device_ids {
            let device = self
                .devices
                .get_mut(&device_id)
                .expect("device_id not found");
            device.send_message(msg.clone()).await;
        }
    }

    pub async fn add_device(&mut self, _context: Arc<WsContext>, _add_device: AddDevice) {}

    pub async fn remove_device(&mut self, _device_id: &DeviceId) {}
}

pub async fn process_ws_pool(
    mut ws_pool: WsPool,
    mut redis_sender: UnboundedSender<(Arc<WsContext>, SendMessageInChat)>,
) {
    loop {
        info!("Pull next");

        let m = ws_pool.incoming_streams.next();

        match m.await {
            None => warn!("Pulled None from stream"),
            Some(Item::SendMessageInChat(context, event)) => {
                info!("SendMessageInChat {:?} {:?}", context, event);

                redis_sender
                    .send((context, event))
                    .await
                    .expect("Unable to send business event");
                info!("SendMessageInChat sent");
            }
            Some(Item::AddDevice(context, add_device)) => {
                info!("add device event {:?}", context);

                ws_pool.incoming_streams.push(add_device.receiver);
                ws_pool
                    .devices
                    .insert(context.device_id.clone(), add_device.sender);
            }
            Some(Item::RemoveDevice(device_id)) => {
                info!("remove device {:?}", device_id);

                ws_pool.devices.remove(&device_id);
            }
            Some(Item::PublishMessage(device_ids, msg)) => {
                info!("publish message {:?}, {:?}", device_ids, msg);

                ws_pool.send_message_to(device_ids, msg).await;
            }
        }
    }

    // info!("end ws_pool.process");
}

pub async fn handle_event(
    socket_context: Arc<WsContext>,
    chat_service: Arc<Mutex<ChatService>>,
    message: Result<Message, Error>,
) -> Option<Item> {
    match message {
        Ok(message) => {
            match (message.is_text(), message.is_close()) {
                // Text
                (true, _) => {
                    let str = message.to_str().expect("Expect text message");
                    let business_event: SendMessageInChat = match serde_json::from_str(str) {
                        Ok(be) => be,
                        // Ignore all invalid json
                        Err(err) => {
                            warn!(
                                "Error on handling event from client: {:?} {}",
                                socket_context, err
                            );
                            return None;
                        }
                    };

                    Some(Item::SendMessageInChat(socket_context, business_event))
                }
                // Close
                (_, true) => remove_device(chat_service, socket_context).await,
                // Ignore others event...
                _ => None,
            }
        }
        Err(err) => {
            warn!(
                "Error on handling event from client: {:?} {}",
                socket_context, err
            );
            remove_device(chat_service, socket_context).await
        }
    }
}

async fn remove_device(
    chat_service: Arc<Mutex<ChatService>>,
    socket_context: Arc<WsContext>,
) -> Option<Item> {
    let mut chat_service = chat_service.lock().await;
    match chat_service.remove_device(socket_context.clone()).await {
        Ok(_) => {}
        Err(e) => {
            warn!("Error on closing client: {:?}: {:?}", socket_context, e);
        }
    }

    Some(Item::RemoveDevice(socket_context.device_id.clone()))
}
