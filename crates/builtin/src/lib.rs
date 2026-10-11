//! What the built-in MCPs share: how a provider is described, and the
//! services its tools use.

pub mod arguments;
pub mod definition;
pub mod error;
pub mod file_link;
pub mod keys;
pub mod oauth;
pub mod places;
pub mod tool_input;
pub mod upload_store;

pub use definition::{
    ApprovalDetail, ApprovalSummary, BuiltinEnv, BuiltinFile, BuiltinMcpDefinition,
    BuiltinOauthConfig, BuiltinPasswordConfig, BuiltinPasswordContext, BuiltinProvider,
    BuiltinRegistry, BuiltinSettingField, BuiltinTool, BuiltinToolContext, BuiltinToolInfo,
    ToolInput,
};
pub use error::{BuiltinError, BuiltinResult};
pub use upload_store::{BuiltinUpload, BuiltinUploadTarget, UploadStore};
