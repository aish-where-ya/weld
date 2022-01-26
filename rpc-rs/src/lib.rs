//! Wasmcloud Weld runtime library
//!
//! This crate provides code generation and runtime support for wasmcloud rpc messages
//! used by [wasmcloud](https://wasmcloud.dev) actors and capability providers.
//!

mod timestamp;
// re-export Timestamp
pub use timestamp::Timestamp;

mod actor_wasm;
pub mod common;
use common::{deserialize, serialize, Context, Message, MessageDispatch, SendOpts, Transport};
pub mod channel_log;
pub mod provider;
pub(crate) mod provider_main;
mod wasmbus_model;
pub mod model {
    // re-export model lib as "model"
    pub use crate::wasmbus_model::*;
}
pub mod cbor;

// re-export nats-aflowt
#[cfg(not(target_arch = "wasm32"))]
pub use nats_aflowt as anats;

/// This will be removed in a later version - use cbor instead to avoid dependence on minicbor crate
/// @deprecated
pub use minicbor;

#[cfg(not(target_arch = "wasm32"))]
pub mod rpc_client;

pub type RpcResult<T> = std::result::Result<T, RpcError>;

/// import module for webassembly linking
#[doc(hidden)]
pub const WASMBUS_RPC_IMPORT_NAME: &str = "wasmbus";

/// This crate's published version
pub const WELD_CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub type CallResult = std::result::Result<Vec<u8>, Box<dyn std::error::Error + Sync + Send>>;
pub type HandlerResult<T> = std::result::Result<T, Box<dyn std::error::Error + Sync + Send>>;
pub type TomlMap = toml::value::Map<String, toml::value::Value>;

mod wasmbus_core;
pub mod core {
    // re-export core lib as "core"
    pub use crate::wasmbus_core::*;
    use crate::{RpcError, RpcResult};
    use std::convert::TryFrom;

    cfg_if::cfg_if! {
        if #[cfg(not(target_arch = "wasm32"))] {

            // allow testing provider outside host
            const TEST_HARNESS: &str = "_TEST_";
            // fallback nats address if host doesn't pass one to provider
            const DEFAULT_NATS_ADDR: &str = "nats://127.0.0.1:4222";

            impl HostData {
                /// returns whether the provider is running under test
                pub fn is_test(&self) -> bool {
                    self.host_id == TEST_HARNESS
                }

                /// Connect to nats using options provided by host
                pub async fn nats_connect(&self) -> RpcResult<crate::anats::Connection> {
                    use std::str::FromStr as _;
                    let nats_addr = if !self.lattice_rpc_url.is_empty() {
                        self.lattice_rpc_url.as_str()
                    } else {
                        DEFAULT_NATS_ADDR
                    };
                    let nats_server = nats_aflowt::ServerAddress::from_str(nats_addr).map_err(|e| {
                        RpcError::InvalidParameter(format!("Invalid nats server url '{}': {}", nats_addr, e))
                    })?;

                    // Connect to nats
                    let nc = nats_aflowt::Options::default()
                        .max_reconnects(None)
                        .connect(vec![nats_server])
                        .await
                        .map_err(|e| {
                            RpcError::ProviderInit(format!("nats connection to {} failed: {}", nats_addr, e))
                        })?;
                    Ok(nc)
                }
            }
        }
    }

    /// url scheme for wasmbus protocol messages
    pub const URL_SCHEME: &str = "wasmbus";

    impl std::fmt::Display for WasmCloudEntity {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.url())
        }
    }

    impl LinkDefinition {
        pub fn actor_entity(&self) -> WasmCloudEntity {
            WasmCloudEntity {
                public_key: self.actor_id.clone(),
                contract_id: String::default(),
                link_name: String::default(),
            }
        }

        pub fn provider_entity(&self) -> WasmCloudEntity {
            WasmCloudEntity {
                public_key: self.provider_id.clone(),
                contract_id: self.contract_id.clone(),
                link_name: self.link_name.clone(),
            }
        }
    }

    impl WasmCloudEntity {
        /// constructor for actor entity
        pub fn new_actor<T: ToString>(public_key: T) -> RpcResult<WasmCloudEntity> {
            let public_key = public_key.to_string();
            if public_key.is_empty() {
                return Err(RpcError::InvalidParameter(
                    "public_key may not be empty".to_string(),
                ));
            }
            Ok(WasmCloudEntity {
                public_key,
                contract_id: String::new(),
                link_name: String::new(),
            })
        }

        /*
        /// create provider entity from link definition
        pub fn from_link(link: &LinkDefinition) -> Self {
            WasmCloudEntity {
                public_key: link.provider_id.clone(),
                contract_id: link.contract_id.clone(),
                link_name: link.link_name.clone(),
            }
        }
         */

        /// constructor for capability provider entity
        /// all parameters are required
        pub fn new_provider<T1: ToString, T2: ToString>(
            contract_id: T1,
            link_name: T2,
        ) -> RpcResult<WasmCloudEntity> {
            let contract_id = contract_id.to_string();
            if contract_id.is_empty() {
                return Err(RpcError::InvalidParameter(
                    "contract_id may not be empty".to_string(),
                ));
            }
            let link_name = link_name.to_string();
            if link_name.is_empty() {
                return Err(RpcError::InvalidParameter(
                    "link_name may not be empty".to_string(),
                ));
            }
            Ok(WasmCloudEntity {
                public_key: "".to_string(),
                contract_id,
                link_name,
            })
        }

        /// Returns URL of the entity
        pub fn url(&self) -> String {
            if self.public_key.to_uppercase().starts_with('M') {
                format!("{}://{}", crate::core::URL_SCHEME, self.public_key)
            } else {
                format!(
                    "{}://{}/{}/{}",
                    URL_SCHEME,
                    self.contract_id
                        .replace(':', "/")
                        .replace(' ', "_")
                        .to_lowercase(),
                    self.link_name.replace(' ', "_").to_lowercase(),
                    self.public_key
                )
            }
        }

        /// Returns the unique (public) key of the entity
        pub fn public_key(&self) -> String {
            self.public_key.to_string()
        }

        /// returns true if this entity refers to an actor
        pub fn is_actor(&self) -> bool {
            self.link_name.is_empty() || self.contract_id.is_empty()
        }

        /// returns true if this entity refers to a provider
        pub fn is_provider(&self) -> bool {
            !self.is_actor()
        }
    }

    impl TryFrom<&str> for WasmCloudEntity {
        type Error = RpcError;

        /// converts string into actor entity
        fn try_from(target: &str) -> Result<WasmCloudEntity, Self::Error> {
            WasmCloudEntity::new_actor(target.to_string())
        }
    }

    impl TryFrom<String> for WasmCloudEntity {
        type Error = RpcError;

        /// converts string into actor entity
        fn try_from(target: String) -> Result<WasmCloudEntity, Self::Error> {
            WasmCloudEntity::new_actor(target)
        }
    }
}

pub mod actor {

    pub mod prelude {
        pub use crate::{
            common::{Context, Message, MessageDispatch, Transport},
            core::{Actor, ActorReceiver},
            RpcError, RpcResult,
        };

        // re-export async_trait
        pub use async_trait::async_trait;
        // derive macros
        pub use wasmbus_macros::{Actor, ActorHealthResponder as HealthResponder};

        #[cfg(feature = "BigInteger")]
        pub use num_bigint::BigInt as BigInteger;

        #[cfg(feature = "BigDecimal")]
        pub use bigdecimal::BigDecimal;

        cfg_if::cfg_if! {

            if #[cfg(target_arch = "wasm32")] {
                pub use crate::actor_wasm::{console_log, WasmHost};
            } else {
                // this code is non-functional, since actors only run in wasm32,
                // but it reduces compiler errors if you are building a cargo multi-project workspace for non-wasm32
                #[derive(Clone, Debug, Default)]
                pub struct WasmHost {}

                #[async_trait]
                impl crate::Transport for WasmHost {
                    async fn send(&self, _ctx: &Context,
                                _msg: Message<'_>, _opts: Option<crate::SendOpts> ) -> crate::RpcResult<Vec<u8>> {
                       unimplemented!();
                    }
                    fn set_timeout(&self, _interval: std::time::Duration) {
                       unimplemented!();
                    }
                }

                pub fn console_log(_s: &str) {}
            }
        }
    }
}

/// An error that can occur in the processing of an RPC. This is not request-specific errors but
/// rather cross-cutting errors that can always occur.
#[derive(thiserror::Error, Debug, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum RpcError {
    /// The request exceeded its deadline.
    #[error("the request exceeded its deadline: {0}")]
    DeadlineExceeded(String),

    /// A capability provider was called before its configure_dispatch was called.
    #[error("the capability provider has not been initialized: {0}")]
    NotInitialized(String),

    #[error("method not handled {0}")]
    MethodNotHandled(String),

    /// Error that can be returned if server has not implemented
    /// an optional interface method
    #[error("method not implemented")]
    NotImplemented,

    #[error("Host send error {0}")]
    HostError(String),

    #[error("deserialization: {0}")]
    Deser(String),

    #[error("serialization: {0}")]
    Ser(String),

    #[error("rpc: {0}")]
    Rpc(String),

    #[error("nats: {0}")]
    Nats(String),

    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    /// Error occurred in actor's rpc handler
    #[error("actor: {0}")]
    ActorHandler(String),

    /// Error occurred during provider initialization or put-link
    #[error("provider initialization or put-link: {0}")]
    ProviderInit(String),

    /// Timeout occurred
    #[error("timeout: {0}")]
    Timeout(String),

    //#[error("IO error")]
    //IO([from] std::io::Error)
    /// Anything else
    #[error("{0}")]
    Other(String),
}

impl From<String> for RpcError {
    fn from(s: String) -> RpcError {
        RpcError::Other(s)
    }
}

impl From<&str> for RpcError {
    fn from(s: &str) -> RpcError {
        RpcError::Other(s.to_string())
    }
}

impl From<std::io::Error> for RpcError {
    fn from(e: std::io::Error) -> RpcError {
        RpcError::Other(format!("io: {}", e))
    }
}

impl From<minicbor::encode::Error<std::io::Error>> for RpcError {
    fn from(e: minicbor::encode::Error<std::io::Error>) -> RpcError {
        RpcError::Ser(format!("cbor-encode: {}", e))
    }
}

impl From<minicbor::encode::Error<RpcError>> for RpcError {
    fn from(e: minicbor::encode::Error<RpcError>) -> RpcError {
        RpcError::Ser(format!("cbor-encode: {}", e))
    }
}

impl From<minicbor::decode::Error> for RpcError {
    fn from(e: minicbor::decode::Error) -> RpcError {
        RpcError::Deser(format!("cbor-decode: {}", e))
    }
}
