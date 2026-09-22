use std::{env, fs};

use mysql_async::{prelude::Queryable, OptsBuilder, Pool, Row};
use serde_json::{json, Value};

fn dotenv_value(contents: &str, key: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        let (name, raw_value) = trimmed.split_once('=')?;
        if name.trim() != key {
            return None;
        }
        let value = raw_value.trim();
        let unquoted = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        (!unquoted.is_empty()).then(|| unquoted.to_string())
    })
}

fn live_pool() -> Pool {
    let env_path = env::var("ERGO_ENV_PATH").expect("ERGO_ENV_PATH is required");
    let contents = fs::read_to_string(env_path).expect("the ERGO .env file must be readable");
    let password = dotenv_value(&contents, "MYSQL_PASSWORD")
        .expect("MYSQL_PASSWORD must be present in the ERGO .env file");

    let options = OptsBuilder::default()
        .ip_or_hostname("10.21.36.17")
        .tcp_port(3306)
        .user(Some("TREU"))
        .pass(Some(password))
        .db_name(Some("treu_replica"))
        .prefer_socket(false);
    Pool::new(options)
}

#[test]
#[ignore = "VPN-t és helyi .env fájlt igényel"]
fn live_read_only_database_connection() {
    tauri::async_runtime::block_on(async {
        let pool = live_pool();
        let mut connection = pool
            .get_conn()
            .await
            .expect("MySQL connection must succeed");
        connection
            .query_drop("SET SESSION TRANSACTION READ ONLY")
            .await
            .expect("read-only session must be accepted");
        let result: Option<u8> = connection
            .query_first("SELECT 1")
            .await
            .expect("read-only probe query must succeed");
        assert_eq!(result, Some(1));
        drop(connection);
        pool.disconnect()
            .await
            .expect("pool must disconnect cleanly");
    });
}

#[test]
#[ignore = "VPN-t és helyi .env fájlt igényel"]
fn live_schema_inventory() {
    tauri::async_runtime::block_on(async {
        let pool = live_pool();
        let mut connection = pool
            .get_conn()
            .await
            .expect("MySQL connection must succeed");
        connection
            .query_drop("SET SESSION TRANSACTION READ ONLY")
            .await
            .expect("read-only session must be accepted");

        let tables: Vec<Row> = connection
            .exec(
                "SELECT TABLE_NAME, TABLE_TYPE, ENGINE, TABLE_ROWS, TABLE_COMMENT \
                 FROM information_schema.TABLES \
                 WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
                ("treu_replica",),
            )
            .await
            .expect("table metadata must be readable");
        let columns: Vec<Row> = connection
            .exec(
                "SELECT TABLE_NAME, COLUMN_NAME, ORDINAL_POSITION, DATA_TYPE, COLUMN_TYPE, \
                        IS_NULLABLE, COLUMN_KEY, EXTRA, COLUMN_COMMENT \
                 FROM information_schema.COLUMNS \
                 WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME, ORDINAL_POSITION",
                ("treu_replica",),
            )
            .await
            .expect("column metadata must be readable");
        let relationships: Vec<Row> = connection
            .exec(
                "SELECT TABLE_NAME, CONSTRAINT_NAME, COLUMN_NAME, ORDINAL_POSITION, \
                        REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
                 FROM information_schema.KEY_COLUMN_USAGE \
                 WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION",
                ("treu_replica",),
            )
            .await
            .expect("relationship metadata must be readable");

        let tables_json = tables
            .into_iter()
            .map(|row| {
                json!({
                    "name": row.get::<String, _>("TABLE_NAME").unwrap_or_default(),
                    "type": row.get::<String, _>("TABLE_TYPE").unwrap_or_default(),
                    "engine": row.get::<Option<String>, _>("ENGINE").flatten(),
                    "estimatedRows": row.get::<Option<u64>, _>("TABLE_ROWS").flatten(),
                    "comment": row.get::<String, _>("TABLE_COMMENT").unwrap_or_default()
                })
            })
            .collect::<Vec<Value>>();
        let columns_json = columns
            .into_iter()
            .map(|row| {
                json!({
                    "table": row.get::<String, _>("TABLE_NAME").unwrap_or_default(),
                    "name": row.get::<String, _>("COLUMN_NAME").unwrap_or_default(),
                    "position": row.get::<u64, _>("ORDINAL_POSITION").unwrap_or_default(),
                    "dataType": row.get::<String, _>("DATA_TYPE").unwrap_or_default(),
                    "columnType": row.get::<String, _>("COLUMN_TYPE").unwrap_or_default(),
                    "nullable": row.get::<String, _>("IS_NULLABLE").unwrap_or_default() == "YES",
                    "key": row.get::<String, _>("COLUMN_KEY").unwrap_or_default(),
                    "extra": row.get::<String, _>("EXTRA").unwrap_or_default(),
                    "comment": row.get::<String, _>("COLUMN_COMMENT").unwrap_or_default()
                })
            })
            .collect::<Vec<Value>>();
        let relationships_json = relationships
            .into_iter()
            .map(|row| {
                json!({
                    "table": row.get::<String, _>("TABLE_NAME").unwrap_or_default(),
                    "constraint": row.get::<String, _>("CONSTRAINT_NAME").unwrap_or_default(),
                    "column": row.get::<String, _>("COLUMN_NAME").unwrap_or_default(),
                    "position": row.get::<u64, _>("ORDINAL_POSITION").unwrap_or_default(),
                    "referencedTable": row
                        .get::<Option<String>, _>("REFERENCED_TABLE_NAME")
                        .flatten(),
                    "referencedColumn": row
                        .get::<Option<String>, _>("REFERENCED_COLUMN_NAME")
                        .flatten()
                })
            })
            .collect::<Vec<Value>>();
        let report = json!({
            "database": "treu_replica",
            "tables": tables_json,
            "columns": columns_json,
            "relationships": relationships_json
        });
        let report_path = env::var("ERGO_SCHEMA_REPORT")
            .unwrap_or_else(|_| "/private/tmp/ergo-schema-audit.json".into());
        fs::write(
            report_path,
            serde_json::to_vec_pretty(&report).expect("report must serialize"),
        )
        .expect("schema report must be writable");

        drop(connection);
        pool.disconnect()
            .await
            .expect("pool must disconnect cleanly");
    });
}
