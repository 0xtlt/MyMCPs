use crate::time::Timestamp;

string_enum! {
    /// What a person decided. `pending` until they have.
    pub enum ApprovalDecision {
        #[default]
        Pending => "pending",
        Approved => "approved",
        Denied => "denied",
    }
}

string_enum! {
    /// Where a request stands for whoever reads it: `used` once the agent ran
    /// the approved call, and `expired` when nobody decided in time or the
    /// approved call was never run.
    pub enum ApprovalState {
        #[default]
        Pending => "pending",
        Approved => "approved",
        Used => "used",
        Denied => "denied",
        Expired => "expired",
    }
}

model! {
    /// A tool call an agent made that waits for a person, or has been
    /// decided. It keeps the call exactly as the agent made it: an approval
    /// is for that call and no other.
    table = "approval_requests", created_at = true, updated_at = true;
    pub struct ApprovalRequest {
        pub public_id: String,
        pub mcp_id: i64,
        pub access_token_id: i64,
        pub tool_name: String,
        pub arguments: String,
        pub arguments_hash: String,
        pub summary: String,
        pub status: ApprovalDecision,
        pub decided_by: Option<i64>,
        pub decided_at: Option<Timestamp>,
        pub consumed_at: Option<Timestamp>,
        pub expires_at: Timestamp,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
    }
}

impl ApprovalRequest {
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Timestamp::now()
    }

    pub fn state(&self) -> ApprovalState {
        match self.status {
            ApprovalDecision::Denied => ApprovalState::Denied,
            ApprovalDecision::Approved if self.consumed_at.is_some() => ApprovalState::Used,
            _ if self.is_expired() => ApprovalState::Expired,
            ApprovalDecision::Approved => ApprovalState::Approved,
            ApprovalDecision::Pending => ApprovalState::Pending,
        }
    }
}
