use serde::Deserialize;

fn default_port() -> u16 {
    8080
}
fn default_redis() -> String {
    "redis://127.0.0.1/".to_owned()
}
fn default_redis_channel() -> String {
    "chats".to_owned()
}

#[derive(Deserialize, Debug, Clone)]
/// Configuration for YAC
pub struct Config {
    #[serde(default = "default_redis")]
    pub redis_url: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_redis_channel")]
    pub redis_channel: String,
}

#[cfg(test)]
impl Default for Config {
    fn default() -> Self {
        Self {
            redis_url: default_redis(),
            port: default_port(),
            redis_channel: default_redis_channel(),
        }
    }
}
