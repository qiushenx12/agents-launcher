use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::cli_capabilities::OpenCodeSession;

fn db_path_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        candidates.push(PathBuf::from(xdg).join("opencode").join("opencode.db"));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("opencode.db"),
        );
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("opencode").join("opencode.db"));
    }
    candidates
}

fn locate_db() -> Option<PathBuf> {
    db_path_candidates().into_iter().find(|path| path.is_file())
}

fn open_db() -> Result<Connection, String> {
    let path = locate_db().ok_or_else(|| "未找到 OpenCode 本地数据库。".to_string())?;
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("无法打开 OpenCode 本地数据库: {error}"))?;
    connection
        .busy_timeout(Duration::from_millis(1500))
        .map_err(|error| format!("无法配置 OpenCode 本地数据库: {error}"))?;
    Ok(connection)
}

pub fn query_sessions(max_count: u32) -> Result<Vec<OpenCodeSession>, String> {
    let connection = open_db()?;
    let mut statement = connection
        .prepare(
            "SELECT id, title, directory, time_created, time_updated, project_id \
             FROM session ORDER BY time_updated DESC LIMIT ?1",
        )
        .map_err(|error| format!("OpenCode 本地数据库会话查询不可用: {error}"))?;
    let rows = statement
        .query_map([max_count.clamp(1, 2000)], |row| {
            Ok(OpenCodeSession {
                id: row.get(0)?,
                title: row.get(1)?,
                directory: row.get(2)?,
                created: row.get(3)?,
                updated: row.get(4)?,
                project_id: row.get(5)?,
            })
        })
        .map_err(|error| format!("OpenCode 本地数据库会话读取失败: {error}"))?;
    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|error| format!("OpenCode 本地数据库会话行损坏: {error}"))?);
    }
    Ok(sessions)
}

pub fn query_projects() -> Result<Vec<(String, String)>, String> {
    let connection = open_db()?;
    let mut statement = connection
        .prepare("SELECT id, worktree FROM project")
        .map_err(|error| format!("OpenCode 本地数据库项目查询不可用: {error}"))?;
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|error| format!("OpenCode 本地数据库项目读取失败: {error}"))?;
    let mut projects = Vec::new();
    for row in rows {
        projects.push(row.map_err(|error| format!("OpenCode 本地数据库项目行损坏: {error}"))?);
    }
    Ok(projects)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_fixture_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("opencode.db");
        let connection = Connection::open(&path).expect("create db");
        connection
            .execute_batch(
                "CREATE TABLE project (id text PRIMARY KEY, worktree text NOT NULL);
                 CREATE TABLE session (
                     id text PRIMARY KEY,
                     project_id text NOT NULL,
                     directory text NOT NULL,
                     title text NOT NULL,
                     time_created integer NOT NULL,
                     time_updated integer NOT NULL
                 );
                 INSERT INTO project (id, worktree) VALUES
                     ('global', '/'),
                     ('p1', 'D:/Work/demo');
                 INSERT INTO session (id, project_id, directory, title, time_created, time_updated) VALUES
                     ('s1', 'p1', 'D:/Work/demo', 'Demo task', 1000, 2000),
                     ('s2', 'global', 'D:/Work/other', 'Other task', 1500, 3000);",
            )
            .expect("seed db");
        drop(connection);
        (dir, path)
    }

    #[test]
    fn sessions_are_read_newest_first_from_local_db() {
        let (_dir, path) = create_fixture_db();
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open fixture");
        let mut statement = connection
            .prepare(
                "SELECT id, title, directory, time_created, time_updated, project_id \
                 FROM session ORDER BY time_updated DESC LIMIT ?1",
            )
            .expect("prepare");
        let sessions: Vec<OpenCodeSession> = statement
            .query_map([500_u32], |row| {
                Ok(OpenCodeSession {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    directory: row.get(2)?,
                    created: row.get(3)?,
                    updated: row.get(4)?,
                    project_id: row.get(5)?,
                })
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "s2");
        assert_eq!(sessions[0].updated, 3000);
        assert_eq!(sessions[1].directory, "D:/Work/demo");
    }
}
