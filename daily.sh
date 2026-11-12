#!/usr/bin/env bash
# Run send_campaign on a CSV every day via cron.
#   ./daily.sh 09:00 job_list/leads.csv   set the time and CSV
#   ./daily.sh 10:30                      change only the time
#   ./daily.sh                            show the current schedule
#   ./daily.sh off                        stop the daily run
# Times are in $MAILBUZZ_TZ (default Asia/Kolkata); cron itself runs in system time.
set -euo pipefail
cd "$(dirname "$0")"

TAG="# mailbuzz-daily"
ZONE="${MAILBUZZ_TZ:-Asia/Kolkata}"
current() { crontab -l 2>/dev/null | grep -F "$TAG" || true; }
others() { crontab -l 2>/dev/null | grep -vF "$TAG" || true; }

case "${1:-}" in
  "")
    line=$(current)
    [[ -n $line ]] && echo "$line" || echo "No daily run scheduled."
    ;;
  off)
    others | crontab -
    echo "Daily run removed."
    ;;
  *)
    time=$1
    [[ $time =~ ^([01][0-9]|2[0-3]):[0-5][0-9]$ ]] || { echo "Time must be HH:MM (24h), got '$time'" >&2; exit 1; }
    csv=${2:-$(current | sed -n "s/.*send_campaign '\([^']*\)'.*/\1/p")}
    [[ -n $csv ]] || { echo "Give a CSV: ./daily.sh $time path/to.csv" >&2; exit 1; }
    [[ $csv != *"'"* ]] || { echo "CSV path can't contain a single quote" >&2; exit 1; }
    [[ -f $csv ]] || { echo "No such CSV: $csv" >&2; exit 1; }
    [[ -x target/release/send_campaign ]] || { echo "Build first: cargo build --release" >&2; exit 1; }
    # shortcut: converts with today's UTC offset, rerun after a DST change if $ZONE has DST.
    min_hour=$(date -d "TZ=\"$ZONE\" $time" +"%-M %-H")
    line="$min_hour * * * cd '$PWD' && target/release/send_campaign '$csv' >> daily.log 2>&1 $TAG"
    { others; echo "$line"; } | crontab -
    echo "Runs every day at $time $ZONE on $csv (log: daily.log)."
    ;;
esac
