use async_trait::async_trait;
use serde::Deserialize;

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};

#[derive(Debug, Deserialize)]
#[serde(tag = "method")]
pub enum Message {
    /// Immediately show the next image, no matter the update rate.
    NextImage,
    UpdateInterval {
        millis: u64,
    },
}

#[async_trait]
pub trait MessageReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Message>;
}

pub struct MQTTReceiver {
    client: AsyncClient,
    connection: EventLoop,
}

impl MQTTReceiver {
    pub async fn new() -> anyhow::Result<Self> {
        let (client, connection) =
            AsyncClient::new(MqttOptions::new("test", "localhost", 1883), 10);

        client.subscribe("#", QoS::AtLeastOnce).await?;

        Ok(MQTTReceiver { client, connection })
    }
}

#[async_trait]
impl MessageReceiver for MQTTReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Message> {
        use rumqttc::{Event::Incoming, Packet::Publish};

        loop {
            match self.connection.poll().await? {
                Incoming(Publish(publish)) => {
                    let text = String::from_utf8_lossy(&publish.payload);
                    return Ok(serde_json::from_str(&text)?);
                }
                _ => {}
            }
        }
    }
}
