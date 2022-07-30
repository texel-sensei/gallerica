use crate::message_api::*;
use async_trait::async_trait;
use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct RequestData {
    #[serde(flatten)]
    pub request: Request,

    // Explicitly include reply topic and correlation data, as rumqttc doesn't support MQTT v5
    pub reply_topic: Option<String>,
    pub correlation_data: Option<String>,
}

struct MQTTRequest {
    pub data: anyhow::Result<RequestData>,
    pub client: AsyncClient,
}

#[async_trait]
impl InflightRequest for MQTTRequest {
    fn request(&self) -> anyhow::Result<&Request> {
        match self.data.as_ref() {
            Ok(data) => Ok(&data.request),
            Err(err) => anyhow::bail!(err.to_string()),
        }
    }

    async fn respond(self: Box<Self>, response: Response) -> anyhow::Result<()> {
        let data = self.data?;
        if let Some(topic) = data.reply_topic {
            #[derive(Serialize)]
            struct ResponseWrapper {
                #[serde(flatten)]
                pub response: Response,
                pub correlation_data: Option<String>,
            }

            self.client
                .publish(
                    topic,
                    QoS::AtMostOnce,
                    false,
                    serde_json::to_vec(&ResponseWrapper {
                        response,
                        correlation_data: data.correlation_data,
                    })?,
                )
                .await?;
        }

        Ok(())
    }
}

pub struct MQTTReceiver {
    connection: EventLoop,
    client: AsyncClient,
}

impl MQTTReceiver {
    pub async fn new() -> anyhow::Result<Self> {
        let (client, connection) =
            AsyncClient::new(MqttOptions::new("test", "localhost", 1883), 10);

        client.subscribe("#", QoS::AtLeastOnce).await?;

        Ok(MQTTReceiver { connection, client })
    }
}

#[async_trait]
impl MessageReceiver for MQTTReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Box<dyn InflightRequest + Send>> {
        use rumqttc::{Event::Incoming, Packet::Publish};

        loop {
            if let Incoming(Publish(publish)) = self.connection.poll().await? {
                return Ok(Box::new(MQTTRequest {
                    data: serde_json::from_slice(&publish.payload).map_err(|e| e.into()),
                    client: self.client.clone(),
                }));
            }
        }
    }
}
