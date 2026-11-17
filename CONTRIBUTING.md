# Contributing to rcli

## Issues

Issues are welcome on this repo: [open one](https://github.com/EigentLab/remote-cli/issues/new). For a bug, give the version (`rcli --version`), the OS and whether rcli runs in User Mode or Service Mode, what you ran and what you saw. A security problem goes through [SECURITY.md](SECURITY.md) instead, not an issue.

## Pull requests

This repo is a snapshot of a private development repo ([ADR 0014](docs/adr/0014-public-repo-is-a-snapshot-of-a-private-dev-repo.md)). Its history is one commit per release, so a pull request is not merged here:

1. Open your pull request here as usual.
2. If we take it, we apply your patch in the development repo with you as the author, and close the pull request with a note.
3. Your change appears here in the next snapshot, and the release notes credit you.

For anything larger than a fix, open an issue first so we can agree on the approach before you write it.

By contributing you agree that your contribution is licensed under the [AGPL-3.0](LICENSE), like the rest of rcli.

## Working on the code

- Read [GLOSSARY.md](GLOSSARY.md) and use its words; the decisions behind the design are in [docs/adr](docs/adr).
- Build: `cargo build -p rcli` (add `--features gui` for the rcli window). More in [Build from source](https://rcli.eigentlab.com/en/build/).
- Test what you touched: `cargo test -p <crate>`, with a filter for one test. The system tests in `crates/testkit` start Postgres in Docker. The Dashboard's Playwright tests are in `dashboard/tests` (`cd dashboard && pnpm install && npx playwright test tests/<spec>.ts`).
- Add a test for the behaviour you change, through the CLI, the Controller API or the Dashboard rather than internal types, and extend an existing test file before adding a new one.
- Run `cargo fmt` and `cargo clippy` before you open the pull request.
- Docs live in the site, `site/src/content/docs/en/` and `vi/`. If you change what a command does, change its page too.
