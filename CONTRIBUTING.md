# Contributing to Wafersmith

Wafersmith is an initiative of DigitalTwin Technology GmbH. Thank you for helping. This page
says how changes get in; [docs/development.md](docs/development.md) has the technical details and
[docs/getting-started.md](docs/getting-started.md) sets up your machine.

## Before you start

- For anything bigger than a small fix, open an issue first (bug or feature template) so we can
  agree on the approach.
- Security problems: don't open a public issue; follow [SECURITY.md](SECURITY.md).

## Making a change

1. Branch from `main`: `feature/<topic>`, `fix/<topic>` or `docs/<topic>`.
2. Keep the change focused; one pull request does one thing.
3. Every change ships with:
   - **tests** (unit tests next to the code, API tests in `crates/wafer-api/tests/`);
   - **documentation**: the relevant page under `docs/`, the [API reference](docs/api.md) and
     [configuration](docs/configuration.md) when they change, and an entry in
     [CHANGELOG.md](CHANGELOG.md) under "Unreleased";
   - **plain-language messages** for anything a user sees: Wafersmith is for engineers who
     aren't chip-design experts.
4. Run the checks CI runs ([docs/development.md](docs/development.md#checks-what-ci-runs)).
5. Open a pull request and fill in the template. CI must pass; a maintainer reviews and merges.

## Commit messages

```
Short imperative subject, at most 72 characters

What changed and why, wrapped at about 100 characters. Mention user-visible
behaviour, limits and anything reviewers should check.
```

## Code style

- Rust: `cargo fmt` (`rustfmt.toml`), `cargo clippy -D warnings`, edition 2024.
- TypeScript: strict, typechecked, no unused code.
- Comments explain why; module headers say what a module is for.
- Generated files (example testbenches, the chip's top level, adapters) are never edited by hand.

## Licence

By contributing you agree that your contribution is licensed under the
[Apache License 2.0](LICENSE), like the rest of the project. Keep the
`SPDX-License-Identifier: Apache-2.0` header in source files that have one.
