# Security policy

rcli drives the screen, the keyboard and a root shell on other people's machines, so we take reports seriously.

## Reporting a vulnerability

Report it privately through GitHub's private vulnerability reporting:
**[Report a vulnerability](https://github.com/EigentLab/remote-cli/security/advisories/new)** (the Security tab of this repo).

Please do not open a public issue, pull request or discussion about it. Include what you can of:

- the version (`rcli --version`, and the Server's version if it is involved) and the OS;
- what an attacker can do, and what they need first (network position, a Server Key, a Device Password, a User);
- steps to reproduce, or a proof of concept.

We answer in the advisory, work on the fix privately and publish the advisory with the release that fixes it, crediting you unless you ask us not to.

## Supported versions

Only the latest release is fixed. Update the Server and its Devices to it (the order is in [Upgrading](https://rcli.eigentlab.com/en/server/#upgrading)).

## What is in scope

The `rcli` binary (Agent and Controller), the Server, the Dashboard, the install scripts and the published images. Known limits that are documented in [Known limitations](https://rcli.eigentlab.com/en/limitations/) are not vulnerabilities, but tell us if one is worse than it says.
