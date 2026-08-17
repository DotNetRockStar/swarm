//! Outbound account email (verification, password reset) over SMTP.
//!
//! Deliberately best-effort: a send failure is logged, never surfaced to
//! the caller. The account/token already exists in the DB by the time an
//! email is sent, and `request_reset` must not leak whether an address
//! has an account — an SMTP outage failing the HTTP request would do
//! exactly that (and would make registration itself depend on a third
//! party being up, which isn't a trade this makes).

use crate::config::SmtpConfig;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

pub struct Mailer {
    inner: Option<Configured>,
}

struct Configured {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    /// `None` config (the `SWARM_SMTP_HOST`-unset default) yields a mailer
    /// that logs instead of sending — matches this codebase's existing
    /// dev/test behavior exactly, just centralized here instead of at each
    /// call site.
    pub fn from_config(smtp: Option<&SmtpConfig>) -> Self {
        let Some(smtp) = smtp else { return Self { inner: None } };
        let configured = || -> Result<Configured, Box<dyn std::error::Error>> {
            let tls_params = TlsParameters::new(smtp.host.clone())?;
            let tls = if smtp.implicit_tls { Tls::Wrapper(tls_params) } else { Tls::Required(tls_params) };
            let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host)
                .port(smtp.port)
                .tls(tls)
                .credentials(Credentials::new(smtp.username.clone(), smtp.password.clone()))
                .build();
            let from = Mailbox::new(Some(smtp.from_name.clone()), smtp.from_email.parse()?);
            Ok(Configured { transport, from })
        };
        match configured() {
            Ok(c) => Self { inner: Some(c) },
            Err(error) => {
                tracing::error!(%error, "SMTP config invalid; falling back to logging verification/reset links");
                Self { inner: None }
            }
        }
    }

    pub async fn send_verification(&self, to_email: &str, verify_url: &str) {
        self.send(
            to_email,
            "Verify your SWARM account",
            format!(
                "Confirm your email address to finish setting up your SWARM account:\n\n{verify_url}\n\n\
                 This link expires in 24 hours. If you didn't create a SWARM account, ignore this email."
            ),
            "verification link",
        )
        .await;
    }

    pub async fn send_password_reset(&self, to_email: &str, reset_url: &str) {
        self.send(
            to_email,
            "Reset your SWARM password",
            format!(
                "Someone requested a password reset for this SWARM account:\n\n{reset_url}\n\n\
                 This link expires in 1 hour. If you didn't request this, ignore this email — \
                 your password won't change."
            ),
            "password reset link",
        )
        .await;
    }

    async fn send(&self, to_email: &str, subject: &str, body: String, kind: &str) {
        let Some(configured) = &self.inner else {
            tracing::info!(to = to_email, kind, "{kind} (SMTP not configured, logging instead): {body}");
            return;
        };
        let to = match to_email.parse() {
            Ok(addr) => Mailbox::new(None, addr),
            Err(error) => {
                tracing::error!(%error, to = to_email, "not a valid mailbox address, dropping {kind}");
                return;
            }
        };
        let message = match Message::builder().from(configured.from.clone()).to(to).subject(subject).body(body) {
            Ok(m) => m,
            Err(error) => {
                tracing::error!(%error, to = to_email, "failed to build {kind} email");
                return;
            }
        };
        match configured.transport.send(message).await {
            Ok(_) => tracing::info!(to = to_email, kind, "sent"),
            Err(error) => tracing::error!(%error, to = to_email, kind, "SMTP send failed"),
        }
    }
}
