use crate::models::OutgoingMessage;
use anyhow::{bail, Context, Result};
use pulldown_cmark::{html, Event, Parser};
use std::io::Read;
use std::path::Path;

pub struct Recruiter {
    pub email: String,
    pub company: String,
    pub name: String,
}

/// Needs `email` and `company` columns (any order), `name` is optional. Any bad row fails the whole file, before anything is sent
pub fn parse_csv(input: impl Read) -> Result<Vec<Recruiter>> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(input);
    let cols: Vec<String> = reader
        .headers()
        .context("could not read the CSV header")?
        .iter()
        .map(|c| c.trim_start_matches('\u{feff}').to_lowercase())
        .collect();
    let column = |name: &str| cols.iter().position(|c| c == name);
    let email_idx = column("email").context("CSV header must include an 'email' column")?;
    let company_idx = column("company").context("CSV header must include a 'company' column")?;
    let name_idx = column("name");

    let mut recruiters = Vec::new();
    for row in reader.records() {
        let row =
            row.context("bad CSV row (a company name with a comma must be in double quotes)")?;
        let email = row.get(email_idx).unwrap_or_default();
        let company = row.get(company_idx).unwrap_or_default();
        // Quotes the row, because the csv crate's line numbers count from before any skipped blank lines
        let text = || row.iter().collect::<Vec<_>>().join(",");
        if email.is_empty() {
            bail!("CSV row '{}' has no email", text());
        }
        if company.is_empty() {
            bail!("CSV row '{}' has no company", text());
        }
        let name = name_idx.and_then(|i| row.get(i)).unwrap_or_default();
        recruiters.push(Recruiter {
            email: email.into(),
            company: company.into(),
            name: name.into(),
        });
    }
    Ok(recruiters)
}

/// The email from `subject.txt` and `body.md`, with `{{Greeting}}` and `{{Company}}` still unfilled
pub struct Template {
    subject: String,
    text: String,
    html: String,
}

impl Template {
    pub fn load(dir: &Path) -> Result<Self> {
        let read = |name: &str| {
            let path = dir.join(name);
            std::fs::read_to_string(&path)
                .with_context(|| format!("could not read {}", path.display()))
        };
        Self::parse(&read("subject.txt")?, &read("body.md")?)
    }

    fn parse(subject: &str, body: &str) -> Result<Self> {
        let (subject, body) = (subject.trim(), body.trim());
        if subject.is_empty() || subject.lines().count() > 1 {
            bail!("subject.txt must be a single line");
        }
        if body.is_empty() {
            bail!("body.md is empty");
        }
        for text in [subject, body] {
            let leftover = text.replace("{{Greeting}}", "").replace("{{Company}}", "");
            if leftover.contains("{{") {
                bail!("Unknown placeholder in the email template, only {{{{Greeting}}}} and {{{{Company}}}} exist: {}", text);
            }
        }

        // Keeps each line of body.md on its own line, like the signature
        let events = Parser::new(body).map(|e| {
            if e == Event::SoftBreak {
                Event::HardBreak
            } else {
                e
            }
        });
        let mut rendered = String::new();
        html::push_html(&mut rendered, events);

        Ok(Self {
            subject: subject.into(),
            // shortcut: only removes the bold and link marks body.md uses, parse the Markdown if it grows headings or tables
            text: body.replace("**", "").replace(['<', '>'], ""),
            html: rendered,
        })
    }

    pub fn render(&self, recruiter: &Recruiter) -> OutgoingMessage {
        let greeting = if recruiter.name.is_empty() {
            "Hi".to_string()
        } else {
            format!("Hi {}", recruiter.name)
        };
        let fill = |text: &str, greeting: &str, company: &str| {
            text.replace("{{Greeting}}", greeting)
                .replace("{{Company}}", company)
        };
        let html = fill(
            &self.html,
            &escape_html(&greeting),
            &escape_html(&recruiter.company),
        );
        OutgoingMessage {
            to: recruiter.email.clone(),
            subject: fill(&self.subject, &greeting, &recruiter.company),
            body: fill(&self.text, &greeting, &recruiter.company),
            // Must stay last: Gmail collapses an ending it has seen before behind "...", so each email needs a unique one
            html: Some(format!(
                "{}<span style=\"display:none\">{}</span>",
                html,
                uuid::Uuid::new_v4()
            )),
        }
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Vec<Recruiter>> {
        parse_csv(text.as_bytes())
    }

    fn recruiter(company: &str, name: &str) -> Recruiter {
        Recruiter {
            email: "a@b.com".into(),
            company: company.into(),
            name: name.into(),
        }
    }

    #[test]
    fn reads_columns_by_header_and_handles_spreadsheet_exports() {
        let rows = parse(
            "\u{feff}Name,Company,Email\nPriya,\"Acme, Inc\",a@acme.com\n\n,FITT, b@fitt.com \n",
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].name.as_str(), rows[0].company.as_str()),
            ("Priya", "Acme, Inc")
        );
        assert_eq!(
            (rows[1].email.as_str(), rows[1].name.as_str()),
            ("b@fitt.com", "")
        );
    }

    #[test]
    fn name_column_is_optional() {
        assert_eq!(parse("email,company\na@b.com,Acme\n").unwrap()[0].name, "");
    }

    #[test]
    fn rejects_rows_that_would_send_a_broken_email() {
        let blank_company = parse("email,company,name\n\na@b.com,,Priya\n")
            .err()
            .unwrap()
            .to_string();
        assert_eq!(blank_company, "CSV row 'a@b.com,,Priya' has no company");
        assert!(parse("email,company,name\n,Acme,Priya\n").is_err());
        assert!(parse("email,company,name\na@b.com,Acme, Inc,Priya\n").is_err());
        assert!(parse("company,name\nAcme,Priya\n").is_err());
    }

    #[test]
    fn one_markdown_body_gives_html_and_plain_text() {
        let body = "{{Greeting}},\n\n- **Title** - at {{Company}}\n\nBest,\nMe\n\nGitHub: <https://x.dev>\n";
        let message = Template::parse("Role at {{Company}}\n", body)
            .unwrap()
            .render(&recruiter("AT&T <Labs>", "Priya"));

        assert_eq!(message.subject, "Role at AT&T <Labs>");
        assert_eq!(
            message.body,
            "Hi Priya,\n\n- Title - at AT&T <Labs>\n\nBest,\nMe\n\nGitHub: https://x.dev"
        );

        let html = message.html.unwrap();
        assert!(html.contains("<p>Hi Priya,</p>"));
        assert!(html.contains("<li><strong>Title</strong> - at AT&amp;T &lt;Labs&gt;</li>"));
        assert!(html.contains("Best,<br />\nMe"));
        assert!(html.contains("<a href=\"https://x.dev\">https://x.dev</a>"));
    }

    #[test]
    fn greeting_falls_back_to_hi() {
        let template = Template::parse("s", "{{Greeting}},").unwrap();
        assert_eq!(template.render(&recruiter("X", "")).body, "Hi,");
    }

    #[test]
    fn every_send_has_a_unique_ending() {
        let template = Template::parse("s", "b").unwrap();
        assert_ne!(
            template.render(&recruiter("X", "")).html,
            template.render(&recruiter("X", "")).html
        );
    }

    #[test]
    fn rejects_templates_that_would_send_badly() {
        assert!(Template::parse("", "b").is_err());
        assert!(Template::parse("one\ntwo", "b").is_err());
        assert!(Template::parse("s", "  ").is_err());
        assert!(Template::parse("Role at {{Comapny}}", "b").is_err());
        assert!(Template::parse("s", "Hi {{Name}}").is_err());
    }

    #[test]
    fn shipped_template_is_valid() {
        let message = Template::load(Path::new("email"))
            .unwrap()
            .render(&recruiter("FITT", "Priya"));
        assert_eq!(message.subject, "Engineering Role at FITT");
        assert!(message.body.starts_with("Hi Priya,\n"));
        assert_eq!(message.html.unwrap().matches("<li><strong>").count(), 4);
    }
}
