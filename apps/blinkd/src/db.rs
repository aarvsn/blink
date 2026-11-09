//! Local SQLite persistence (blinkd.db).

use crate::error::{AppError, AppResult};
use crate::models::ServerRow;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS servers (
              id TEXT PRIMARY KEY,
              docker_id TEXT,
              name TEXT NOT NULL,
              image TEXT NOT NULL,
              state TEXT NOT NULL,
              memory_limit INTEGER NOT NULL,
              cpu_limit INTEGER NOT NULL,
              variables TEXT NOT NULL,
              startup_command TEXT NOT NULL,
              install_script TEXT NOT NULL,
              allocation TEXT NOT NULL,
              config_files TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_servers_state ON servers(state);
            "#,
        )?;
        Ok(Db { conn: Mutex::new(conn) })
    }

    fn row_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<ServerRow> {
        Ok(ServerRow {
            id: row.get("id")?,
            docker_id: row.get("docker_id")?,
            name: row.get("name")?,
            image: row.get("image")?,
            state: row.get("state")?,
            memory_limit: row.get("memory_limit")?,
            cpu_limit: row.get("cpu_limit")?,
            variables: row.get("variables")?,
            startup_command: row.get("startup_command")?,
            install_script: row.get("install_script")?,
            allocation: row.get("allocation")?,
            config_files: row.get("config_files")?,
            status: None,
        })
    }

    pub fn list_servers(&self) -> AppResult<Vec<ServerRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT * FROM servers")?;
        let rows = stmt
            .query_map([], Self::row_from)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_server(&self, id: &str) -> AppResult<Option<ServerRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT * FROM servers WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], Self::row_from)?;
        match rows.next() {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_server(
        &self,
        id: &str,
        name: &str,
        state: &str,
        memory_limit: i64,
        cpu_limit: i64,
        image: &str,
        variables_json: &str,
        startup_command: &str,
        install_script_json: &str,
        allocation_json: &str,
    ) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"INSERT INTO servers (
                id, docker_id, name, state, memory_limit, cpu_limit, image,
                variables, startup_command, install_script, allocation
            ) VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
            rusqlite::params![
                id,
                name,
                state,
                memory_limit,
                cpu_limit,
                image,
                variables_json,
                startup_command,
                install_script_json,
                allocation_json
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_server_full(
        &self,
        id: &str,
        docker_id: Option<&str>,
        state: &str,
        name: &str,
        memory_limit: i64,
        cpu_limit: i64,
        image: &str,
    ) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE servers SET docker_id = ?2, state = ?3, name = ?4, memory_limit = ?5, cpu_limit = ?6, image = ?7 WHERE id = ?1",
            rusqlite::params![id, docker_id, state, name, memory_limit, cpu_limit, image],
        )?;
        Ok(())
    }

    pub fn update_state(&self, id: &str, state: &str) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE servers SET state = ?2 WHERE id = ?1", rusqlite::params![id, state])?;
        Ok(())
    }

    pub fn update_docker_id_and_state(&self, id: &str, docker_id: Option<&str>, state: &str) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE servers SET docker_id = ?2, state = ?3 WHERE id = ?1",
            rusqlite::params![id, docker_id, state],
        )?;
        Ok(())
    }

    pub fn update_startup_command(&self, id: &str, startup_command: &str) -> AppResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE servers SET startup_command = ?2 WHERE id = ?1",
            rusqlite::params![id, startup_command],
        )?;
        Ok(())
    }

    pub fn delete_server(&self, id: &str) -> AppResult<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM servers WHERE id = ?1", [id])?;
        Ok(n > 0)
    }

    pub fn server_exists(&self, id: &str) -> AppResult<bool> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM servers WHERE id = ?1", [id], |r| r.get(0))?;
        Ok(n > 0)
    }

    pub fn get_docker_id(&self, id: &str) -> AppResult<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT docker_id FROM servers WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], |r| r.get::<_, Option<String>>(0))?;
        match rows.next() {
            Some(Ok(v)) => Ok(v),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    pub fn get_memory_limit(&self, id: &str) -> AppResult<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let v = conn.query_row("SELECT memory_limit FROM servers WHERE id = ?1", [id], |r| {
            r.get::<_, i64>(0)
        });
        match v {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::from(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crud_roundtrip() {
        let db = Db::open(":memory:").unwrap();
        assert!(db.get_server("s1").unwrap().is_none());

        db.insert_server(
            "s1", "Test", "installing", 1024, 512, "img:latest", "[]", "run", "{}", "{}",
        )
        .unwrap();

        let row = db.get_server("s1").unwrap().unwrap();
        assert_eq!(row.name, "Test");
        assert_eq!(row.state, "installing");

        db.update_state("s1", "installed").unwrap();
        assert_eq!(db.get_server("s1").unwrap().unwrap().state, "installed");

        db.update_docker_id_and_state("s1", Some("abc"), "running").unwrap();
        assert_eq!(db.get_docker_id("s1").unwrap().as_deref(), Some("abc"));

        assert!(db.server_exists("s1").unwrap());
        assert!(db.delete_server("s1").unwrap());
        assert!(!db.server_exists("s1").unwrap());
    }
}
