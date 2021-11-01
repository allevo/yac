use websocket::start;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    start().await;
}
