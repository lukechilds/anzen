//! The few Trezor protocol-buffer messages this crate exchanges, declared by hand from Trezor's
//! `common/protob` definitions (no build step, no `protoc`).
//!
//! Every field is optional here, whatever the definition says: a required field left at its
//! default would otherwise be left off the wire, and the Trezor refuses a message that lacks one.

use prost::Message;

/// Wire ids of the messages (`MessageType` and `ThpMessageType` in Trezor's definitions).
pub mod kind {
    pub const SUCCESS: u16 = 2;
    pub const FAILURE: u16 = 3;
    pub const BUTTON_REQUEST: u16 = 26;
    pub const BUTTON_ACK: u16 = 27;
    pub const CREATE_NEW_SESSION: u16 = 1000;
    pub const PAIRING_REQUEST: u16 = 1008;
    pub const PAIRING_REQUEST_APPROVED: u16 = 1009;
    pub const SELECT_METHOD: u16 = 1010;
    pub const CREDENTIAL_REQUEST: u16 = 1016;
    pub const CREDENTIAL_RESPONSE: u16 = 1017;
    pub const END_REQUEST: u16 = 1018;
    pub const END_RESPONSE: u16 = 1019;
    pub const CODE_ENTRY_COMMITMENT: u16 = 1024;
    pub const CODE_ENTRY_CHALLENGE: u16 = 1025;
    pub const CODE_ENTRY_CPACE_TREZOR: u16 = 1026;
    pub const CODE_ENTRY_CPACE_HOST_TAG: u16 = 1027;
    pub const CODE_ENTRY_SECRET: u16 = 1028;
    pub const APP_LOAD: u16 = 9200;
    pub const APP_LOADED: u16 = 9201;
    pub const APP_HEADER_REQUEST: u16 = 9202;
    pub const APP_HEADER_ACK: u16 = 9203;
    pub const APP_ROOT_PACKET_REQUEST: u16 = 9204;
    pub const APP_ROOT_PACKET_ACK: u16 = 9205;
    pub const APP_DATA_CHUNK_REQUEST: u16 = 9206;
    pub const APP_DATA_CHUNK_ACK: u16 = 9207;
    pub const APP_MESSAGE: u16 = 9208;
    pub const APP_RESPONSE: u16 = 9209;
    pub const DEBUG_DECISION: u16 = 100;
    pub const DEBUG_GET_STATE: u16 = 101;
    pub const DEBUG_STATE: u16 = 102;
    pub const DEBUG_GET_PAIRING_INFO: u16 = 9011;
    pub const DEBUG_PAIRING_INFO: u16 = 9012;
}

/// `ThpPairingMethod`.
pub mod pairing_method {
    pub const SKIP: i32 = 1;
    pub const CODE_ENTRY: i32 = 2;
}

/// `Failure.FailureType` values this crate tells apart.
pub mod failure {
    pub const ACTION_CANCELLED: i32 = 4;
    pub const PIN_CANCELLED: i32 = 6;
    pub const NOT_INITIALIZED: i32 = 11;
}

#[derive(Clone, PartialEq, Message)]
pub struct Failure {
    #[prost(int32, optional, tag = "1")]
    pub code: Option<i32>,
    #[prost(string, optional, tag = "2")]
    pub message: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct ButtonRequest {
    #[prost(int32, optional, tag = "1")]
    pub code: Option<i32>,
    #[prost(string, optional, tag = "4")]
    pub name: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DeviceProperties {
    #[prost(string, optional, tag = "1")]
    pub internal_model: Option<String>,
    #[prost(uint32, optional, tag = "3")]
    pub protocol_version_major: Option<u32>,
    #[prost(uint32, optional, tag = "4")]
    pub protocol_version_minor: Option<u32>,
    #[prost(int32, repeated, packed = "false", tag = "5")]
    pub pairing_methods: Vec<i32>,
}

/// The payload of the handshake's last message: the credential that shows this host is paired.
#[derive(Clone, PartialEq, Message)]
pub struct HandshakeCompletion {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub host_pairing_credential: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CreateNewSession {
    #[prost(string, optional, tag = "1")]
    pub passphrase: Option<String>,
    #[prost(bool, optional, tag = "2")]
    pub on_device: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct PairingRequest {
    #[prost(string, optional, tag = "1")]
    pub host_name: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub app_name: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SelectMethod {
    #[prost(int32, optional, tag = "1")]
    pub selected_pairing_method: Option<i32>,
}

/// One `bytes` field, tag 1: the shape of most pairing messages.
#[derive(Clone, PartialEq, Message)]
pub struct Bytes {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub value: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CodeEntryCpaceHostTag {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub cpace_host_public_key: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub tag: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CredentialRequest {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub host_static_public_key: Option<Vec<u8>>,
    #[prost(bool, optional, tag = "2")]
    pub autoconnect: Option<bool>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CredentialResponse {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub trezor_static_public_key: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub credential: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Version {
    #[prost(uint32, optional, tag = "1")]
    pub major: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub minor: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub patch: Option<u32>,
    #[prost(uint32, optional, tag = "4")]
    pub build: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppLoad {
    #[prost(string, optional, tag = "1")]
    pub id: Option<String>,
    #[prost(message, optional, tag = "2")]
    pub version: Option<Version>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub fingerprint: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppLoaded {
    #[prost(uint32, optional, tag = "1")]
    pub instance_id: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppHeaderAck {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub header: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub proof: Option<Vec<u8>>,
    #[prost(sint64, optional, tag = "3")]
    pub root_packet_timestamp: Option<i64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppDataChunkRequest {
    #[prost(uint32, optional, tag = "1")]
    pub index: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppDataChunkAck {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub data: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub hash: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppMessage {
    #[prost(uint32, optional, tag = "1")]
    pub instance_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub message_id: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub data: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct AppResponse {
    #[prost(uint32, optional, tag = "1")]
    pub message_id: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub data: Option<Vec<u8>>,
}

/// Debug link (emulator and debug firmware only): press a button.
#[derive(Clone, PartialEq, Message)]
pub struct DebugDecision {
    /// 0 rejects, 1 confirms.
    #[prost(int32, optional, tag = "1")]
    pub button: Option<i32>,
    /// A touch at this point of the screen.
    #[prost(uint32, optional, tag = "4")]
    pub x: Option<u32>,
    #[prost(uint32, optional, tag = "5")]
    pub y: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DebugGetState {
    /// 0 answers at once, 1 waits for the next screen, 2 for a screen to be up.
    #[prost(int32, optional, tag = "3")]
    pub wait_layout: Option<i32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DebugState {
    /// The screen's content, as text fragments.
    #[prost(string, repeated, tag = "13")]
    pub tokens: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DebugGetPairingInfo {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub channel_id: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub handshake_hash: Option<Vec<u8>>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DebugPairingInfo {
    #[prost(uint32, optional, tag = "3")]
    pub code_entry_code: Option<u32>,
}

pub fn encode(message: &impl Message) -> Vec<u8> {
    message.encode_to_vec()
}

pub fn decode<M: Message + Default>(bytes: &[u8]) -> anyhow::Result<M> {
    M::decode(bytes).map_err(|_| anyhow::anyhow!("the Trezor sent a message this app cannot read"))
}

/// A `bytes` field that must be present.
pub fn bytes(message: Bytes, what: &str) -> anyhow::Result<Vec<u8>> {
    message
        .value
        .ok_or_else(|| anyhow::anyhow!("the Trezor's answer lacks its {what}"))
}
