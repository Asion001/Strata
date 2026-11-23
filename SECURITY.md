# Security policy

## Supported versions

Strata is alpha software. Security fixes land on `main` and in the latest
release only.

| Version          | Supported |
| ---------------- | --------- |
| `main` / latest  | yes       |
| older releases   | no        |

## Reporting a vulnerability

Please **do not open a public issue**. Report it privately through
[GitHub private vulnerability reporting](https://github.com/Asion001/Strata/security/advisories/new).

Include what you can of:

- the affected version or commit,
- the component (server API, storage backend, web app, deployment files),
- steps to reproduce or a proof of concept,
- the impact you expect (data exposure, data loss, privilege escalation, …).

## What to expect

| Step                               | Target            |
| ---------------------------------- | ----------------- |
| Acknowledgement of your report     | within 3 days     |
| First assessment and severity      | within 7 days     |
| Fix for critical or high severity  | within 30 days    |
| Public advisory                    | after a fix ships |

We credit reporters in the advisory unless you'd rather stay anonymous.
Strata is maintained by volunteers, so these are targets rather than guarantees.

## Scope

In scope: the Strata server, web app, sync client and the deployment files in
this repository. Out of scope: vulnerabilities in your own reverse proxy,
storage provider or operating system, and denial of service through volume
alone.
