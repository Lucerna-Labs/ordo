//! ordo-email: Remote-control channel for Ordo via email.
//!
//! Polls an IMAP inbox for commands from authorized senders,
//! publishes them onto the Ordo bus as `EmailCommandReceived`,
//! and sends SMTP replies when the brain/assistant publishes
//! `EmailReplyRequested`.
//!
//! Architecture:
//!   ┌─────────────┐     ┌──────────────┐     ┌──────────────┐
//!   │ IMAP Poller  │ ──> │ Command Parser│ ──> │   Ordo Bus   │
//!   │ (every 30s)  │     │ subject: ordo │     │ EmailCmdRecv │
//!   └─────────────┘     └──────────────┘     └──────┬───────┘
//!                                                    │
//!   ┌─────────────┐     ┌──────────────┐     ┌──────▼───────┐
//!   │ SMTP Sender  │ <── │ Reply Builder │ <── │   Ordo Bus   │
//!   │ (lettre)     │     │              │     │ EmailReplyReq│
//!   └─────────────┘     └──────────────┘     └──────────────┘

mod bus_bridge;
mod command;
mod config;
mod imap_poller;
mod smtp_sender;

pub use bus_bridge::EmailBridge;
pub use command::ParsedCommand;
pub use config::EmailConfig;
pub use smtp_sender::{send_notification, send_reply};
