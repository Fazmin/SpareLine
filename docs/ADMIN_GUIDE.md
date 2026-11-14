# SpareLine Server: admin guide

## Before an outage (prepare once)

1. **Install SpareLine Server** on a computer that stays on and is connected to the office network by cable. Open it. The one-screen setup asks for a server name and an admin password, then shows a **recovery key**. Store the recovery key offline; it's the only way back in if you forget the password.
2. **Allow it through the firewall.** Diagnostics shows a check for this. Click **Add firewall rules**, which opens a Windows approval prompt, or allow TCP 7443 and UDP 5353 yourself.
3. **Create groups** under Groups, for example one per team plus "Everyone". People can only find and message others who share a group with them.
4. **Create invites** under Invites. Pick the groups, then create one invite per person. Codes are shown once. Copy each invite message, save them as CSV, or save printable cards.
5. **Install SpareLine on staff computers.** Each person opens it, chooses **I have an invite** and pastes their invite. The app sits in the system tray and starts at sign-in.
6. **Turn on standby copies.** Under **Settings → Standby copy**, choose a folder that would survive this computer failing, such as a USB drive or a shared folder on another computer, and set a standby password. Store the password with the recovery key. See [If the server computer fails](#if-the-server-computer-fails). Without the panel, use `--standby-folder` (see [Running without the control panel](#running-without-the-control-panel)).
7. **Check readiness.** The Overview page shows "Devices ready (seen this week)". Aim for everyone.

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

## If the server computer fails

A standby copy lets another computer take over with the same identity, accounts and groups, so staff apps trust it and reconnect without new invites.

**How copies work.** Once standby copies are on, SpareLine Server saves an encrypted copy of itself to your folder every 15 minutes, and again within about a minute of any change to people, groups or invites. Each copy holds the database, the server's certificate and settings, and opens only with the standby password. The last 4 are kept. Diagnostics shows a warning if copies stop, for example because the USB drive was unplugged.

**Taking over:**

1. **Keep the old computer switched off.** Two servers with the same identity would confuse staff apps.
2. **Install SpareLine Server on another computer and open it.** On the setup screen, choose **Restore a standby copy**, pick the newest copy and enter the standby password. Without the panel: `SPARELINE_STANDBY_PASSWORD="…" spareline-server --headless --restore <copy>` (see [Running without the control panel](#running-without-the-control-panel)).
3. **Let staff reach it.** On the same network, staff apps find the new server by themselves. If staff type the server's address, give the new computer the old IP address (the restore screen shows it), or tell staff the new one.
4. **Check what changed after the copy.** Anything done after the copy isn't in it. Check People for anyone you added, removed or signed out since then: a device you removed after the copy can connect again until you remove it again, and accounts created after the copy need new invites.
5. **Unlock the panel with the old server's admin password.**

**What staff notice.** Their apps reconnect, and any chat that had moved on since the copy shows "SpareLine Server was restored from a standby copy". Messages people already received stay on their computers. Messages sent shortly before the old server stopped may not reach people who were offline at the time.

## Setup codes

**People → select someone → Create a setup code.** Use one for a new computer or a forgotten password. The code works once, expires in 24 hours, sets a new password and signs out the person's other devices. Their chats continue on the new device, but past messages stay on the old one.

## Running without the control panel

`spareline-server --headless` runs the server with no tray icon or control panel, for example on a computer nobody signs in to. Run `spareline-server --help` to see every option.

| Option | What it does |
|---|---|
| `--headless` | Runs without the tray icon or control panel. Stop it with <kbd>Ctrl</kbd>+<kbd>C</kbd>. |
| `--data-dir DIR` | Keeps the server's data in `DIR` instead of the usual data folder. |
| `--standby-folder DIR` | Turns on standby copies and saves them in `DIR`. The first time, set `SPARELINE_STANDBY_PASSWORD` to choose the standby password. |
| `--standby-every MINUTES` | How often to make a copy, from 5 to 1440 minutes. The default is 15. |
| `--standby-keep N` | How many copies to keep, from 1 to 50. The default is 4. |
| `--standby-off` | Turns standby copies off. Copies already made stay in the folder. |
| `--restore COPY` | Takes over from a server that stopped working, using one of its standby copies. Works only on a fresh install or an empty data folder, and needs `SPARELINE_STANDBY_PASSWORD`. |

| Environment variable | Used for |
|---|---|
| `SPARELINE_ADMIN_PASSWORD` | The admin password, to finish first-run setup without the panel. The recovery key is printed once; store it offline. |
| `SPARELINE_STANDBY_PASSWORD` | The standby password, to restore a copy or to set or change it with `--standby-folder`. At least 10 characters. |

Settings you make this way are saved, and the panel shows them too. The standby options do nothing when they match what's already set, so they can stay in a startup script or scheduled task. If an option can't be applied, for example because the folder can't be reached, the server explains why and doesn't start.

**Set up a new server and turn on standby copies (Windows, PowerShell):**

```powershell
$env:SPARELINE_ADMIN_PASSWORD = "…"
$env:SPARELINE_STANDBY_PASSWORD = "…"
spareline-server --headless --standby-folder "\\fileserver\it\SpareLine standby"
```

**Take over on another computer (macOS or Linux):**

```sh
SPARELINE_STANDBY_PASSWORD="…" spareline-server --headless \
  --restore "/Volumes/USB/spareline-standby-20261114-104200-01a1dc.slstandby"
```

The restored server keeps the old server's standby settings. Add `--standby-folder` to save new copies somewhere else, for example if the old folder was on the computer that failed.

## Where things live

| | |
|---|---|
| Data folder | `%LOCALAPPDATA%\SpareLine\Server`. Contains `config.toml`, `spareline.db`, `tls\`, `logs\`, `backups\`, `reports\` and, once standby copies are on, `standby.key`. |
| Backups | Daily; the last 7 are kept. **Settings → Back up now** makes one on demand. |
| Standby copies | In the folder you choose, as `spareline-standby-<date>-<time>-<id>.slstandby`. Every 15 minutes by default; the last 4 are kept. |
| Port | 7443. Changing it under Settings needs a restart and a firewall update. |
| Certificate | Self-signed, valid for 10 years. Clients pin its fingerprint, so don't regenerate it unless you must: every device would then need a setup code. |

## Known gaps

These are planned but not built yet:

- Windows service mode. The tray app currently starts when an admin signs in to Windows; `--headless` can be run as a scheduled task instead (see [Running without the control panel](#running-without-the-control-panel)).
- Restoring a daily backup from the panel. For now, stop the server and copy the backup file over `spareline.db`. To move to another computer, use a standby copy instead.
- Code-signed installers, macOS notarisation, and remote access through Headscale (phase 2).
- A full-database encryption option (SQLCipher) for the client. Message bodies and titles are already encrypted at rest; encryption state is protected by OS account permissions.
