# SpareLine Server: admin guide

## Before an outage (prepare once)

1. **Install SpareLine Server** on a computer that stays on and is connected to the office network by cable. Open it. The one-screen setup asks for a server name and an admin password, then shows a **recovery key**. Store the recovery key offline; it's the only way back in if you forget the password.
2. **Allow it through the firewall.** Diagnostics shows a check for this. Click **Add firewall rules**, which opens a Windows approval prompt, or allow TCP 7443 and UDP 5353 yourself.
3. **Create groups** under Groups, for example one per team plus "Everyone". People can only find and message others who share a group with them.
4. **Create invites** under Invites. Pick the groups, then create one invite per person. Codes are shown once. Copy each invite message, save them as CSV, or save printable cards.
5. **Install SpareLine on staff computers.** Each person opens it, chooses **I have an invite** and pastes their invite. The app sits in the system tray and starts at sign-in.
6. **Check readiness.** The Overview page shows "Devices ready (seen this week)". Aim for everyone.

## During an outage

- **Announcements:** send a banner such as "Our usual chat is down. Use SpareLine for now." Urgent banners can't be dismissed.
- **Overview** shows whether the server is running, who's connected, and anything that needs attention.

## When someone can't connect

1. **People → select them → Connection history** shows each attempt and its reason. For example:
   - "Secure connection failed": usually an old pin after the certificate was regenerated. Give them a setup code.
   - "Sign-in failed: wrong password": give them a setup code to reset it.
   - "Too many attempts": wait a minute.
2. **Problems their app reported:** after reconnecting, the app sends a short summary, such as "TCP timeout ×3", which points to the network or firewall.
3. **Diagnostics → Test like a staff computer would** connects to the server over its LAN address and checks discovery.
4. **Diagnostics → Debug logging** adds detail for up to an hour, then switches itself off. Logs never contain messages.
5. **Diagnostics → Create report** saves a text file you can send to whoever supports the server.

If automatic discovery doesn't work (some networks block multicast), staff can type the address shown on the Overview page. Invite messages include it too.

## Setup codes

**People → select someone → Create a setup code.** Use one for a new computer or a forgotten password. The code works once, expires in 24 hours, sets a new password and signs out the person's other devices. Their chats continue on the new device, but past messages stay on the old one.

## Where things live

| | |
|---|---|
| Data folder | `%LOCALAPPDATA%\SpareLine\Server`. Contains `config.toml`, `spareline.db`, `tls\`, `logs\`, `backups\` and `reports\`. |
| Backups | Daily; the last 7 are kept. **Settings → Back up now** makes one on demand. |
| Port | 7443. Changing it under Settings needs a restart and a firewall update. |
| Certificate | Self-signed, valid for 10 years. Clients pin its fingerprint, so don't regenerate it unless you must: every device would then need a setup code. |

## Known gaps

These are planned but not built yet:

- Windows service mode. The tray app currently starts when an admin signs in to Windows; `--headless` can be run as a scheduled task instead.
- Restoring a backup from the panel. For now, stop the server and copy the backup file over `spareline.db`.
- Code-signed installers, macOS notarisation, and remote access through Headscale (phase 2).
- A full-database encryption option (SQLCipher) for the client. Message bodies and titles are already encrypted at rest; encryption state is protected by OS account permissions.
