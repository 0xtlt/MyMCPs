use crate::crypto::random_hex;
use crate::models::UserRole;
use crate::time::Timestamp;

model! {
    table = "invites", created_at = true, updated_at = true;
    pub struct Invite {
        pub token: String,
        pub email: String,
        pub role: UserRole,
        pub created_by: i64,
        pub accepted_at: Option<Timestamp>,
        pub expires_at: Timestamp,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
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
