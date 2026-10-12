# vikunja_ics_conv

An ICS bridge for [Vikunja](https://vikunja.io) by calling API with API key

## Configuration

Create `cred.json` in the root directory of the docker compose/binary.

```json
{
  "long_secret_key_for_ics_bridge": {
    "base": "https://vikunja.example.com",
    "project_id": 8,
    "filter": "done = false", // optional
    "api_key": "tk_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    "name": "Vikunja — Work" // optional
  }
}
```

## Running

```bash
cargo run --release
```

| Env var | Default | Purpose |
| --- | --- | --- |
| `CRED_PATH` | `cred.json` | Path to the config file. |
| `LISTEN_ADDR` | `127.0.0.1:8080` | Bind address. The Docker image defaults to `0.0.0.0:8080`. |

### Endpoints

| Route | Purpose |
| --- | --- |
| `GET /calendar/<secret>.ics` | The calendar. The `.ics` suffix is optional. |
| `GET /healthz` | Liveness probe, returns `ok`. |

### Recurrence exclusions

For a recurring task, put this TOML line in its description to omit individual
occurrences from the generated calendar:

```toml
skip = "2026-12-25;2027-01-01;2027-04-02"
```

Dates may be written as `YYYY-MM-DD`, `DD-MM-YYYY`, `DD/MM/YYYY`, or
`YYYY/MM/DD`. The bridge emits them as `EXDATE` values at the event's start
time. Combine this with the existing `tz` directive when the recurrence uses a
local timezone.
