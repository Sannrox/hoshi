# Support reports

Run `hoshi report` or select **Support report** from the title menu. In the
Help overlay, press `r`. Review the scrollable preview, then choose whether to
write it. Nothing is uploaded. Attach the resulting file to a bug report after
reviewing it yourself.

```sh
hoshi report
hoshi report --format archive --log-lines 200
hoshi report --include-save
hoshi report --out report.md --yes
```

Reports go into `reports/` beside the selected save unless `--out` names a
new file. Existing files are never overwritten. `--yes` skips confirmation;
`--open-issue` prints a bug-report URL for you to open.

The report contains build and contract versions, terminal capabilities,
redacted configuration, environment variable names, save metadata and a hash,
recent history, bounded logs, recent crashes, and the active balance hash.
Unavailable save details are reported without migrating or repairing the save.
Update status uses the local cache and does not check for updates. An explicitly
configured numeric loopback `serve_bind` allows a bounded, unauthenticated
`/healthz` probe. Other configured addresses are not contacted.

An archive adds the redacted diagnostic text files. Save contents are excluded
unless you explicitly select **Include save** or pass `--include-save`. That
option requires an archive and includes the galaxy seed and world state. A
SQLite attachment is a consistent snapshot including committed journal data.

Secrets, bearer credentials, private URLs, hostnames and non-loopback addresses
are redacted before diagnostic text is persisted. Home paths and user names
are replaced. Redaction cannot infer every personal detail from arbitrary
free text, so review the preview and attachment before sharing.

## Logs and crashes

Logs are stored in `logs/hoshi.log` or `logs/serve.log` beside the save. Each
retains up to five files of approximately 1 MiB. Set `HOSHI_LOG` or `log_level`
in `config.toml` to `error`, `warn`, `info` or `debug`; the default is `info`.

A panic writes a redacted file in `crashes/` with a backtrace and signature.
The next title screen offers to collect a report. Reports include crash files
from the preceding seven days. Logs, reports and crashes remain local until
you remove them.

## Server reports

`GET /v1/admin/support-report` returns redacted Markdown plus live uptime,
client, request and rate-limit counters. Set `serve_admin_token` in config or
`HOSHI_SERVE_ADMIN_TOKEN` to a separate admin bearer credential. Ordinary game
tokens have no admin access. Token-file entries may use `SECRET admin:PATH`
to grant access to the named save. With authentication disabled, both the bind
address and requesting peer must be loopback.

Set `serve_support_report = false` to disable this endpoint (404). Responses
use `Cache-Control: no-store`. They never include a save attachment.
