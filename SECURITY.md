# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue: use GitHub's
**Security → Report a vulnerability** (private security advisory) on this repository. Include what
you found, how to reproduce it, and the impact you expect. We aim to acknowledge reports within
three working days and to agree on a fix and disclosure timeline with you.

## Scope

SiliconCommons runs untrusted Verilog (simulation in the API process, synthesis in containers) and
calls an AI provider with project content. The design of those boundaries, what was reviewed before
the pilot and the risks accepted for it are in [docs/security-review.md](docs/security-review.md)
and [docs/architecture.md](docs/architecture.md#trust-boundaries).

## Supported versions

There are no releases yet; fixes go to `main` and the pilot deployment.

## Development credentials

The accounts and secrets under `deploy/compose/` are for local development only and are publicly
known. Never use them anywhere reachable from another machine; the pilot deployment
(`deploy/pilot/setup.sh`) generates its own.
