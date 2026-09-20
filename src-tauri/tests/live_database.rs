use std::{env, fs};

use mysql_async::{prelude::Queryable, OptsBuilder, Pool};

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

#[test]
#[ignore = "VPN-t és helyi .env fájlt igényel"]
fn live_read_only_database_connection() {
    let env_path = env::var("ERGO_ENV_PATH").expect("ERGO_ENV_PATH is required");
    let contents = fs::read_to_string(env_path).expect("the ERGO .env file must be readable");
    let password = dotenv_value(&contents, "MYSQL_PASSWORD")
        .expect("MYSQL_PASSWORD must be present in the ERGO .env file");

    tauri::async_runtime::block_on(async move {
        let options = OptsBuilder::default()
            .ip_or_hostname("10.21.36.17")
            .tcp_port(3306)
            .user(Some("TREU"))
            .pass(Some(password))
            .db_name(Some("treu_replica"))
            .prefer_socket(false);
        let pool = Pool::new(options);
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
