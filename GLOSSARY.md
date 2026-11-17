# rcli

Remote management and control of machines: track their state and resources, wake them, and drive their mouse, keyboard and terminal from a CLI, mainly on behalf of an AI agent.

## Language

**Device**:
A physical or virtual machine under management; the thing being controlled.
_Avoid_: machine, host, node, máy bị điều khiển

**Device Status**:
Whether a Device is reachable: Online (its Agent is connected) or Offline (disconnected for more than 30 seconds). Screen lock and logged-in user are properties of an Online Device, not statuses.
_Avoid_: state, presence

**Agent**:
The software installed on a Device that reports its state and carries out Actions.
_Avoid_: client, daemon

**Server**:
The central service that connects Controllers to Devices and relays Actions between them. It knows a Device only by its Device ID and Device Password, and keeps no list of whose Devices they are beyond each User's Address Book.
_Avoid_: hub, master

**Controller**:
Whoever issues Actions to a Device: a person or an AI agent, through the CLI or the Dashboard.
_Avoid_: client, máy điều khiển, AI agent (when it means the controlling side)

**Action**:
A single operation a Controller asks a Device to perform, such as a click, a keystroke or a terminal command.
_Avoid_: command, operation, thao tác

**Action Batch**:
An ordered list of Actions sent in one request and carried out by the Agent in sequence, returning a single Observation.
_Avoid_: script, macro, chain

**Observation**:
What a Device reports back after an Action or Action Batch so the Controller can see the result: nothing, one screenshot, a series of change-triggered frames, or a short clip.
_Avoid_: result, feedback, screenshot (when meaning the general concept)

**Mark**:
A numbered on-screen element (button, field, text) or container (window, dialog, menu, list) the Agent detects and labels in an Observation, so a Controller can target it by number or text instead of raw coordinates. Each Mark carries its Mark States, its value (never for a password field) and the container it sits in.
_Avoid_: element id, hotspot, target

**Mark State**:
A condition a Mark is in that differs from the default: disabled, focused, checked, selected, expanded or collapsed.
_Avoid_: state (alone, it clashes with Device Status), flags, status

**Wait Condition**:
Something a Controller asks the Agent to wait for on screen before returning an Observation: text or a Mark appearing or disappearing, a Mark's value, or a Mark State.
_Avoid_: assertion, expectation, verify, wait_for

**Wake Relay**:
An online Agent on the same LAN as an offline Device that sends the Wake-on-LAN packet on the Server's behalf.
_Avoid_: WoL proxy, gateway

**Server Key**:
The secret every Agent and Controller must present to be allowed to connect to the Server; together with the Server's address it is all a new install needs. Replacing it locks out new installs but not Devices that already have a Device ID.
_Avoid_: enrollment code, install key, token

**Server Address**:
The host and port Agents use to reach the Server, set by a User; together with the Server Key it is all a new install needs.
_Avoid_: hostname, server URL, endpoint

**API Address**:
The public `https://` URL of the Dashboard, through which Controllers also call the Server. The Server learns it from a User's visit and hands it to every install, so nobody types it; a User can correct it.
_Avoid_: Controller API URL, Controller Address

**Secret Store**:
The encrypted place on a Device where its Agent keeps the Device Password, readable only by the Device Owner. It never holds the Device's OS login passwords: a Controller types those itself at the lock or login screen.
_Avoid_: vault, keystore, credential store

**Device ID**:
The 9-digit number the Server gives a Device the first time it connects, used to address it. An install with no Server has none yet. It never changes by itself, only when deliberately changed.
_Avoid_: device uuid, machine id

**Device Password**:
The static secret of a Device, generated when the Device gets its Device ID and viewable and changeable on the Device by its Device Owner; with the Device ID it is all a Controller needs to connect.
_Avoid_: access code, PIN, one-time password, login password, password máy agent (the Device Password is not the OS account's password)

**Device Settings**:
The rules set on the Device itself, not on the Server, that govern every Controller connecting to it: which Terminal User runs commands, whether root is never allowed, asked for or always allowed, whether a person at the Device must accept each connection, and which capabilities (mouse and keyboard, terminal, power) a Controller gets.
_Avoid_: permissions, policy, roles (when meaning what a Controller may do on a Device)

**Terminal User**:
The local account a Device runs terminal commands and shells as, by default the account that installed the Agent.
_Avoid_: run-as user, shell user

**Control Lock**:
Exclusive right held by one Controller at a time to send mouse and keyboard Actions to a Device; others may still observe, read metrics and use the terminal. It cannot be taken over by another Controller: it ends only when its holder releases it, sends no Action for 10 minutes, or stays disconnected for 60 seconds, or when a User or the Device Owner force-releases it.
_Avoid_: session lock, ownership

**Live View**:
A continuous stream of a Device's screen to a person in the Dashboard; the holder of the Control Lock can also send mouse and keyboard Actions through it. Unlike an Observation, it is not tied to any Action.
_Avoid_: screen share, remote desktop, xem màn hình

**Live View Quality**:
The level one viewer picks for their own Live View: Quality, Balanced (the default) or Speed. Every level keeps the Device screen's native resolution; they differ only in sharpness against smoothness.
_Avoid_: preset, stream mode, bitrate mode

**User**:
A person with a Dashboard account (or its API token). Every User has their own Address Book and may administer the Server (Users, Server Key, settings, Agent Builds). There are no roles. Controlling a Device from the CLI or GUI needs no User.
_Avoid_: Admin, Operator, administrator, quản trị viên

**Address Book**:
The Devices a Controller has added by typing each one's Device ID and Device Password, with the name it gave them. The CLI and GUI keep one per OS user on the machine; the Dashboard keeps one per User. A Device appears in no Address Book by itself.
_Avoid_: device list, registry, inventory, danh sách máy

**Install Mode**:
How the Agent runs on a Device: User Mode (installed without root, runs in the user's login session and starts at login) or Service Mode (a root/SYSTEM service that starts with the OS and can reach lock and login screens). Every install starts in User Mode.
_Avoid_: daemon mode, unattended mode, portable

**Device Owner**:
The person entitled to change a Device's Device Password, Device Settings, Server and Install Mode: the OS user running the Agent in User Mode, root/Administrator in Service Mode. Each such change asks for the OS account password.
_Avoid_: admin, local admin, installer

**Local Device**:
The Device rcli is installed on, addressed as `local`. Its Device Owner, and in Service Mode the user logged in at its screen, controls it through the Agent with no Server, Device Password or Control Lock.
_Avoid_: this machine, localhost, self

**Session Window**:
The small window the Agent shows on a Device for each connected Controller: who it is, what it is doing, and the controls to accept, restrict or disconnect it.
_Avoid_: connection manager, CM, popup

**Agent Build**:
The `rcli` binary for one version and one target (OS and architecture), uploaded to the Server. Every install, update and download takes one.
_Avoid_: release, package, artifact

**Latest Build**:
For a target, the Agent Build with the highest version the Server holds. It is what `rcli update` installs and what a new install downloads; deleting it makes the next-highest one latest.
_Avoid_: fleet build, rolled-out build, current release

**Minimum Version**:
The lowest Agent version the Server recommends, set in Server settings. Devices and Controllers below it keep working but are flagged as needing an update, and an Agent below it updates itself to the Latest Build.
_Avoid_: required version, compatible version, forced update
