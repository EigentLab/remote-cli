# The Server only connects; Address Books hold the Devices

Every install used to appear on the Server for every User, and a Controller needed a User login to see more than one Device. Like RustDesk's open-source server, the Server now only connects Controllers to Devices: per Device it keeps the Device ID, the certificate fingerprint, the Device Password hash and whether it is online. A Device reaches a Controller only when someone types its Device ID and Device Password into an Address Book: local to the OS user for the CLI and GUI, per User on the Server for the Dashboard. Resource history, stored inventory, the per-Device audit log and Server-side Device management are removed. This supersedes the registry parts of ADR 0003 and ADR 0006; Users stay (ADR 0009) for the Dashboard and Server administration, with TOTP optional per User.

## Considered Options

- The Agent generates and checks its own Device Password, so the Server cannot impersonate a Controller: rejected for now. Every Action already passes through the Server, so it is trusted anyway, and moving the check is a rewrite of Controller authentication.
- No Dashboard accounts at all: rejected, the Dashboard is public and a User's Address Book holds Device Passwords.

## Consequences

- The Server still assigns the Device ID and the Device Password, so an install has neither until it has a Server.
- Wake needs the Address Book to remember a Device's MAC and subnet; the Server knows subnets only of Agents currently online.
- Nothing is migrated from 0.4: Devices are installed again.
