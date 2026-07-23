//! Best-effort SMTP delivery for password-reset links. When SMTP is not
//! configured the caller receives the link back instead (shown in the admin
//! UI so it can be handed over manually).

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::config::Config;

/// Try to send the reset link by mail. Returns true when a mail went out.
pub async fn send_reset_link(cfg: &Config, to_email: &str, link: &str) -> bool {
    let Some(host) = cfg.smtp_host.clone() else {
        return false;
    };
    if to_email.is_empty() {
        return false;
    }
    let email = match Message::builder()
        .from(match cfg.smtp_from.parse() {
            Ok(f) => f,
            Err(_) => return false,
        })
        .to(match to_email.parse() {
            Ok(t) => t,
            Err(_) => return false,
        })
        .subject("Reset your Memories password")
        .header(ContentType::TEXT_PLAIN)
        .body(format!(
            "A password reset was requested for your Memories account.\n\n\
             Open this link to choose a new password (valid for 1 hour):\n\n{link}\n\n\
             If you did not request this, you can ignore this message."
        )) {
        Ok(m) => m,
        Err(_) => return false,
    };

    let mut builder = match AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host) {
        Ok(b) => b,
        Err(_) => return false,
    }
    .port(cfg.smtp_port);
    if let (Some(user), Some(pass)) = (cfg.smtp_username.clone(), cfg.smtp_password.clone()) {
        builder = builder.credentials(Credentials::new(user, pass));
    }
    let mailer = builder.build();
    match mailer.send(email).await {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!("SMTP delivery failed: {err}");
            false
        }
    }
}
