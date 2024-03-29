use crate::message_api::*;
use anyhow::{bail, Context};
use async_trait::async_trait;
use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
pub struct MqttListenerConfig {
    pub client_id: String,
    pub host: String,
    pub port: u16,

    pub topic: String,
}

#[derive(Deserialize)]
struct RequestData {
    #[serde(flatten)]
    pub request: Request,

    // Explicitly include reply topic and correlation data, as rumqttc doesn't support MQTT v5
    pub reply_topic: Option<String>,
    pub correlation_data: Option<String>,
}

struct MqttRequest {
    pub data: anyhow::Result<RequestData>,
    pub client: AsyncClient,
}

#[async_trait]
impl InflightRequest for MqttRequest {
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

pub struct MqttReceiver {
    connection: EventLoop,
    client: AsyncClient,
}

impl MqttReceiver {
    pub async fn new(config: &MqttListenerConfig) -> anyhow::Result<Self> {
        let (client, mut connection) = AsyncClient::new(
            MqttOptions::new(&config.client_id, &config.host, config.port),
            10,
        );

        let event = connection.poll().await.with_context(|| {
            format!(
                "Connecting to MQTT server at '{}:{}'",
                config.host, config.port
            )
        })?;

        use rumqttc::{Event::Incoming, Packet::ConnAck};
        if !matches!(event, Incoming(ConnAck(_))) {
            bail!("Failed to connect");
        }

        client
            .subscribe(&config.topic, QoS::AtLeastOnce)
            .await
            .with_context(|| format!("Subscribing to topic '{}'", config.topic))?;

        Ok(MqttReceiver { connection, client })
    }
}

#[async_trait]
impl MessageReceiver for MqttReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Box<dyn InflightRequest>> {
        use rumqttc::{Event::Incoming, Packet::Publish};

        loop {
            if let Incoming(Publish(publish)) = self.connection.poll().await? {
                return Ok(Box::new(MqttRequest {
                    data: serde_json::from_slice(&publish.payload).map_err(|e| e.into()),
                    client: self.client.clone(),
                }));
            }
        }
    }
}
