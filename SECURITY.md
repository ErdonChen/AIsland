# Security Policy

## Supported versions

Only the latest published AIsland Community Edition release receives security fixes. Until a signed binary is published, security reports should reference the current `main` branch or an exact commit.

| Version | Supported |
| --- | --- |
| Latest release / `main` | Yes |
| Older releases | No |

## Report a vulnerability privately

Do not open a public Issue for a suspected vulnerability.

Use [GitHub Private Vulnerability Reporting](https://github.com/ErdonChen/AIsland/security/advisories/new). Include the affected version or commit, impact, reproduction steps, and any suggested mitigation. Do not include real agent conversations, credentials, private logs, or another person's data.

If private reporting is temporarily unavailable, contact `aisland_support@163.com` only to request a private reporting channel. Do not send exploit details until a private channel is confirmed.

## Response targets

These are good-faith targets, not a service-level agreement:

- acknowledge a report within 7 calendar days;
- provide an initial reproduction and severity assessment within 14 calendar days;
- aim to fix or mitigate critical and high-severity issues within 30 days;
- aim to address other valid issues within 90 days;
- keep the reporter updated privately when a target cannot be met.

Please coordinate disclosure until a fix or mitigation is available. The maintainer may publish a GitHub Security Advisory after affected users can update safely.

## Scope and safe testing

Security reports may cover local data exposure, unsafe Hook configuration changes, updater or release integrity, privilege boundaries, and vulnerabilities in AIsland code. Test only systems and data you own or are authorized to use. Do not disrupt third-party services, access another user's data, or publish unredacted private content.

There is currently no bug bounty or guaranteed monetary reward.
