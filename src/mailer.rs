use crate::config::Config;
use crate::models::{EmailResult, OutgoingMessage};
use crate::sent_log::log_sent;
use crate::validation::is_valid_email;
use anyhow::{bail, Context, Result};
use lettre::message::{header::ContentType, Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::path::Path;
use std::sync::Arc;
use tokio::time::{timeout, Duration};

/// Gap between messages so Gmail doesn't flag a burst of connections
const SEND_GAP: Duration = Duration::from_millis(1500);

/// Locked while a batch sends. A file lock, because send_campaign is a separate process from the server
const SEND_LOCK: &str = "send.lock";

/// Waits for any other batch to finish. The OS releases the lock when the returned file is dropped or the process dies
pub async fn acquire_send_lock() -> Result<std::fs::File> {
    tokio::task::spawn_blocking(|| {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(SEND_LOCK)
            .with_context(|| format!("could not open {}", SEND_LOCK))?;
        file.lock()
            .with_context(|| format!("could not lock {}", SEND_LOCK))?;
        Ok(file)
    })
    .await?
}

/// The one PDF in `dir` as (filename, bytes). Errors on anything else, because a wrong or missing resume can't be recalled once sent
pub fn find_resume(dir: &Path) -> Result<(String, Vec<u8>)> {
    let mut pdfs = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("could not open the {} folder", dir.display()))?
    {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            pdfs.push(path);
        }
    }
    let [path] = pdfs.as_slice() else {
        bail!(
            "{} must contain exactly one PDF, found {}",
            dir.display(),
            pdfs.len()
        );
    };
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    if !bytes.starts_with(b"%PDF-") {
        bail!("{} is not a valid PDF", path.display());
    }
    Ok((
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        bytes,
    ))
}

fn build_message(
    cfg: &Config,
    to: &str,
    subject: &str,
    body: &str,
    html: Option<&str>,
    attachment: Option<(&str, &[u8])>,
) -> Result<Message> {
    let sender = Mailbox::new(Some(cfg.sender_name.clone()), cfg.email.parse()?);
    let builder = Message::builder()
        .from(sender)
        .to(to.parse()?)
        .subject(subject);

    let alternative =
        |html: &str| MultiPart::alternative_plain_html(body.to_string(), html.to_string());

    Ok(match (html, attachment) {
        (None, None) => builder.body(body.to_string())?,
        (Some(html), None) => builder.multipart(alternative(html))?,
        (html, Some((filename, bytes))) => {
            let file = Attachment::new(filename.to_string())
                .body(bytes.to_vec(), ContentType::parse("application/pdf")?);
            let mixed = match html {
                Some(html) => MultiPart::mixed().multipart(alternative(html)),
                None => MultiPart::mixed().singlepart(SinglePart::plain(body.to_string())),
            };
            builder.multipart(mixed.singlepart(file))?
        }
    })
}

pub async fn send_email(
    cfg: &Config,
    to: &str,
    subject: &str,
    body: &str,
    html: Option<&str>,
    attachment: Option<(&str, &[u8])>,
) -> Result<()> {
    let email = build_message(cfg, to, subject, body, html, attachment)?;

    let creds = Credentials::new(cfg.email.clone(), cfg.app_password.clone());

    let tls_parameters = TlsParameters::new(cfg.smtp_server.clone())?;

    let mailer: AsyncSmtpTransport<Tokio1Executor> =
        AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp_server)?
            .port(cfg.smtp_port)
            .tls(Tls::Required(tls_parameters))
            .credentials(creds)
            .build();

    mailer.send(email).await?;
    // Every endpoint sends through here, so the log lists every mail sent, however it was triggered
    log_sent(to);
    println!("Email sent to {}", to);
    Ok(())
}

pub async fn send_bulk_emails(
    cfg: &Arc<Config>,
    recipients: &[String],
    subject: &str,
    body: &str,
    html: Option<&str>,
) -> Vec<EmailResult> {
    let mut tasks = Vec::new();

    // Create concurrent tasks for sending emails
    for recipient in recipients {
        let cfg = Arc::clone(cfg);
        let recipient = recipient.clone();
        let subject = subject.to_string();
        let body = body.to_string();
        let html = html.map(|h| h.to_string());

        let task = tokio::spawn(async move {
            let result = timeout(
                Duration::from_secs(30), // 30 second timeout per email
                send_email(&cfg, &recipient, &subject, &body, html.as_deref(), None),
            )
            .await;

            match result {
                Ok(Ok(_)) => EmailResult {
                    email: recipient,
                    success: true,
                    message: "Email sent successfully".to_string(),
                },
                Ok(Err(e)) => EmailResult {
                    email: recipient,
                    success: false,
                    message: format!("Failed to send email: {}", e),
                },
                Err(_) => EmailResult {
                    email: recipient,
                    success: false,
                    message: "Email sending timed out".to_string(),
                },
            }
        });

        tasks.push(task);
    }

    // Wait for all tasks to complete
    let mut results = Vec::new();
    for task in tasks {
        match task.await {
            Ok(result) => results.push(result),
            Err(e) => {
                // This shouldn't happen unless the task panics
                results.push(EmailResult {
                    email: "unknown".to_string(),
                    success: false,
                    message: format!("Task error: {}", e),
                });
            }
        }
    }

    let successful = results.iter().filter(|r| r.success).count();
    let failed = results.len() - successful;

    println!(
        "Bulk email sending completed: {} successful, {} failed",
        successful, failed
    );

    results
}

/// Sends different messages one after another (each person gets their own subject and body)
pub async fn send_batch(cfg: &Arc<Config>, messages: &[OutgoingMessage]) -> Vec<EmailResult> {
    let _lock = match acquire_send_lock().await {
        Ok(lock) => lock,
        Err(e) => {
            return messages
                .iter()
                .map(|m| EmailResult {
                    email: m.to.clone(),
                    success: false,
                    message: format!("Failed to send email: {:#}", e),
                })
                .collect();
        }
    };
    send_paced(cfg, messages, None, || SEND_GAP, |_, _| {}).await
}

/// Sends messages in order with `gap()` between them, reporting each result as it happens.
/// The caller must hold `acquire_send_lock`.
pub async fn send_paced(
    cfg: &Config,
    messages: &[OutgoingMessage],
    attachment: Option<(&str, &[u8])>,
    gap: impl Fn() -> Duration,
    mut on_result: impl FnMut(usize, &EmailResult),
) -> Vec<EmailResult> {
    let mut results: Vec<EmailResult> = Vec::with_capacity(messages.len());
    let mut slept = false;

    for (index, m) in messages.iter().enumerate() {
        let failure = |message: &str| EmailResult {
            email: m.to.clone(),
            success: false,
            message: message.to_string(),
        };

        let result = if !is_valid_email(&m.to) {
            failure("Invalid email address format")
        } else if m.subject.trim().is_empty() || m.body.trim().is_empty() {
            failure("Subject and body cannot be empty")
        } else {
            if slept {
                tokio::time::sleep(gap()).await;
            }
            slept = true;

            let sent = timeout(
                Duration::from_secs(30),
                send_email(
                    cfg,
                    &m.to,
                    &m.subject,
                    &m.body,
                    m.html.as_deref(),
                    attachment,
                ),
            )
            .await;

            match sent {
                Ok(Ok(_)) => EmailResult {
                    email: m.to.clone(),
                    success: true,
                    message: "Email sent successfully".to_string(),
                },
                Ok(Err(e)) => failure(&format!("Failed to send email: {}", e)),
                Err(_) => failure("Email sending timed out"),
            }
        };

        on_result(index, &result);
        results.push(result);
    }

    let successful = results.iter().filter(|r| r.success).count();
    println!(
        "Batch completed: {} sent, {} failed",
        successful,
        results.len() - successful
    );

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            email: "me@example.com".into(),
            sender_name: "Me".into(),
            app_password: "x".into(),
            smtp_server: "localhost".into(),
            smtp_port: 587,
            server_port: 6969,
            daily_send_limit: 300,
        }
    }

    fn formatted(html: Option<&str>, attachment: Option<(&str, &[u8])>) -> String {
        let message = build_message(
            &cfg(),
            "you@example.com",
            "s",
            "plain body",
            html,
            attachment,
        )
        .unwrap();
        String::from_utf8_lossy(&message.formatted()).into_owned()
    }

    fn temp_dir_with(files: &[(&str, &[u8])]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("resume-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        for (name, bytes) in files {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        dir
    }

    #[test]
    fn message_structure_only_changes_when_there_is_an_attachment() {
        let plain = formatted(None, None);
        assert!(plain.contains("plain body") && !plain.contains("multipart/"));

        let html = formatted(Some("<p>x</p>"), None);
        assert!(html.contains("multipart/alternative") && !html.contains("multipart/mixed"));

        let pdf: Option<(&str, &[u8])> = Some(("cv.pdf", b"%PDF-1.7"));
        let attached = formatted(None, pdf);
        assert!(attached.contains("multipart/mixed") && attached.contains("application/pdf"));
        assert!(attached.contains("cv.pdf") && attached.contains("plain body"));

        let both = formatted(Some("<p>x</p>"), pdf);
        assert!(both.contains("multipart/mixed") && both.contains("multipart/alternative"));
    }

    #[test]
    fn find_resume_needs_exactly_one_real_pdf() {
        let dir = temp_dir_with(&[("cv.PDF", b"%PDF-1.7 data"), ("notes.txt", b"x")]);
        assert_eq!(find_resume(&dir).unwrap().0, "cv.PDF");

        assert!(find_resume(&dir.join("missing")).is_err());
        assert!(find_resume(&temp_dir_with(&[("notes.txt", b"x")])).is_err());
        assert!(find_resume(&temp_dir_with(&[("a.pdf", b"%PDF-"), ("b.pdf", b"%PDF-")])).is_err());
        assert!(find_resume(&temp_dir_with(&[("a.pdf", b"")])).is_err());
    }
}
