# Security policy

## Reporting a vulnerability

Report a vulnerability in kartta privately, through GitHub's private vulnerability reporting on this repository: open
the repository's security tab and choose **Report a vulnerability**, as GitHub's
[Privately reporting a security vulnerability](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/report-privately)
describes. The report stays private between you and the maintainers.

Do not open a public issue, pull request or discussion for a vulnerability, and do not name it in one.

A report the maintainers can act on names:

- the artifact and its version: a crate, the static library, the Go module, the npm package or the MCP server;
- the platform and target it runs on;
- the input or the steps that reproduce it;
- what an attacker gains.

## How a fix ships

A fix ships as a new release of every artifact at once, as [`RELEASING.md`](/RELEASING.md) describes for a defect in a
published release, under [ADR 4.55](/design-docs/adr/4.55-release-workflow.md): the next patch version, with the
affected version retracted in the Go module, yanked on crates.io and deprecated on npm.
