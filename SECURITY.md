# Security

laoban is a community open-source project. It is not a SUSE product and is not covered by SUSE support.

## Reporting a vulnerability

Please **do not** open a public issue for a security problem. Report it privately through GitHub: **Security → Report a vulnerability** on this repository ([private vulnerability reporting](https://github.com/yansheng133/laoban/security/advisories/new)).

Include the laoban version, your Rancher and Kubernetes versions, what an attacker needs (for example, which role), and the steps to reproduce. You will get an answer within a week.

## What to keep in mind when running laoban

- **Who can change apps.** Anyone with the laoban user role (or `edit`) in the apps' namespace can change an app's code, by hand or through the AI. The generated apps are opened through the Rancher proxy, on Rancher's own address; treat an app as code written by those people, and give the laoban user role only to people you trust.
- **What goes to the AI.** Requests, opinions and the app's code are sent to the AI provider (Anthropic, or the local model you configure). Common kinds of secrets are masked first, but masking cannot catch everything: do not put secrets in requests or opinions.
- **What goes to GitHub.** Each app's `laoban.json` and README record the requests (masked) and the Rancher user IDs of the people who changed it. Use a private repository for real use.
- **Generated apps.** They run as non-root with a read-only file system, no service account token and resource limits, in a namespace enforced with Pod Security `restricted`. They have no NetworkPolicy yet: add one if the apps' namespace should not reach other services.
