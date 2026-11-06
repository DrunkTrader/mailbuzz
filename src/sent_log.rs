use std::collections::HashSet;
use std::io::Write;

/// One line per successful send, from any endpoint: `address<TAB>unix time`. Old lines with just an address still work. Survives restarts and cleared browsers.
const SENT_LOG: &str = "sent_log.txt";

pub fn load_sent() -> HashSet<String> {
    // First word of each line is the address; anything after it (timestamp, note) is ignored
    std::fs::read_to_string(SENT_LOG)
        .map(|t| {
            t.lines()
                .filter_map(|l| l.split_whitespace().next())
                .map(|a| a.to_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Sends logged in the last 24 hours, the window Gmail's daily limit is counted over
pub fn sent_last_24h() -> usize {
    let log = std::fs::read_to_string(SENT_LOG).unwrap_or_default();
    count_since(&log, unix_now().saturating_sub(24 * 60 * 60))
}

// Old lines with no timestamp don't count, they predate the timestamped format
fn count_since(log: &str, cutoff: u64) -> usize {
    log.lines()
        .filter_map(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        .filter(|&time| time >= cutoff)
        .count()
}

pub fn log_sent(address: &str) {
    // One write per line, so concurrent sends (/send-bulk-email) can't interleave
    let line = format!("{}\t{}\n", address.to_lowercase(), unix_now());
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(SENT_LOG)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = written {
        eprintln!("Could not record {} in {}: {}", address, SENT_LOG, e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_timestamped_lines_inside_the_window() {
        let log = "old@x.com\t100\nedge@x.com\t200\nnew@x.com\t300\nno-time@x.com\n\n";
        assert_eq!(count_since(log, 200), 2);
    }
}
