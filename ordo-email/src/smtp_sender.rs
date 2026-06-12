use lettre::transport::smtp::authentication::Credentials;
use lettre::{
    message::Mailbox,
    transport::smtp::client::{Tls, TlsParameters},
    AsyncSmtpTransport, AsyncTransport, Message,
};

/// Send an email reply via SMTP.
pub async fn send_reply(
    config: &crate::config::EmailConfig,
    to_address: &str,
    subject: &str,
    body_plain: &str,
    body_html: Option<&str>,
    in_reply_to: Option<&str>,
) -> Result<(), String> {
    let from_name = config.display_name.as_deref().unwrap_or("");
    let from_mbox: Mailbox = if from_name.is_empty() {
        config
            .address
            .parse()
            .map_err(|e| format!("invalid from: {e}"))?
    } else {
        format!("{} <{}>", from_name, config.address)
            .parse()
            .map_err(|e| format!("invalid from: {e}"))?
    };

    let to_mbox: Mailbox = to_address.parse().map_err(|e| format!("invalid to: {e}"))?;

    let mut builder = Message::builder()
        .from(from_mbox)
        .to(to_mbox)
        .subject(subject);

    if let Some(ref_id) = in_reply_to {
        builder = builder.in_reply_to(ref_id.to_string());
    }

    let email = if let Some(html) = body_html {
        builder
            .multipart(
                lettre::message::MultiPart::alternative()
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(lettre::message::header::ContentType::TEXT_PLAIN)
                            .body(body_plain.to_string()),
                    )
                    .singlepart(
                        lettre::message::SinglePart::builder()
                            .header(lettre::message::header::ContentType::TEXT_HTML)
                            .body(html.to_string()),
                    ),
            )
            .map_err(|e| format!("build multipart: {e}"))?
    } else {
        builder
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(body_plain.to_string())
            .map_err(|e| format!("build plain: {e}"))?
    };

    let creds = Credentials::new(config.address.clone(), config.imap_password.clone());

    // Use rustls-tls path
    let tls_params = TlsParameters::builder(config.smtp_host.clone())
        .build_rustls()
        .map_err(|e| format!("TLS params for {}: {e}", config.smtp_host))?;

    let mailer = AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(&config.smtp_host)
        .map_err(|e| format!("SMTP relay: {e}"))?
        .port(config.smtp_port)
        .credentials(creds)
        .tls(Tls::Required(tls_params))
        .build();

    mailer
        .send(email)
        .await
        .map_err(|e| format!("SMTP send: {e}"))?;

    Ok(())
}

/// Send a notification — a one-way informational email.
pub async fn send_notification(
    config: &crate::config::EmailConfig,
    to_address: &str,
    subject: &str,
    body: &str,
) -> Result<(), String> {
    send_reply(config, to_address, subject, body, None, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_notification_without_server_fails_gracefully() {
        let config = crate::config::EmailConfig {
            address: "test@example.com".into(),
            imap_host: "imap.example.com".into(),
            imap_username: "test".into(),
            imap_password: "wrong".into(),
            smtp_host: "localhost".into(),
            smtp_port: 587,
            ..Default::default()
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(send_notification(
            &config,
            "dst@example.com",
            "Test",
            "Body",
        ));
        assert!(result.is_err());
    }
}
