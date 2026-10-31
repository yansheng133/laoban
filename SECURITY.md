# Security

laoban is a community open-source project. It is not a SUSE product and is not covered by SUSE support.

## Reporting a vulnerability

Please **do not** open a public issue for a security problem. Report it privately through GitHub: **Security → Report a vulnerability** on this repository ([private vulnerability reporting](https://github.com/yansheng133/laoban/security/advisories/new)).

Include the laoban version, your Rancher and Kubernetes versions, what an attacker needs (for example, which role), and the steps to reproduce. You will get an answer within a week.

## What to keep in mind when running laoban

- **Who can change apps.** Anyone with the laoban user role (or `edit`) in the apps' namespace can change an app's code, by hand or through the AI. Treat an app as code written by those people, and give the laoban user role only to people you trust.
- **Where apps are opened.** Set the chart value `apps.url` so each app opens on its own address (an Ingress per app). Without it, apps are opened through the Rancher proxy, on Rancher's own address, and an app's page runs with the viewer's Rancher login: a malicious or manipulated app can read and change Rancher resources as whoever opens it, including an administrator. The page warns next to every such link. With `apps.url`, the apps are no longer behind Rancher's permission check; limit who can reach their addresses with the network, or require a login with your ingress controller's authentication through `apps.ingressAnnotations`.
- **What goes to the AI.** Requests, opinions and the app's code are sent to the AI provider (Anthropic, or the local model you configure). Common kinds of secrets are masked first, but masking cannot catch everything: do not put secrets in requests or opinions.
- **What goes to GitHub.** Each app's `laoban.json` and README record the requests (masked) and the Rancher user IDs of the people who changed it. Use a private repository for real use.
- **Generated apps.** They run as non-root with a read-only file system, no service account token and resource limits, in a namespace enforced with Pod Security `restricted`. With `apps.networkPolicy` (the default) each app gets NetworkPolicies: its API accepts only the app's own web pods, and both reach nothing but the app's API and the cluster DNS. This needs a CNI that enforces NetworkPolicy.
