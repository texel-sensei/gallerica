use async_trait::async_trait;
use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use crate::message_api::*;

struct MQTTRequest {
    pub request: Request,
}

#[async_trait]
impl InflightRequest for MQTTRequest {
    fn request(&self) -> &Request { &self.request }

    async fn respond(self: Box<Self>, _response: Response) -> anyhow::Result<()> {
        Ok(())
    }
}

pub struct MQTTReceiver {
    connection: EventLoop,
}

impl MQTTReceiver {
    pub async fn new() -> anyhow::Result<Self> {
        let (client, connection) =
            AsyncClient::new(MqttOptions::new("test", "localhost", 1883), 10);

        client.subscribe("#", QoS::AtLeastOnce).await?;

        Ok(MQTTReceiver { connection })
    }
}

#[async_trait]
impl MessageReceiver for MQTTReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Box<dyn InflightRequest>> {
        use rumqttc::{Event::Incoming, Packet::Publish};

        loop {
            if let Incoming(Publish(publish)) = self.connection.poll().await? {
                let text = String::from_utf8_lossy(&publish.payload);
                return Ok(Box::new(MQTTRequest{
                    request: serde_json::from_str(&text)?,
                }));
            }
        }
    }
}
