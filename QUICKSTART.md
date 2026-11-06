# Quick Start: Send a CSV Campaign

Run every command from the project root (the folder with `Cargo.toml`). You need [Rust](https://rustup.rs) installed.

## 1. Set up `.env`

```bash
cp .env.example .env
```

Then fill in your name, Gmail address and [App Password](README.md#3-gmail-setup):

```env
SENDER_NAME="Your Name"
EMAIL_ADDRESS=you@gmail.com
APP_PASSWORD="abcd efgh ijkl mnop"
```

Keep the double quotes if the password has spaces, otherwise you'll get `APP_PASSWORD not set`.

## 2. Add your resume

Put exactly one PDF in the `resume/` folder:

```bash
mkdir -p resume
cp ~/path/to/your_resume.pdf resume/
```

## 3. Write your email

Edit the two files in the `email/` folder:

- `email/subject.txt`: the subject line
- `email/body.md`: the body, in Markdown (`**bold**`, `- ` bullets, `<https://link>`)

Both can use `{{Company}}` and `{{Greeting}}` ("Hi Priya", or just "Hi" when `name` is blank). Changes apply on the next run, with no rebuild.

## 4. Create your CSV

Save it as `recruiters.csv` in the project root. `email` and `company` are required, `name` is optional:

```csv
email,company,name
priya@fitt.com,FITT,Priya
jobs@acme.com,"Acme, Inc",
```

Keep the trailing comma when `name` is blank, and put double quotes around any value containing a comma.

## 5. Build

```bash
cargo build --release
```

## 6. Send

```bash
cargo run --release --bin send_campaign recruiters.csv
```

Emails go out one at a time, 20-30 seconds apart. Send a one-row CSV to your own address first to check how it looks.

## 7. Check the results

```bash
cat recruiters.csv.results.csv
```

Each row shows `sent`, `failed` or `skipped`. `sent` means Gmail accepted the email, not that it reached the inbox.

To continue after stopping, or to retry failed rows, run the step 6 command again. Anyone already emailed is skipped. To email someone again on purpose, delete their line from `sent_log.txt`.

## Common errors

| Error | Fix |
|---|---|
| `APP_PASSWORD not set` | Check `.env` exists and the password is in double quotes |
| `resume must contain exactly one PDF` | Leave one PDF in `resume/` and run from the project root |
| `Set SENDER_NAME in .env` | Add `SENDER_NAME="Your Name"` to `.env` |
| `Unknown placeholder in the email template` | Fix the typo in `email/`. Only `{{Greeting}}` and `{{Company}}` exist |
| `CSV header must include an 'email' column` | The first row must be the header: `email,company,name` |
| `CSV row '...' has no company` | Fill in that row. Nothing was sent |
| `bad CSV row` | The row has the wrong number of columns. Check for an unquoted comma or a missing trailing comma |

For the REST API and all other options, see the [README](README.md).
