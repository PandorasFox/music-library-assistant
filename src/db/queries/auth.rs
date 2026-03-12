//! Authentication queries: user management.

use anyhow::Result;

use super::Database;

/// A row from the `users` table.
#[derive(Debug, Clone)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
}

impl Database {
    /// Create a new user. Returns the user's row id.
    pub fn create_user(&self, username: &str, password_hash: &str) -> Result<i64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        self.conn().execute(
            "INSERT INTO users (username, password_hash, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![username, password_hash, now],
        )?;

        Ok(self.conn().last_insert_rowid())
    }

    /// Count the number of users in the database.
    pub fn user_count(&self) -> Result<i64> {
        let count: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Look up a user by username.
    pub fn get_user_by_username(&self, username: &str) -> Result<Option<UserRow>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT id, username, password_hash FROM users WHERE username = ?1")?;

        let mut rows = stmt.query_map(rusqlite::params![username], |row| {
            Ok(UserRow {
                id: row.get(0)?,
                username: row.get(1)?,
                password_hash: row.get(2)?,
            })
        })?;

        match rows.next() {
            Some(Ok(user)) => Ok(Some(user)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_user() {
        let db = Database::open_in_memory();
        let id = db.create_user("alice", "$argon2id$hash").unwrap();
        assert!(id > 0);

        let user = db.get_user_by_username("alice").unwrap().unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(user.password_hash, "$argon2id$hash");
    }

    #[test]
    fn test_create_duplicate_username() {
        let db = Database::open_in_memory();
        db.create_user("alice", "hash1").unwrap();
        let result = db.create_user("alice", "hash2");
        assert!(result.is_err());
    }

    #[test]
    fn test_user_count_empty() {
        let db = Database::open_in_memory();
        assert_eq!(db.user_count().unwrap(), 0);
    }

    #[test]
    fn test_user_count_after_create() {
        let db = Database::open_in_memory();
        db.create_user("alice", "hash1").unwrap();
        assert_eq!(db.user_count().unwrap(), 1);
        db.create_user("bob", "hash2").unwrap();
        assert_eq!(db.user_count().unwrap(), 2);
    }

    #[test]
    fn test_get_user_by_username() {
        let db = Database::open_in_memory();
        db.create_user("alice", "hash1").unwrap();

        let user = db.get_user_by_username("alice").unwrap().unwrap();
        assert_eq!(user.username, "alice");

        let none = db.get_user_by_username("bob").unwrap();
        assert!(none.is_none());
    }
}
