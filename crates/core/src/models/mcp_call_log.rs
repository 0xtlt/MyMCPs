use crate::time::Timestamp;

string_enum! {
    pub enum CallOutcome {
        #[default]
        Success => "success",
        Error => "error",
    }
}

string_enum! {
    pub enum CallErrorCategory {
        #[default]
        InvalidTool => "invalid_tool",
        DisallowedMcp => "disallowed_mcp",
        UpstreamException => "upstream_exception",
        ToolError => "tool_error",
        /// The call waits for a person to approve it, and was not run.
        ApprovalRequired => "approval_required",
        /// A person refused the call, and it was not run.
        ApprovalDenied => "approval_denied",
    }
}

model! {
    table = "mcp_call_logs", created_at = true, updated_at = false;
    pub struct McpCallLog {
        pub access_token_id: Option<i64>,
        pub access_token_name: String,
        pub access_token_prefix: String,
        pub mcp_id: Option<i64>,
        pub mcp_name: Option<String>,
        pub mcp_slug: Option<String>,
        pub requested_tool_name: String,
        pub tool_name: Option<String>,
        pub outcome: CallOutcome,
        pub error_category: Option<CallErrorCategory>,
        pub error_summary: Option<String>,
        pub arguments: Option<String>,
        pub arguments_captured: bool,
        pub duration_ms: i64,
        pub created_at: Timestamp,
        pub response: Option<String>,
        pub response_captured: bool,
        pub caller_ip: Option<String>,
    }
}
