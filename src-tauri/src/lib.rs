use std::{fs, path::PathBuf, time::Duration};

use keyring::Entry;
use mysql_async::{prelude::Queryable, OptsBuilder, Pool, Row, SslOpts, Value};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tauri::{AppHandle, Manager};
use tokio::time::timeout;

const KEYCHAIN_SERVICE: &str = "hu.transeurope.ergo";
const MYSQL_PASSWORD_ACCOUNT: &str = "mysql-password";
const AI_API_KEY_ACCOUNT: &str = "ai-api-key";
const MAX_SCHEMA_COLUMNS: usize = 1_500;
const MAX_RESULT_ROWS: usize = 200;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredSettings {
    mysql_host: String,
    mysql_port: u16,
    mysql_database: String,
    mysql_user: String,
    mysql_ssl: bool,
    ai_base_url: String,
    ai_model: String,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            mysql_host: "10.21.36.17".into(),
            mysql_port: 3306,
            mysql_database: "treu_replica".into(),
            mysql_user: "TREU".into(),
            mysql_ssl: false,
            ai_base_url: "https://api.openai.com/v1".into(),
            ai_model: "gpt-6-astra".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsInput {
    mysql_host: String,
    mysql_port: u16,
    mysql_database: String,
    mysql_user: String,
    mysql_ssl: bool,
    mysql_password: String,
    ai_base_url: String,
    ai_model: String,
    ai_api_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicSettings {
    mysql_host: String,
    mysql_port: u16,
    mysql_database: String,
    mysql_user: String,
    mysql_ssl: bool,
    ai_base_url: String,
    ai_model: String,
    has_mysql_password: bool,
    has_ai_api_key: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct HistoryMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueryPlan {
    sql: String,
    title: String,
    visualization: String,
    max_rows: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QueryResult {
    kind: &'static str,
    title: String,
    visualization: String,
    columns: Vec<String>,
    rows: Vec<JsonValue>,
    row_count: usize,
    truncated: bool,
    sql: String,
}

#[derive(Debug, Serialize)]
struct AnalyzeResponse {
    summary: String,
    result: QueryResult,
}

#[derive(Debug, Deserialize)]
struct AiModelList {
    data: Vec<AiModel>,
}

#[derive(Debug, Deserialize)]
struct AiModel {
    id: String,
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("settings.json"))
        .map_err(|error| format!("A helyi konfiguráció útvonala nem érhető el: {error}"))
}

fn read_settings(app: &AppHandle) -> Result<StoredSettings, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(StoredSettings::default());
    }

    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("A helyi beállítások nem olvashatók: {error}"))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("A helyi beállítások sérültek: {error}"))
}

fn write_settings(app: &AppHandle, settings: &StoredSettings) -> Result<(), String> {
    let path = config_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "Érvénytelen konfigurációs útvonal.".to_string())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("A konfigurációs mappa nem hozható létre: {error}"))?;
    let contents = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("A beállítások nem alakíthatók át: {error}"))?;
    fs::write(path, contents).map_err(|error| format!("A helyi beállítások nem menthetők: {error}"))
}

fn secret_entry(account: &str) -> Result<Entry, String> {
    Entry::new(KEYCHAIN_SERVICE, account)
        .map_err(|error| format!("A macOS Kulcskarika nem érhető el: {error}"))
}

fn dotenv_value(app: &AppHandle, key: &str) -> Option<String> {
    let mut paths = vec![config_path(app).ok()?.with_file_name(".env")];
    if let Ok(current_dir) = std::env::current_dir() {
        paths.push(current_dir.join(".env.local"));
        paths.push(current_dir.join(".env"));
    }

    paths.into_iter().find_map(|path| {
        let contents = fs::read_to_string(path).ok()?;
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
    })
}

fn resolved_secret(app: &AppHandle, env_key: &str, account: &str) -> Option<String> {
    std::env::var(env_key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| dotenv_value(app, env_key))
        .or_else(|| secret_entry(account).ok()?.get_password().ok())
}

fn secret_exists(app: &AppHandle, env_key: &str, account: &str) -> bool {
    resolved_secret(app, env_key, account)
        .map(|value| !value.is_empty())
        .unwrap_or(false)
}

fn read_secret(
    app: &AppHandle,
    env_key: &str,
    account: &str,
    label: &str,
) -> Result<String, String> {
    resolved_secret(app, env_key, account).ok_or_else(|| {
        format!("A {label} nincs beállítva a helyi .env fájlban vagy a macOS Kulcskarikában.")
    })
}

fn write_secret(account: &str, value: &str) -> Result<(), String> {
    secret_entry(account)?
        .set_password(value)
        .map_err(|error| format!("A titok nem menthető a macOS Kulcskarikába: {error}"))
}

fn public_settings(app: &AppHandle, settings: StoredSettings) -> PublicSettings {
    PublicSettings {
        mysql_host: settings.mysql_host,
        mysql_port: settings.mysql_port,
        mysql_database: settings.mysql_database,
        mysql_user: settings.mysql_user,
        mysql_ssl: settings.mysql_ssl,
        ai_base_url: settings.ai_base_url,
        ai_model: settings.ai_model,
        has_mysql_password: secret_exists(app, "MYSQL_PASSWORD", MYSQL_PASSWORD_ACCOUNT),
        has_ai_api_key: secret_exists(app, "AI_API_KEY", AI_API_KEY_ACCOUNT),
    }
}

fn validate_settings(settings: &SettingsInput) -> Result<(), String> {
    if settings.mysql_host.trim().is_empty()
        || settings.mysql_database.trim().is_empty()
        || settings.mysql_user.trim().is_empty()
        || settings.ai_base_url.trim().is_empty()
        || settings.ai_model.trim().is_empty()
    {
        return Err("Minden kapcsolati mezőt ki kell tölteni.".into());
    }
    if !(settings.ai_base_url.starts_with("https://")
        || settings.ai_base_url.starts_with("http://localhost"))
    {
        return Err("Az AI API-címnek HTTPS-címet kell használnia.".into());
    }
    validate_model(&settings.ai_model)?;
    Ok(())
}

fn validate_model(model: &str) -> Result<&str, String> {
    let trimmed = model.trim();
    if trimmed.is_empty()
        || trimmed.len() > 160
        || !trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.:".contains(character))
    {
        return Err("Érvénytelen AI-modellazonosító.".into());
    }
    Ok(trimmed)
}

fn is_chat_model(model: &str) -> bool {
    let excluded = [
        "audio",
        "codex",
        "embedding",
        "image",
        "instruct",
        "moderation",
        "realtime",
        "search",
        "transcribe",
        "tts",
    ];
    let is_reasoning_model = model
        .strip_prefix('o')
        .and_then(|suffix| suffix.chars().next())
        .is_some_and(|character| character.is_ascii_digit());
    (model.starts_with("gpt-") || is_reasoning_model)
        && !excluded.iter().any(|part| model.contains(part))
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<PublicSettings, String> {
    read_settings(&app).map(|settings| public_settings(&app, settings))
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: SettingsInput) -> Result<PublicSettings, String> {
    validate_settings(&settings)?;

    if !settings.mysql_password.trim().is_empty() {
        write_secret(MYSQL_PASSWORD_ACCOUNT, settings.mysql_password.trim())?;
    }
    if !settings.ai_api_key.trim().is_empty() {
        write_secret(AI_API_KEY_ACCOUNT, settings.ai_api_key.trim())?;
    }
    if !secret_exists(&app, "MYSQL_PASSWORD", MYSQL_PASSWORD_ACCOUNT) {
        return Err("Az első beállításkor meg kell adni a MySQL-jelszót.".into());
    }
    if !secret_exists(&app, "AI_API_KEY", AI_API_KEY_ACCOUNT) {
        return Err("Az első beállításkor meg kell adni az AI API-kulcsot.".into());
    }

    let stored = StoredSettings {
        mysql_host: settings.mysql_host.trim().into(),
        mysql_port: settings.mysql_port,
        mysql_database: settings.mysql_database.trim().into(),
        mysql_user: settings.mysql_user.trim().into(),
        mysql_ssl: settings.mysql_ssl,
        ai_base_url: settings.ai_base_url.trim().trim_end_matches('/').into(),
        ai_model: settings.ai_model.trim().into(),
    };
    write_settings(&app, &stored)?;
    Ok(public_settings(&app, stored))
}

#[tauri::command]
fn select_ai_model(app: AppHandle, model: String) -> Result<PublicSettings, String> {
    let mut settings = read_settings(&app)?;
    settings.ai_model = validate_model(&model)?.into();
    write_settings(&app, &settings)?;
    Ok(public_settings(&app, settings))
}

#[tauri::command]
async fn list_ai_models(app: AppHandle) -> Result<Vec<String>, String> {
    let settings = read_settings(&app)?;
    let api_key = read_secret(&app, "AI_API_KEY", AI_API_KEY_ACCOUNT, "AI API-kulcs")?;
    let endpoint = format!("{}/models", settings.ai_base_url.trim_end_matches('/'));
    let response = timeout(
        Duration::from_secs(15),
        Client::new().get(endpoint).bearer_auth(api_key).send(),
    )
    .await
    .map_err(|_| "A modelllista lekérése időtúllépés miatt megszakadt.".to_string())?
    .map_err(|error| format!("A modelllista nem érhető el: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "Az AI-szolgáltatás nem adta át a modelllistát ({status})."
        ));
    }

    let mut models = response
        .json::<AiModelList>()
        .await
        .map_err(|error| format!("A modelllista formátuma érvénytelen: {error}"))?
        .data
        .into_iter()
        .map(|model| model.id)
        .filter(|model| is_chat_model(model))
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

fn database_pool(settings: &StoredSettings, password: String) -> Pool {
    let mut builder = OptsBuilder::default()
        .ip_or_hostname(settings.mysql_host.clone())
        .tcp_port(settings.mysql_port)
        .user(Some(settings.mysql_user.clone()))
        .pass(Some(password))
        .db_name(Some(settings.mysql_database.clone()))
        .prefer_socket(false);

    if settings.mysql_ssl {
        builder = builder.ssl_opts(Some(SslOpts::default()));
    }

    Pool::new(builder)
}

async fn test_database(settings: &StoredSettings, password: String) -> Result<(), String> {
    let pool = database_pool(settings, password);
    let operation = async {
        let mut connection = pool
            .get_conn()
            .await
            .map_err(|error| format!("A MySQL-szerver nem érhető el: {error}"))?;
        connection
            .query_drop("SET SESSION TRANSACTION READ ONLY")
            .await
            .map_err(|error| format!("A csak olvasható munkamenet nem indítható: {error}"))?;
        let _: Option<u8> = connection
            .query_first("SELECT 1")
            .await
            .map_err(|error| format!("A próbalekérdezés sikertelen: {error}"))?;
        Ok::<(), String>(())
    };

    let result = timeout(Duration::from_secs(12), operation)
        .await
        .map_err(|_| "Az adatbázis-kapcsolat időtúllépés miatt megszakadt.".to_string())?;
    pool.disconnect().await.ok();
    result
}

#[tauri::command]
async fn check_database(app: AppHandle) -> Result<(), String> {
    let settings = read_settings(&app)?;
    let password = read_secret(
        &app,
        "MYSQL_PASSWORD",
        MYSQL_PASSWORD_ACCOUNT,
        "MySQL-jelszó",
    )?;
    test_database(&settings, password).await
}

async fn inspect_schema(
    settings: &StoredSettings,
    password: String,
) -> Result<Vec<JsonValue>, String> {
    let pool = database_pool(settings, password);
    let mut connection = timeout(Duration::from_secs(12), pool.get_conn())
        .await
        .map_err(|_| "Az adatbázis-kapcsolat időtúllépés miatt megszakadt.".to_string())?
        .map_err(|error| format!("A MySQL-szerver nem érhető el: {error}"))?;
    connection
        .query_drop("SET SESSION TRANSACTION READ ONLY")
        .await
        .map_err(|error| format!("A csak olvasható munkamenet nem indítható: {error}"))?;

    let sql = format!(
        "SELECT c.TABLE_NAME, t.TABLE_TYPE, c.COLUMN_NAME, c.COLUMN_TYPE, c.IS_NULLABLE \
         FROM information_schema.COLUMNS c \
         INNER JOIN information_schema.TABLES t \
           ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
         WHERE c.TABLE_SCHEMA = ? \
         ORDER BY c.TABLE_NAME, c.ORDINAL_POSITION LIMIT {}",
        MAX_SCHEMA_COLUMNS
    );
    let rows: Vec<Row> = connection
        .exec(sql, (settings.mysql_database.clone(),))
        .await
        .map_err(|error| format!("Az adatbázis-séma nem olvasható: {error}"))?;
    drop(connection);
    pool.disconnect().await.ok();

    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "table": row.get::<String, _>("TABLE_NAME").unwrap_or_default(),
                "tableType": row.get::<String, _>("TABLE_TYPE").unwrap_or_default(),
                "column": row.get::<String, _>("COLUMN_NAME").unwrap_or_default(),
                "dataType": row.get::<String, _>("COLUMN_TYPE").unwrap_or_default(),
                "nullable": row.get::<String, _>("IS_NULLABLE").unwrap_or_default() == "YES"
            })
        })
        .collect())
}

fn normalize_sql(sql: &str) -> String {
    let mut normalized = sql.trim().to_string();
    if normalized.starts_with("```sql") {
        normalized = normalized.trim_start_matches("```sql").trim().into();
    } else if normalized.starts_with("```") {
        normalized = normalized.trim_start_matches("```").trim().into();
    }
    if normalized.ends_with("```") {
        normalized = normalized.trim_end_matches("```").trim().into();
    }
    normalized.trim_end_matches(';').trim().into()
}

fn assert_read_only_sql(sql: &str) -> Result<String, String> {
    let normalized = normalize_sql(sql);
    let lower = normalized.to_lowercase();
    if normalized.is_empty() || normalized.len() > 12_000 {
        return Err("Az SQL-lekérdezés üres vagy túl hosszú.".into());
    }
    if !(lower.starts_with("select") || lower.starts_with("with")) {
        return Err("Csak SELECT vagy WITH lekérdezés engedélyezett.".into());
    }
    if normalized.contains(';') {
        return Err("Egyszerre csak egy SQL-utasítás engedélyezett.".into());
    }

    let forbidden = [
        r"(?i)\b(insert|update|delete|replace|drop|alter|create|truncate|rename)\b",
        r"(?i)\b(grant|revoke|call|execute|prepare|deallocate|handler|load|lock|unlock)\b",
        r"(?i)\b(into\s+(outfile|dumpfile)|load_file|sleep|benchmark)\s*\(",
        r"(?i)\b(mysql|performance_schema|sys)\s*\.",
        r"(?i)\b(for\s+update|for\s+share|lock\s+in\s+share\s+mode)\b",
        r"(?i)\b(user|current_user|session_user|system_user)\s*\(",
        r"@@|--|#|/\*|\*/",
    ];
    if forbidden.iter().any(|pattern| {
        Regex::new(pattern)
            .expect("valid SQL guard regex")
            .is_match(&normalized)
    }) {
        return Err("A lekérdezés tiltott vagy nem csak olvasható elemet tartalmaz.".into());
    }

    Ok(normalized)
}

fn mysql_value_to_json(value: Option<&Value>) -> JsonValue {
    match value {
        None | Some(Value::NULL) => JsonValue::Null,
        Some(Value::Bytes(bytes)) => String::from_utf8(bytes.clone())
            .map(JsonValue::String)
            .unwrap_or_else(|_| JsonValue::String(format!("[bináris adat: {} bájt]", bytes.len()))),
        Some(Value::Int(number)) => json!(number),
        Some(Value::UInt(number)) => json!(number),
        Some(Value::Float(number)) => json!(number),
        Some(Value::Double(number)) => json!(number),
        Some(Value::Date(year, month, day, hour, minute, second, micros)) => JsonValue::String(
            format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{micros:06}"),
        ),
        Some(Value::Time(negative, days, hours, minutes, seconds, micros)) => {
            JsonValue::String(format!(
                "{}{days}d {hours:02}:{minutes:02}:{seconds:02}.{micros:06}",
                if *negative { "-" } else { "" }
            ))
        }
    }
}

async fn run_query(
    settings: &StoredSettings,
    password: String,
    sql: &str,
    max_rows: usize,
) -> Result<(Vec<String>, Vec<JsonValue>, bool), String> {
    let normalized = assert_read_only_sql(sql)?;
    let applied_limit = max_rows.clamp(1, MAX_RESULT_ROWS);
    let wrapped = format!(
        "SELECT * FROM ({normalized}) AS ergo_readonly_query LIMIT {}",
        applied_limit + 1
    );
    let pool = database_pool(settings, password);
    let mut connection = timeout(Duration::from_secs(12), pool.get_conn())
        .await
        .map_err(|_| "Az adatbázis-kapcsolat időtúllépés miatt megszakadt.".to_string())?
        .map_err(|error| format!("A MySQL-szerver nem érhető el: {error}"))?;
    connection
        .query_drop("START TRANSACTION READ ONLY")
        .await
        .map_err(|error| format!("A csak olvasható tranzakció nem indítható: {error}"))?;

    let query_result = timeout(Duration::from_secs(30), connection.query::<Row, _>(wrapped)).await;
    let rows = match query_result {
        Ok(Ok(rows)) => rows,
        Ok(Err(error)) => {
            connection.query_drop("ROLLBACK").await.ok();
            return Err(format!("Az ERP-lekérdezés sikertelen: {error}"));
        }
        Err(_) => {
            connection.query_drop("ROLLBACK").await.ok();
            return Err("Az ERP-lekérdezés 30 másodperces időkorlátja lejárt.".into());
        }
    };
    connection.query_drop("COMMIT").await.ok();
    drop(connection);
    pool.disconnect().await.ok();

    let columns = rows
        .first()
        .map(|row| {
            row.columns_ref()
                .iter()
                .map(|column| column.name_str().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let truncated = rows.len() > applied_limit;
    let serialized = rows
        .into_iter()
        .take(applied_limit)
        .map(|row| {
            let object = columns
                .iter()
                .enumerate()
                .map(|(index, column)| (column.clone(), mysql_value_to_json(row.as_ref(index))))
                .collect();
            JsonValue::Object(object)
        })
        .collect();

    Ok((columns, serialized, truncated))
}

fn extract_json<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, String> {
    let start = text
        .find('{')
        .ok_or_else(|| "Az AI nem adott értelmezhető lekérdezési tervet.".to_string())?;
    let end = text
        .rfind('}')
        .ok_or_else(|| "Az AI nem adott értelmezhető lekérdezési tervet.".to_string())?;
    serde_json::from_str(&text[start..=end])
        .map_err(|error| format!("Az AI lekérdezési terve érvénytelen: {error}"))
}

async fn call_ai(
    settings: &StoredSettings,
    api_key: &str,
    system: &str,
    user: &str,
    json_mode: bool,
) -> Result<String, String> {
    let endpoint = format!(
        "{}/chat/completions",
        settings.ai_base_url.trim_end_matches('/')
    );
    let mut request = json!({
        "model": settings.ai_model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "max_completion_tokens": 2000
    });
    if json_mode {
        request["response_format"] = json!({ "type": "json_object" });
    }

    let response = timeout(
        Duration::from_secs(55),
        Client::new()
            .post(endpoint)
            .bearer_auth(api_key)
            .json(&request)
            .send(),
    )
    .await
    .map_err(|_| "Az AI-szolgáltatás időtúllépés miatt megszakadt.".to_string())?
    .map_err(|error| format!("Az AI-szolgáltatás nem érhető el: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Az AI-válasz nem olvasható: {error}"))?;
    if !status.is_success() {
        let concise = body.chars().take(500).collect::<String>();
        return Err(format!(
            "Az AI-szolgáltatás hibát jelzett ({status}): {concise}"
        ));
    }

    let response_json: JsonValue = serde_json::from_str(&body)
        .map_err(|error| format!("Az AI-válasz formátuma érvénytelen: {error}"))?;
    response_json["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "Az AI-válasz nem tartalmazott szöveget.".into())
}

#[tauri::command]
async fn analyze_erp(
    app: AppHandle,
    question: String,
    history: Vec<HistoryMessage>,
) -> Result<AnalyzeResponse, String> {
    if question.trim().is_empty() {
        return Err("A kérdés nem lehet üres.".into());
    }
    let settings = read_settings(&app)?;
    let mysql_password = read_secret(
        &app,
        "MYSQL_PASSWORD",
        MYSQL_PASSWORD_ACCOUNT,
        "MySQL-jelszó",
    )?;
    let api_key = read_secret(&app, "AI_API_KEY", AI_API_KEY_ACCOUNT, "AI API-kulcs")?;
    let schema = inspect_schema(&settings, mysql_password.clone()).await?;
    let schema_json = serde_json::to_string(&schema)
        .map_err(|error| format!("A séma nem alakítható át: {error}"))?;
    let history_text = history
        .iter()
        .filter(|message| message.role == "user" || message.role == "assistant")
        .map(|message| format!("{}: {}", message.role, message.content))
        .collect::<Vec<_>>()
        .join("\n");

    let planner_system = r#"Te az ERGO, a Trans-Europe óvatos ERP-adatelemzője vagy.
Készíts pontos MySQL lekérdezési tervet a megadott adatbázis-séma alapján.
Kizárólag egy SELECT vagy WITH lekérdezést adhatsz. Tilos minden adatmódosítás, DDL, zárolás, fájlművelet, komment, rendszer-séma és több utasítás.
Ne találj ki táblát vagy oszlopot. Használj explicit oszlopokat és aggregálj SQL-ben. A sorok száma legfeljebb 200 legyen.
Kizárólag JSON objektummal válaszolj ebben az alakban:
{"sql":"...","title":"rövid magyar cím","visualization":"table|bar|line","maxRows":50}"#;
    let planner_user = format!(
        "Korábbi beszélgetés:\n{history_text}\n\nFelhasználói kérdés:\n{}\n\nAdatbázis-séma:\n{schema_json}",
        question.trim()
    );
    let plan_text = call_ai(&settings, &api_key, planner_system, &planner_user, true).await?;
    let mut plan: QueryPlan = extract_json(&plan_text)?;
    plan.sql = assert_read_only_sql(&plan.sql)?;
    plan.max_rows = plan.max_rows.clamp(1, MAX_RESULT_ROWS);
    if !matches!(plan.visualization.as_str(), "table" | "bar" | "line") {
        plan.visualization = "table".into();
    }

    let (columns, rows, truncated) =
        run_query(&settings, mysql_password, &plan.sql, plan.max_rows).await?;
    let result_preview = serde_json::to_string(&rows)
        .map_err(|error| format!("Az eredmény nem alakítható át: {error}"))?;
    let summary_system = r#"Te az ERGO, a Trans-Europe üzleti adatelemzője vagy.
Magyarul válaszolj. Kezdd a legfontosabb üzleti következtetéssel, majd támaszd alá a kapott számokkal.
Ne találj ki adatot, pénznemet, mértékegységet vagy üzleti definíciót. Ha az eredmény üres vagy kétértelmű, ezt mondd ki.
Legyél tömör, gyakorlatias, és jelezd, ha a látható eredmény korlátozott. Markdown használható."#;
    let summary_user = format!(
        "Eredeti kérdés: {}\n\nLekérdezési eredmény (JSON):\n{}\n\nCsonkolt eredmény: {}",
        question.trim(),
        result_preview,
        truncated
    );
    let summary = call_ai(&settings, &api_key, summary_system, &summary_user, false).await?;
    let row_count = rows.len();

    Ok(AnalyzeResponse {
        summary,
        result: QueryResult {
            kind: "query-result",
            title: plan.title,
            visualization: plan.visualization,
            columns,
            rows,
            row_count,
            truncated,
            sql: plan.sql,
        },
    })
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            analyze_erp,
            check_database,
            list_ai_models,
            load_settings,
            save_settings,
            select_ai_model
        ])
        .run(tauri::generate_context!())
        .expect("az ERGO alkalmazás nem indítható");
}

#[cfg(test)]
mod tests {
    use super::{assert_read_only_sql, extract_json, is_chat_model, validate_model, QueryPlan};

    #[test]
    fn accepts_a_single_select() {
        assert_eq!(
            assert_read_only_sql("SELECT id, total FROM invoices LIMIT 10").unwrap(),
            "SELECT id, total FROM invoices LIMIT 10"
        );
    }

    #[test]
    fn rejects_data_changes_and_multiple_statements() {
        assert!(assert_read_only_sql("DELETE FROM invoices").is_err());
        assert!(assert_read_only_sql("SELECT 1; SELECT 2").is_err());
        assert!(assert_read_only_sql("SELECT SLEEP(10)").is_err());
    }

    #[test]
    fn extracts_a_json_plan_from_a_fenced_response() {
        let plan: QueryPlan = extract_json(
            "```json\n{\"sql\":\"SELECT 1\",\"title\":\"Próba\",\"visualization\":\"table\",\"maxRows\":1}\n```",
        )
        .unwrap();
        assert_eq!(plan.sql, "SELECT 1");
        assert_eq!(plan.max_rows, 1);
    }

    #[test]
    fn filters_non_chat_models() {
        assert!(is_chat_model("gpt-6-astra"));
        assert!(is_chat_model("o3"));
        assert!(!is_chat_model("gpt-image-1"));
        assert!(!is_chat_model("text-embedding-3-large"));
    }

    #[test]
    fn validates_model_identifiers() {
        assert_eq!(validate_model("gpt-5.6-terra").unwrap(), "gpt-5.6-terra");
        assert!(validate_model("../invalid model").is_err());
    }
}
