use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use futures::{channel::mpsc::UnboundedSender, SinkExt};
use hyperid::HyperId;

use crate::ws_pool::{
    AddDevice, AddToChat, ChatId, DeviceId, Item, PublishedMessage, RemoveFromChat,
    SendMessageInChat, SocketEvent, UserId, WsContext,
};

#[derive(Debug)]
pub enum ChatServiceError {
    ChatNotFound,
    YouAreNotAPartecipant,
}

pub struct ChatService {
    user_devices: HashMap<UserId, HashSet<DeviceId>>,
    item_sender: UnboundedSender<Item>,
    chat_id_generator: HyperId,
    chats: HashMap<ChatId, Chat>,
}

impl ChatService {
    pub fn new(item_sender: UnboundedSender<Item>) -> Self {
        Self {
            user_devices: Default::default(),
            chats: Default::default(),
            chat_id_generator: HyperId::new(),
            item_sender,
        }
    }

    pub async fn list_chats(&self) -> Chats {
        Chats(self.chats.values().cloned().collect())
    }

    pub async fn create_chat(&mut self, creator: UserId, name: String) -> Chat {
        let chat = Chat {
            id: self.chat_id_generator.generate().to_url_safe().into(),
            creator,
            name,
            user_ids: HashSet::new(),
        };
        self.chats.insert(chat.id.clone(), chat.clone());

        chat
    }

    pub async fn add_device(&mut self, context: Arc<WsContext>, add_device: AddDevice) {
        info!("add device {:?}", context);

        self.user_devices
            .entry(context.user_id.clone())
            .or_insert_with(Default::default)
            .insert(context.device_id.clone());

        let event = Item::SocketEvent(SocketEvent::AddDevice(context.clone(), add_device));
        self.item_sender
            .send(event)
            .await
            .expect("Unable to add a new device");
    }

    pub fn join_chat(
        &mut self,
        context: Arc<WsContext>,
        add_to_chat: AddToChat,
    ) -> Result<(), ChatServiceError> {
        info!("join chat {:?}, {:?}", context, add_to_chat);

        let chat = match self.chats.get_mut(&add_to_chat.chat_id) {
            Some(chat) => chat,
            None => return Err(ChatServiceError::ChatNotFound),
        };
        chat.user_ids.insert(context.user_id.clone());

        Ok(())
    }

    pub fn disjoin_chat(
        &mut self,
        context: Arc<WsContext>,
        remove_from_chat: RemoveFromChat,
    ) -> Result<(), ChatServiceError> {
        info!("disjoin chat {:?}, {:?}", context, remove_from_chat);

        let chat = match self.chats.get_mut(&remove_from_chat.chat_id) {
            Some(chat) => chat,
            None => return Err(ChatServiceError::ChatNotFound),
        };
        chat.user_ids.remove(&context.user_id);

        Ok(())
    }

    pub fn remove_device(&mut self, context: Arc<WsContext>) -> Result<(), ChatServiceError> {
        info!("remove device {:?}", context);

        self.user_devices
            .entry(context.user_id.clone())
            .or_insert_with(Default::default)
            .remove(&context.device_id);

        Ok(())
    }

    pub async fn send_message(
        &mut self,
        context: Arc<WsContext>,
        smic: SendMessageInChat,
    ) -> Result<(), ChatServiceError> {
        let chat = {
            match self.chats.get(&smic.chat_id) {
                None => return Err(ChatServiceError::ChatNotFound),
                Some(chat) => chat,
            }
        };

        let users = &chat.user_ids;

        if !users.contains(&context.user_id) {
            return Err(ChatServiceError::YouAreNotAPartecipant);
        }

        let devices: HashSet<DeviceId> = users
            .iter()
            .filter_map(|user_id| self.user_devices.get(user_id))
            .flatten()
            .cloned()
            .collect();

        let msg = PublishedMessage {
            writer: context.user_id.clone(),
            chat_id: smic.chat_id,
            text: smic.text,
        };

        let event = Item::SocketEvent(SocketEvent::PublishMessage(devices, msg));

        self.item_sender
            .send(event)
            .await
            .expect("Unable to add a new device");

        Ok(())
    }
}

use serde::Serialize;

#[cfg(test)]
use serde::Deserialize;

#[cfg_attr(test, derive(Deserialize))]
#[derive(Serialize, Clone, Debug)]
pub struct Chat {
    pub id: ChatId,
    creator: UserId,
    name: String,
    user_ids: HashSet<UserId>,
}

#[derive(Serialize)]
pub struct Chats(pub Vec<Chat>);
