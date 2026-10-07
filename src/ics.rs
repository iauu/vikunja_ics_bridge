use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use ics::components::Property;
use ics::properties::{
    Categories, Created, Description, DtEnd, DtStart, LastModified, Priority, RRule, Status,
    Summary, Transp, URL,
};
use ics::{Event, ICalendar, escape_text};

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

#[derive(Clone, Copy)]
struct RenderOptions {
    keep_directive_lines: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            keep_directive_lines: true,
        }
    }
}

#[derive(Default)]
struct DescriptionDirectives {
    recurrence_until: Option<DateTime<Utc>>,
}

struct ParsedDescription {
    text: String,
    directives: DescriptionDirectives,
}

/// Render tasks that have both a start and an end date as VEVENTs.
pub fn render(bridge: &Bridge, tasks: &[Task]) -> Calendar {
    render_with_options(bridge, tasks, RenderOptions::default())
}

fn render_with_options(bridge: &Bridge, tasks: &[Task], options: RenderOptions) -> Calendar {
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

        let parsed_description = parse_description(&task.description, options);
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

fn parse_description(raw_html: &str, options: RenderOptions) -> ParsedDescription {
    let notes = html_to_text(raw_html);
    if notes.is_empty() {
        return ParsedDescription {
            text: notes,
            directives: DescriptionDirectives::default(),
        };
    }

    let mut directives = DescriptionDirectives::default();
    let mut kept_lines = Vec::new();
    for line in notes.lines() {
        let mut keep_line = true;
        if let Some(until) = parse_recurrence_end_directive(line) {
            directives.recurrence_until = Some(until);
            keep_line = options.keep_directive_lines;
        }
        if keep_line {
            kept_lines.push(line);
        }
    }

    ParsedDescription {
        text: kept_lines.join("\n").trim().to_string(),
        directives,
    }
}

fn parse_recurrence_end_directive(line: &str) -> Option<DateTime<Utc>> {
    let line = line.trim();
    let (key, value) = line.split_once(':')?;
    if !key.trim().eq_ignore_ascii_case("end") {
        return None;
    }

    let value = value.trim();
    let date_token = value
        .split_once('(')
        .map_or(value, |(before, _)| before)
        .trim();

    let mut parts = date_token.split('/');
    let day: u32 = parts.next()?.trim().parse().ok()?;
    let month: u32 = parts.next()?.trim().parse().ok()?;
    let year: i32 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }

    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let dt = date.and_hms_opt(23, 59, 59)?.and_utc();
    Some(dt)
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
    use chrono::TimeZone;

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
    fn parses_end_directive_with_comment() {
        let until = parse_recurrence_end_directive("end: 31/12/2026 (# optional note)")
            .expect("must parse end directive");
        assert_eq!(stamp(&until), "20261231T235959Z");
    }

    #[test]
    fn creates_weekly_rrule_with_until() {
        let task = task_with_repeat(Some(2 * SECONDS_PER_WEEK));
        let directives = DescriptionDirectives {
            recurrence_until: Some(Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap()),
        };
        let rrule = recurrence_rrule(&task, &directives).expect("must build rule");
        assert_eq!(rrule, "FREQ=WEEKLY;INTERVAL=2;UNTIL=20261231T235959Z");
    }

    #[test]
    fn keeps_directive_lines_by_default() {
        let parsed = parse_description(
            "<p>end: 31/12/2026</p><p>Details</p>",
            RenderOptions::default(),
        );
        assert!(parsed.text.contains("end: 31/12/2026"));
        assert!(parsed.text.contains("Details"));
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
}
