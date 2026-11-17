# Owner changes ask for the OS account password

In User Mode the Agent, the GUI and any program the user runs share one OS account, and rcli is itself a tool an AI agent uses to drive that machine (`local`). Being the Device Owner is therefore not enough to show or change the Device Password, change Device Settings, change the Server, or switch Install Mode: each asks for the OS account password through the OS authentication dialog, or on the tty for the CLI, and is refused without one. Local Actions and Observations are not gated, so an AI agent can drive the screen but cannot open the Device to others or loosen its settings.

## Consequences

- A script cannot change Device Settings unattended in User Mode; in Service Mode it can as root.
- An AI agent with local control could still type into a visible authentication dialog only if the user gave it the password.
- Accepted limit: in User Mode the Device Settings (`settings.json`) and the saved Server live in the user's own files, so a program running as that user can edit them and restart the Agent, past the gate. The gate stops a program that uses rcli, not one that edits rcli's files; Service Mode keeps them root's. No encryption is added for this.
- Accepted limit: in User Mode a Controller with the terminal capability runs commands as the Device Owner's own user (ADR 0012), so it could do the same.
