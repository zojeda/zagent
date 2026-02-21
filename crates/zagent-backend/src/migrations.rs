use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use zagent_core::Result;

#[derive(Debug, Clone, Copy)]
pub struct MigrationDef {
    pub version: u32,
    pub name: &'static str,
    pub up: &'static str,
    pub down: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationStatus {
    pub version: u32,
    pub name: String,
    pub applied: bool,
    pub applied_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MigrationRow {
    version: u32,
    applied_at: Option<String>,
}

const MIGRATIONS: &[MigrationDef] = &[MigrationDef {
    version: 1,
    name: "session_schema_v3",
    up: include_str!("../migrations/001_session_schema_v3.up.surql"),
    down: include_str!("../migrations/001_session_schema_v3.down.surql"),
}];

pub struct Migrator<'a> {
    db: &'a Surreal<Any>,
}

impl<'a> Migrator<'a> {
    pub fn new(db: &'a Surreal<Any>) -> Self {
        Self { db }
    }

    pub fn latest_version(&self) -> u32 {
        MIGRATIONS.last().map(|m| m.version).unwrap_or(0)
    }

    pub async fn current_version(&self) -> Result<u32> {
        self.ensure_meta().await?;
        let applied = self.applied_versions().await?;
        Ok(applied.keys().last().copied().unwrap_or(0))
    }

    pub async fn status(&self) -> Result<Vec<MigrationStatus>> {
        self.ensure_meta().await?;
        let applied = self.applied_versions().await?;
        let mut out = Vec::with_capacity(MIGRATIONS.len());
        for m in MIGRATIONS {
            out.push(MigrationStatus {
                version: m.version,
                name: m.name.to_string(),
                applied: applied.contains_key(&m.version),
                applied_at: applied.get(&m.version).cloned().flatten(),
            });
        }
        Ok(out)
    }

    pub async fn migrate_to_latest(&self) -> Result<()> {
        self.migrate_to(self.latest_version()).await
    }

    pub async fn migrate_to(&self, target_version: u32) -> Result<()> {
        self.ensure_meta().await?;
        let latest = self.latest_version();
        if target_version > latest {
            return Err(zagent_core::Error::session(format!(
                "Target migration version {target_version} is greater than latest {latest}"
            )));
        }

        let current = self.current_version().await?;
        if current == target_version {
            return Ok(());
        }

        if current < target_version {
            for migration in MIGRATIONS
                .iter()
                .filter(|m| m.version > current && m.version <= target_version)
            {
                self.apply_up(migration).await?;
            }
            return Ok(());
        }

        for migration in MIGRATIONS
            .iter()
            .rev()
            .filter(|m| m.version > target_version && m.version <= current)
        {
            self.apply_down(migration).await?;
        }
        Ok(())
    }

    async fn apply_up(&self, migration: &MigrationDef) -> Result<()> {
        self.db
            .query(migration.up)
            .await
            .map_err(|e| {
                zagent_core::Error::session(format!(
                    "Failed to run migration {} up: {e}",
                    migration.version
                ))
            })?
            .check()
            .map_err(|e| {
                zagent_core::Error::session(format!(
                    "Migration {} up statement failed: {e}",
                    migration.version
                ))
            })?;

        self.db
            .query(
                "UPSERT type::record('schema_migrations', $id) CONTENT { version: $version, name: $name, applied_at: time::now() }",
            )
            .bind(("id", migration.version.to_string()))
            .bind(("version", migration.version as u64))
            .bind(("name", migration.name.to_string()))
            .await
            .map_err(|e| {
                zagent_core::Error::session(format!(
                    "Failed to record migration {}: {e}",
                    migration.version
                ))
            })?
            .check()
            .map_err(|e| {
                zagent_core::Error::session(format!(
                    "Migration {} metadata statement failed: {e}",
                    migration.version
                ))
            })?;

        Ok(())
    }

    async fn apply_down(&self, migration: &MigrationDef) -> Result<()> {
        self.db
            .query(migration.down)
            .await
            .map_err(|e| {
                zagent_core::Error::session(format!(
                    "Failed to run migration {} down: {e}",
                    migration.version
                ))
            })?
            .check()
            .map_err(|e| {
                zagent_core::Error::session(format!(
                    "Migration {} down statement failed: {e}",
                    migration.version
                ))
            })?;

        self.db
            .query("DELETE type::record('schema_migrations', $id)")
            .bind(("id", migration.version.to_string()))
            .await
            .map_err(|e| {
                zagent_core::Error::session(format!(
                    "Failed to remove migration {} record: {e}",
                    migration.version
                ))
            })?
            .check()
            .map_err(|e| {
                zagent_core::Error::session(format!(
                    "Migration {} metadata removal failed: {e}",
                    migration.version
                ))
            })?;

        Ok(())
    }

    async fn ensure_meta(&self) -> Result<()> {
        self.db
            .query(
                "DEFINE TABLE IF NOT EXISTS schema_migrations SCHEMAFULL; \
                 DEFINE FIELD IF NOT EXISTS version ON TABLE schema_migrations TYPE int; \
                 DEFINE FIELD IF NOT EXISTS name ON TABLE schema_migrations TYPE string; \
                 DEFINE FIELD IF NOT EXISTS applied_at ON TABLE schema_migrations TYPE datetime; \
                 DEFINE INDEX IF NOT EXISTS schema_migrations_version_uq ON TABLE schema_migrations FIELDS version UNIQUE;",
            )
            .await
            .map_err(|e| zagent_core::Error::session(format!("Failed to ensure migrations schema: {e}")))?
            .check()
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to ensure migrations schema (statement): {e}"))
            })?;
        Ok(())
    }

    async fn applied_versions(&self) -> Result<BTreeMap<u32, Option<String>>> {
        let mut response = self
            .db
            .query(
                "SELECT version, <string>applied_at AS applied_at FROM schema_migrations ORDER BY version ASC",
            )
            .await
            .map_err(|e| {
                zagent_core::Error::session(format!("Failed to list applied migrations: {e}"))
            })?;

        let rows: Vec<serde_json::Value> = response.take(0).map_err(|e| {
            zagent_core::Error::session(format!("Failed to decode applied migrations: {e}"))
        })?;

        let mut out = BTreeMap::new();
        for row in rows {
            let parsed: MigrationRow = serde_json::from_value(row).map_err(|e| {
                zagent_core::Error::session(format!("Failed to parse applied migration row: {e}"))
            })?;
            out.insert(parsed.version, parsed.applied_at);
        }
        Ok(out)
    }
}
