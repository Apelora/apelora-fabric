// SPDX-License-Identifier: AGPL-3.0-only
//! Directories people sign in with over LDAP with TLS, configured in the Fabric Console.
//! Nothing secret is stored: Apelora binds as the person with the password they type.

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Pool, Result};

/// The editable settings of a directory (already validated by the API).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DirectoryFields {
    pub name: String,
    pub url: String,
    pub start_tls: bool,
    pub ca_pem: Option<String>,
    pub bind_template: String,
    pub base_dn: Option<String>,
    pub user_filter: Option<String>,
    pub id_attribute: String,
    pub username_attribute: String,
    pub email_attribute: String,
    pub name_attribute: String,
    pub group_attribute: Option<String>,
    pub group_base_dn: Option<String>,
    pub group_filter: Option<String>,
    pub group_name_attribute: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct DirectoryRecord {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub start_tls: bool,
    pub ca_pem: Option<String>,
    pub bind_template: String,
    pub base_dn: Option<String>,
    pub user_filter: Option<String>,
    pub id_attribute: String,
    pub username_attribute: String,
    pub email_attribute: String,
    pub name_attribute: String,
    pub group_attribute: Option<String>,
    pub group_base_dn: Option<String>,
    pub group_filter: Option<String>,
    pub group_name_attribute: String,
    pub enabled: bool,
    pub updated_at: OffsetDateTime,
    pub updated_by: String,
}

impl DirectoryRecord {
    /// The settings without the bookkeeping columns.
    pub fn fields(&self) -> DirectoryFields {
        DirectoryFields {
            name: self.name.clone(),
            url: self.url.clone(),
            start_tls: self.start_tls,
            ca_pem: self.ca_pem.clone(),
            bind_template: self.bind_template.clone(),
            base_dn: self.base_dn.clone(),
            user_filter: self.user_filter.clone(),
            id_attribute: self.id_attribute.clone(),
            username_attribute: self.username_attribute.clone(),
            email_attribute: self.email_attribute.clone(),
            name_attribute: self.name_attribute.clone(),
            group_attribute: self.group_attribute.clone(),
            group_base_dn: self.group_base_dn.clone(),
            group_filter: self.group_filter.clone(),
            group_name_attribute: self.group_name_attribute.clone(),
            enabled: self.enabled,
        }
    }
}

const COLUMNS: &str = "id, name, url, start_tls, ca_pem, bind_template, base_dn, user_filter,
    id_attribute, username_attribute, email_attribute, name_attribute, group_attribute,
    group_base_dn, group_filter, group_name_attribute, enabled, updated_at, updated_by";

pub async fn list_directories(pool: &Pool) -> Result<Vec<DirectoryRecord>> {
    Ok(sqlx::query_as::<_, DirectoryRecord>(&format!(
        "SELECT {COLUMNS} FROM directories ORDER BY name"
    ))
    .fetch_all(pool)
    .await?)
}

pub async fn get_directory(
    db: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> Result<Option<DirectoryRecord>> {
    Ok(sqlx::query_as::<_, DirectoryRecord>(&format!(
        "SELECT {COLUMNS} FROM directories WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(db)
    .await?)
}

/// Creates (`id` none) or replaces a directory; returns its id.
pub async fn save_directory(
    db: impl sqlx::PgExecutor<'_>,
    id: Option<Uuid>,
    f: &DirectoryFields,
    actor: &str,
) -> Result<Uuid> {
    let id = id.unwrap_or_else(Uuid::new_v4);
    sqlx::query(
        "INSERT INTO directories (id, name, url, start_tls, ca_pem, bind_template, base_dn,
             user_filter, id_attribute, username_attribute, email_attribute, name_attribute,
             group_attribute, group_base_dn, group_filter, group_name_attribute, enabled,
             created_by, updated_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $18)
         ON CONFLICT (id) DO UPDATE SET
           name = EXCLUDED.name, url = EXCLUDED.url, start_tls = EXCLUDED.start_tls,
           ca_pem = EXCLUDED.ca_pem, bind_template = EXCLUDED.bind_template,
           base_dn = EXCLUDED.base_dn, user_filter = EXCLUDED.user_filter,
           id_attribute = EXCLUDED.id_attribute, username_attribute = EXCLUDED.username_attribute,
           email_attribute = EXCLUDED.email_attribute, name_attribute = EXCLUDED.name_attribute,
           group_attribute = EXCLUDED.group_attribute, group_base_dn = EXCLUDED.group_base_dn,
           group_filter = EXCLUDED.group_filter,
           group_name_attribute = EXCLUDED.group_name_attribute, enabled = EXCLUDED.enabled,
           updated_at = now(), updated_by = EXCLUDED.updated_by",
    )
    .bind(id)
    .bind(&f.name)
    .bind(&f.url)
    .bind(f.start_tls)
    .bind(&f.ca_pem)
    .bind(&f.bind_template)
    .bind(&f.base_dn)
    .bind(&f.user_filter)
    .bind(&f.id_attribute)
    .bind(&f.username_attribute)
    .bind(&f.email_attribute)
    .bind(&f.name_attribute)
    .bind(&f.group_attribute)
    .bind(&f.group_base_dn)
    .bind(&f.group_filter)
    .bind(&f.group_name_attribute)
    .bind(f.enabled)
    .bind(actor)
    .execute(db)
    .await?;
    Ok(id)
}

pub async fn delete_directory(db: impl sqlx::PgExecutor<'_>, id: Uuid) -> Result<bool> {
    Ok(sqlx::query("DELETE FROM directories WHERE id = $1")
        .bind(id)
        .execute(db)
        .await?
        .rows_affected()
        > 0)
}

/// The Apelora groups whose external name is one of `names` (a directory's group names), as
/// `(external name, group name)`, sorted by external name.
pub async fn mapped_groups(pool: &Pool, names: &[String]) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT external_name, name FROM groups WHERE external_name = ANY($1) ORDER BY external_name, name",
    )
    .bind(names)
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(name: &str, url: &str) -> DirectoryFields {
        DirectoryFields {
            name: name.into(),
            url: url.into(),
            bind_template: "uid={username},ou=people,dc=example,dc=org".into(),
            id_attribute: "entryUUID".into(),
            username_attribute: "uid".into(),
            email_attribute: "mail".into(),
            name_attribute: "cn".into(),
            group_name_attribute: "cn".into(),
            enabled: true,
            ..DirectoryFields::default()
        }
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn directories_are_saved_listed_replaced_and_removed(pool: Pool) {
        let f = fields("Staff", "ldaps://ldap.example.org");
        let id = save_directory(&pool, None, &f, "user:a")
            .await
            .expect("save");
        let got = get_directory(&pool, id).await.expect("get").expect("found");
        assert_eq!(got.fields(), f);
        assert_eq!(got.updated_by, "user:a");

        let mut changed = f.clone();
        changed.group_attribute = Some("memberOf".into());
        changed.enabled = false;
        save_directory(&pool, Some(id), &changed, "user:b")
            .await
            .expect("replace");
        let all = list_directories(&pool).await.expect("list");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].fields(), changed);
        assert_eq!(all[0].updated_by, "user:b");

        assert!(delete_directory(&pool, id).await.expect("delete"));
        assert!(!delete_directory(&pool, id).await.expect("again"));
        assert!(get_directory(&pool, id).await.expect("get").is_none());
    }

    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn the_database_refuses_plain_ldap_and_duplicate_names(pool: Pool) {
        save_directory(&pool, None, &fields("Staff", "ldaps://a.example.org"), "x")
            .await
            .expect("save");
        assert!(
            save_directory(&pool, None, &fields("Staff", "ldaps://b.example.org"), "x")
                .await
                .is_err(),
            "the name is unique"
        );
        assert!(
            save_directory(&pool, None, &fields("Plain", "ldap://c.example.org"), "x")
                .await
                .is_err(),
            "plain LDAP without StartTLS"
        );
        let mut start_tls = fields("StartTLS", "ldap://c.example.org");
        start_tls.start_tls = true;
        save_directory(&pool, None, &start_tls, "x")
            .await
            .expect("StartTLS is fine");
    }
}
