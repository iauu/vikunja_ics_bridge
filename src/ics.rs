use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use ics::components::Property;
use ics::properties::{
    Categories, Created, Description, DtEnd, DtStart, LastModified, Priority, RRule, Status,
    Summary, Transp, URL,
};
use ics::{Event, ICalendar, escape_text};
use serde::{Deserialize, Deserializer};

use html2text::config;

use crate::config::Bridge;
use crate::vikunja::Task;

const PRODID: &str = "-//vikunja_ics_conv//Vikunja ICS bridge//EN";

const DESCRIPTION_WIDTH: usize = 10_000;
const SECONDS_PER_DAY: i64 = 24 * 60 * 60;
const SECONDS_PER_WEEK: i64 = 7 * SECONDS_PER_DAY;

pub struct Calendar {
    pub body: String,
    pub event_count: usize,
    pub skipped: usize,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct DescriptionDirectives {
    #[serde(rename = "end", deserialize_with = "deserialize_optional_end_of_day_utc")]
    recurrence_until: Option<DateTime<Utc>>,
}

struct ParsedDescription {
    text: String,
    directives: DescriptionDirectives,
}

/// Render tasks that have both a start and an end date as VEVENTs.
pub fn render(bridge: &Bridge, tasks: &[Task]) -> Calendar {
    let now = stamp(&Utc::now());
    let domain = bridge.uid_domain();
    let task_base = bridge.base.trim().trim_end_matches('/');

    let mut cal = ICalendar::new("2.0", PRODID);
    cal.push(Property::new("CALSCALE", "GREGORIAN"));
    cal.push(Property::new("METHOD", "PUBLISH"));
    let cal_name = bridge
        .name
        .clone()
        .unwrap_or_else(|| format!("Vikunja project {}", bridge.project_id));
    cal.push(Property::new("X-WR-CALNAME", escape_text(cal_name)));
    cal.push(Property::new("X-WR-TIMEZONE", "UTC"));

    let mut event_count = 0;
    let mut skipped = 0;

    for task in tasks {
        let (Some(start), Some(end)) = (task.start_date, task.end_date) else {
            skipped += 1;
            continue;
        };
        let end = if end > start {
            end
        } else {
            start + chrono::Duration::minutes(1)
        };

        let mut ev = Event::new(
            format!("vikunja-{}-{}@{}", bridge.project_id, task.id, domain),
            now.clone(),
        );
        ev.push(DtStart::new(stamp(&start)));
        ev.push(DtEnd::new(stamp(&end)));

        let summary = if task.title.trim().is_empty() {
            format!("Task #{}", task.id)
        } else {
            task.title.clone()
        };
        ev.push(Summary::new(escape_text(summary)));

        let parsed_description = parse_description_and_strip_directives(&task.description);
        let mut notes = parsed_description.text;
        if let Some(due) = task.due_date {
            let due_line = format!("Due: {}", due.to_rfc3339_opts(SecondsFormat::Secs, true));
            notes = if notes.is_empty() {
                due_line
            } else {
                format!("{notes}\n\n{due_line}")
            };
        }
        if !notes.is_empty() {
            ev.push(Description::new(escape_text(notes)));
        }
        if let Some(rrule) = recurrence_rrule(task, &parsed_description.directives) {
            ev.push(RRule::new(rrule));
        }

        ev.push(URL::new(format!("{task_base}/tasks/{}", task.id)));

        if let Some(labels) = &task.labels {
            let cats: Vec<String> = labels
                .iter()
                .map(|l| l.title.trim())
                .filter(|t| !t.is_empty())
                .map(|t| escape_text(t).into_owned())
                .collect();
            if !cats.is_empty() {
                ev.push(Categories::new(cats.join(",")));
            }
        }

        if let Some(p) = ics_priority(task.priority) {
            ev.push(Priority::new(p.to_string()));
        }

        ev.push(Status::confirmed());
        if task.done {
            ev.push(Property::new("X-VIKUNJA-DONE", "TRUE"));
        }
        ev.push(Transp::opaque());

        if let Some(created) = task.created {
            ev.push(Created::new(stamp(&created)));
        }
        if let Some(updated) = task.updated {
            ev.push(LastModified::new(stamp(&updated)));
        }
        let identifier = task.identifier.trim();
        if !identifier.is_empty() {
            ev.push(Property::new(
                "X-VIKUNJA-IDENTIFIER",
                escape_text(identifier),
            ));
        }
        if task.percent_done > 0.0 {
            let pct = (task.percent_done * 100.0).round().clamp(0.0, 100.0) as i64;
            ev.push(Property::new("X-VIKUNJA-PERCENT-DONE", pct.to_string()));
        }

        cal.add_event(ev);
        event_count += 1;
    }

    Calendar {
        body: cal.to_string(),
        event_count,
        skipped,
    }
}

fn stamp(dt: &DateTime<Utc>) -> String {
    dt.format("%Y%m%dT%H%M%SZ").to_string()
}

fn ics_priority(p: i32) -> Option<u8> {
    match p {
        5 => Some(1),
        4 => Some(2),
        3 => Some(4),
        2 => Some(6),
        1 => Some(8),
        _ => None,
    }
}

fn html_to_text(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }

    match config::plain().string_from_read(html.as_bytes(), DESCRIPTION_WIDTH) {
        Ok(text) => text.trim().to_string(),
        Err(e) => {
            tracing::warn!("cannot render description as text: {e}");
            String::new()
        }
    }
}

fn parse_description_and_strip_directives(raw_html: &str) -> ParsedDescription {
    parse_description_with_mode(raw_html, true)
}

fn parse_description_and_keep_directives(raw_html: &str) -> ParsedDescription {
    parse_description_with_mode(raw_html, false)
}

fn parse_description_with_mode(raw_html: &str, strip_directive_lines: bool) -> ParsedDescription {
    let notes = normalize_smart_quotes(&html_to_text(raw_html));
    if notes.is_empty() {
        return ParsedDescription {
            text: notes,
            directives: DescriptionDirectives::default(),
        };
    }

    let mut raw_fields = std::collections::BTreeMap::new();
    let mut kept_lines = Vec::new();
    for line in notes.lines() {
        let mut is_directive_line = false;
        if let Some((key, value)) = parse_directive_line(line) {
            if is_recognized_directive_key(&key) {
                raw_fields.insert(key, value);
                is_directive_line = true;
            }
        }
        if !strip_directive_lines || !is_directive_line {
            kept_lines.push(line);
        }
    }

    let directives = parse_description_directives(&raw_fields);

    ParsedDescription {
        text: kept_lines.join("\n").trim().to_string(),
        directives,
    }
}

fn parse_description_directives(
    raw_fields: &std::collections::BTreeMap<String, String>,
) -> DescriptionDirectives {
    let mut table = toml::map::Map::new();
    for (key, value) in raw_fields {
        table.insert(key.clone(), toml::Value::String(value.clone()));
    }
    toml::Value::Table(table).try_into().unwrap_or_default()
}

fn is_recognized_directive_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("end")
}

fn parse_directive_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let (key, value) = line.split_once(':')?;
    let key = key.trim().to_ascii_lowercase();
    if key.is_empty() {
        return None;
    }

    let value = strip_optional_comment_suffix(value.trim());
    if value.is_empty() {
        return None;
    }
    Some((key, value.to_string()))
}

fn strip_optional_comment_suffix(value: &str) -> &str {
    let value = value.trim_end();
    let Some(start) = value.rfind("(#") else {
        return value;
    };
    let suffix = value[start..].trim();
    if suffix.starts_with("(#") && suffix.ends_with(')') {
        value[..start].trim_end()
    } else {
        value
    }
}

fn normalize_smart_quotes(input: &str) -> String {
    input
        .replace(['\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'], "'")
        .replace(['\u{201C}', '\u{201D}', '\u{201E}', '\u{201F}'], "\"")
}

fn deserialize_optional_end_of_day_utc<'de, D>(
    deserializer: D,
) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let raw = raw.trim();

    let mut parts = raw.split('/');
    let day: u32 = match parts.next().and_then(|v| v.trim().parse().ok()) {
        Some(v) => v,
        None => return Ok(None),
    };
    let month: u32 = match parts.next().and_then(|v| v.trim().parse().ok()) {
        Some(v) => v,
        None => return Ok(None),
    };
    let year: i32 = match parts.next().and_then(|v| v.trim().parse().ok()) {
        Some(v) => v,
        None => return Ok(None),
    };
    if parts.next().is_some() {
        return Ok(None);
    }

    let Some(date) = NaiveDate::from_ymd_opt(year, month, day) else {
        return Ok(None);
    };
    Ok(date.and_hms_opt(23, 59, 59).map(|dt| dt.and_utc()))
}

fn recurrence_rrule(task: &Task, directives: &DescriptionDirectives) -> Option<String> {
    let repeat_after = task.repeat_after?;
    if repeat_after <= 0 {
        return None;
    }

    let (freq, interval) = if repeat_after % SECONDS_PER_WEEK == 0 {
        ("WEEKLY", repeat_after / SECONDS_PER_WEEK)
    } else if repeat_after % SECONDS_PER_DAY == 0 {
        ("DAILY", repeat_after / SECONDS_PER_DAY)
    } else {
        return None;
    };
    if interval <= 0 {
        return None;
    }

    let mut rule = format!("FREQ={freq};INTERVAL={interval}");
    if let Some(until) = directives.recurrence_until.as_ref() {
        rule.push_str(";UNTIL=");
        rule.push_str(&stamp(until));
    }
    Some(rule)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_with_repeat(repeat_after: Option<i64>) -> Task {
        Task {
            id: 1,
            identifier: String::new(),
            title: String::new(),
            description: String::new(),
            done: false,
            priority: 0,
            percent_done: 0.0,
            start_date: None,
            end_date: None,
            due_date: None,
            created: None,
            updated: None,
            labels: None,
            repeat_after,
            repeat_mode: 0,
        }
    }

    #[test]
    fn strips_directive_line_and_parses_until() {
        let parsed = parse_description_and_strip_directives(
            "<p>end: 31/12/2026 (# optional note)</p><p>Details</p>",
        );
        assert_eq!(parsed.text, "Details");
        assert_eq!(
            parsed
                .directives
                .recurrence_until
                .as_ref()
                .map(stamp)
                .as_deref(),
            Some("20261231T235959Z")
        );
    }

    #[test]
    fn keep_mode_keeps_directive_line() {
        let parsed = parse_description_and_keep_directives(
            "<p>end: 31/12/2026 (# optional note)</p><p>Details</p>",
        );
        assert!(parsed.text.contains("end: 31/12/2026"));
        assert!(parsed.text.contains("Details"));
    }

    #[test]
    fn creates_weekly_rrule_with_until() {
        let task = task_with_repeat(Some(2 * SECONDS_PER_WEEK));
        let parsed = parse_description_and_strip_directives("<p>end: 31/12/2026</p>");
        let rrule = recurrence_rrule(&task, &parsed.directives).expect("must build rule");
        assert_eq!(rrule, "FREQ=WEEKLY;INTERVAL=2;UNTIL=20261231T235959Z");
    }

    #[test]
    fn normalizes_smart_quotes() {
        let parsed = parse_description_and_keep_directives("<p>“alpha” and ‘beta’</p>");
        assert_eq!(parsed.text, "\"alpha\" and 'beta'");
    }
}
