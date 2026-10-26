# Remote desktop into Windows modules

Status: **slice 1 built (2026-10-26): MOCK VERIFIED and CONTRACT VERIFIED; never run against a Windows guest or a real Incus.** Off unless `APELORA_RDP_GATEWAY=enabled`. Related: [secure remote access contract](../../.agents/skills/secure-remote-access/SKILL.md), [terminal sessions guide](../guides/terminal-sessions.md#remote-desktop-windows-modules), [identity and authorization §7](../architecture/IDENTITY-AND-AUTHORIZATION.md#7-interactive-access-to-modules-terminal-powershell).

## The problem

A person needs a Windows module's desktop. The contract rules out the obvious ways: port 3389 is never published on a tenant network or the internet, the browser talks only to the Apelora API, Apelora never stores or sees Windows credentials, and every session goes through the same permission, approval, ticket, limit and audit rules as a terminal.

## Design

```mermaid
sequenceDiagram
  autonumber
  actor P as Person
  participant C as Fabric Console
  participant T as apelora-desktop-tunnel (person's computer)
  participant R as RDP client (person's computer)
  participant A as Control plane
  participant I as Incus
  participant G as Windows guest
  P->>C: Remote desktop… (instance.desktop; approval in production)
  C->>A: POST …/sessions {kind: rdp}
  A-->>C: single-use ticket (about a minute)
  C-->>P: command and ticket to copy
  P->>T: run the command, paste the ticket
  R->>T: connect to the loopback port the tunnel printed
  T->>A: websocket, ticket as sub-protocol, Origin of the Console
  A->>I: exec without a terminal: the fixed relay (stdin, stdout, stderr, control)
  I->>G: relay connects to 127.0.0.1:3389 inside the guest
  R-->>G: RDP (TLS, network level authentication) between client and guest, bytes relayed unchanged
```

- **Same broker.** `SessionKind::Rdp` uses the existing tickets, approvals (`instance.desktop` on `instance:<application>/<instance>`), per-person limits, idle and maximum time, access re-checks and audit (`session.open` with `kind: rdp`, `runsAs: null`). Only the transport differs.
- **Inside the guest: a fixed relay through Incus exec, without a terminal.** `access::RDP_RELAY` starts Windows PowerShell with a constant script that connects to `127.0.0.1:3389` and copies bytes between that connection and its standard input and output. Without a terminal, Incus hands out separate standard input, output and error channels that carry bytes unchanged (`ShellRequest::interactive = false`, `shell::pump_streams`); standard error is read and dropped. Nothing a caller sends goes into the command. No route into the application network is opened and no port is published.
- **Outside: the person's own RDP client and a small tunnel.** A browser can neither speak RDP nor listen on a local port, so `apelora-desktop-tunnel` (released with the services) listens on a loopback address only (a port the system picks unless `--listen` names one), refuses an address with a user name in it, allows plain `ws://` only to a literal loopback address, reads the ticket from standard input (never from the command line, which shells keep in their history), and opens the session's websocket when the client connects. One connection, one session.
- **Credentials.** TLS and network level authentication run between the RDP client and Windows; Apelora relays the encrypted bytes and does not decrypt them. That holds only if the person checks the certificate Windows presents: the client connects to a loopback address, so it always warns, and Apelora does not yet show the guest's certificate fingerprint to compare with. A person who clicks through relies on the tunnel, the control plane and the address being trustworthy.
- **Studio.** The right-click menu lists the remote desktop but sends the person to the Fabric Console, which shows the hand-over; the Studio's session window is a terminal.

## Why not the alternatives

- **An RDP client in the browser:** needs a server-side RDP implementation that decrypts the session, so Apelora would see the screen and the keyboard, including the password.
- **A proxy device or a route from the control plane to the guest:** opens a network path into a tenant network that the connections design keeps closed.
- **Publishing 3389 behind an allow list:** forbidden by the contract.

## What is not claimed

- The first program on the person's computer to connect to the tunnel's port gets the session. On a computer shared with other people, another user could take it before the person's client connects; the guide says not to use the tunnel there.
- The Console shows the ticket until it expires, then removes it; while it is shown, anything that can read the page or the clipboard could use it first.

- Never run against a Windows guest: that Incus's agent runs the relay with binary-safe standard streams on Windows, and that the relay's throughput is enough for a usable desktop, are unverified. That is why it is off by default.
- The relay assumes the guest's RDP service listens on 3389 and accepts connections from loopback.
- The tunnel is released for Linux (x86-64 and ARM64) only; other systems build it with `cargo build --release -p apelora-desktop-tunnel`.
- Throughput: input from the client is limited like terminal input (256 KiB/s); output is not limited.
