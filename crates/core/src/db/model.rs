//! A small active-record layer with the semantics the app relied on in
//! Lucid: `save()` writes only the columns that changed since the row was
//! read, so two requests that change different columns of the same row (an
//! OAuth token refresh and a status update, for example) do not undo each
//! other.

/// A text column with a closed set of values.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
        pub enum $name {
            $( $(#[$variant_meta])* #[serde(rename = $text)] $variant ),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];

            pub fn as_str(&self) -> &'static str {
                match self { $( $name::$variant => $text ),+ }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value { $( $text => Some($name::$variant), )+ _ => None }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;

            fn from_str(value: &str) -> Result<Self, String> {
                Self::parse(value).ok_or_else(|| format!("unknown {}: {value:?}", stringify!($name)))
            }
        }

        impl sqlx::Type<sqlx::Sqlite> for $name {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <String as sqlx::Type<sqlx::Sqlite>>::type_info()
            }

            fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
                <String as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
            }
        }

        impl sqlx::Encode<'_, sqlx::Sqlite> for $name {
            fn encode_by_ref(
                &self,
                buffer: &mut sqlx::sqlite::SqliteArgumentsBuffer,
            ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                sqlx::Encode::<sqlx::Sqlite>::encode(self.as_str().to_string(), buffer)
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for $name {
            fn decode(value: sqlx::sqlite::SqliteValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
                let text = <String as sqlx::Decode<sqlx::Sqlite>>::decode(value)?;
                text.parse().map_err(Into::into)
            }
        }
    };
}

/// A struct backed by a table whose primary key is `id`.
///
/// `created_at` and `updated_at` say whether the table has the column, in
/// which case `insert` and `save` maintain it as Lucid did.
macro_rules! model {
    (
        $(#[$meta:meta])*
        table = $table:literal, created_at = $created:tt, updated_at = $updated:tt;
        pub struct $name:ident {
            $( $(#[$field_meta:meta])* pub $field:ident : $ty:ty, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default)]
        pub struct $name {
            pub id: i64,
            $( $(#[$field_meta])* pub $field: $ty, )+
            /// The row as the database holds it, to tell which columns changed.
            #[doc(hidden)]
            pub stored: Option<Box<$name>>,
        }

        impl<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> for $name {
            fn from_row(row: &'r sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
                use sqlx::Row;
                let mut model = Self {
                    id: row.try_get("id")?,
                    $( $field: row.try_get(stringify!($field))?, )+
                    stored: None,
                };
                model.remember();
                Ok(model)
            }
        }

        impl $name {
            pub const TABLE: &'static str = $table;
            const COLUMNS: &'static [&'static str] = &[ $( stringify!($field) ),+ ];

            fn remember(&mut self) {
                let mut copy = self.clone();
                copy.stored = None;
                self.stored = Some(Box::new(copy));
            }

            /// Whether the row exists in the database, as far as this value knows.
            pub fn is_persisted(&self) -> bool {
                self.stored.is_some()
            }

            pub async fn find<'e, E>(db: E, id: i64) -> Result<Option<Self>, sqlx::Error>
            where
                E: sqlx::sqlite::SqliteExecutor<'e>,
            {
                sqlx::query_as(concat!("select * from `", $table, "` where `id` = ?"))
                    .bind(id)
                    .fetch_optional(db)
                    .await
            }

            /// Insert the row and read back its id. Sets the timestamps.
            pub async fn insert<'e, E>(&mut self, db: E) -> Result<(), sqlx::Error>
            where
                E: sqlx::sqlite::SqliteExecutor<'e>,
            {
                model!(@touch_created self $created);
                model!(@touch_updated self $updated);

                let columns: Vec<String> =
                    Self::COLUMNS.iter().map(|column| format!("`{column}`")).collect();
                let placeholders = vec!["?"; Self::COLUMNS.len()].join(", ");
                let sql = format!(
                    "insert into `{}` ({}) values ({})",
                    $table,
                    columns.join(", "),
                    placeholders
                );
                let result = sqlx::query(sqlx::AssertSqlSafe(sql))
                    $( .bind(self.$field.clone()) )+
                    .execute(db)
                    .await?;
                self.id = result.last_insert_rowid();
                self.remember();
                Ok(())
            }

            /// Write the columns that changed since the row was read, or
            /// insert the row when it was never stored. Does nothing when
            /// nothing changed.
            pub async fn save<'e, E>(&mut self, db: E) -> Result<(), sqlx::Error>
            where
                E: sqlx::sqlite::SqliteExecutor<'e>,
            {
                let Some(stored) = self.stored.clone() else {
                    return self.insert(db).await;
                };
                if true $( && self.$field == stored.$field )+ {
                    return Ok(());
                }
                model!(@touch_updated self $updated);

                let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(concat!("update `", $table, "` set "));
                let mut first = true;
                $(
                    if self.$field != stored.$field {
                        if !first {
                            query.push(", ");
                        }
                        first = false;
                        query.push(concat!("`", stringify!($field), "` = "));
                        query.push_bind(self.$field.clone());
                    }
                )+
                let _ = first;
                query.push(" where `id` = ");
                query.push_bind(self.id);
                query.build().execute(db).await?;
                self.remember();
                Ok(())
            }

            pub async fn delete<'e, E>(&self, db: E) -> Result<(), sqlx::Error>
            where
                E: sqlx::sqlite::SqliteExecutor<'e>,
            {
                sqlx::query(concat!("delete from `", $table, "` where `id` = ?"))
                    .bind(self.id)
                    .execute(db)
                    .await?;
                Ok(())
            }

            /// Read the row again, dropping changes that were not saved.
            /// Fails with `RowNotFound` when it was deleted meanwhile.
            pub async fn refresh<'e, E>(&mut self, db: E) -> Result<(), sqlx::Error>
            where
                E: sqlx::sqlite::SqliteExecutor<'e>,
            {
                *self = Self::find(db, self.id).await?.ok_or(sqlx::Error::RowNotFound)?;
                Ok(())
            }
        }
    };

    (@touch_created $model:ident true) => { $model.created_at = $crate::time::Timestamp::now(); };
    (@touch_created $model:ident false) => {};
    (@touch_updated $model:ident true) => { $model.updated_at = Some($crate::time::Timestamp::now()); };
    (@touch_updated $model:ident false) => {};
}
