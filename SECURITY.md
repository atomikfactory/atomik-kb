# Security policy

## Supported versions

Only the latest build is supported: the most recent [Nightly](../../releases/tag/nightly) and the tip of `main`.
Fixes land on `main` and ship in the next nightly; older builds do not receive backports.

## Reporting a vulnerability

Please report vulnerabilities privately, not in public issues or discussions.

Use GitHub's private vulnerability reporting: open the **Security** tab of this repository, choose
**Report a vulnerability**, and fill in the form. Only the maintainers can see the report.

Useful to include:

- what is affected (component, version or commit, platform) and the impact you expect;
- steps to reproduce, or a proof of concept, with the smallest input that triggers it;
- whether the issue needs a malicious sync server, a malicious shared-space member, a crafted file or link
  (import, `atomik-kb://` URL, MCP client), or local access to the machine;
- any suggested fix, and whether you plan to publish your findings and when.

You can expect an acknowledgement within a few days. We will keep you informed while the issue is validated and
fixed, and credit you in the advisory if you wish.

## Scope

In scope: the desktop app, `kb-server` (the sync server), the web viewer, the Rust crates and the TypeScript packages
in this repository, and the release and CI configuration. Of particular interest are anything that weakens the
end-to-end encryption or the key handling, lets the sync server or another party read or forge content it should
not, escapes a sandbox or permission boundary (MCP, AI tool access, shared-space roles), or runs code through
imported or shared content.

Out of scope: vulnerabilities in third-party dependencies that have no impact here (report those upstream; dependency
advisories are also tracked with `cargo deny` and Dependabot), attacks that need an already-compromised device or
operating system account, denial of service through unreasonable resource use by a user's own data, social
engineering, and self-hosted deployments that ignore the documented setup (for example a `kb-server` exposed without
TLS).
