//! The schema, as the 25 migrations of the AdonisJS app built it, followed
//! by the migrations of this app alone.
//!
//! Generated from the SQL that knex ran for each migration, so that a
//! database created here is the one the Node app would have created, table
//! for table. Statements that moved data are written by hand below, since
//! their bindings were not part of the capture. The migrations that came
//! after only add tables, in the same SQL dialect, so that the Node app
//! keeps running on the database.
//!
//! Do not edit a migration that has shipped: add a new one.

/// One migration: the name recorded in `adonis_schema`, and its statements.
pub struct Migration {
    pub name: &'static str,
    pub statements: &'static [&'static str],
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "database/migrations/1761885935168_create_users_table",
        statements: &[
            r#"create table `users` (`id` integer not null primary key autoincrement, `full_name` varchar(255) null, `email` varchar(254) not null, `password` varchar(255) not null, `created_at` datetime not null, `updated_at` datetime null)"#,
            r#"create unique index `users_email_unique` on `users` (`email`)"#,
        ],
    },
    Migration {
        name: "database/migrations/1785578521548_alter_users_table",
        statements: &[
            r#"alter table `users` add column `role` varchar(32) not null default 'member'"#,
            r#"update `users` set `role` = 'admin'"#,
        ],
    },
    Migration {
        name: "database/migrations/1785578521962_create_invites_table",
        statements: &[
            r#"create table `invites` (`id` integer not null primary key autoincrement, `token` varchar(64) not null, `email` varchar(254) not null, `role` varchar(32) not null default 'member', `created_by` integer not null, `accepted_at` datetime null, `expires_at` datetime not null, `created_at` datetime not null, `updated_at` datetime null, foreign key(`created_by`) references `users`(`id`) on delete CASCADE)"#,
            r#"create unique index `invites_token_unique` on `invites` (`token`)"#,
        ],
    },
    Migration {
        name: "database/migrations/1785578522500_ensure_admin_user",
        statements: &[
            r#"update `users` set `role` = 'admin' where `id` = (select `id` from `users` order by `id` asc limit 1) and not exists (select 1 from `users` where `role` = 'admin')"#,
        ],
    },
    Migration {
        name: "database/migrations/1785583605821_create_mcps_table",
        statements: &[
            r#"create table `mcps` (`id` integer not null primary key autoincrement, `name` varchar(120) not null, `slug` varchar(120) not null, `description` varchar(500) null, `transport` varchar(16) not null, `http_url` varchar(2048) null, `npm_package` varchar(254) null, `npm_version` varchar(64) null, `npm_args` text null, `auth_type` varchar(16) not null default 'none', `auth_bearer` text null, `auth_header_name` varchar(120) null, `auth_header_value` text null, `oauth_authorize_url` varchar(2048) null, `oauth_token_url` varchar(2048) null, `oauth_scopes` varchar(500) null, `oauth_client_id` varchar(254) null, `oauth_client_secret` text null, `oauth_access_token` text null, `oauth_refresh_token` text null, `oauth_token_expires_at` datetime null, `status` varchar(16) not null default 'draft', `last_error` text null, `enabled` boolean not null default '1', `created_by` integer not null, `created_at` datetime not null, `updated_at` datetime null, foreign key(`created_by`) references `users`(`id`) on delete CASCADE)"#,
            r#"create unique index `mcps_slug_unique` on `mcps` (`slug`)"#,
        ],
    },
    Migration {
        name: "database/migrations/1785583605859_create_access_tokens_table",
        statements: &[
            r#"create table `access_tokens` (`id` integer not null primary key autoincrement, `name` varchar(120) not null, `token_prefix` varchar(16) not null, `token_hash` varchar(64) not null, `scope_mode` varchar(16) not null default 'all', `expires_at` datetime null, `revoked_at` datetime null, `last_used_at` datetime null, `created_by` integer not null, `created_at` datetime not null, `updated_at` datetime null, foreign key(`created_by`) references `users`(`id`) on delete CASCADE)"#,
            r#"create unique index `access_tokens_token_hash_unique` on `access_tokens` (`token_hash`)"#,
        ],
    },
    Migration {
        name: "database/migrations/1785583605895_create_access_token_mcps_table",
        statements: &[
            r#"create table `access_token_mcps` (`id` integer not null primary key autoincrement, `access_token_id` integer not null, `mcp_id` integer not null, `created_at` datetime not null, `updated_at` datetime null, foreign key(`access_token_id`) references `access_tokens`(`id`) on delete CASCADE, foreign key(`mcp_id`) references `mcps`(`id`) on delete CASCADE)"#,
            r#"create unique index `access_token_mcps_access_token_id_mcp_id_unique` on `access_token_mcps` (`access_token_id`, `mcp_id`)"#,
        ],
    },
    Migration {
        name: "database/migrations/1785610958128_add_oauth_discovery_to_mcps_table",
        statements: &[
            r#"alter table `mcps` add column `oauth_issuer` varchar(2048) null"#,
            r#"alter table `mcps` add column `oauth_resource` varchar(2048) null"#,
            r#"alter table `mcps` add column `oauth_redirect_uri` varchar(2048) null"#,
            r#"alter table `mcps` add column `oauth_client_auth_method` varchar(64) null"#,
            r#"alter table `mcps` add column `oauth_token_type` varchar(64) null"#,
        ],
    },
    Migration {
        name: "database/migrations/1785617732545_add_npm_env_to_mcps_table",
        statements: &[r#"alter table `mcps` add column `npm_env` text null"#],
    },
    Migration {
        name: "database/migrations/1785649943829_create_remember_me_tokens_table",
        statements: &[
            r#"create table `remember_me_tokens` (`id` integer not null primary key autoincrement, `tokenable_id` integer not null, `hash` varchar(255) not null, `created_at` datetime not null, `updated_at` datetime not null, `expires_at` datetime not null, foreign key(`tokenable_id`) references `users`(`id`) on delete CASCADE)"#,
            r#"create unique index `remember_me_tokens_hash_unique` on `remember_me_tokens` (`hash`)"#,
        ],
    },
    Migration {
        name: "database/migrations/1785658632028_create_mcp_call_logs",
        statements: &[
            r#"create table `instance_settings` (`id` integer not null primary key autoincrement, `mcp_log_level` varchar(16) not null default 'metadata', `mcp_log_retention_days` integer not null default '14', `updated_by` integer null, `created_at` datetime not null, `updated_at` datetime null, foreign key(`updated_by`) references `users`(`id`) on delete SET NULL)"#,
            r#"create table `mcp_call_logs` (`id` integer not null primary key autoincrement, `access_token_id` integer null, `access_token_name` varchar(120) not null, `access_token_prefix` varchar(16) not null, `mcp_id` integer null, `mcp_name` varchar(120) null, `mcp_slug` varchar(120) null, `requested_tool_name` varchar(512) not null, `tool_name` varchar(254) null, `outcome` varchar(16) not null, `error_category` varchar(32) null, `error_summary` varchar(500) null, `arguments` text null, `arguments_captured` boolean not null default '0', `duration_ms` integer not null, `created_at` datetime not null, foreign key(`access_token_id`) references `access_tokens`(`id`) on delete SET NULL, foreign key(`mcp_id`) references `mcps`(`id`) on delete SET NULL)"#,
            r#"create index `mcp_call_logs_created_at_index` on `mcp_call_logs` (`created_at`)"#,
            r#"create index `mcp_call_logs_outcome_created_at_index` on `mcp_call_logs` (`outcome`, `created_at`)"#,
            r#"create index `mcp_call_logs_mcp_slug_created_at_index` on `mcp_call_logs` (`mcp_slug`, `created_at`)"#,
            r#"create index `mcp_call_logs_access_token_prefix_created_at_index` on `mcp_call_logs` (`access_token_prefix`, `created_at`)"#,
            r#"insert into `instance_settings` (`created_at`, `id`, `mcp_log_level`, `mcp_log_retention_days`) values (strftime('%Y-%m-%d %H:%M:%f', 'now'), 1, 'metadata', 14)"#,
        ],
    },
    Migration {
        name: "database/migrations/1785678171625_add_response_capture_to_mcp_call_logs",
        statements: &[
            r#"alter table `mcp_call_logs` add column `response` text null"#,
            r#"alter table `mcp_call_logs` add column `response_captured` boolean not null default '0'"#,
        ],
    },
    Migration {
        name: "database/migrations/1785687355000_add_auto_auth_to_mcps_table",
        statements: &[
            r#"alter table `mcps` add column `oauth_required` boolean not null default '0'"#,
            r#"UPDATE mcps SET oauth_required = CASE WHEN auth_type = 'oauth' AND oauth_access_token IS NULL THEN 1 ELSE 0 END, auth_type = CASE WHEN auth_type IN ('none', 'oauth') THEN 'auto' ELSE auth_type END"#,
            r#"CREATE TABLE `_knex_temp_alter` (`id` integer PRIMARY KEY AUTOINCREMENT NOT NULL, `name` varchar(120) NOT NULL, `slug` varchar(120) NOT NULL, `description` varchar(500) NULL, `transport` varchar(16) NOT NULL, `http_url` varchar(2048) NULL, `npm_package` varchar(254) NULL, `npm_version` varchar(64) NULL, `npm_args` text NULL, `auth_type` varchar(16) NOT NULL DEFAULT 'auto', `auth_bearer` text NULL, `auth_header_name` varchar(120) NULL, `auth_header_value` text NULL, `oauth_authorize_url` varchar(2048) NULL, `oauth_token_url` varchar(2048) NULL, `oauth_scopes` varchar(500) NULL, `oauth_client_id` varchar(254) NULL, `oauth_client_secret` text NULL, `oauth_access_token` text NULL, `oauth_refresh_token` text NULL, `oauth_token_expires_at` datetime NULL, `status` varchar(16) NOT NULL DEFAULT 'draft', `last_error` text NULL, `enabled` boolean NOT NULL DEFAULT '1', `created_by` integer NOT NULL, `created_at` datetime NOT NULL, `updated_at` datetime NULL, `oauth_issuer` varchar(2048) NULL, `oauth_resource` varchar(2048) NULL, `oauth_redirect_uri` varchar(2048) NULL, `oauth_client_auth_method` varchar(64) NULL, `oauth_token_type` varchar(64) NULL, `npm_env` text NULL, `oauth_required` boolean NOT NULL DEFAULT '0', FOREIGN KEY (`created_by`) REFERENCES `users` (`id`) ON DELETE CASCADE)"#,
            r#"INSERT INTO "_knex_temp_alter" SELECT * FROM "mcps""#,
            r#"DROP TABLE "mcps""#,
            r#"ALTER TABLE "_knex_temp_alter" RENAME TO "mcps""#,
            r#"CREATE UNIQUE INDEX `mcps_slug_unique` on `mcps` (`slug`)"#,
        ],
    },
    Migration {
        name: "database/migrations/1785733804070_add_caller_ip_to_mcp_call_logs",
        statements: &[r#"alter table `mcp_call_logs` add column `caller_ip` varchar(64) null"#],
    },
    Migration {
        name: "database/migrations/1786080605152_create_rate_limits_table",
        statements: &[
            r#"create table `rate_limits` (`key` varchar(255) not null, `points` integer not null default '0', `expire` bigint, primary key (`key`))"#,
        ],
    },
    Migration {
        name: "database/migrations/1786089239430_add_gateway_oauth_server",
        statements: &[
            r#"create table `oauth_clients` (`id` integer not null primary key autoincrement, `client_id` varchar(80) not null, `client_secret_hash` varchar(64) null, `client_secret_prefix` varchar(20) null, `client_secret_expires_at` datetime null, `client_name` varchar(120) not null, `redirect_uris` text not null, `token_endpoint_auth_method` varchar(32) not null, `grant_types` text not null, `response_types` text not null, `scope` varchar(500) not null, `created_at` datetime not null, `updated_at` datetime null)"#,
            r#"create unique index `oauth_clients_client_id_unique` on `oauth_clients` (`client_id`)"#,
            r#"create table `oauth_authorization_codes` (`id` integer not null primary key autoincrement, `code_hash` varchar(64) not null, `oauth_client_id` integer not null, `user_id` integer not null, `redirect_uri` varchar(2048) not null, `code_challenge` varchar(128) not null, `scopes` varchar(500) not null, `resource` varchar(2048) not null, `expires_at` datetime not null, `created_at` datetime not null, foreign key(`oauth_client_id`) references `oauth_clients`(`id`) on delete CASCADE, foreign key(`user_id`) references `users`(`id`) on delete CASCADE)"#,
            r#"create unique index `oauth_authorization_codes_code_hash_unique` on `oauth_authorization_codes` (`code_hash`)"#,
            r#"alter table `access_tokens` add column `source` varchar(16) not null default 'manual'"#,
            r#"alter table `access_tokens` add column `oauth_client_id` integer null"#,
            r#"alter table `access_tokens` add column `oauth_scopes` varchar(500) null"#,
            r#"alter table `access_tokens` add column `oauth_resource` varchar(2048) null"#,
            r#"alter table `access_tokens` add column `oauth_refresh_token_hash` varchar(64) null"#,
            r#"alter table `access_tokens` add column `oauth_refresh_token_prefix` varchar(20) null"#,
            r#"alter table `access_tokens` add column `oauth_refresh_expires_at` datetime null"#,
            r#"CREATE TABLE `_knex_temp_alter` (`id` integer PRIMARY KEY AUTOINCREMENT NOT NULL, `name` varchar(120) NOT NULL, `token_prefix` varchar(16) NOT NULL, `token_hash` varchar(64) NOT NULL, `scope_mode` varchar(16) NOT NULL DEFAULT 'all', `expires_at` datetime NULL, `revoked_at` datetime NULL, `last_used_at` datetime NULL, `created_by` integer NOT NULL, `created_at` datetime NOT NULL, `updated_at` datetime NULL, `source` varchar(16) NOT NULL DEFAULT 'manual', `oauth_client_id` integer NULL, `oauth_scopes` varchar(500) NULL, `oauth_resource` varchar(2048) NULL, `oauth_refresh_token_hash` varchar(64) NULL, `oauth_refresh_token_prefix` varchar(20) NULL, `oauth_refresh_expires_at` datetime NULL, FOREIGN KEY (`created_by`) REFERENCES `users` (`id`) ON DELETE CASCADE, FOREIGN KEY (`oauth_client_id`) REFERENCES `oauth_clients` (`id`) ON DELETE SET NULL)"#,
            r#"INSERT INTO "_knex_temp_alter" SELECT * FROM "access_tokens""#,
            r#"DROP TABLE "access_tokens""#,
            r#"ALTER TABLE "_knex_temp_alter" RENAME TO "access_tokens""#,
            r#"CREATE UNIQUE INDEX `access_tokens_token_hash_unique` on `access_tokens` (`token_hash`)"#,
            r#"create unique index `access_tokens_oauth_refresh_token_hash_unique` on `access_tokens` (`oauth_refresh_token_hash`)"#,
        ],
    },
    Migration {
        name: "database/migrations/1786091080755_create_oauth_refresh_token_history",
        statements: &[
            r#"create table `oauth_refresh_token_history` (`id` integer not null primary key autoincrement, `access_token_id` integer not null, `token_hash` varchar(64) not null, `invalidated_at` datetime not null, foreign key(`access_token_id`) references `access_tokens`(`id`) on delete CASCADE)"#,
            r#"create unique index `oauth_refresh_token_history_token_hash_unique` on `oauth_refresh_token_history` (`token_hash`)"#,
            r#"create index `oauth_refresh_token_history_access_token_id_index` on `oauth_refresh_token_history` (`access_token_id`)"#,
        ],
    },
    Migration {
        name: "database/migrations/1786130456426_add_gateway_tool_mode_to_instance_settings",
        statements: &[
            r#"alter table `instance_settings` add column `gateway_tool_mode` varchar(16) not null default 'eager'"#,
        ],
    },
    Migration {
        name: "database/migrations/1786629788266_add_mcp_auto_update_to_instance_settings",
        statements: &[
            r#"alter table `instance_settings` add column `mcp_auto_update_enabled` boolean not null default '0'"#,
            r#"alter table `instance_settings` add column `mcp_auto_update_cron` varchar(64) not null default '0 2 * * *'"#,
        ],
    },
    Migration {
        name: "database/migrations/1791105398450_add_builtin_key_to_mcps_table",
        statements: &[r#"alter table `mcps` add column `builtin_key` varchar(64) null"#],
    },
    Migration {
        name: "database/migrations/1791107107531_add_builtin_write_enabled_to_mcps_table",
        statements: &[
            r#"alter table `mcps` add column `builtin_write_enabled` boolean not null default '0'"#,
        ],
    },
    Migration {
        name: "database/migrations/1791125112564_add_builtin_password_sign_in_to_mcps_table",
        statements: &[
            r#"alter table `mcps` add column `builtin_username` varchar(254) null"#,
            r#"alter table `mcps` add column `builtin_password` text null"#,
            r#"alter table `mcps` add column `builtin_permissions` text null"#,
            r#"alter table `mcps` add column `builtin_aliases` text null"#,
        ],
    },
    Migration {
        name: "database/migrations/1791132214318_add_session_version_to_users_table",
        statements: &[
            r#"alter table `users` add column `session_version` integer not null default '1'"#,
        ],
    },
    Migration {
        name: "database/migrations/1791378864221_add_tool_approvals_to_mcps_table",
        statements: &[
            r#"alter table `mcps` add column `tool_approvals` text null"#,
            r#"alter table `mcps` add column `builtin_settings` text null"#,
        ],
    },
    Migration {
        name: "database/migrations/1791378864679_create_approval_requests_table",
        statements: &[
            r#"create table `approval_requests` (`id` integer not null primary key autoincrement, `public_id` varchar(64) not null, `mcp_id` integer not null, `access_token_id` integer not null, `tool_name` varchar(254) not null, `arguments` text not null, `arguments_hash` varchar(64) not null, `summary` text not null, `status` varchar(16) not null default 'pending', `decided_by` integer null, `decided_at` datetime null, `consumed_at` datetime null, `expires_at` datetime not null, `created_at` datetime not null, `updated_at` datetime null, foreign key(`mcp_id`) references `mcps`(`id`) on delete CASCADE, foreign key(`access_token_id`) references `access_tokens`(`id`) on delete CASCADE, foreign key(`decided_by`) references `users`(`id`) on delete SET NULL)"#,
            r#"create unique index `approval_requests_public_id_unique` on `approval_requests` (`public_id`)"#,
            r#"create index `approval_requests_access_token_id_mcp_id_tool_name_arguments_hash_index` on `approval_requests` (`access_token_id`, `mcp_id`, `tool_name`, `arguments_hash`)"#,
            r#"create index `approval_requests_status_expires_at_index` on `approval_requests` (`status`, `expires_at`)"#,
        ],
    },
    // The migrations above are those of the Node app. The ones below are
    // this app's own: the Node app runs on a database that has them, and
    // ignores their tables, but cannot import a backup that holds them.
    Migration {
        name: "database/migrations/1791567196927_create_two_factor_tables",
        statements: &[
            r#"create table `user_totp_secrets` (`id` integer not null primary key autoincrement, `user_id` integer not null, `secret` text not null, `confirmed_at` datetime null, `last_used_step` integer null, `created_at` datetime not null, `updated_at` datetime null, foreign key(`user_id`) references `users`(`id`) on delete CASCADE)"#,
            r#"create unique index `user_totp_secrets_user_id_unique` on `user_totp_secrets` (`user_id`)"#,
            r#"create table `user_recovery_codes` (`id` integer not null primary key autoincrement, `user_id` integer not null, `code_hash` varchar(64) not null, `used_at` datetime null, `created_at` datetime not null, foreign key(`user_id`) references `users`(`id`) on delete CASCADE)"#,
            r#"create index `user_recovery_codes_user_id_index` on `user_recovery_codes` (`user_id`)"#,
            r#"create table `user_passkeys` (`id` integer not null primary key autoincrement, `user_id` integer not null, `name` varchar(120) not null, `credential_id` varchar(1400) not null, `user_handle` varchar(36) not null, `passkey` text not null, `last_used_at` datetime null, `created_at` datetime not null, `updated_at` datetime null, foreign key(`user_id`) references `users`(`id`) on delete CASCADE)"#,
            r#"create unique index `user_passkeys_credential_id_unique` on `user_passkeys` (`credential_id`)"#,
            r#"create index `user_passkeys_user_id_index` on `user_passkeys` (`user_id`)"#,
        ],
    },
];

/// How many of [`MIGRATIONS`] the Node app has. The others are this app's own.
pub const NODE_MIGRATIONS: usize = 25;
