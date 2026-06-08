use async_trait::async_trait;
use futures::Stream;
use ordo_protocol::BusEnvelope;
use tokio::sync::broadcast;

pub mod correlator;
pub mod registry;

pub use correlator::{BusCorrelator, CorrelatorError};
pub use registry::{ProviderRegistry, ProviderRegistryEntry};

#[async_trait]
pub trait Bus: Send + Sync {
    async fn publish(
        &self,
        topic: &str,
        message: BusEnvelope,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
    async fn subscribe(
        &self,
        topic: &str,
    ) -> Result<
        Box<dyn Stream<Item = BusEnvelope> + Unpin + Send>,
        Box<dyn std::error::Error + Send + Sync>,
    >;
}

pub struct InProcessBus {
    sender: broadcast::Sender<(String, BusEnvelope)>,
}

impl Default for InProcessBus {
    fn default() -> Self {
        Self::new()
    }
}

impl InProcessBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self { sender }
    }
}

fn matches_topic(subscription: &str, topic: &str) -> bool {
    subscription == "*"
        || subscription == topic
        || (subscription.ends_with(".*") && topic.starts_with(subscription.trim_end_matches('*')))
}

#[async_trait]
impl Bus for InProcessBus {
    async fn publish(
        &self,
        topic: &str,
        message: BusEnvelope,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _ = self.sender.send((topic.to_string(), message));
        Ok(())
    }

    async fn subscribe(
        &self,
        topic: &str,
    ) -> Result<
        Box<dyn Stream<Item = BusEnvelope> + Unpin + Send>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let mut rx = self.sender.subscribe();
        let topic = topic.to_string();

        let stream = async_stream::stream! {
            while let Ok((msg_topic, envelope)) = rx.recv().await {
                if matches_topic(&topic, &msg_topic) {
                    yield envelope;
                }
            }
        };

        Ok(Box::new(Box::pin(stream)))
    }
}
