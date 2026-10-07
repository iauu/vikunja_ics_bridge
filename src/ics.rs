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
    #[serde(
        rename = "end",
        deserialize_with = "deserialize_optional_end_of_day_utc"
    )]
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

        let description_text = normalize_smart_quotes(&html_to_text(&task.description));
        let parsed_description = parse_description(&description_text, true);
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

fn parse_description(desc: &str, strip_directive_lines: bool) -> ParsedDescription {
    if desc.is_empty() {
        return ParsedDescription {
            text: String::new(),
            directives: DescriptionDirectives::default(),
        };
    }

    let mut directives = DescriptionDirectives::default();
    let mut kept_lines = Vec::new();
    for line in desc.lines() {
        let mut parsed_as_directive = false;
        if let Some(line_directives) = parse_directives_from_toml_line(line) {
            directives.merge(line_directives);
            parsed_as_directive = true;
        }
        if !strip_directive_lines || !parsed_as_directive {
            kept_lines.push(line);
        }
    }

    ParsedDescription {
        text: kept_lines.join("\n").trim().to_string(),
        directives,
    }
}

fn parse_directives_from_toml_line(line: &str) -> Option<DescriptionDirectives> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let directives: DescriptionDirectives = toml::from_str(line).ok()?;
    if directives.recurrence_until.is_some() {
        Some(directives)
    } else {
        None
    }
}

fn normalize_smart_quotes(input: &str) -> String {
    input
        .replace(['\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'], "'")
        .replace(['\u{201C}', '\u{201D}', '\u{201E}', '\u{201F}'], "\"")
}

impl DescriptionDirectives {
    fn merge(&mut self, other: DescriptionDirectives) {
        if other.recurrence_until.is_some() {
            self.recurrence_until = other.recurrence_until;
        }
    }
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
