use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
    sync::Arc,
};

use futures::{
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
    lock::Mutex,
    stream::{select_all, SelectAll},
    SinkExt, Stream, StreamExt,
};

use warp::{
    ws::{Message, WebSocket},
    Error,
};

use crate::{
    chat_service::ChatService,
    models::{
        AddDevice, DeviceId, Item, MessageSender, PublishedMessage, ReceiverStream,
        SendMessageInChat, SenderStream, WsContext,
    },
};

trait GetId {
    fn get_id(&self) -> &DeviceId;
}

#[async_trait]
impl MessageSender for futures::stream::SplitSink<WebSocket, Message> {
    async fn send_message(&mut self, msg: Arc<PublishedMessage>) -> Result<(), ()> {
        let text = match serde_json::to_string(&msg) {
            // That's not so ok. Anyway we ignore serialization errors
            Err(e) => {
                error!("Error in serialization published message {:?}", e);
                return Ok(());
            }
            Ok(text) => text,
        };
        self.send(Message::text(text)).await.map_err(|_| ())?;
        Ok(())
    }
}

pub struct WsPool {
    // All WebSocket (the read side) will be added into that stream
    // In this way we cal poll just from this for actually polling
    // from the whole connected websocketin a simplest way
    incoming_streams: SelectAll<ReceiverStream>,
    // Store which DeviceId has which sender in order to identify
    // which websocket is identified by which DeviceId
    devices: HashMap<DeviceId, SenderStream>,
}

impl WsPool {
    pub fn new(item_receiver: UnboundedReceiver<Item>) -> Self {
        let recv: Vec<ReceiverStream> =
            vec![ReceiverStream::new("".into(), Box::pin(item_receiver))];

        let select_all = select_all(recv);

        Self {
            incoming_streams: select_all,
            devices: Default::default(),
        }
    }

    /// Converts device_ids into WebSocket and sends message to them
    pub async fn send_message_to(&mut self, device_ids: HashSet<DeviceId>, msg: PublishedMessage) {
        info!("send message to devices {:?}", device_ids);

        // Avoid cloning the whole structure. Just a "pointer"
        let msg = Arc::new(msg);

        for device_id in device_ids {
            let device = match self.devices.get_mut(&device_id) {
                // This can happen if a device goes away in the mean time
                // the ChatService "cloned"s device_ids and WsPool sends data
                None => continue,
                Some(device) => device,
            };

            // if the sending fails, we want to remove the device
            match device.send_message(msg.clone()).await {
                Ok(_) => {}
                Err(_) => self.remove_device(&device_id).await,
            };
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

        // This can be written better for avoiding the iteration among all WebSocket
        let new = SelectAll::new();
        let old_new = std::mem::replace(&mut self.incoming_streams, new);
        for s in old_new {
            if s.get_id() == device_id {
                continue;
            }
            self.incoming_streams.push(s);
        }
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
            None => {
                warn!("Pulled None from stream");
                // There's no receiver anymore
                break;
            }
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

    info!("end ws_pool.process");
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

impl ReceiverStream {
    pub fn new(
        device_id: DeviceId,
        stream: Pin<Box<dyn Stream<Item = Item> + Send + 'static>>,
    ) -> Self {
        Self(device_id, stream)
    }
}

impl Stream for ReceiverStream {
    type Item = Item;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.1.poll_next_unpin(cx)
    }
}
impl ReceiverStream {
    pub fn get_id(&self) -> &DeviceId {
        &self.0
    }
}
