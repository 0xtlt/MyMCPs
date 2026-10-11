use sqlx::sqlite::SqliteExecutor;
use sqlx::{Sqlite, Transaction};

use crate::models::User;
use crate::time::Timestamp;

string_enum! {
    /// What someone may do in a team, least first.
    ///
    /// A member reads what the team has and makes access tokens of their
    /// own. An administrator also changes it: MCPs, members, invites. An
    /// owner also deletes the team, and decides who else owns it.
    pub enum TeamRole {
        #[default]
        Member => "member",
        Admin => "admin",
        Owner => "owner",
    }
}

impl TeamRole {
    fn rank(self) -> u8 {
        match self {
            Self::Member => 0,
            Self::Admin => 1,
            Self::Owner => 2,
        }
    }

    /// An administrator or an owner.
    pub fn is_admin(self) -> bool {
        self.rank() >= Self::Admin.rank()
    }

    pub fn is_owner(self) -> bool {
        self == Self::Owner
    }

    /// The roles someone with this one may give: an administrator up to
    /// their own, an owner any, a member none.
    pub fn may_grant(self, role: TeamRole) -> bool {
        self.is_admin() && role.rank() <= self.rank()
    }

    /// Whether someone with this role may change or remove a member who has
    /// `other`: never one ranked above them.
    pub fn may_manage(self, other: TeamRole) -> bool {
        self.is_admin() && other.rank() <= self.rank()
    }
}

impl PartialOrd for TeamRole {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TeamRole {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

model! {
    /// What MCPs, access tokens and call logs belong to. Every account has
    /// a personal team, and may be a member of others.
    table = "teams", created_at = true, updated_at = true;
    pub struct Team {
        pub name: String,
        pub description: Option<String>,
        /// The team an account came with. It is never deleted on its own.
        pub personal_team: bool,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
    }
}

model! {
    /// One person in one team.
    table = "team_user", created_at = true, updated_at = true;
    pub struct TeamMember {
        pub team_id: i64,
        pub user_id: i64,
        pub role: TeamRole,
        pub created_at: Timestamp,
        pub updated_at: Option<Timestamp>,
    }
}

/// A team, and what someone is in it.
#[derive(Debug, Clone)]
pub struct Membership {
    pub team: Team,
    pub role: TeamRole,
}

impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for Membership {
    fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            team: Team::from_row(row)?,
            role: row.try_get("member_role")?,
        })
    }
}

/// The columns of a [`Membership`]: join `teams` with `team_user`.
const MEMBERSHIP_COLUMNS: &str = "`teams`.*, `team_user`.`role` as `member_role`";

/// Orders the rows of `team_user` owners first, members last.
const BY_RANK: &str = "case `team_user`.`role` when 'owner' then 0 when 'admin' then 1 else 2 end";

/// Why an account cannot be deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountRemovalRefusal {
    /// Nobody else is in the Root Team: the instance would have no one to run it.
    LastOfRootTeam,
}

impl Team {
    /// The team of the instance. The migration that added teams created it,
    /// and it is never deleted. Its owners and administrators run the
    /// instance: its settings, its backups, its accounts.
    pub const ROOT_ID: i64 = 1;

    pub fn is_root(&self) -> bool {
        self.id == Self::ROOT_ID
    }

    /// What the personal team of an account is called.
    pub fn personal_name(user: &User) -> String {
        let name: String = user.display_name().chars().take(113).collect();
        format!("{name}'s Team")
    }

    /// Create the personal team of an account, which owns it.
    pub async fn create_personal(
        transaction: &mut Transaction<'_, Sqlite>,
        user: &User,
    ) -> Result<Self, sqlx::Error> {
        let mut team = Self {
            name: Self::personal_name(user),
            personal_team: true,
            ..Default::default()
        };
        team.insert(&mut **transaction).await?;
        Self::attach(&mut **transaction, team.id, user.id, TeamRole::Owner).await?;
        Ok(team)
    }

    /// Create a team that `owner` owns.
    pub async fn create_owned(
        transaction: &mut Transaction<'_, Sqlite>,
        name: &str,
        description: Option<&str>,
        owner: &User,
    ) -> Result<Self, sqlx::Error> {
        let mut team = Self {
            name: name.to_string(),
            description: description.map(str::to_string),
            ..Default::default()
        };
        team.insert(&mut **transaction).await?;
        Self::attach(&mut **transaction, team.id, owner.id, TeamRole::Owner).await?;
        Ok(team)
    }

    /// Make someone a member of a team. Fails when they already are one.
    pub async fn attach<'e, E>(
        db: E,
        team_id: i64,
        user_id: i64,
        role: TeamRole,
    ) -> Result<(), sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        let mut member = TeamMember {
            team_id,
            user_id,
            role,
            ..Default::default()
        };
        member.insert(db).await
    }

    /// What someone is in a team, or `None` when they are not in it.
    pub async fn role_of<'e, E>(
        db: E,
        team_id: i64,
        user_id: i64,
    ) -> Result<Option<TeamRole>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_scalar("select `role` from `team_user` where `team_id` = ? and `user_id` = ?")
            .bind(team_id)
            .bind(user_id)
            .fetch_optional(db)
            .await
    }

    /// One team of an account, with what the account is in it.
    pub async fn membership<'e, E>(
        db: E,
        team_id: i64,
        user_id: i64,
    ) -> Result<Option<Membership>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {MEMBERSHIP_COLUMNS} from `teams` \
             inner join `team_user` on `team_user`.`team_id` = `teams`.`id` \
             where `teams`.`id` = ? and `team_user`.`user_id` = ?"
        )))
        .bind(team_id)
        .bind(user_id)
        .fetch_optional(db)
        .await
    }

    /// The teams of an account, in the order it joined them.
    pub async fn memberships<'e, E>(db: E, user_id: i64) -> Result<Vec<Membership>, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {MEMBERSHIP_COLUMNS} from `teams` \
             inner join `team_user` on `team_user`.`team_id` = `teams`.`id` \
             where `team_user`.`user_id` = ? order by `team_user`.`id` asc"
        )))
        .bind(user_id)
        .fetch_all(db)
        .await
    }

    /// How many owners a team has.
    pub async fn owner_count<'e, E>(db: E, team_id: i64) -> Result<i64, sqlx::Error>
    where
        E: SqliteExecutor<'e>,
    {
        sqlx::query_scalar("select count(*) from `team_user` where `team_id` = ? and `role` = ?")
            .bind(team_id)
            .bind(TeamRole::Owner)
            .fetch_one(db)
            .await
    }

    /// Take someone out of a team. The access tokens they made there stop
    /// working: a token acts for the person who made it.
    pub async fn detach(
        transaction: &mut Transaction<'_, Sqlite>,
        team_id: i64,
        user_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "update `access_tokens` set `revoked_at` = ? \
             where `team_id` = ? and `created_by` = ? and `revoked_at` is null",
        )
        .bind(Timestamp::now())
        .bind(team_id)
        .bind(user_id)
        .execute(&mut **transaction)
        .await?;
        // Nor does a grant they had just made for an MCP client become a token.
        sqlx::query(
            "delete from `oauth_authorization_codes` where `team_id` = ? and `user_id` = ?",
        )
        .bind(team_id)
        .bind(user_id)
        .execute(&mut **transaction)
        .await?;
        sqlx::query("delete from `team_user` where `team_id` = ? and `user_id` = ?")
            .bind(team_id)
            .bind(user_id)
            .execute(&mut **transaction)
            .await?;
        sqlx::query(
            "update `users` set `current_team_id` = null where `id` = ? and `current_team_id` = ?",
        )
        .bind(user_id)
        .bind(team_id)
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }

    /// Delete an account, and settle the teams it was in, as the write of
    /// one transaction.
    ///
    /// A team it was alone in goes with it, with everything the team had.
    /// A team it leaves without an owner gets one: its longest-standing
    /// administrator, or else its longest-standing member. What the account
    /// made in a team that stays (MCPs, invites, access tokens, the tokens
    /// revoked) is handed to an owner of that team.
    pub async fn delete_account(
        transaction: &mut Transaction<'_, Sqlite>,
        user_id: i64,
    ) -> Result<Result<(), AccountRemovalRefusal>, sqlx::Error> {
        let teams: Vec<(i64, i64)> = sqlx::query_as(
            "select `team_user`.`team_id`, \
             (select count(*) from `team_user` as `others` \
              where `others`.`team_id` = `team_user`.`team_id` and `others`.`user_id` <> `team_user`.`user_id`) \
             from `team_user` where `team_user`.`user_id` = ? order by `team_user`.`id` asc",
        )
        .bind(user_id)
        .fetch_all(&mut **transaction)
        .await?;
        if teams
            .iter()
            .any(|(team_id, others)| *team_id == Self::ROOT_ID && *others == 0)
        {
            return Ok(Err(AccountRemovalRefusal::LastOfRootTeam));
        }

        for (team_id, others) in teams {
            if others == 0 {
                sqlx::query("delete from `teams` where `id` = ?")
                    .bind(team_id)
                    .execute(&mut **transaction)
                    .await?;
                continue;
            }
            Self::detach(transaction, team_id, user_id).await?;
            Self::ensure_owner(transaction, team_id).await?;
        }

        // Also what the account made in a team it had left since: a row
        // goes with the account that made it.
        for table in ["mcps", "access_tokens", "invites"] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "update `{table}` set `created_by` = (\
                   select `team_user`.`user_id` from `team_user` \
                   where `team_user`.`team_id` = `{table}`.`team_id` \
                   order by {BY_RANK}, `team_user`.`id` asc limit 1) \
                 where `created_by` = ?"
            )))
            .bind(user_id)
            .execute(&mut **transaction)
            .await?;
        }

        sqlx::query("delete from `users` where `id` = ?")
            .bind(user_id)
            .execute(&mut **transaction)
            .await?;
        Ok(Ok(()))
    }

    /// Give a team that has members an owner when it has none: the member
    /// who ranks highest, and joined first.
    async fn ensure_owner(
        transaction: &mut Transaction<'_, Sqlite>,
        team_id: i64,
    ) -> Result<(), sqlx::Error> {
        let (member_id, role): (i64, TeamRole) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select `id`, `role` from `team_user` where `team_id` = ? \
             order by {BY_RANK}, `id` asc limit 1"
        )))
        .bind(team_id)
        .fetch_one(&mut **transaction)
        .await?;
        if !role.is_owner() {
            sqlx::query("update `team_user` set `role` = ?, `updated_at` = ? where `id` = ?")
                .bind(TeamRole::Owner)
                .bind(Timestamp::now())
                .bind(member_id)
                .execute(&mut **transaction)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_rank_member_admin_owner() {
        assert!(TeamRole::Member < TeamRole::Admin && TeamRole::Admin < TeamRole::Owner);
        assert!(!TeamRole::Member.is_admin());
        assert!(TeamRole::Admin.is_admin() && TeamRole::Owner.is_admin());
        assert!(TeamRole::Owner.is_owner() && !TeamRole::Admin.is_owner());
    }

    #[test]
    fn nobody_grants_or_manages_above_their_own_role() {
        use TeamRole::{Admin, Member, Owner};
        for role in TeamRole::ALL {
            assert!(!Member.may_grant(*role), "a member invites nobody");
            assert!(!Member.may_manage(*role));
            assert!(Owner.may_grant(*role) && Owner.may_manage(*role));
        }
        assert!(Admin.may_grant(Member) && Admin.may_grant(Admin));
        assert!(!Admin.may_grant(Owner));
        assert!(Admin.may_manage(Member) && Admin.may_manage(Admin));
        assert!(!Admin.may_manage(Owner));
    }
}
