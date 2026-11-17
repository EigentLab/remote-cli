# Installs start in User Mode; only Service Mode needs root

ADR 0001 made the Agent privileged and ADR 0006 accepted that every install needs sudo/UAC. That kept people from trying rcli, from using it only to control other Devices, and from driving their own machine. Like RustDesk, `rcli install` now needs no root (User Mode: the Agent runs in the login session and starts at login), and `rcli service install` switches to Service Mode, which is the privileged Agent of ADR 0001. Root is asked once, for that switch, at the Device itself; a Controller can never trigger it. This amends ADR 0001 (privileged only in Service Mode) and ADR 0006 (the Controller configuration is per user in User Mode).

## Consequences

- User Mode cannot reach lock/login screens or UAC, work before login, run the terminal as another user, or send input on Wayland. Those features say Service Mode is needed; they never ask for root.
- The Device Owner is the OS user running the Agent in User Mode, root/Administrator in Service Mode.
- A Server is optional at install: without one the install only serves local control.
