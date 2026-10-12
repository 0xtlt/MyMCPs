use crate::crypto::random_hex;
use crate::models::TeamRole;
use crate::time::Timestamp;

model! {
    /// A link that lets one email address into one team. Whoever follows it
    /// joins with the account of that address, or makes that account first.
    table = "invites", created_at = true, updated_at = true;
    pub struct Invite {
        pub token: String,
        pub email: String,
        /// What the person will be in the team.
        pub role: TeamRole,
        pub created_by: i64,
        pub accepted_at: Option<Timestamp>,
        pub expires_at: Timestamp,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
        pub team_id: i64,
    }
}

impl Invite {
    pub fn is_accepted(&self) -> bool {
        self.accepted_at.is_some()
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.is_past()
    }

    pub fn is_usable(&self) -> bool {
        !self.is_accepted() && !self.is_expired()
    }

    pub fn generate_token() -> String {
        random_hex(32)
    }
}
