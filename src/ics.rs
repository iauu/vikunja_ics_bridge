use chrono::{DateTime, SecondsFormat, Utc};
use ics::components::Property;
use ics::properties::{
    Categories, Created, Description, DtEnd, DtStart, LastModified, Priority, Status, Summary,
    Transp, URL,
};
use ics::{Event, ICalendar, escape_text};

use html2text::config;

use crate::config::Bridge;
use crate::vikunja::Task;

const PRODID: &str = "-//vikunja_ics_conv//Vikunja ICS bridge//EN";

const DESCRIPTION_WIDTH: usize = 10_000;

pub struct Calendar {
    pub body: String,
    pub event_count: usize,
    pub skipped: usize,
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

        let mut notes = html_to_text(&task.description);
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
