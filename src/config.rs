use anyhow::{Context, Result};
use std::env;

pub struct Config {
    pub email: String,
    pub sender_name: String,
    pub app_password: String,
    pub smtp_server: String,
    pub smtp_port: u16,
    pub server_port: u16,
    pub daily_send_limit: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let email = env::var("EMAIL_ADDRESS").context("EMAIL_ADDRESS not set")?;
        let sender_name = env::var("SENDER_NAME").unwrap_or_else(|_| "MailBuzz".into());
        let app_password = env::var("APP_PASSWORD").context("APP_PASSWORD not set")?;
        let smtp_server = env::var("SMTP_SERVER").unwrap_or_else(|_| "smtp.gmail.com".into());
        let smtp_port = env::var("SMTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(587);
        let server_port = env::var("PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6969);
        // A typo here must stop the run, not quietly send a different number of emails
        let daily_send_limit = match env::var("DAILY_SEND_LIMIT") {
            Ok(s) => s
                .trim()
                .parse()
                .context("DAILY_SEND_LIMIT must be a whole number")?,
            Err(_) => 300,
        };

        Ok(Self {
            email,
            sender_name,
            app_password,
            smtp_server,
            smtp_port,
            server_port,
            daily_send_limit,
        })
    }
}
