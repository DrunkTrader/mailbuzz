use anyhow::{bail, Context, Result};
use mailbuzz::campaign::{parse_csv, Template};
use mailbuzz::sent_log::{load_sent, sent_last_24h};
use mailbuzz::{config::Config, mailer, models::OutgoingMessage};
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::Duration;

const RESUME_DIR: &str = "resume";
const TEMPLATE_DIR: &str = "email";

fn random_delay() -> Duration {
    Duration::from_secs(20 + (uuid::Uuid::new_v4().as_u128() % 11) as u64)
}

/// Appends, so a rerun keeps the earlier status rows. Flushed per row, so an interrupted run still has its record
fn write_row(out: &mut csv::Writer<File>, row: [&str; 4]) -> Result<()> {
    out.write_record(row)?;
    out.flush()?;
    Ok(())
}

fn open_results(path: &str) -> Result<csv::Writer<File>> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("could not open {}", path))?;
    let is_new = file.metadata()?.len() == 0;
    let mut out = csv::Writer::from_writer(file);
    if is_new {
        write_row(&mut out, ["email", "company", "status", "message"])?;
    }
    Ok(out)
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let csv_path = std::env::args()
        .nth(1)
        .context("usage: send_campaign <recruiters.csv>")?;
    let cfg = Config::from_env()?;

    // Everything that can stop the campaign is checked here, before the first email goes out
    if std::env::var("SENDER_NAME").map_or(true, |name| name.trim().is_empty()) {
        bail!("Set SENDER_NAME in .env, recipients see it as the From name");
    }
    let template = Template::load(Path::new(TEMPLATE_DIR))?;
    let file = File::open(&csv_path).with_context(|| format!("could not read {}", csv_path))?;
    let recruiters = parse_csv(file)?;
    let (resume_name, resume_bytes) = mailer::find_resume(Path::new(RESUME_DIR))?;
    let results_path = format!("{}.results.csv", csv_path);
    let mut out = open_results(&results_path)?;
    println!(
        "Loaded {} recruiter(s) from {}, attaching {}",
        recruiters.len(),
        csv_path,
        resume_name
    );

    println!("Waiting for any running batch to finish...");
    let _lock = mailer::acquire_send_lock().await?;

    // Read after locking, so it includes anything a batch just sent
    let mut already_sent = load_sent();
    let mut pending = Vec::new();
    for r in &recruiters {
        if already_sent.insert(r.email.to_lowercase()) {
            pending.push(r);
        } else {
            write_row(
                &mut out,
                [
                    &r.email,
                    &r.company,
                    "skipped",
                    "Already in sent_log.txt or earlier in this CSV",
                ],
            )?;
        }
    }
    let skipped = recruiters.len() - pending.len();
    let sent_recently = sent_last_24h();
    let room = cfg.daily_send_limit.saturating_sub(sent_recently);
    let held_back = pending.len().saturating_sub(room);
    pending.truncate(room);
    println!(
        "{} sent in the last 24 hours, limit {}",
        sent_recently, cfg.daily_send_limit
    );
    println!(
        "Sending {}, skipping {} already emailed, leaving {} for a later run",
        pending.len(),
        skipped,
        held_back
    );

    let messages: Vec<OutgoingMessage> = pending.iter().map(|r| template.render(r)).collect();
    let attachment = Some((resume_name.as_str(), resume_bytes.as_slice()));
    let results = mailer::send_paced(
        &cfg,
        &messages,
        attachment,
        random_delay,
        |index, result| {
            let r = pending[index];
            let status = if result.success { "sent" } else { "failed" };
            // Stop rather than keep emailing people with no record of it
            write_row(&mut out, [&r.email, &r.company, status, &result.message])
                .expect("could not write the results file");
        },
    )
    .await;

    let sent = results.iter().filter(|r| r.success).count();
    println!(
        "Done: {} sent, {} failed, {} skipped. Status written to {}",
        sent,
        results.len() - sent,
        skipped,
        results_path
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_stays_between_20_and_30_seconds() {
        assert!((0..1000).all(|_| (20..=30).contains(&random_delay().as_secs())));
    }
}
