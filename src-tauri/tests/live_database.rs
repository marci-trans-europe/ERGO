use std::{
    env, fs,
    io::{BufWriter, Write},
    path::Path,
};

use mysql_async::{prelude::Queryable, OptsBuilder, Pool, Row, Value as MyValue};
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

#[test]
#[ignore = "VPN-t és helyi .env fájlt igényel"]
fn live_2020_finance_inventory() {
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

        let queries = [
            (
                "outgoing_invoices",
                "invoice",
                "invoicedate",
                "SELECT MIN(invoicedate), MAX(invoicedate), COUNT(*), \
                        COUNT(CASE WHEN invoicedate LIKE '2020%' THEN 1 END), \
                        COUNT(CASE WHEN invoicedate LIKE '%2020%' THEN 1 END) FROM invoice",
            ),
            (
                "outgoing_invoice_lines",
                "invoiceline",
                "invoicedate",
                "SELECT MIN(invoicedate), MAX(invoicedate), COUNT(*), \
                        COUNT(CASE WHEN invoicedate LIKE '2020%' THEN 1 END), \
                        COUNT(CASE WHEN invoicedate LIKE '%2020%' THEN 1 END) FROM invoiceline",
            ),
            (
                "incoming_invoice_journal",
                "vendorinvoicejournal",
                "invoicedate",
                "SELECT MIN(invoicedate), MAX(invoicedate), COUNT(*), \
                        COUNT(CASE WHEN invoicedate LIKE '2020%' THEN 1 END), \
                        COUNT(CASE WHEN invoicedate LIKE '%2020%' THEN 1 END) FROM vendorinvoicejournal",
            ),
            (
                "incoming_vendor_entries",
                "vendorentry",
                "invoicedate",
                "SELECT MIN(invoicedate), MAX(invoicedate), COUNT(*), \
                        COUNT(CASE WHEN invoicedate LIKE '2020%' THEN 1 END), \
                        COUNT(CASE WHEN invoicedate LIKE '%2020%' THEN 1 END) FROM vendorentry",
            ),
            (
                "customer_entries",
                "customerentry",
                "invoicedate",
                "SELECT MIN(invoicedate), MAX(invoicedate), COUNT(*), \
                        COUNT(CASE WHEN invoicedate LIKE '2020%' THEN 1 END), \
                        COUNT(CASE WHEN invoicedate LIKE '%2020%' THEN 1 END) FROM customerentry",
            ),
            (
                "general_ledger",
                "financeentry",
                "postingdate",
                "SELECT MIN(postingdate), MAX(postingdate), COUNT(*), \
                        COUNT(CASE WHEN postingdate LIKE '2020%' THEN 1 END), \
                        COUNT(CASE WHEN postingdate LIKE '%2020%' THEN 1 END) FROM financeentry",
            ),
        ];

        let mut inventory = Vec::new();
        for (label, table, date_column, sql) in queries {
            let row: Option<Row> = connection
                .query_first(sql)
                .await
                .unwrap_or_else(|error| panic!("{table} inventory query failed: {error}"));
            let row = row.expect("aggregate query must return one row");
            inventory.push(json!({
                "label": label,
                "table": table,
                "dateColumn": date_column,
                "minDate": row.get::<Option<String>, _>(0).flatten(),
                "maxDate": row.get::<Option<String>, _>(1).flatten(),
                "totalRows": row.get::<u64, _>(2).unwrap_or_default(),
                "startsWith2020": row.get::<u64, _>(3).unwrap_or_default(),
                "contains2020": row.get::<u64, _>(4).unwrap_or_default(),
            }));
        }

        let fiscal_years: Vec<Row> = connection
            .query(
                "SELECT fiscalyear, COUNT(*) AS row_count FROM financeentry \
                    GROUP BY fiscalyear ORDER BY fiscalyear",
            )
            .await
            .expect("fiscal year inventory must be readable");
        let fiscal_years = fiscal_years
            .into_iter()
            .map(|row| {
                json!({
                    "fiscalYear": row.get::<Option<String>, _>("fiscalyear").flatten(),
                    "rowCount": row.get::<u64, _>("row_count").unwrap_or_default()
                })
            })
            .collect::<Vec<_>>();

        let report = json!({
            "database": "treu_replica",
            "inventory": inventory,
            "financeEntryFiscalYears": fiscal_years
        });
        let report_path = env::var("ERGO_2020_INVENTORY_REPORT")
            .unwrap_or_else(|_| "/private/tmp/ergo-2020-finance-inventory.json".into());
        fs::write(
            report_path,
            serde_json::to_vec_pretty(&report).expect("report must serialize"),
        )
        .expect("inventory report must be writable");

        drop(connection);
        pool.disconnect()
            .await
            .expect("pool must disconnect cleanly");
    });
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn mysql_value_text(value: MyValue) -> String {
    match value {
        MyValue::NULL => String::new(),
        MyValue::Bytes(bytes) => String::from_utf8(bytes).unwrap_or_else(|error| {
            let bytes = error.into_bytes();
            let mut encoded = String::with_capacity(2 + bytes.len() * 2);
            encoded.push_str("0x");
            for byte in bytes {
                use std::fmt::Write as _;
                write!(&mut encoded, "{byte:02x}").expect("writing to a string must succeed");
            }
            encoded
        }),
        MyValue::Int(value) => value.to_string(),
        MyValue::UInt(value) => value.to_string(),
        MyValue::Float(value) => value.to_string(),
        MyValue::Double(value) => value.to_string(),
        MyValue::Date(year, month, day, hour, minute, second, micros) => {
            if hour == 0 && minute == 0 && second == 0 && micros == 0 {
                format!("{year:04}-{month:02}-{day:02}")
            } else {
                format!(
                    "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{micros:06}"
                )
            }
        }
        MyValue::Time(negative, days, hours, minutes, seconds, micros) => format!(
            "{}{days} {hours:02}:{minutes:02}:{seconds:02}.{micros:06}",
            if negative { "-" } else { "" }
        ),
    }
}

#[test]
#[ignore = "VPN-t és helyi .env fájlt igényel"]
fn live_export_2020_finance_extract() {
    tauri::async_runtime::block_on(async {
        let output_dir = env::var("ERGO_2020_EXPORT_DIR")
            .unwrap_or_else(|_| "/private/tmp/ergo-2020-finance-extract".into());
        fs::create_dir_all(&output_dir).expect("export directory must be writable");

        let exports = [
            ("01_fokonyvi_tetelek.csv", "Főkönyvi tételek", "fiscalyear = 2020.01.01", "SELECT * FROM financeentry WHERE fiscalyear = '2020.01.01'"),
            ("02_kimeno_szamlak.csv", "Kimenő számlafejek", "invoicedate 2020", "SELECT * FROM invoice WHERE invoicedate >= '2020.01.01' AND invoicedate <= '2020.12.31'"),
            ("03_kimeno_szamlasorok.csv", "Kimenő számlasorok", "invoicedate 2020", "SELECT * FROM invoiceline WHERE invoicedate >= '2020.01.01' AND invoicedate <= '2020.12.31'"),
            ("04_kimeno_szamla_afak.csv", "Kimenő számlák áfabontása", "2020-as számlaszámhoz kapcsolódik", "SELECT v.* FROM invoicevatspecification v WHERE EXISTS (SELECT 1 FROM invoice i WHERE i.invoicenumber = v.invoicenumber AND i.invoicedate >= '2020.01.01' AND i.invoicedate <= '2020.12.31')"),
            ("05_kimeno_szamla_szovegek.csv", "Kimenő számlák szövegsorai", "2020-as számlaszámhoz kapcsolódik", "SELECT t.* FROM invoicetextline t WHERE EXISTS (SELECT 1 FROM invoice i WHERE i.invoicenumber = t.invoicenumber AND i.invoicedate >= '2020.01.01' AND i.invoicedate <= '2020.12.31')"),
            ("06_szamla_felosztasi_sorok.csv", "Számlafelosztási sorok", "invoicedate 2020", "SELECT * FROM invoiceallocationline WHERE invoicedate >= '2020.01.01' AND invoicedate <= '2020.12.31'"),
            ("07_vevo_tetelek.csv", "Vevőkönyvelési tételek", "postingdate 2020", "SELECT * FROM customerentry WHERE postingdate >= '2020.01.01' AND postingdate <= '2020.12.31'"),
            ("08_bejovo_szamla_naplo.csv", "Bejövő számlanapló", "postingdate vagy invoicedate 2020", "SELECT * FROM vendorinvoicejournal WHERE (postingdate >= '2020.01.01' AND postingdate <= '2020.12.31') OR (invoicedate >= '2020.01.01' AND invoicedate <= '2020.12.31')"),
            ("09_szallitoi_tetelek.csv", "Szállítói könyvelési tételek", "postingdate vagy invoicedate 2020", "SELECT * FROM vendorentry WHERE (postingdate >= '2020.01.01' AND postingdate <= '2020.12.31') OR (invoicedate >= '2020.01.01' AND invoicedate <= '2020.12.31')"),
            ("10_beszerzesi_bizonylatok.csv", "Beszerzési bizonylatok", "postingdate vagy invoicedate 2020", "SELECT * FROM purchasevoucher WHERE (postingdate >= '2020.01.01' AND postingdate <= '2020.12.31') OR (invoicedate >= '2020.01.01' AND invoicedate <= '2020.12.31')"),
            ("11_vevoi_fizetesek.csv", "Vevői fizetések", "entrydate 2020", "SELECT * FROM customerpayment WHERE entrydate >= '2020.01.01' AND entrydate <= '2020.12.31'"),
            ("12_szallitoi_fizetesek.csv", "Szállítói fizetések", "postingdate vagy entrydate 2020", "SELECT * FROM vendorpayment WHERE (postingdate >= '2020.01.01' AND postingdate <= '2020.12.31') OR (entrydate >= '2020.01.01' AND entrydate <= '2020.12.31')"),
            ("13_vevoi_kiegyenlitesek.csv", "Vevői kiegyenlítések", "reconciliationdate 2020", "SELECT * FROM customerreconciliation WHERE reconciliationdate >= '2020.01.01' AND reconciliationdate <= '2020.12.31'"),
            ("14_szallitoi_kiegyenlitesek.csv", "Szállítói kiegyenlítések", "reconciliationdate 2020", "SELECT * FROM vendorreconciliation WHERE reconciliationdate >= '2020.01.01' AND reconciliationdate <= '2020.12.31'"),
            ("15_vevoi_kiegyenlitesi_naplo.csv", "Vevői kiegyenlítési napló", "entrydate 2020", "SELECT * FROM customerreconciliationjourna WHERE entrydate >= '2020.01.01' AND entrydate <= '2020.12.31'"),
            ("16_szallitoi_kiegyenlitesi_naplo.csv", "Szállítói kiegyenlítési napló", "entrydate 2020", "SELECT * FROM vendorreconciliationjournal WHERE entrydate >= '2020.01.01' AND entrydate <= '2020.12.31'"),
            ("17_afa_tetelek.csv", "Áfaelszámolási tételek", "vatdate vagy entrydate 2020", "SELECT * FROM vatsettlemententry WHERE (vatdate >= '2020.01.01' AND vatdate <= '2020.12.31') OR (entrydate >= '2020.01.01' AND entrydate <= '2020.12.31')"),
            ("18_mozgasi_bizonylatok.csv", "Mozgási bizonylatok", "entrydate 2020", "SELECT * FROM movementvoucher WHERE entrydate >= '2020.01.01' AND entrydate <= '2020.12.31'"),
            ("19_altalanos_naplo.csv", "Általános napló", "entrydate 2020", "SELECT * FROM generaljournal WHERE entrydate >= '2020.01.01' AND entrydate <= '2020.12.31'"),
            ("20_naplok.csv", "Naplók", "fiscalyear vagy postingdate 2020", "SELECT * FROM journal WHERE fiscalyear = '2020.01.01' OR (postingdate >= '2020.01.01' AND postingdate <= '2020.12.31')"),
            ("21_penzugyi_periodusok.csv", "Pénzügyi periódusok", "fiscalyear = 2020.01.01", "SELECT * FROM financeperiod WHERE fiscalyear = '2020.01.01'"),
            ("22_szamlakeret.csv", "Számlakeret", "teljes törzs", "SELECT * FROM account"),
            ("23_vevotorzs.csv", "Vevőtörzs", "teljes törzs", "SELECT * FROM customer"),
            ("24_vallalati_vevotorzs.csv", "Vállalati vevőtörzs", "teljes törzs", "SELECT * FROM companycustomer"),
            ("25_szallitotorzs.csv", "Szállítótörzs", "teljes törzs", "SELECT * FROM vendor"),
            ("26_vallalati_szallitotorzs.csv", "Vállalati szállítótörzs", "teljes törzs", "SELECT * FROM companyvendor"),
            ("27_vallalati_adatok.csv", "Vállalati adatok", "teljes törzs", "SELECT * FROM companyinformation"),
        ];

        let pool = live_pool();
        let mut connection = pool
            .get_conn()
            .await
            .expect("MySQL connection must succeed");
        connection
            .query_drop("SET SESSION TRANSACTION READ ONLY")
            .await
            .expect("read-only session must be accepted");

        let mut manifest_entries = Vec::new();
        for (file_name, description, filter, sql) in exports {
            let path = Path::new(&output_dir).join(file_name);
            let file = fs::File::create(&path).expect("CSV file must be writable");
            let mut writer = BufWriter::new(file);
            let mut result = connection
                .query_iter(sql)
                .await
                .unwrap_or_else(|error| panic!("{file_name} query failed: {error}"));

            let headers = result
                .columns_ref()
                .iter()
                .map(|column| csv_field(&column.name_str()))
                .collect::<Vec<_>>()
                .join(",");
            writeln!(writer, "{headers}").expect("CSV header must be writable");

            let mut row_count = 0_u64;
            while let Some(row) = result
                .next()
                .await
                .unwrap_or_else(|error| panic!("{file_name} row read failed: {error}"))
            {
                let line = row
                    .unwrap()
                    .into_iter()
                    .map(mysql_value_text)
                    .map(|value| csv_field(&value))
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(writer, "{line}").expect("CSV row must be writable");
                row_count += 1;
            }
            writer.flush().expect("CSV file must flush cleanly");
            let file_size = fs::metadata(&path)
                .expect("CSV metadata must be readable")
                .len();
            manifest_entries.push(json!({
                "file": file_name,
                "description": description,
                "filter": filter,
                "rows": row_count,
                "bytes": file_size
            }));
        }

        let manifest = json!({
            "database": "treu_replica",
            "businessYear": 2020,
            "periodStart": "2020.01.01",
            "periodEnd": "2020.12.31",
            "readOnly": true,
            "format": "UTF-8, comma-separated CSV, quoted as needed",
            "nullRepresentation": "empty field",
            "exports": manifest_entries
        });
        fs::write(
            Path::new(&output_dir).join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest must serialize"),
        )
        .expect("manifest must be writable");
        fs::write(
            Path::new(&output_dir).join("README.txt"),
            "ERGO 2020. üzleti évi pénzügyi kivonat\n\nA csomag a treu_replica adatbázisból, kizárólag olvasási lekérdezésekkel készült.\nA főkönyvi tételek szűrése a fiscalyear mező, a kapcsolódó moduloké a manifest.json fájlban dokumentált dátummezők alapján történt.\nA CSV-fájlok UTF-8 kódolásúak, vesszővel tagoltak és fejlécet tartalmaznak.\nA NULL értékek üres mezőként jelennek meg.\nA törzsadat-fájlok teljes törzseket tartalmaznak, hogy a bizonylatok feloldhatók legyenek.\nA csomag bizalmas üzleti adatokat tartalmaz; kizárólag jogosult személynek továbbítható.\n",
        )
        .expect("README must be writable");

        drop(connection);
        pool.disconnect()
            .await
            .expect("pool must disconnect cleanly");
    });
}
