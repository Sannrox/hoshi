# Consented bug reports

Status: Accepted

Source: [Issue #233](https://github.com/Sannrox/hoshi/issues/233)

## Context

Players need a useful issue draft without granting the game permission to send
reports. Server diagnostic attachments already require a separate admin scope.

## Decision

One local builder redacts the form and diagnostic report. The player previews
before confirming a new atomic report file. The file includes the prefilled
issue URL, so offline recovery needs no second durable write. Opening the browser
requires confirmation and honors the session's offline setting. Submission stays
with the player on the tracker page. No token or issue API is used.

The authenticated `/v1/bug` contract returns a bounded prefilled URL and optional
local report id. It never returns diagnostic contents or opens a browser.
`confirm` controls server-side file creation. A disabled destination returns 404;
a custom HTTPS template changes the destination. Diagnostic attachments remain
local under the existing admin-access boundary in ADR 0012.

## Consequences

CLI, TUI and clients share redaction, URL limits and crash signatures. A failed
browser launch leaves the report and URL usable. URL fields can be shortened;
the redacted local attachment retains the full input. Ordinary game credentials
do not gain access to server diagnostic configuration.

## Rejected alternatives

Automatic submission would add credentials and an upload trust boundary. A
second file for the URL would create a partial-publication recovery problem.
Returning the full server report to ordinary clients would bypass admin scope.
