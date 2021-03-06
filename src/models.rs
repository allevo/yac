use async_trait::async_trait;
use futures::Stream;
use std::{collections::HashSet, fmt::Debug, pin::Pin, str::FromStr, sync::Arc};

use serde::{Deserialize, Serialize};

#[async_trait]
pub trait MessageSender {
    async fn send_message(&mut self, msg: Arc<PublishedMessage>) -> Result<(), ()>;
}

pub type SenderStream = Box<dyn MessageSender + Send + 'static>;

pub struct ReceiverStream(
    pub DeviceId,
    pub Pin<Box<dyn Stream<Item = Item> + Send + 'static>>,
);

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
