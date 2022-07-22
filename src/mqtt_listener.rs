use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};

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
    async fn receive_message(&mut self) -> anyhow::Result<Request> {
        use rumqttc::{Event::Incoming, Packet::Publish};

        loop {
            if let Incoming(Publish(publish)) = self.connection.poll().await? {
                let text = String::from_utf8_lossy(&publish.payload);
                return Ok(serde_json::from_str(&text)?);
            }
        }
    }
}
