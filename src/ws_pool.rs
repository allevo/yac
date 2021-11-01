use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    sync::Arc,
};

use async_trait::async_trait;
use futures::{
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
    stream::{select_all, SelectAll},
    SinkExt, Stream, StreamExt,
};

pub type SenderStream = Box<dyn MessageSender + Send + 'static>;
pub type ReceiverStream = Pin<Box<dyn Stream<Item = Item> + Send + 'static>>;

use serde::{Deserialize, Serialize};

#[async_trait]
pub trait MessageSender {
    async fn send_message(&mut self, msg: PublishedMessage);
}

pub enum Item {
    SocketEvent(SocketEvent),
    BusinessEvent(Arc<WsContext>, BusinessEvent),
}

pub enum SocketEvent {
    AddDevice(Arc<WsContext>, AddDevice),
    RemoveDevice(Arc<WsContext>),
    PublishMessage(HashSet<DeviceId>, PublishedMessage),
}

pub struct AddDevice {
    pub sender: SenderStream,
    pub receiver: ReceiverStream,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum BusinessEvent {
    AddToChat(AddToChat),
    RemoveFromChat(RemoveFromChat),
    RemoveDevice,
    SendMessageInChat(SendMessageInChat),
    Stop,
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
#[cfg_attr(test, derive(Serialize))]
#[derive(Debug, Deserialize)]
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

#[derive(Debug)]
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

#[derive(Hash, PartialEq, Eq, Clone, Debug, Deserialize, Serialize)]
pub struct DeviceId(String);
impl<S: Into<String>> From<S> for DeviceId {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}
#[derive(Hash, PartialEq, Eq, Clone, Debug, Deserialize, Serialize)]
pub struct ChatId(String);
impl<S: Into<String>> From<S> for ChatId {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}

pub struct WsPool {
    incoming_streams: SelectAll<ReceiverStream>,
    devices: HashMap<DeviceId, SenderStream>,
    business_sender: UnboundedSender<(Arc<WsContext>, BusinessEvent)>,
}

impl WsPool {
    pub fn new(
        item_receiver: UnboundedReceiver<Item>,
        business_sender: UnboundedSender<(Arc<WsContext>, BusinessEvent)>,
    ) -> Self {
        let recv: Vec<ReceiverStream> = vec![Box::pin(item_receiver)];

        let select_all = select_all(recv);

        Self {
            incoming_streams: select_all,
            devices: Default::default(),
            business_sender,
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

    pub async fn process(&mut self) {
        loop {
            info!("Pull next");
            let m = self.incoming_streams.next();

            match m.await {
                None => warn!("Pulled None from stream"),
                Some(Item::BusinessEvent(context, event)) => {
                    info!("Business event {:?} {:?}", context, event);

                    let stop = matches!(event, BusinessEvent::Stop);

                    // Business event is handled by ChatService!
                    self.business_sender
                        .send((context, event))
                        .await
                        .expect("Unable to send business event");

                    if stop {
                        break;
                    }
                }
                Some(Item::SocketEvent(event)) => match event {
                    SocketEvent::AddDevice(context, add_device) => {
                        info!("add device event {:?}", context);

                        self.incoming_streams.push(add_device.receiver);
                        self.devices
                            .insert(context.device_id.clone(), add_device.sender);
                    }
                    SocketEvent::RemoveDevice(context) => {
                        info!("remove device {:?}", context);

                        self.devices.remove(&context.device_id);
                        self.business_sender
                            .send((context.clone(), BusinessEvent::RemoveDevice))
                            .await
                            .expect("Unable to send remove device event");
                    }
                    SocketEvent::PublishMessage(device_ids, msg) => {
                        info!("publish message {:?}, {:?}", device_ids, msg);

                        self.send_message_to(device_ids, msg).await;
                    }
                },
            }
        }

        info!("end ws_pool.process");
    }
}
