# AGENTS.md

Guidance for AI coding agents working in this repository.

## What this is

A REST API (Rust/Axum) for sending email through an SMTP account (e.g. Gmail + App Password), plus a CLI binary for CSV-driven cold-mail campaigns. Package and repo name: `mailbuzz`.

## Commands

```bash
cargo build --release                               # build everything (lib + both binaries)
cargo run                                            # run the server (reads .env, listens on $PORT or 6969)
cargo run --bin send_campaign path/to/recruiters.csv # run the CSV cold-mail campaign CLI
cargo test                                            # run tests (in scheduler.rs, mailer.rs, campaign.rs and bin/send_campaign.rs)
cargo test takes_only_due_jobs_earliest_first        # run a single test by name
```

Requires a `.env` file with `EMAIL_ADDRESS` and `APP_PASSWORD` (required); `SMTP_SERVER`, `SMTP_PORT`, `SENDER_NAME`, `PORT`, `DAILY_SEND_LIMIT` are optional. `send_campaign` also needs `SENDER_NAME` and exactly one PDF in `resume/` (git-ignored). Run both binaries from the repo root, because `email/`, `resume/`, `sent_log.txt`, `scheduled_jobs.json` and `send.lock` are all relative to the working directory.

## Architecture

`src/lib.rs` declares `pub mod campaign, config, handlers, mailer, models, scheduler, sent_log, validation, verifier;` so both binary targets can share them: `src/main.rs` (the Axum server) and `src/bin/send_campaign.rs` (the CSV campaign CLI).

- `src/main.rs` wires routes via `use mailbuzz::{config, handlers, scheduler};`, with `AppState { config: Arc<Config>, scheduler: Scheduler }` shared via `FromRef` so handlers extract only the state they need.
- `src/config.rs` — env-based `Config`, loaded once at startup.
- `src/models.rs` — all request/response DTOs (serde structs) for every endpoint, in one file.
- `src/handlers/` — one module per route group (`single`, `bulk`, `batch`, `schedule`, `sent`, `validate`, `health`), re-exported from `handlers/mod.rs`. Handlers do request validation/shaping and delegate to `mailer`, `scheduler`, or `verifier`.
- `src/mailer.rs` — actual SMTP sending via `lettre`. Three entry points:
  - `send_email(cfg, to, subject, body, html, attachment)` — single message; always logs to the sent log on success. `attachment` is `Option<(&str, &[u8])>` (filename, bytes). `build_message` only uses `multipart/mixed` when there is an attachment, so plain and HTML mail keep their original structure.
  - `send_bulk_emails` — same subject/body to many recipients, sent concurrently (one `tokio::spawn` per recipient, 30s timeout each, no attachment).
  - `send_batch` — personalised per-recipient messages for `/send-batch` and the scheduler: takes the send lock, then calls `send_paced` with the 1.5s `SEND_GAP` and no attachment.
  - `send_paced` — the shared sequential loop: validates each message, sleeps `gap()` between real sends, reports each result through `on_result`. The caller must hold `acquire_send_lock()`.
  - `acquire_send_lock` — an OS file lock on `send.lock`, so only one batch sends at a time across the server **and** `send_campaign`, which is a separate process. The OS releases it if the process dies.
  - `find_resume(dir)` — returns the single PDF in `dir`, or an error for zero, several, unreadable or non-PDF files.
- `src/scheduler.rs` — `Scheduler` holds jobs in memory (`Arc<Mutex<Vec<Job>>>`) and persists them to `scheduled_jobs.json` via atomic write-tmp-then-rename (`save`). A background task (`spawn`, started in `main.rs`) polls every `TICK` (10s) for due jobs and runs them through `mailer::send_batch`. Job lifecycle: `Pending -> Running -> Done`, or `Cancelled` (only from `Pending`), or `Interrupted` (if the process dies mid-`Running`; not auto-retried). On `Scheduler::load()`, any job found `Running` from a previous crash is marked `Interrupted`.
- `src/sent_log.rs` — append-only `sent_log.txt` (`address<TAB>unix_time` per line, lowercased), one `OpenOptions::append` write per send so concurrent bulk sends don't interleave. Read-only/advisory: nothing skips re-sending to a logged address.
- `src/validation.rs` — `is_valid_email` (regex) and the batch size caps `MAX_BATCH` (50, for `/send-batch` and `/schedule-batch`) and `MAX_VALIDATE_BATCH` (100, for `/validate-emails`).
- `src/verifier.rs` — thin client for the external `rapid-email-verifier.fly.dev` service (single `verify` and `verify_batch`, 15s timeout), used by the `/validate-email(s)` endpoints. This only checks syntax/domain/MX/disposable/role-based — it does not confirm a specific mailbox exists.
- `email/` — the only place the campaign's wording lives: `subject.txt` and `body.md` (Markdown), read at runtime, so edits need no rebuild.
- `src/campaign.rs` — CSV parsing (`parse_csv`, `csv` crate; `email`, `company` required, `name` optional, any column order) and `Template`. `Template::load` turns `body.md` into HTML with `pulldown-cmark` and into plain text by stripping `**`, `<` and `>`, and rejects empty files or unknown placeholders. `render` fills `{{Greeting}}` (`"Hi {name}"`, or `"Hi"` when `name` is blank) and `{{Company}}`, HTML-escaping them in the HTML body, and appends a hidden per-send ID that must stay last so Gmail doesn't collapse a repeated ending behind "...".
- `src/bin/send_campaign.rs` — thin standalone CLI, not part of the HTTP API, and the only sender that attaches the resume or uses the random 20-30s gap. Everything that can abort runs before the first send: `SENDER_NAME`, the template, any bad row, the resume check, and opening the results file. After taking the send lock it skips addresses in `sent_log.txt` or repeated in the CSV, trims the rest to what `DAILY_SEND_LIMIT` (default 300) allows given the last 24 hours of `sent_log.txt`, sends through `mailer::send_paced`, and appends each row to `<input>.results.csv` (`email,company,status,message`, status `sent`/`failed`/`skipped`) as it finishes.

### Key behaviors to preserve when touching this code

- `mailer::send_email` is the single choke point that writes to the sent log — any new send path should go through it (or through `send_batch`/`send_bulk_emails`, which already do) rather than calling `lettre` directly.
- `acquire_send_lock` serializes `/send-batch`, the scheduler's background task, and `send_campaign` — don't bypass it when adding new batch-sending call sites. A campaign holds it for its whole run, so server batches wait until it finishes.
- `/send-batch` and `/schedule-batch` must stay short and attachment-free: a slow gap there means a request lasting many minutes, and the resume is personal. Long paced runs belong in `send_campaign`.
- `Scheduler` writes to disk (`save`) after every mutation (`add`, `cancel`, `take_next_due`, `finish`) — keep that pattern for new state transitions so a crash can't lose job state.
- Git-ignored local files: `scheduled_jobs.json`, `sent_log.txt`, `send.lock`, `resume/`, `reference/`, and all `*.csv` (recruiter lists and results). Tests that touch `Scheduler` should load from a temp path (see `temp_path()` in `scheduler.rs` tests), not the real `scheduled_jobs.json`.
- `send_campaign` must fail before the first send on anything that would make a whole run wrong. Cold mail can't be recalled, so don't soften those checks into warnings.

Full endpoint-by-endpoint request/response documentation (including error codes, the `send_campaign` CSV format, and example curl calls) is in README.md — read it before changing any handler's wire format or the campaign CLI's CSV schema.
