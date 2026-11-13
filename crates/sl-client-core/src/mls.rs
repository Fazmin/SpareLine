//! End-to-end encryption with MLS (RFC 9420) via OpenMLS.
//!
//! Every device is one MLS leaf with a `BasicCredential` of `"<user>:<device>"` and an
//! Ed25519 signature key registered with the server. Handshake messages are sent as
//! PublicMessage (so the server can order them and enforce isolation); application
//! messages are always PrivateMessage.

use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::prelude::*;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::RustCrypto;
use openmls_sqlite_storage::{Codec, Connection, SqliteStorageProvider};
use sl_proto::{DeviceId, DeviceRef};

pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
const MAX_PAST_EPOCHS: usize = 3;
/// Key package lifetime: one year (devices refresh their packages regularly anyway).
const KP_LIFETIME_SECS: u64 = 365 * 24 * 3600;

#[derive(Default)]
pub struct JsonCodec;

impl Codec for JsonCodec {
    type Error = serde_json::Error;
    fn to_vec<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }
    fn from_slice<T: serde::de::DeserializeOwned>(slice: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(slice)
    }
}

pub struct Provider {
    crypto: RustCrypto,
    storage: SqliteStorageProvider<JsonCodec, Connection>,
}

impl OpenMlsProvider for Provider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = SqliteStorageProvider<JsonCodec, Connection>;
    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }
    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }
    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

/// A commit we created and must send; merged only after the server accepts it.
pub struct PendingCommit {
    pub group_id: Vec<u8>,
    /// Epoch the commit was created in (what the server must be at).
    pub epoch: u64,
    pub commit: Vec<u8>,
    pub welcome: Option<Vec<u8>>,
    pub group_info: Vec<u8>,
}

pub struct CreatedGroup {
    pub group_id: Vec<u8>,
    pub epoch: u64,
    pub welcome: Option<Vec<u8>>,
    pub group_info: Vec<u8>,
}

pub enum Processed {
    Application {
        sender: DeviceRef,
        bytes: Vec<u8>,
    },
    Commit {
        sender: Option<DeviceRef>,
        added: Vec<DeviceRef>,
        removed: Vec<DeviceRef>,
        self_removed: bool,
    },
    Proposal,
    /// Our own message echoed back (should not happen; server excludes own envelopes).
    Own,
}

pub struct Mls {
    provider: Provider,
    signer: SignatureKeyPair,
    identity: Option<DeviceRef>,
    key_file: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct KeyFile {
    public_hex: String,
}

impl Mls {
    /// Open MLS storage at `dir/mls.db`, creating a device signature key if needed.
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = Connection::open(dir.join("mls.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let mut storage = SqliteStorageProvider::<JsonCodec, Connection>::new(conn);
        storage
            .run_migrations()
            .map_err(|e| anyhow!("mls storage migrations: {e}"))?;
        let provider = Provider {
            crypto: RustCrypto::default(),
            storage,
        };
        let key_file = dir.join("mls_identity.json");
        let signer = match std::fs::read_to_string(&key_file)
            .ok()
            .and_then(|s| serde_json::from_str::<KeyFile>(&s).ok())
        {
            Some(k) => {
                let public = data_encoding::HEXLOWER.decode(k.public_hex.as_bytes())?;
                SignatureKeyPair::read(
                    provider.storage(),
                    &public,
                    CIPHERSUITE.signature_algorithm(),
                )
                .context("device signature key missing from MLS storage")?
            }
            None => {
                let kp = SignatureKeyPair::new(CIPHERSUITE.signature_algorithm())
                    .map_err(|e| anyhow!("{e:?}"))?;
                kp.store(provider.storage())?;
                let kf = KeyFile {
                    public_hex: data_encoding::HEXLOWER.encode(kp.public()),
                };
                std::fs::write(&key_file, serde_json::to_string(&kf)?)?;
                kp
            }
        };
        Ok(Self {
            provider,
            signer,
            identity: None,
            key_file,
        })
    }

    /// Remove all MLS state (new identity on next open).
    pub fn wipe(dir: &Path) {
        for f in ["mls.db", "mls.db-wal", "mls.db-shm", "mls_identity.json"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
    }

    pub fn signature_public(&self) -> Vec<u8> {
        self.signer.to_public_vec()
    }

    pub fn set_identity(&mut self, me: DeviceRef) {
        self.identity = Some(me);
    }

    pub fn identity(&self) -> Option<DeviceRef> {
        self.identity
    }

    fn cwk(&self) -> anyhow::Result<CredentialWithKey> {
        let me = self.identity.context("MLS identity not set")?;
        let ident = sl_proto::credential_identity(me.user, me.device);
        Ok(CredentialWithKey {
            credential: BasicCredential::new(ident.into_bytes()).into(),
            signature_key: self.signer.public().into(),
        })
    }

    fn capabilities() -> Capabilities {
        Capabilities::builder()
            .extensions(vec![ExtensionType::LastResort])
            .build()
    }

    fn create_config() -> MlsGroupCreateConfig {
        MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
            .max_past_epochs(MAX_PAST_EPOCHS)
            .use_ratchet_tree_extension(true)
            .capabilities(Self::capabilities())
            .build()
    }

    fn join_config() -> MlsGroupJoinConfig {
        MlsGroupJoinConfig::builder()
            .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
            .max_past_epochs(MAX_PAST_EPOCHS)
            .use_ratchet_tree_extension(true)
            .build()
    }

    fn load(&self, gid: &[u8]) -> anyhow::Result<MlsGroup> {
        MlsGroup::load(self.provider.storage(), &GroupId::from_slice(gid))?
            .context("unknown MLS group")
    }

    pub fn has_group(&self, gid: &[u8]) -> bool {
        matches!(MlsGroup::load(self.provider.storage(), &GroupId::from_slice(gid)), Ok(Some(g)) if g.is_active())
    }

    pub fn epoch(&self, gid: &[u8]) -> Option<u64> {
        self.load(gid).ok().map(|g| g.epoch().as_u64())
    }

    pub fn delete_group(&self, gid: &[u8]) {
        if let Ok(Some(mut g)) = MlsGroup::load(self.provider.storage(), &GroupId::from_slice(gid))
        {
            let _ = g.delete(self.provider.storage());
        }
    }

    /// `(leaf index, device)` for every member.
    pub fn members(&self, gid: &[u8]) -> anyhow::Result<Vec<(u32, Option<DeviceRef>)>> {
        let g = self.load(gid)?;
        Ok(g.members()
            .map(|m| (m.index.u32(), device_of(&m.credential)))
            .collect())
    }

    // ---- key packages ------------------------------------------------------------------

    /// Generate `n` one-time key packages plus one last-resort package.
    pub fn key_packages(&self, n: usize) -> anyhow::Result<(Vec<Vec<u8>>, Vec<u8>)> {
        let cwk = self.cwk()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let kp = KeyPackage::builder()
                .leaf_node_capabilities(Self::capabilities())
                .key_package_lifetime(Lifetime::new(KP_LIFETIME_SECS))
                .build(CIPHERSUITE, &self.provider, &self.signer, cwk.clone())
                .map_err(|e| anyhow!("key package: {e:?}"))?;
            out.push(kp.key_package().tls_serialize_detached()?);
        }
        let lr = KeyPackage::builder()
            .leaf_node_capabilities(Self::capabilities())
            .key_package_lifetime(Lifetime::new(KP_LIFETIME_SECS))
            .mark_as_last_resort()
            .build(CIPHERSUITE, &self.provider, &self.signer, cwk)
            .map_err(|e| anyhow!("key package: {e:?}"))?;
        Ok((out, lr.key_package().tls_serialize_detached()?))
    }

    /// Verify a key package handed out by the server and check it belongs to `expected`.
    pub fn validate_key_package(
        &self,
        bytes: &[u8],
        expected: DeviceRef,
    ) -> anyhow::Result<KeyPackage> {
        let kp_in = KeyPackageIn::tls_deserialize_exact(bytes)?;
        let kp = kp_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .map_err(|e| anyhow!("invalid key package: {e:?}"))?;
        let who = device_of(kp.leaf_node().credential())
            .context("key package has no SpareLine identity")?;
        if who != expected {
            bail!("key package identity mismatch");
        }
        if kp.ciphersuite() != CIPHERSUITE {
            bail!("unsupported ciphersuite");
        }
        Ok(kp)
    }

    // ---- groups ------------------------------------------------------------------------

    /// Create a group and add the given key packages in one commit (merged immediately:
    /// nobody else knows the group yet).
    pub fn create_group(&self, kps: &[KeyPackage]) -> anyhow::Result<CreatedGroup> {
        let gid = GroupId::random(self.provider.rand());
        let mut group = MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &Self::create_config(),
            gid,
            self.cwk()?,
        )
        .map_err(|e| anyhow!("create group: {e:?}"))?;
        let mut welcome = None;
        if !kps.is_empty() {
            let (_commit, w, _gi) = group
                .add_members(&self.provider, &self.signer, kps)
                .map_err(|e| anyhow!("add members: {e:?}"))?;
            group
                .merge_pending_commit(&self.provider)
                .map_err(|e| anyhow!("merge: {e:?}"))?;
            welcome = Some(w.to_bytes()?);
        }
        let gi = group
            .export_group_info(self.provider.crypto(), &self.signer, true)
            .map_err(|e| anyhow!("{e:?}"))?;
        Ok(CreatedGroup {
            group_id: group.group_id().to_vec(),
            epoch: group.epoch().as_u64(),
            welcome,
            group_info: gi.to_bytes()?,
        })
    }

    /// Join from a Welcome. Returns the MLS group id.
    pub fn join_welcome(&self, welcome: &[u8]) -> anyhow::Result<Vec<u8>> {
        let msg = MlsMessageIn::tls_deserialize_exact(welcome)?;
        let MlsMessageBodyIn::Welcome(w) = msg.extract() else {
            bail!("not a welcome")
        };
        let staged = StagedWelcome::build_from_welcome(&self.provider, &Self::join_config(), w)
            .map_err(|e| anyhow!("welcome: {e:?}"))?
            .replace_old_group()
            .skip_lifetime_validation()
            .build()
            .map_err(|e| anyhow!("welcome: {e:?}"))?;
        let group = staged
            .into_group(&self.provider)
            .map_err(|e| anyhow!("welcome: {e:?}"))?;
        Ok(group.group_id().to_vec())
    }

    /// Join an existing group by external commit. The group is stored locally; if the
    /// server rejects the commit, call [`Mls::delete_group`] and retry.
    pub fn external_join(&self, group_info: &[u8]) -> anyhow::Result<PendingCommit> {
        let msg = MlsMessageIn::tls_deserialize_exact(group_info)?;
        let MlsMessageBodyIn::GroupInfo(vgi) = msg.extract() else {
            bail!("not a group info")
        };
        let epoch = vgi.epoch().as_u64();
        // Replace any stale local copy of this group.
        let gid = vgi.group_id().to_vec();
        self.delete_group(&gid);
        let (group, bundle) = MlsGroup::external_commit_builder()
            .with_config(Self::join_config())
            .skip_lifetime_validation()
            .build_group(&self.provider, vgi, self.cwk()?)
            .map_err(|e| anyhow!("external join: {e:?}"))?
            .load_psks(self.provider.storage())
            .map_err(|e| anyhow!("external join: {e:?}"))?
            .build(
                self.provider.rand(),
                self.provider.crypto(),
                &self.signer,
                |_| true,
            )
            .map_err(|e| anyhow!("external join: {e:?}"))?
            .finalize(&self.provider)
            .map_err(|e| anyhow!("external join: {e:?}"))?;
        let (commit, _welcome, _gi) = bundle.into_messages();
        let gi = group
            .export_group_info(self.provider.crypto(), &self.signer, true)
            .map_err(|e| anyhow!("{e:?}"))?;
        Ok(PendingCommit {
            group_id: gid,
            epoch,
            commit: commit.to_bytes()?,
            welcome: None,
            group_info: gi.to_bytes()?,
        })
    }

    /// Add key packages to an existing group (pending until the server accepts).
    pub fn add(&self, gid: &[u8], kps: &[KeyPackage]) -> anyhow::Result<PendingCommit> {
        let mut g = self.load(gid)?;
        let epoch = g.epoch().as_u64();
        let (commit, welcome, gi) = g
            .add_members(&self.provider, &self.signer, kps)
            .map_err(|e| anyhow!("add: {e:?}"))?;
        let gi = next_group_info(gi)?;
        Ok(PendingCommit {
            group_id: gid.to_vec(),
            epoch,
            commit: commit.to_bytes()?,
            welcome: Some(welcome.to_bytes()?),
            group_info: gi,
        })
    }

    /// Remove every leaf whose device is not in `allowed` (never ourselves).
    /// Returns `None` if nothing needs removing.
    pub fn remove_disallowed(
        &self,
        gid: &[u8],
        allowed: &[DeviceRef],
    ) -> anyhow::Result<Option<(PendingCommit, Vec<DeviceId>)>> {
        let mut g = self.load(gid)?;
        let own = g.own_leaf_index();
        let mut leaves = Vec::new();
        let mut devices = Vec::new();
        for m in g.members() {
            if m.index == own {
                continue;
            }
            let dev = device_of(&m.credential);
            let ok = dev.is_some_and(|d| allowed.contains(&d));
            if !ok {
                leaves.push(m.index);
                if let Some(d) = dev {
                    devices.push(d.device);
                }
            }
        }
        if leaves.is_empty() {
            return Ok(None);
        }
        let epoch = g.epoch().as_u64();
        let (commit, _w, gi) = g
            .remove_members(&self.provider, &self.signer, &leaves)
            .map_err(|e| anyhow!("remove: {e:?}"))?;
        let gi = next_group_info(gi)?;
        Ok(Some((
            PendingCommit {
                group_id: gid.to_vec(),
                epoch,
                commit: commit.to_bytes()?,
                welcome: None,
                group_info: gi,
            },
            devices,
        )))
    }

    pub fn export_group_info(&self, gid: &[u8]) -> anyhow::Result<Vec<u8>> {
        let g = self.load(gid)?;
        let gi = g
            .export_group_info(self.provider.crypto(), &self.signer, true)
            .map_err(|e| anyhow!("{e:?}"))?;
        Ok(gi.to_bytes()?)
    }

    pub fn merge_pending(&self, gid: &[u8]) -> anyhow::Result<()> {
        let mut g = self.load(gid)?;
        g.merge_pending_commit(&self.provider)
            .map_err(|e| anyhow!("merge: {e:?}"))?;
        Ok(())
    }

    pub fn clear_pending(&self, gid: &[u8]) {
        if let Ok(mut g) = self.load(gid) {
            let _ = g.clear_pending_commit(self.provider.storage());
        }
    }

    // ---- messages ----------------------------------------------------------------------

    /// Encrypt an application message. Returns `(ciphertext, epoch)`.
    pub fn encrypt(&self, gid: &[u8], plaintext: &[u8]) -> anyhow::Result<(Vec<u8>, u64)> {
        let mut g = self.load(gid)?;
        let out = g
            .create_message(&self.provider, &self.signer, plaintext)
            .map_err(|e| anyhow!("encrypt: {e:?}"))?;
        Ok((out.to_bytes()?, g.epoch().as_u64()))
    }

    /// Process an incoming MLS message for this group.
    pub fn process(&self, gid: &[u8], data: &[u8]) -> anyhow::Result<Processed> {
        let mut g = self.load(gid)?;
        let msg = MlsMessageIn::tls_deserialize_exact(data)?;
        let pm = msg
            .try_into_protocol_message()
            .map_err(|e| anyhow!("not a protocol message: {e:?}"))?;
        let processed = g
            .process_message(&self.provider, pm)
            .map_err(|e| anyhow!("process: {e:?}"))?;
        let sender = device_of(processed.credential());
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(m) => {
                let sender = sender.context("application message without SpareLine identity")?;
                Ok(Processed::Application {
                    sender,
                    bytes: m.into_bytes(),
                })
            }
            ProcessedMessageContent::StagedCommitMessage(sc) => {
                let added: Vec<DeviceRef> = sc
                    .add_proposals()
                    .filter_map(|a| {
                        device_of(a.add_proposal().key_package().leaf_node().credential())
                    })
                    .collect();
                let removed: Vec<DeviceRef> = sc
                    .remove_proposals()
                    .filter_map(|r| g.member(r.remove_proposal().removed()).and_then(device_of))
                    .collect();
                let self_removed = sc.self_removed();
                g.merge_staged_commit(&self.provider, *sc)
                    .map_err(|e| anyhow!("merge commit: {e:?}"))?;
                Ok(Processed::Commit {
                    sender,
                    added,
                    removed,
                    self_removed,
                })
            }
            ProcessedMessageContent::ProposalMessage(p)
            | ProcessedMessageContent::ExternalJoinProposalMessage(p) => {
                g.store_pending_proposal(self.provider.storage(), *p)?;
                Ok(Processed::Proposal)
            }
            ProcessedMessageContent::OwnPendingCommit
            | ProcessedMessageContent::OwnPrivateMessage => Ok(Processed::Own),
            #[allow(unreachable_patterns)]
            _ => Ok(Processed::Proposal),
        }
    }

    pub fn key_file(&self) -> &Path {
        &self.key_file
    }
}

/// GroupInfo for the epoch a pending commit creates (returned by OpenMLS because the
/// ratchet-tree extension is enabled). Stored by the server for external joins.
fn next_group_info(
    gi: Option<openmls::messages::group_info::GroupInfo>,
) -> anyhow::Result<Vec<u8>> {
    let gi = gi.context("commit did not produce a GroupInfo")?;
    Ok(MlsMessageOut::from(gi).to_bytes()?)
}

fn device_of(c: &Credential) -> Option<DeviceRef> {
    let b = BasicCredential::try_from(c.clone()).ok()?;
    let s = std::str::from_utf8(b.identity()).ok()?;
    sl_proto::parse_credential_identity(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn dev() -> DeviceRef {
        DeviceRef {
            user: Uuid::now_v7(),
            device: Uuid::now_v7(),
        }
    }

    fn open(dir: &Path, me: DeviceRef) -> Mls {
        let mut m = Mls::open(dir).expect("open");
        m.set_identity(me);
        m
    }

    #[test]
    fn two_party_roundtrip_and_external_join() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (a_id, b_id, b2_id) = (
            dev(),
            dev(),
            DeviceRef {
                user: Uuid::nil(),
                device: Uuid::now_v7(),
            },
        );
        let a = open(&tmp.path().join("a"), a_id);
        let b = open(&tmp.path().join("b"), b_id);

        // Bob publishes key packages; Alice validates one and creates a group.
        let (kps, _lr) = b.key_packages(2).expect("kps");
        let kp = a.validate_key_package(&kps[0], b_id).expect("valid");
        assert!(
            a.validate_key_package(&kps[1], dev()).is_err(),
            "identity must match"
        );
        let created = a.create_group(&[kp]).expect("create");
        assert_eq!(created.epoch, 1);
        let gid = b
            .join_welcome(created.welcome.as_ref().expect("welcome"))
            .expect("join");
        assert_eq!(gid, created.group_id);

        // Alice → Bob.
        let (ct, epoch) = a.encrypt(&gid, b"hello bob").expect("encrypt");
        assert_eq!(epoch, 1);
        assert!(
            !ct.windows(9).any(|w| w == b"hello bob"),
            "ciphertext must not contain plaintext"
        );
        match b.process(&gid, &ct).expect("process") {
            Processed::Application { sender, bytes } => {
                assert_eq!(sender, a_id);
                assert_eq!(bytes, b"hello bob");
            }
            _ => panic!("expected application message"),
        }

        // Bob's second device joins by external commit.
        let b2_id = DeviceRef {
            user: b_id.user,
            device: b2_id.device,
        };
        let b2 = open(&tmp.path().join("b2"), b2_id);
        let gi = a.export_group_info(&gid).expect("gi");
        let pending = b2.external_join(&gi).expect("external join");
        assert_eq!(pending.epoch, 1);
        for m in [&a, &b] {
            match m.process(&gid, &pending.commit).expect("process join") {
                Processed::Commit { sender, .. } => assert_eq!(sender, Some(b2_id)),
                _ => panic!("expected commit"),
            }
        }
        assert_eq!(a.epoch(&gid), Some(2));

        // Alice removes Bob's second device (not allowed any more).
        let (rm, devices) = a
            .remove_disallowed(&gid, &[a_id, b_id])
            .expect("remove")
            .expect("something to remove");
        assert_eq!(devices, vec![b2_id.device]);
        a.merge_pending(&gid).expect("merge");
        match b.process(&gid, &rm.commit).expect("process remove") {
            Processed::Commit { removed, .. } => assert_eq!(removed, vec![b2_id]),
            _ => panic!("expected commit"),
        }
        match b2.process(&gid, &rm.commit).expect("process own removal") {
            Processed::Commit { self_removed, .. } => assert!(self_removed),
            _ => panic!("expected commit"),
        }
        let (ct, _) = b.encrypt(&gid, b"after removal").expect("encrypt");
        assert!(matches!(
            a.process(&gid, &ct),
            Ok(Processed::Application { .. })
        ));

        // The GroupInfo shipped with the removal commit is for the new epoch: a fresh device
        // can join from it.
        let b3_id = DeviceRef {
            user: b_id.user,
            device: Uuid::now_v7(),
        };
        let b3 = open(&tmp.path().join("b3"), b3_id);
        let join = b3
            .external_join(&rm.group_info)
            .expect("join from new-epoch group info");
        assert_eq!(join.epoch, 3);
    }
}
