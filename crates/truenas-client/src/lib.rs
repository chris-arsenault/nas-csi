//! TrueNAS JSON-RPC request/response primitives.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::fmt;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    pub id: u64,
    pub result: Option<T>,
    pub error: Option<JsonRpcErrorObject>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct JsonRpcErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonRpcError {
    InvalidResponse(String),
    Remote(JsonRpcErrorObject),
    Serde(String),
}

impl fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidResponse(message) => write!(f, "invalid JSON-RPC response: {message}"),
            Self::Remote(error) => {
                write!(f, "remote JSON-RPC error {}: {}", error.code, error.message)
            }
            Self::Serde(message) => write!(f, "JSON serialization error: {message}"),
        }
    }
}

impl std::error::Error for JsonRpcError {}

pub fn serialize_request(request: &JsonRpcRequest) -> Result<String, JsonRpcError> {
    serde_json::to_string(request).map_err(|error| JsonRpcError::Serde(error.to_string()))
}

pub fn parse_response<T>(text: &str, expected_id: u64) -> Result<T, JsonRpcError>
where
    T: DeserializeOwned,
{
    let response: JsonRpcResponse<T> =
        serde_json::from_str(text).map_err(|error| JsonRpcError::Serde(error.to_string()))?;

    if response.jsonrpc != "2.0" {
        return Err(JsonRpcError::InvalidResponse(format!(
            "unexpected jsonrpc version {}",
            response.jsonrpc
        )));
    }

    if response.id != expected_id {
        return Err(JsonRpcError::InvalidResponse(format!(
            "expected id {}, got {}",
            expected_id, response.id
        )));
    }

    if let Some(error) = response.error {
        return Err(JsonRpcError::Remote(error));
    }

    response
        .result
        .ok_or_else(|| JsonRpcError::InvalidResponse("missing result".to_string()))
}

pub trait RpcTransport {
    type Error: std::error::Error + Send + Sync + 'static;

    fn send(&mut self, message: &str) -> Result<String, Self::Error>;
}

pub struct JsonRpcClient<T> {
    transport: T,
    next_id: u64,
}

impl<T> JsonRpcClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: 1,
        }
    }

    pub fn into_inner(self) -> T {
        self.transport
    }
}

impl<T> JsonRpcClient<T>
where
    T: RpcTransport,
{
    pub fn call<R>(&mut self, method: &str, params: Option<Value>) -> Result<R, ClientError>
    where
        R: DeserializeOwned,
    {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest::new(id, method, params);
        let request_text = serialize_request(&request)?;
        let response_text = self
            .transport
            .send(&request_text)
            .map_err(|error| ClientError::Transport(error.to_string()))?;
        parse_response(&response_text, id).map_err(ClientError::JsonRpc)
    }

    pub fn system_info(&mut self) -> Result<Value, ClientError> {
        self.call("system.info", None)
    }

    pub fn system_ready(&mut self) -> Result<bool, ClientError> {
        self.call("system.ready", None)
    }

    pub fn login_with_api_key(&mut self, api_key: &str) -> Result<bool, ClientError> {
        self.call("auth.login_with_api_key", Some(json!([api_key])))
    }

    pub fn pool_dataset_query(&mut self) -> Result<Vec<DatasetRecord>, ClientError> {
        self.call("pool.dataset.query", Some(json!([])))
    }

    pub fn sharing_smb_query(&mut self) -> Result<Vec<SmbShareRecord>, ClientError> {
        self.call("sharing.smb.query", Some(json!([])))
    }

    pub fn pool_dataset_create(&mut self, name: &str) -> Result<Value, ClientError> {
        self.call(
            "pool.dataset.create",
            Some(json!([{
                "name": name,
                "type": "FILESYSTEM"
            }])),
        )
    }

    pub fn pool_dataset_update_properties(
        &mut self,
        id: &str,
        properties: Value,
    ) -> Result<Value, ClientError> {
        self.call("pool.dataset.update", Some(json!([id, properties])))
    }

    pub fn pool_dataset_delete(&mut self, id: &str, recursive: bool) -> Result<bool, ClientError> {
        self.call(
            "pool.dataset.delete",
            Some(json!([id, { "recursive": recursive }])),
        )
    }

    pub fn pool_snapshot_query(&mut self) -> Result<Vec<SnapshotRecord>, ClientError> {
        self.call("pool.snapshot.query", Some(json!([])))
    }

    pub fn pool_snapshot_create(
        &mut self,
        dataset: &str,
        name: &str,
        recursive: bool,
    ) -> Result<SnapshotRecord, ClientError> {
        self.call(
            "pool.snapshot.create",
            Some(json!([{
                "dataset": dataset,
                "name": name,
                "recursive": recursive
            }])),
        )
    }

    pub fn pool_snapshot_delete(&mut self, id: &str) -> Result<bool, ClientError> {
        self.call("pool.snapshot.delete", Some(json!([id])))
    }

    pub fn sharing_smb_create(
        &mut self,
        name: &str,
        path: &str,
        enabled: bool,
    ) -> Result<SmbShareRecord, ClientError> {
        self.call(
            "sharing.smb.create",
            Some(json!([{
                "name": name,
                "path": path,
                "enabled": enabled
            }])),
        )
    }

    pub fn sharing_smb_update(
        &mut self,
        id: u64,
        name: Option<&str>,
        path: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<SmbShareRecord, ClientError> {
        let mut patch = serde_json::Map::new();
        if let Some(name) = name {
            patch.insert("name".to_string(), json!(name));
        }
        if let Some(path) = path {
            patch.insert("path".to_string(), json!(path));
        }
        if let Some(enabled) = enabled {
            patch.insert("enabled".to_string(), json!(enabled));
        }
        self.call("sharing.smb.update", Some(json!([id, patch])))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    JsonRpc(JsonRpcError),
    Transport(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::JsonRpc(error) => write!(f, "{error}"),
            Self::Transport(error) => write!(f, "transport error: {error}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<JsonRpcError> for ClientError {
    fn from(value: JsonRpcError) -> Self {
        Self::JsonRpc(value)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DatasetRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub mountpoint: Option<ValueField<String>>,
    #[serde(rename = "type", default)]
    pub kind: Option<ValueField<String>>,
}

impl DatasetRecord {
    pub fn mountpoint_value(&self) -> Option<&str> {
        self.mountpoint.as_ref().map(|field| field.value.as_str())
    }

    pub fn kind_value(&self) -> Option<&str> {
        self.kind.as_ref().map(|field| field.value.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SmbShareRecord {
    pub id: u64,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SnapshotRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub dataset: Option<String>,
    #[serde(default)]
    pub created: Option<ValueField<String>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ValueField<T> {
    pub value: T,
}

pub mod methods {
    use super::*;

    pub fn system_info(id: u64) -> JsonRpcRequest {
        JsonRpcRequest::new(id, "system.info", None)
    }

    pub fn system_ready(id: u64) -> JsonRpcRequest {
        JsonRpcRequest::new(id, "system.ready", None)
    }

    pub fn auth_login_with_api_key(id: u64, api_key: &str) -> JsonRpcRequest {
        JsonRpcRequest::new(id, "auth.login_with_api_key", Some(json!([api_key])))
    }

    pub fn pool_dataset_query(id: u64) -> JsonRpcRequest {
        JsonRpcRequest::new(id, "pool.dataset.query", Some(json!([])))
    }

    pub fn sharing_smb_query(id: u64) -> JsonRpcRequest {
        JsonRpcRequest::new(id, "sharing.smb.query", Some(json!([])))
    }

    pub fn pool_dataset_create(id: u64, name: &str) -> JsonRpcRequest {
        JsonRpcRequest::new(
            id,
            "pool.dataset.create",
            Some(json!([{ "name": name, "type": "FILESYSTEM" }])),
        )
    }

    pub fn pool_dataset_update_properties(
        id: u64,
        dataset: &str,
        properties: Value,
    ) -> JsonRpcRequest {
        JsonRpcRequest::new(
            id,
            "pool.dataset.update",
            Some(json!([dataset, properties])),
        )
    }

    pub fn pool_dataset_delete(id: u64, dataset: &str, recursive: bool) -> JsonRpcRequest {
        JsonRpcRequest::new(
            id,
            "pool.dataset.delete",
            Some(json!([dataset, { "recursive": recursive }])),
        )
    }

    pub fn pool_snapshot_query(id: u64) -> JsonRpcRequest {
        JsonRpcRequest::new(id, "pool.snapshot.query", Some(json!([])))
    }

    pub fn pool_snapshot_create(
        id: u64,
        dataset: &str,
        name: &str,
        recursive: bool,
    ) -> JsonRpcRequest {
        JsonRpcRequest::new(
            id,
            "pool.snapshot.create",
            Some(json!([{ "dataset": dataset, "name": name, "recursive": recursive }])),
        )
    }

    pub fn pool_snapshot_delete(id: u64, snapshot_id: &str) -> JsonRpcRequest {
        JsonRpcRequest::new(id, "pool.snapshot.delete", Some(json!([snapshot_id])))
    }

    pub fn sharing_smb_create(id: u64, name: &str, path: &str, enabled: bool) -> JsonRpcRequest {
        JsonRpcRequest::new(
            id,
            "sharing.smb.create",
            Some(json!([{ "name": name, "path": path, "enabled": enabled }])),
        )
    }

    pub fn sharing_smb_update(id: u64, share_id: u64, patch: Value) -> JsonRpcRequest {
        JsonRpcRequest::new(id, "sharing.smb.update", Some(json!([share_id, patch])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::io;

    #[test]
    fn serializes_request() {
        let request = methods::system_info(7);
        let text = serialize_request(&request).expect("serialize");
        assert_eq!(text, r#"{"jsonrpc":"2.0","id":7,"method":"system.info"}"#);
    }

    #[test]
    fn parses_success_response() {
        let value: Value =
            parse_response(r#"{"jsonrpc":"2.0","id":2,"result":{"version":"x"}}"#, 2)
                .expect("parse response");
        assert_eq!(value, json!({"version": "x"}));
    }

    #[test]
    fn parses_remote_error() {
        let error = parse_response::<Value>(
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":123,"message":"nope"}}"#,
            2,
        )
        .expect_err("remote error");

        assert!(matches!(error, JsonRpcError::Remote(_)));
    }

    #[test]
    fn client_increments_ids_and_calls_transport() {
        let transport = FakeTransport::new(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"version":"test"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":{"version":"again"}}"#,
        ]);
        let mut client = JsonRpcClient::new(transport);

        let first = client.system_info().expect("first call");
        let second = client.system_info().expect("second call");
        let transport = client.into_inner();

        assert_eq!(first, json!({"version": "test"}));
        assert_eq!(second, json!({"version": "again"}));
        assert!(transport.sent[0].contains(r#""id":1"#));
        assert!(transport.sent[1].contains(r#""id":2"#));
    }

    #[test]
    fn builds_api_key_login_request() {
        let request = methods::auth_login_with_api_key(9, "secret-key");
        let text = serialize_request(&request).expect("serialize");
        assert_eq!(
            text,
            r#"{"jsonrpc":"2.0","id":9,"method":"auth.login_with_api_key","params":["secret-key"]}"#
        );
    }

    #[test]
    fn client_can_login_with_api_key() {
        let transport = FakeTransport::new(vec![r#"{"jsonrpc":"2.0","id":1,"result":true}"#]);
        let mut client = JsonRpcClient::new(transport);

        assert!(client.login_with_api_key("secret-key").expect("login"));
    }

    #[test]
    fn parses_dataset_records() {
        let result: Vec<DatasetRecord> = parse_response(
            r#"{"jsonrpc":"2.0","id":1,"result":[{"id":"pool/data","name":"pool/data","type":{"value":"FILESYSTEM"},"mountpoint":{"value":"/mnt/pool/data"}}]}"#,
            1,
        )
        .expect("parse datasets");

        assert_eq!(result[0].name, "pool/data");
        assert_eq!(result[0].kind_value(), Some("FILESYSTEM"));
        assert_eq!(result[0].mountpoint_value(), Some("/mnt/pool/data"));
    }

    #[test]
    fn builds_dataset_snapshot_and_smb_requests() {
        assert_eq!(
            serialize_request(&methods::pool_dataset_create(1, "tank/repos")).expect("dataset"),
            r#"{"jsonrpc":"2.0","id":1,"method":"pool.dataset.create","params":[{"name":"tank/repos","type":"FILESYSTEM"}]}"#
        );
        assert_eq!(
            serialize_request(&methods::pool_snapshot_create(
                2,
                "tank/repos",
                "nas-csi-manual",
                false
            ))
            .expect("snapshot"),
            r#"{"jsonrpc":"2.0","id":2,"method":"pool.snapshot.create","params":[{"dataset":"tank/repos","name":"nas-csi-manual","recursive":false}]}"#
        );
        assert_eq!(
            serialize_request(&methods::sharing_smb_create(
                3,
                "repos",
                "/mnt/tank/repos",
                true
            ))
            .expect("smb"),
            r#"{"jsonrpc":"2.0","id":3,"method":"sharing.smb.create","params":[{"enabled":true,"name":"repos","path":"/mnt/tank/repos"}]}"#
        );
    }

    #[test]
    fn client_exposes_mutating_dataset_snapshot_and_smb_calls() {
        let transport = FakeTransport::new(vec![
            r#"{"jsonrpc":"2.0","id":1,"result":{"id":"tank/scratch"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"result":{"id":"tank/scratch@manual","name":"manual","dataset":"tank/scratch"}}"#,
            r#"{"jsonrpc":"2.0","id":3,"result":{"id":7,"name":"scratch","path":"/mnt/tank/scratch","enabled":true}}"#,
            r#"{"jsonrpc":"2.0","id":4,"result":true}"#,
        ]);
        let mut client = JsonRpcClient::new(transport);

        let dataset = client
            .pool_dataset_create("tank/scratch")
            .expect("create dataset");
        let snapshot = client
            .pool_snapshot_create("tank/scratch", "manual", false)
            .expect("create snapshot");
        let share = client
            .sharing_smb_create("scratch", "/mnt/tank/scratch", true)
            .expect("create share");
        assert!(
            client
                .pool_dataset_delete("tank/scratch", false)
                .expect("delete dataset")
        );

        assert_eq!(dataset, json!({"id": "tank/scratch"}));
        assert_eq!(snapshot.id, "tank/scratch@manual");
        assert_eq!(share.id, 7);
        let transport = client.into_inner();
        assert!(transport.sent[0].contains("pool.dataset.create"));
        assert!(transport.sent[1].contains("pool.snapshot.create"));
        assert!(transport.sent[2].contains("sharing.smb.create"));
        assert!(transport.sent[3].contains("pool.dataset.delete"));
    }

    struct FakeTransport {
        responses: VecDeque<String>,
        sent: Vec<String>,
    }

    impl FakeTransport {
        fn new(responses: Vec<&str>) -> Self {
            Self {
                responses: responses
                    .into_iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                sent: Vec::new(),
            }
        }
    }

    impl RpcTransport for FakeTransport {
        type Error = io::Error;

        fn send(&mut self, message: &str) -> Result<String, Self::Error> {
            self.sent.push(message.to_string());
            self.responses
                .pop_front()
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no response"))
        }
    }
}
