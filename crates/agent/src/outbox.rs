//! Durable delivery of exact execution receipts, independent of execution-lease validity.
use crate::{
    config::{private_directory, read_private},
    management::{AgentError, Result},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};
use webdock_agent_protocol::Receipt;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    pending: Option<String>,
}
pub struct MessageOutbox<T: Message + Default, const LIMIT: usize> {
    message: std::marker::PhantomData<T>,
    path: PathBuf,
    pending: Option<Vec<u8>>,
    poisoned: bool,
}
pub type ReceiptOutbox = MessageOutbox<Receipt, { 128 * 1024 }>;
pub type MailConfigurationOutbox =
    MessageOutbox<webdock_agent_protocol::MailConfigurationDelivery, { 1024 * 1024 }>;
pub type MailOutbox = MessageOutbox<webdock_agent_protocol::MailDelivery, { 64 * 1024 }>;
pub type RecoveryOutbox = MessageOutbox<webdock_agent_protocol::RecoveryDelivery, { 512 * 1024 }>;
pub type DatabaseOutbox = MessageOutbox<
    webdock_agent_protocol::DatabaseDelivery,
    { webdock_agent_protocol::MAX_MESSAGE_BYTES },
>;
impl<T: Message + Default, const LIMIT: usize> MessageOutbox<T, LIMIT> {
    /// The daemon holds the executor's exclusive process lock for this directory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        private_directory(path.parent().ok_or(AgentError::State)?)?;
        let pending = match std::fs::symlink_metadata(path) {
            Ok(_) => {
                let state: State = serde_json::from_slice(&read_private(path, (LIMIT * 2) as u64)?)
                    .map_err(|_| AgentError::State)?;
                if state.version != 1 {
                    return Err(AgentError::State);
                }
                state
                    .pending
                    .map(|value| URL_SAFE_NO_PAD.decode(value).map_err(|_| AgentError::State))
                    .transpose()?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(AgentError::State),
        };
        if pending.as_ref().is_some_and(|bytes| bytes.len() > LIMIT) {
            return Err(AgentError::State);
        }
        let outbox = Self {
            message: std::marker::PhantomData,
            path: path.into(),
            pending,
            poisoned: false,
        };
        outbox.pending()?;
        Ok(outbox)
    }
    pub fn pending(&self) -> Result<Option<T>> {
        if self.poisoned {
            return Err(AgentError::State);
        }
        self.pending
            .as_ref()
            .map(|bytes| T::decode(bytes.as_slice()).map_err(|_| AgentError::State))
            .transpose()
    }
    pub fn put(&mut self, receipt: &T) -> Result<()> {
        let bytes = receipt.encode_to_vec();
        if bytes.len() > LIMIT {
            return Err(AgentError::State);
        }
        if let Some(pending) = &self.pending {
            if *pending != bytes {
                return Err(AgentError::State);
            }
            return Ok(());
        }
        self.persist(Some(bytes))
    }
    pub fn acknowledge(&mut self, receipt: &T) -> Result<()> {
        if self.pending.as_ref() != Some(&receipt.encode_to_vec()) {
            return Err(AgentError::State);
        }
        self.persist(None)
    }
    fn persist(&mut self, next: Option<Vec<u8>>) -> Result<()> {
        if self.poisoned {
            return Err(AgentError::State);
        }
        let result = (|| -> std::io::Result<()> {
            let parent = self
                .path
                .parent()
                .ok_or_else(|| std::io::Error::other("outbox directory"))?;
            let state = State {
                version: 1,
                pending: next.as_ref().map(|v| URL_SAFE_NO_PAD.encode(v)),
            };
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&serde_json::to_vec(&state)?)?;
            file.as_file().sync_all()?;
            file.persist(&self.path).map_err(|e| e.error)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
            return Err(AgentError::State);
        }
        self.pending = next;
        Ok(())
    }
}

pub type PluginRuntimeOutbox =
    MessageOutbox<webdock_agent_protocol::PluginRuntimeDelivery, { 64 * 1024 }>;
