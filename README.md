# YAC
Yac is Another Chat

This repo contains an example of Chat using WebSocket.
Because often the example you find in internet are "not so production ready", I would like to try to implement something more robust.

## ToC

  - [Main struture](#main-struture)
    - [`ChatService`](#chatservice)
    - [`WsPool`](#wspool)
    - [`Handler`](#handler)
  - [Component interation](#component-interation)
    - [Channel approach](#channel-approach)
      - [To `WsPool`](#to-wspool)
      - [To/From Redis (`Pub/Sub`)](#tofrom-redis-pubsub)
  - [Production considerations](#production-considerations)
  - [Development](#development)

## Main struture

The main parts in this project are:
- `ChatService`
- `WsPool`
- `Handler`

Also other parts are still important like `CredentialService` but it is not so WebSocket related.

### `ChatService`

This service keeps track which users are in which chats. It stores also the chats.

### `WsPool`

This pool contains all websockets currently connected to the server

### `Handler`

HTTP handlers allows to expose business functionality through a JSON REST interface.

## Component interation

### Channel approach

#### To `WsPool`

Near by `WsPool` struct, there's a `process_ws_pool` function that iterate over all WebSocket events. In this way, we don't have a lot of futures polling: just one. `process_ws_pool`, also, is the only one piece of the code that can access to the WsPool instance. That is guarandee by Rust itself: `WsPool` cannot send among threads safetly and there's no `Arc<Mutex<WsPool>>`. So At compile time, we have the guarandee that `process_ws_pool` has an exclusive access to `WsPool`. And that is amazing.

But a issue raised here: if `WsPool` stores all WebSockets and only `process_ws_pool` can access to it, how we can add to it new WebSockets?

The answer is: use channels. In fact `process_ws_pool` iterate over a channel typed `Item` that is a enum. One variant is used for adding WebSocket and another one for removing them.

In this way, the ws handler is able to add and remove WebSockets from `WsPool`.

But also `ChatService` needs to interact with `WsPool`: when `ChatService` decides to send messages, it needs to request to `WsPool` to send those messages. So, how is `ChatService` able to interact with `WsPool`?

The answer is always the same: use channels. In fact, the `receiver` side of a channel is not clonable, but the sender one is.

So, both the handler and the `ChatService` is able to interact with `WsPool`.

#### To/From Redis (`Pub/Sub`)

Because in production you may want to deploy multiple instances of you web service and because the WebSocket is stateful, you need to dispatch an event among the instances in order to inform them that a new message arrived.
To do that, the project uses redis in `Pub/Sub` mode. 

So, when a device sends a new message requests, `WsPool` intercept the request and publish it to a redis channel. All running instances capture that message (thanksful to the subscription) and invoke the `ChatService` in order to know which is the users are in that chat. For that users, `ChatService` fetch which `DeviceId` belong to the users and invoke asynchronously `WsPool`. `WsPool` iterates over the `DeviceId` list and sends the message to them.

But again, I would not like to link deeper `WsPool` and `ChatService` with redis: in the future, we cann choose to use a different approach/service.
So, for interacting with redis (aka for sending messages to redis and for listening messages from redis), we used the channel capability: when `WsPool` needs to inform that a new message arrives, it that message into the channel and externally sends it to redis channel (see `to_redis` in lib.rs).
From external process (see `from_redis` in lib.rs), we listen the messages from redis and invoke `ChatService` indirectly using a channel (see `process_chat` in chat_service.rs)

## Production considerations
The code uses always asychronous code for simulating database interation even where not needed. For production, this could impact the performance so please consider to change it before go live.


## Development

For running it locally:

```shell
cargo run
```

For running test:
```shell
cargo test
```

**NB**: you need always a running redis server on "redis://127.0.0.1/" (port 6379). You can change it, if you want.
