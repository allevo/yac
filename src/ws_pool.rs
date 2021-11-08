use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use futures::{
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
    lock::Mutex,
    SinkExt, StreamExt,
};

use warp::{ws::Message, Error};

use crate::{
    chat_service::ChatService,
    models::{
        AddDevice, DeviceId, Item, PublishedMessage, ReceiverStream, SendMessageInChat,
        SenderStream, WsContext,
    },
    my_select_all,
};

pub struct WsPool {
    incoming_streams: my_select_all::MySelectAll<ReceiverStream>,
    devices: HashMap<DeviceId, SenderStream>,
}

impl WsPool {
    pub fn new(item_receiver: UnboundedReceiver<Item>) -> Self {
        let recv: Vec<ReceiverStream> =
            vec![ReceiverStream::new("".into(), Box::pin(item_receiver))];

        let select_all = my_select_all::my_select_all(recv);

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

    pub async fn add_device(&mut self, context: Arc<WsContext>, add_device: AddDevice) {
        info!("add device event {:?}", context);

        self.incoming_streams.push(add_device.receiver);
        self.devices
            .insert(context.device_id.clone(), add_device.sender);
    }

    pub async fn remove_device(&mut self, device_id: &DeviceId) {
        info!("remove device {:?}", device_id);

        self.devices.remove(device_id);
        self.incoming_streams.remove_by_device_id(device_id)
    }
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
                ws_pool.add_device(context, add_device).await;
            }
            Some(Item::RemoveDevice(device_id)) => {
                ws_pool.remove_device(&device_id).await;
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
