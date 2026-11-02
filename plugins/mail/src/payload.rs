//! Immutable private custody only. Core is the sole send ledger and cleanup authority.
use crate::store::Store;
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use webdock_mail_runtime::{Error, PrivatePayloadStore, Result, SavedPayload, secrets::MailCipher};

pub fn payload_key(retained_hex: &str) -> Result<String> {
    webdock_mail_runtime::secrets::private_payload_key_from_hex(retained_hex)
}
fn purpose(customer: &str, reference: &str) -> Result<String> {
    if !webdock_mail_runtime::proof::valid_id(customer)
        || !customer.parse::<i64>().is_ok_and(|n| n > 0)
        || !uuid::Uuid::parse_str(reference).is_ok_and(|v| {
            v.to_string() == reference
                && v.get_version_num() == 4
                && v.get_variant() == uuid::Variant::RFC4122
        })
    {
        return Err(Error::Invalid);
    }
    Ok(format!("payload:{customer}:{reference}"))
}
pub struct PgPrivatePayloadStore {
    store: Arc<Store>,
    cipher: Arc<MailCipher>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl PgPrivatePayloadStore {
    pub async fn new(store: Arc<Store>, retained_hex: &str) -> Result<Self> {
        let cipher = MailCipher::new(&payload_key(retained_hex)?)?;
        let sealed = cipher.seal(&"webdock-private-payload-v1", "payload-store")?;
        if store
            .0
            .query_opt(
                "SELECT singleton FROM private_payload_key WHERE singleton",
                &[],
            )
            .await
            .map_err(|_| Error::Storage)?
            .is_none()
        {
            store.0.execute("INSERT INTO private_payload_key(singleton,sealed) VALUES(true,$1) ON CONFLICT DO NOTHING", &[&sealed]).await.map_err(|_| Error::Storage)?;
        }
        let row = store
            .0
            .query_one(
                "SELECT sealed FROM private_payload_key WHERE singleton",
                &[],
            )
            .await
            .map_err(|_| Error::Storage)?;
        let check: String = cipher.open(row.get(0), "payload-store")?;
        if check != "webdock-private-payload-v1" {
            return Err(Error::Secret);
        }
        Ok(Self {
            store,
            cipher: Arc::new(cipher),
            slots: Arc::new(tokio::sync::Semaphore::new(4)),
        })
    }
}
#[async_trait::async_trait]
impl PrivatePayloadStore for PgPrivatePayloadStore {
    async fn put(&self, customer: &str, reference: &str, bytes: &[u8]) -> Result<SavedPayload> {
        let purpose = purpose(customer, reference)?;
        if bytes.is_empty() || bytes.len() > webdock_mail_message::MAX_MESSAGE {
            return Err(Error::Invalid);
        }
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Storage)?;
        let (cipher, data) = (self.cipher.clone(), bytes.to_vec());
        let (sealed, digest) = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            Ok::<_, Error>((
                cipher.seal(&STANDARD.encode(&data), &purpose)?,
                hex::encode(Sha256::digest(&data)),
            ))
        })
        .await
        .map_err(|_| Error::Storage)??;
        let id = uuid::Uuid::parse_str(reference).map_err(|_| Error::Invalid)?;
        let len = i32::try_from(bytes.len()).map_err(|_| Error::Invalid)?;
        if self.store.0.execute("INSERT INTO private_payloads(reference,customer_id,sealed,payload_bytes,sha256) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING", &[&id,&customer,&sealed,&len,&digest]).await.map_err(|_| Error::Storage)? != 1 { return Err(Error::Storage); }
        Ok(SavedPayload {
            reference: reference.into(),
            bytes: bytes.len(),
        })
    }
    async fn get(&self, customer: &str, reference: &str) -> Result<Vec<u8>> {
        let purpose = purpose(customer, reference)?;
        let permit = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Storage)?;
        let id = uuid::Uuid::parse_str(reference).map_err(|_| Error::Invalid)?;
        let row=self.store.0.query_opt("SELECT sealed,payload_bytes,sha256 FROM private_payloads WHERE reference=$1 AND customer_id=$2", &[&id,&customer]).await.map_err(|_| Error::Storage)?.ok_or(Error::Storage)?;
        let (sealed, len, digest): (String, i32, String) = (row.get(0), row.get(1), row.get(2));
        let cipher = self.cipher.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let encoded: String = cipher.open(&sealed, &purpose)?;
            let data = STANDARD.decode(&encoded).map_err(|_| Error::Secret)?;
            if data.is_empty()
                || data.len() > webdock_mail_message::MAX_MESSAGE
                || data.len() != len as usize
                || hex::encode(Sha256::digest(&data)) != digest
                || STANDARD.encode(&data) != encoded
            {
                return Err(Error::Secret);
            }
            Ok(data)
        })
        .await
        .map_err(|_| Error::Storage)?
    }
    async fn remove(&self, customer: &str, reference: &str) -> Result<()> {
        purpose(customer, reference)?;
        let id = uuid::Uuid::parse_str(reference).map_err(|_| Error::Invalid)?;
        let owner = self
            .store
            .0
            .query_opt(
                "SELECT customer_id FROM private_payloads WHERE reference=$1",
                &[&id],
            )
            .await
            .map_err(|_| Error::Storage)?;
        if let Some(row) = owner {
            if row.get::<_, String>(0) != customer {
                return Err(Error::Secret);
            }
            self.get(customer, reference).await?;
        }
        self.store
            .0
            .execute(
                "DELETE FROM private_payloads WHERE reference=$1 AND customer_id=$2",
                &[&id, &customer],
            )
            .await
            .map_err(|_| Error::Storage)?;
        Ok(())
    }
}
