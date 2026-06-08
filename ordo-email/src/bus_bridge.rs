use std::sync::Arc;

use ordo_bus::Bus;
use ordo_protocol::{BusEnvelope, Envelope, NodeId, OrdoMessage};
use tokio::task::JoinHandle;
use tracing::{error, info, debug};

use crate::config::EmailConfig;
use crate::imap_poller::{self, ImapPoller, poll_inbox};
use crate::smtp_sender;

/// The main email bridge — polls IMAP, publishes commands on the bus,
/// listens for reply requests, and sends replies via SMTP.
pub struct EmailBridge {
    config: EmailConfig,
    bus: Arc<InProcessBusWrapper>,
    node_id: NodeId,
    poll_handle: Option<JoinHandle<()>>,
    reply_handle: Option<JoinHandle<()>>,
}

/// Wrapper around the bus so we don't need a generic parameter everywhere.
struct InProcessBusWrapper {
    inner: Arc<dyn Bus>,
}

impl InProcessBusWrapper {
    fn new(bus: Arc<dyn Bus>) -> Self {
        Self { inner: bus }
    }

    async fn publish(&self, topic: &str, message: OrdoMessage, node_id: &NodeId) {
        let envelope = Envelope::new(node_id.clone(), message);
        if let Err(e) = self.inner.publish(topic, envelope).await {
            error!("ordo-email: bus publish error: {e}");
        }
    }

    async fn subscribe(&self, topic: &str) -> Result<
        Box<dyn futures::Stream<Item = BusEnvelope> + Unpin + Send>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        self.inner.subscribe(topic).await
    }
}

impl EmailBridge {
    pub fn new(
        config: EmailConfig,
        bus: Arc<dyn Bus>,
        node_id: NodeId,
    ) -> Self {
        let wrapper = InProcessBusWrapper::new(bus);
        Self {
            config,
            bus: Arc::new(wrapper),
            node_id,
            poll_handle: None,
            reply_handle: None,
        }
    }

    /// Start the IMAP poll loop and the reply listener.
    pub fn start(&mut self) {
        self.start_polling();
        self.start_reply_listener();
        info!(
            "ordo-email: bridge started (addr={}, poll={}s, prefix=\"{}\")",
            self.config.address,
            self.config.poll_seconds,
            self.config.command_prefix
        );
    }

    /// Stop both loops.
    pub async fn stop(&mut self) {
        if let Some(h) = self.poll_handle.take() {
            h.abort();
        }
        if let Some(h) = self.reply_handle.take() {
            h.abort();
        }
    }

    fn start_polling(&mut self) {
        let config = self.config.clone();
        let bus = Arc::clone(&self.bus);
        let node_id = self.node_id.clone();
        let poll_interval = std::time::Duration::from_secs(config.poll_seconds);
        let mut poller = ImapPoller::new(config.clone());

        self.poll_handle = Some(tokio::spawn(async move {
            // First poll: wait a short time to let the system boot
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            loop {
                match poll_inbox(&config).await {
                    Ok(emails) => {
                        let count = emails.len();
                        if count > 0 {
                            info!("ordo-email: fetched {count} new email(s) from inbox");
                        }

                        for email in &emails {
                            if poller.process_new_messages(email).is_none() {
                                continue;
                            }

                            let commands = imap_poller::filter_commands(
                                vec![email.clone()],
                                &config,
                            );

                            for (email, cmd) in commands {
                                info!(
                                    "ordo-email: command from {}: \"{}\"",
                                    cmd.from_address, cmd.raw
                                );

                                bus.publish(
                                    "ordo.email.command.received",
                                    OrdoMessage::EmailCommandReceived {
                                        email_id: format!("{}", email.seq),
                                        from_address: cmd.from_address.clone(),
                                        subject: email.subject.clone(),
                                        body_plain: cmd.body_plain.clone(),
                                        body_html: cmd.body_html.clone(),
                                        received_at: chrono::Utc::now(),
                                    },
                                    &node_id,
                                ).await;
                            }
                        }
                    }
                    Err(e) => {
                        debug!("ordo-email: IMAP poll error: {e}");
                    }
                }

                tokio::time::sleep(poll_interval).await;
            }
        }));
    }

    fn start_reply_listener(&mut self) {
        let config = self.config.clone();
        let bus = Arc::clone(&self.bus);

        self.reply_handle = Some(tokio::spawn(async move {
            let mut stream = match bus
                .subscribe("ordo.email.reply.requested")
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    error!("ordo-email: failed to subscribe: {e}");
                    return;
                }
            };

            use futures::StreamExt;
            while let Some(envelope) = stream.next().await {
                match envelope.payload {
                    OrdoMessage::EmailReplyRequested {
                        email_id: _,
                        to_address,
                        subject,
                        body_plain,
                        body_html,
                        in_reply_to_subject,
                    } => {
                        info!(
                            "ordo-email: sending reply to {to_address} — subject: {subject}"
                        );

                        match smtp_sender::send_reply(
                            &config,
                            &to_address,
                            &subject,
                            &body_plain,
                            body_html.as_deref(),
                            in_reply_to_subject.as_deref(),
                        ).await {
                            Ok(()) => {
                                info!("ordo-email: reply sent to {to_address}");
                            }
                            Err(e) => {
                                error!("ordo-email: failed to send reply: {e}");
                            }
                        }
                    }
                    _ => {}
                }
            }
        }));
    }
}

impl Drop for EmailBridge {
    fn drop(&mut self) {
        if let Some(h) = self.poll_handle.take() {
            h.abort();
        }
        if let Some(h) = self.reply_handle.take() {
            h.abort();
        }
        info!("ordo-email: bridge stopped");
    }
}
