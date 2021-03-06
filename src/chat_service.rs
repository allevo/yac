use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use futures::{
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
    lock::Mutex,
    SinkExt, StreamExt,
};
use hyperid::HyperId;

use crate::models::*;

#[derive(Debug)]
pub enum ChatServiceError {
    ChatNotFound,
    YouAreNotAPartecipant,
    PermissionError,
}

pub struct ChatService {
    // Update the links between devices and users:
    // - one user can have multiple devices
    // - one device can belong to a unique user
    user_devices: HashMap<UserId, HashSet<DeviceId>>,
    // Track id -> Chat
    chats: HashMap<ChatId, Chat>,
    item_sender: UnboundedSender<Item>,
    chat_id_generator: HyperId,
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

    pub async fn list_chats(&self) -> Result<Chats, ChatServiceError> {
        Ok(Chats(self.chats.values().cloned().collect()))
    }

    pub async fn create_chat(
        &mut self,
        creator: UserId,
        name: String,
    ) -> Result<Chat, ChatServiceError> {
        let chat_id = self.chat_id_generator.generate().to_url_safe();
        let chat = Chat {
            id: chat_id.into(),
            creator,
            name,
            user_ids: HashSet::new(),
        };
        self.chats.insert(chat.id.clone(), chat.clone());

        Ok(chat)
    }

    pub async fn join_chat(
        &mut self,
        auth_user_id: UserId,
        user_id: UserId,
        add_to_chat: AddToChat,
    ) -> Result<(), ChatServiceError> {
        info!("user {:?} join chat {:?}", user_id, add_to_chat);

        // This logic could be more complicate:ù
        // can I add another user to a chat?
        // When it could be possibile?
        // For example I can invite another user to chat
        if auth_user_id != user_id {
            return Err(ChatServiceError::PermissionError);
        }

        // Find the chat with the right id
        let chat = match self.chats.get_mut(&add_to_chat.chat_id) {
            Some(chat) => chat,
            None => return Err(ChatServiceError::ChatNotFound),
        };
        chat.user_ids.insert(user_id);

        Ok(())
    }

    pub async fn disjoin_chat(
        &mut self,
        auth_user_id: UserId,
        user_id: UserId,
        remove_from_chat: RemoveFromChat,
    ) -> Result<(), ChatServiceError> {
        info!("disjoin chat {:?}, {:?}", user_id, remove_from_chat);

        // This logic could be more complicate:ù
        // can I remove another user to a chat?
        // When it could be possibile?
        // For example I can remove a user for moderation stuff
        if auth_user_id != user_id {
            return Err(ChatServiceError::PermissionError);
        }

        // Find the chat with the right id
        let chat = match self.chats.get_mut(&remove_from_chat.chat_id) {
            Some(chat) => chat,
            None => return Err(ChatServiceError::ChatNotFound),
        };
        chat.user_ids.remove(&user_id);

        Ok(())
    }

    pub async fn add_device(&mut self, context: Arc<WsContext>) {
        info!("add device {:?}", context);

        self.user_devices
            .entry(context.user_id.clone())
            .or_insert_with(Default::default)
            .insert(context.device_id.clone());
    }

    pub async fn remove_device(&mut self, context: Arc<WsContext>) -> Result<(), ChatServiceError> {
        info!("remove device {:?}", context);

        self.user_devices
            .entry(context.user_id.clone())
            .or_insert_with(Default::default)
            .remove(&context.device_id);

        Ok(())
    }

    /// Find a chat, know which users join inside that chat, find all users' devices and ask to ws_pool to send message the them
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
            // Dump which devices are now connected
            .cloned()
            .collect();

        let msg = PublishedMessage {
            writer: context.user_id.clone(),
            chat_id: smic.chat_id,
            text: smic.text,
        };

        info!(
            "UserId {:?} Sending message to devices {:?}",
            context.user_id.clone(),
            devices
        );

        // says to WsPool to send message "msg" to "devices"
        let event = Item::PublishMessage(devices, msg);
        self.item_sender
            .send(event)
            .await
            .expect("Unable to send message to devices");

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

pub async fn process_chat(
    chat_service: Arc<Mutex<ChatService>>,
    mut internal_receiver: UnboundedReceiver<(Arc<WsContext>, SendMessageInChat)>,
) {
    loop {
        let business_event = internal_receiver.next().await;

        info!("process business event {:?}", business_event);

        let res = match business_event {
            None => {
                warn!("NONE!");
                // The sender side is destroyed:
                // that means we are not able to fetch from this receiver anymore
                // So we need to stop this process!
                break;
            }
            Some((context, smic)) => {
                let mut chat_service = chat_service.lock().await;
                Some(chat_service.send_message(context, smic).await)
            }
        };

        if let Some(Err(e)) = res {
            // Maybe here we need to send a error message to client...
            warn!("process business event error {:?}", e);
        }
    }
}
