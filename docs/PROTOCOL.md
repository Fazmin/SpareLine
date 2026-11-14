# SpareLine protocol (v1)

## Transport

- **Connection:** TLS 1.2/1.3 WebSocket at `wss://<host>:7443/v1/ws`, carrying JSON text frames. The types are in `crates/sl-proto/src/wire.rs`.
- **Health check:** `GET /v1/health` returns the server info and needs no authentication.
- **Discovery:** mDNS service `_spareline._tcp.local.`. The TXT record holds `v` (protocol version), `id` (server id), `name`, and `fp` (the first 4 bytes of the SPKI hash, as a hint only).

## Server identity (no CA)

The server's certificate is self-signed. Clients pin `SHA-256(SubjectPublicKeyInfo)` and ignore host names. The first pin comes from an invite:

1. An invite or setup code is 16 Crockford base32 symbols (80 bits). Both sides derive `invite_id` and `proof_key` from it with HKDF-SHA256 (salt `spareline/invite/v1`).
2. The client connects, accepting any certificate, and records the SPKI hash `s`.
3. The client sends `Prove { invite_id, nonce }`. The server replies `Proof { HMAC-SHA256(proof_key, label ‖ len(nonce) ‖ nonce ‖ s) }`.
4. The client checks the proof in constant time. Only then does it pin `s` and send the code and account details (`Register` or `Redeem`) on the same connection.

A rogue server or a spoofed mDNS answer can't produce the proof without knowing the invite.

## Sessions

- **Token:** a 256-bit token, stored by the client in the OS keychain. The server stores only its SHA-256.
- **Expiry:** 30 days without use, sliding.
- **Resuming:** `Resume { token, device_id }` re-authenticates a reconnecting device.
- **Rate limits:** each source address gets 120 attempts a minute (bursts of 60). Password attempts are limited to 10 a minute (bursts of 5) per username and address.

## End-to-end encryption (MLS)

| | |
|---|---|
| Ciphersuite | `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`, via OpenMLS 0.9. |
| Leaves | One leaf per device. Each has a `BasicCredential` of `"<user uuid>:<device uuid>"` and an Ed25519 key registered at sign-up. |
| Wire format | Handshake messages are `PublicMessage`; application messages are `PrivateMessage` (`MIXED_PLAINTEXT_WIRE_FORMAT_POLICY`). Each device keeps 3 past epochs. |
| Key packages | 50 at a time plus one last-resort package (the `LastResort` capability is declared). The server asks for more when fewer than 20 remain. |

**Ordering.** The server accepts a commit or application message only for the conversation's current epoch, and otherwise replies `StaleEpoch`. Each conversation has a strictly increasing `seq`. Clients process envelopes in `seq` order and buffer any gaps. Their own envelopes come back without payload, so the sequence stays contiguous.

**Membership.**

- The server is authoritative about who *may* be in a chat, through `ConversationInfo.devices`.
- Clients reconcile against it. Any member removes leaves that are no longer allowed, and the first commit wins.
- The server routes ciphertext only to devices that are both in the tree and allowed.
- Group isolation is enforced on every add.

**Joining.** Devices join through a Welcome from the member who added them, or by **External Commit** from the latest GroupInfo the server stores. External commits are used for new devices and to rejoin after falling behind.

**Restoring from a standby copy.** A server restored from a standby copy has gone back in time: envelopes, commits and accounts after the copy are gone, while devices that were online kept them.

- `ServerInfo.restore` carries a `RestoreMarker { id, copy_taken_at }`, new for every restore.
- `ConversationInfo.restored` carries a `RestorePoint { seq, epoch }`: the conversation's `last_seq` and epoch in the copy.
- On the first connection after seeing a new marker id, a device rewinds each conversation whose watermark is past `restored.seq` back to it, so the restored server's new envelopes, which reuse those sequence numbers, aren't skipped. If its MLS epoch is past `restored.epoch`, it drops the group and rejoins by External Commit. It then records the marker id so this happens once.
- A device that gets `StaleEpoch` with a server epoch *lower* than its own also drops the group and rejoins, rather than catching up.

Both fields are optional and older clients ignore them, so `PROTOCOL_VERSION` is unchanged.

**Application payloads** inside MLS are versioned JSON: `Text`, `Edit`, `Delete`, `Reaction` and `ChatMeta`. Clients accept edits and deletes only from the author, as authenticated by MLS.
