//! Fixed, bounded native Mail control transport. The adapter must fence the actual peer and
//! current source/runtime in the same transaction as the command; transport is not authorization.
use crate::wire::{
    self,
    native_mail_control_client::NativeMailControlClient,
    native_mail_control_server::{NativeMailControl, NativeMailControlServer},
};
use serde::Deserialize;
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use tonic::{Request, Response, Status, transport::Channel};
use webdock_plugin_runtime::{PeerIdentity, authenticated_peer};
pub const COMMAND_BYTES: usize = 160 * 1024;
pub const RESULT_BYTES: usize = 131_072;
pub const DECODING_BYTES: usize = COMMAND_BYTES + 1024;
pub const ENCODING_BYTES: usize = 132_096;
pub const CLIENT_DEADLINE: Duration = Duration::from_secs(15);
#[derive(Debug, thiserror::Error)]
#[error("Mail control request denied")]
pub struct ControlError {
    pub status: u16,
}
impl ControlError {
    pub fn invalid() -> Self {
        Self { status: 400 }
    }
    pub fn unavailable() -> Self {
        Self { status: 503 }
    }
    pub fn denied() -> Self {
        Self { status: 403 }
    }
}
#[async_trait::async_trait]
pub trait Control: Send + Sync + 'static {
    async fn call(
        &self,
        peer: &PeerIdentity,
        generation: u64,
        command: Value,
    ) -> Result<Value, ControlError>;
}
pub struct ControlServer {
    control: Arc<dyn Control>,
    permits: tokio::sync::Semaphore,
}
impl ControlServer {
    pub fn service(control: Arc<dyn Control>) -> NativeMailControlServer<Self> {
        NativeMailControlServer::new(Self {
            control,
            permits: tokio::sync::Semaphore::new(64),
        })
        .max_decoding_message_size(DECODING_BYTES)
        .max_encoding_message_size(ENCODING_BYTES)
    }
}
fn status(error: ControlError) -> Status {
    match error.status {
        400 => Status::invalid_argument("Invalid Mail control request"),
        401 | 403 => Status::permission_denied("Mail control request denied"),
        _ => Status::unavailable("Mail control unavailable"),
    }
}
#[tonic::async_trait]
impl NativeMailControl for ControlServer {
    async fn call(
        &self,
        request: Request<wire::NativeMailControlRequest>,
    ) -> Result<Response<wire::NativeMailControlReply>, Status> {
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("Mail control busy"))?;
        let peer = authenticated_peer(&request).map_err(|_| status(ControlError::denied()))?;
        let input = request.into_inner();
        if input.plugin_id != "mail" || input.generation == 0 || input.generation > i64::MAX as u64
        {
            return Err(status(ControlError::denied()));
        }
        let command = decode_command(&input.command_json).map_err(status)?;
        let result = self
            .control
            .call(&peer, input.generation, command)
            .await
            .map_err(status)?;
        let result_json =
            serde_json::to_vec(&result).map_err(|_| status(ControlError::unavailable()))?;
        if result_json.len() > RESULT_BYTES {
            return Err(status(ControlError::unavailable()));
        }
        Ok(Response::new(wire::NativeMailControlReply { result_json }))
    }
}
/// Decode one JSON value while rejecting duplicate keys, including nested authority fields.
#[derive(Deserialize)]
#[serde(transparent)]
struct Strict(#[serde(deserialize_with = "strict_value")] Value);
fn strict_value<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Value;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("JSON value")
        }
        fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
            Ok(v.into())
        }
        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
            Ok(v.into())
        }
        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
            Ok(v.into())
        }
        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
            serde_json::Number::from_f64(v)
                .map(Value::Number)
                .ok_or_else(|| E::custom("Invalid number"))
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
            Ok(v.into())
        }
        fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Value, E> {
            Ok(v.into())
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
            let mut v = Vec::new();
            while let Some(Strict(x)) = a.next_element()? {
                v.push(x);
            }
            Ok(Value::Array(v))
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> Result<Value, A::Error> {
            let mut v = serde_json::Map::new();
            while let Some(k) = a.next_key::<String>()? {
                if v.contains_key(&k) {
                    return Err(serde::de::Error::custom("Duplicate field"));
                }
                let Strict(x) = a.next_value()?;
                v.insert(k, x);
            }
            Ok(Value::Object(v))
        }
    }
    d.deserialize_any(Visitor)
}
pub fn decode_command(raw: &[u8]) -> Result<Value, ControlError> {
    if raw.len() > COMMAND_BYTES {
        return Err(ControlError::invalid());
    }
    let Strict(value) = serde_json::from_slice(raw).map_err(|_| ControlError::invalid())?;
    if !value.is_object() {
        return Err(ControlError::invalid());
    }
    Ok(value)
}
#[derive(Clone)]
pub struct NativeControlClient {
    client: NativeMailControlClient<Channel>,
    generation: u64,
}
impl NativeControlClient {
    /// Use the same authenticated/pinned callback Channel as the existing host connector.
    pub fn new(channel: Channel, generation: u64) -> Result<Self, ControlError> {
        if generation == 0 || generation > i64::MAX as u64 {
            return Err(ControlError::invalid());
        }
        Ok(Self {
            client: NativeMailControlClient::new(channel)
                .max_decoding_message_size(ENCODING_BYTES)
                .max_encoding_message_size(DECODING_BYTES),
            generation,
        })
    }
    pub async fn call(&self, command: Value) -> Result<Value, ControlError> {
        let command_json = serde_json::to_vec(&command).map_err(|_| ControlError::invalid())?;
        decode_command(&command_json)?;
        let mut request = Request::new(wire::NativeMailControlRequest {
            plugin_id: "mail".into(),
            generation: self.generation,
            command_json,
        });
        request.set_timeout(CLIENT_DEADLINE);
        let result = tokio::time::timeout(CLIENT_DEADLINE, self.client.clone().call(request))
            .await
            .map_err(|_| ControlError::unavailable())?
            .map_err(|e| ControlError {
                status: match e.code() {
                    tonic::Code::InvalidArgument => 400,
                    tonic::Code::PermissionDenied | tonic::Code::Unauthenticated => 403,
                    _ => 503,
                },
            })?
            .into_inner()
            .result_json;
        if result.len() > RESULT_BYTES {
            return Err(ControlError::unavailable());
        }
        serde_json::from_slice(&result).map_err(|_| ControlError::unavailable())
    }
}
