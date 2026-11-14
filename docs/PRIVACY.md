# SpareLine privacy statement

SpareLine keeps your organisation talking when its usual cloud chat, email or meeting tools are unavailable. It runs entirely on equipment your organisation controls.

## Messages are end-to-end encrypted

Every message is encrypted on the sender's computer using MLS (Messaging Layer Security, RFC 9420). It can only be decrypted on the devices of the people in that chat. This also covers replies, edits, deletions, reactions and chat names.

- **The SpareLine server can't read messages.** It stores and forwards encrypted data only.
- **Your admin can't read messages.** The control panel has no screen, export or tool that shows conversations, because the server has nothing it could show.
- **History on your computer is encrypted too.** The key is kept in your operating system's secure store (Windows Credential Manager or the macOS Keychain).

## What the server knows

To deliver messages, the server knows:

- your account: username, display name and the groups your admin put you in;
- your devices: name, operating system, app version, and when each last connected;
- which accounts are in which chats;
- when a message is sent and how large it is, but not what it says;
- whether you're online or away, when you're typing (this is not stored), and how far you've read, if read receipts are on. You can turn read receipts off in Settings.

## What your admin can see

- Accounts, groups, devices and sign-ins.
- Connection history for troubleshooting, such as "secure connection failed from 192.168.1.54", and problem summaries your app sends after it has trouble connecting.
- Totals only: how many messages were relayed per minute and how many devices are connected. There are no per-person or per-chat message counts.

## One limitation to know about

An admin can issue a setup code for your account, and a setup code adds a new device. That device would receive new messages from then on, but never your past messages. Whenever any device joins a chat, every member sees a notice in that chat, such as "Alex joined from a new device", so this can't happen silently.

## Announcements

Admin announcements, the banners at the top of SpareLine, are not encrypted. Admins should not put confidential information in them.

## Retention

- **Encrypted copies on the server:** kept for 30 days by default, for devices that are offline, then deleted.
- **Connection history:** kept for 7 days.
- **Server logs:** kept for 14 days, and never contain message content.
- **Standby copies,** if your admin turns them on: the last 4 by default, in a folder your admin chooses. They hold the same as the server, encrypted with a standby password, so another computer can take over if the server fails.
