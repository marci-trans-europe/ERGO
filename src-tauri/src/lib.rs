use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashSet},
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

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
const MAX_RESULT_ROWS: usize = 200;
const MAX_SCHEMA_TABLES_PER_QUESTION: usize = 8;
const MAX_SCHEMA_COLUMNS_PER_TABLE: usize = 36;
const SCHEMA_CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const SUMMARY_PREVIEW_ROWS: usize = 40;
const SUMMARY_PREVIEW_CHARACTERS: usize = 18_000;
const FEEDBACK_EMAIL: &str = "marton.trautmann@icloud.com";

fn default_analysis_fy_window() -> u8 {
    5
}

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
    #[serde(default = "default_analysis_fy_window")]
    analysis_fy_window: u8,
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
            ai_model: "gpt-6-sol".into(),
            analysis_fy_window: default_analysis_fy_window(),
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
    analysis_fy_window: u8,
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
    analysis_fy_window: u8,
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
    query_source: &'static str,
    token_usage: TokenUsage,
    schema_selection: SchemaSelectionStats,
}

#[derive(Debug, Serialize)]
struct AnalyzeResponse {
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<QueryResult>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenUsage {
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    total_tokens: u64,
}

impl TokenUsage {
    fn add(&mut self, other: &Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.total_tokens += other.total_tokens;
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaSelectionStats {
    total_tables: usize,
    total_columns: usize,
    selected_tables: usize,
    selected_columns: usize,
    context_characters: usize,
}

#[derive(Clone, Debug)]
struct AiCallResult {
    content: String,
    usage: TokenUsage,
}

fn fixed_quick_plan(quick_analysis: &str, analysis_fy_window: u8) -> Result<QueryPlan, String> {
    let fiscal_year_start = format!(
        "MAKEDATE(YEAR(CURRENT_DATE) - {}, 1)",
        analysis_fy_window - 1
    );
    match quick_analysis {
        "revenue-trend" => Ok(QueryPlan {
            sql: format!(
                "SELECT DATE_FORMAT(STR_TO_DATE(il.invoicedate, '%Y.%m.%d'), '%Y-%m') AS honap, ROUND(SUM(il.vatbase), 2) AS arbevetel FROM invoiceline AS il WHERE STR_TO_DATE(il.invoicedate, '%Y.%m.%d') >= GREATEST(DATE_SUB(CURRENT_DATE, INTERVAL 12 MONTH), {fiscal_year_start}) AND STR_TO_DATE(il.invoicedate, '%Y.%m.%d') <= CURRENT_DATE GROUP BY DATE_FORMAT(STR_TO_DATE(il.invoicedate, '%Y.%m.%d'), '%Y-%m') ORDER BY honap ASC"
            ),
            title: "Havi árbevételi trend".into(),
            visualization: "line".into(),
            max_rows: 50,
        }),
        "top-customers" => Ok(QueryPlan {
            sql: "SELECT il.customernumber AS ugyfelszam, COALESCE(NULLIF(TRIM(CONCAT_WS(' ', c.name1, c.name2, c.name3, c.name4, c.name5)), ''), il.customernumber) AS ugyfel, ROUND(SUM(il.vatbase), 2) AS arbevetel FROM invoiceline AS il LEFT JOIN customer AS c ON c.customernumber = il.customernumber WHERE STR_TO_DATE(il.invoicedate, '%Y.%m.%d') >= MAKEDATE(YEAR(CURRENT_DATE), 1) AND STR_TO_DATE(il.invoicedate, '%Y.%m.%d') <= CURRENT_DATE GROUP BY il.customernumber, c.name1, c.name2, c.name3, c.name4, c.name5 ORDER BY arbevetel DESC LIMIT 10".into(),
            title: "Top 10 ügyfél idei árbevétel szerint".into(),
            visualization: "bar".into(),
            max_rows: 10,
        }),
        "overdue-receivables" => Ok(QueryPlan {
            sql: format!(
                "SELECT ce.customernumber AS ugyfelszam, COALESCE(NULLIF(TRIM(CONCAT_WS(' ', c.name1, c.name2, c.name3, c.name4, c.name5)), ''), ce.customernumber) AS ugyfel, ce.transactionnumber AS szamlaszam, DATE_FORMAT(STR_TO_DATE(ce.invoicedate, '%Y.%m.%d'), '%Y-%m-%d') AS szamla_datum, DATE_FORMAT(STR_TO_DATE(ce.duedate, '%Y.%m.%d'), '%Y-%m-%d') AS esedekesseg, DATEDIFF(CURRENT_DATE, STR_TO_DATE(ce.duedate, '%Y.%m.%d')) AS kesedelmes_napok, ROUND(COALESCE(i.totalstandard, ce.debitstandard - ce.creditstandard), 2) AS szamla_osszeg, ROUND(ce.remainderstandard, 2) AS kintlevoseg, COALESCE(i.standardcurrency, ce.originalcurrency) AS penznemkod, COUNT(il.linenumber) AS cikksorok_szama, GROUP_CONCAT(CASE WHEN il.linenumber IS NULL THEN 'Nincs kapcsolt számlasor' ELSE CONCAT_WS(' · ', COALESCE(NULLIF(il.itemnumber, ''), 'cikkszám nélkül'), COALESCE(NULLIF(il.itemtext1, ''), NULLIF(il.externalitemtext, ''), 'megnevezés nélkül'), CONCAT('menny.: ', COALESCE(il.numberinvoiced, 0)), CONCAT('nettó: ', COALESCE(il.vatbase, 0))) END ORDER BY il.linenumber SEPARATOR ' | ') AS cikkek FROM customerentry AS ce LEFT JOIN invoice AS i ON i.invoicenumber = ce.transactionnumber AND i.customernumber = ce.customernumber AND i.companynumber = ce.companynumber LEFT JOIN invoiceline AS il ON il.invoicenumber = i.invoicenumber AND il.companynumber = i.companynumber LEFT JOIN customer AS c ON c.customernumber = ce.customernumber WHERE STR_TO_DATE(ce.duedate, '%Y.%m.%d') < CURRENT_DATE AND STR_TO_DATE(ce.postingdate, '%Y.%m.%d') >= {fiscal_year_start} AND STR_TO_DATE(ce.postingdate, '%Y.%m.%d') <= CURRENT_DATE AND ce.remainderstandard > 0 GROUP BY ce.customernumber, c.name1, c.name2, c.name3, c.name4, c.name5, ce.transactionnumber, ce.invoicedate, ce.duedate, i.totalstandard, ce.debitstandard, ce.creditstandard, ce.remainderstandard, i.standardcurrency, ce.originalcurrency ORDER BY kintlevoseg DESC LIMIT 50"
            ),
            title: "Lejárt kintlévőségek számlánként és cikksoronként".into(),
            visualization: "table".into(),
            max_rows: 50,
        }),
        _ => Err("Ismeretlen gyors elemzés.".into()),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaCatalog {
    database: String,
    source: String,
    generated_at: u64,
    tables: Vec<SchemaTable>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaTable {
    name: String,
    table_type: String,
    estimated_rows: Option<u64>,
    columns: Vec<CatalogColumn>,
    relationships: Vec<CatalogRelationship>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogColumn {
    name: String,
    column_type: String,
    nullable: bool,
    key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogRelationship {
    column: String,
    referenced_table: String,
    referenced_column: String,
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

fn schema_cache_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("schema-catalog.json"))
        .map_err(|error| format!("A helyi sémagyorsítótár útvonala nem érhető el: {error}"))
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
        analysis_fy_window: settings.analysis_fy_window,
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
    validate_analysis_fy_window(settings.analysis_fy_window)?;
    Ok(())
}

fn validate_analysis_fy_window(years: u8) -> Result<u8, String> {
    if matches!(years, 1 | 3 | 5) {
        Ok(years)
    } else {
        Err("Az elemzési időablak csak 1, 3 vagy 5 üzleti év lehet.".into())
    }
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
        analysis_fy_window: settings.analysis_fy_window,
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
fn select_analysis_fy_window(app: AppHandle, years: u8) -> Result<PublicSettings, String> {
    let mut settings = read_settings(&app)?;
    settings.analysis_fy_window = validate_analysis_fy_window(years)?;
    write_settings(&app, &settings)?;
    Ok(public_settings(&app, settings))
}

fn percent_encode_mailto(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[tauri::command]
fn open_feedback_email(app: AppHandle, message: String) -> Result<(), String> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err("A visszajelzés nem lehet üres.".into());
    }
    if trimmed.chars().count() > 2_000 {
        return Err("A visszajelzés legfeljebb 2000 karakter lehet.".into());
    }
    let settings = read_settings(&app)?;
    let subject = format!("ERGO {} visszajelzés", env!("CARGO_PKG_VERSION"));
    let body = format!(
        "ERGO visszajelzés\n\n{trimmed}\n\n---\nVerzió: {}\nModell: {}\nElemzési időablak: {} FY\nRendszer: {} {}\n\nA levél nem tartalmaz automatikusan ERP-adatot vagy lekérdezési eredményt.",
        env!("CARGO_PKG_VERSION"),
        settings.ai_model,
        settings.analysis_fy_window,
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    let mailto = format!(
        "mailto:{FEEDBACK_EMAIL}?subject={}&body={}",
        percent_encode_mailto(&subject),
        percent_encode_mailto(&body)
    );

    Command::new("open")
        .arg(mailto)
        .spawn()
        .map_err(|error| format!("A levelezőalkalmazás nem nyitható meg: {error}"))?;
    Ok(())
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

async fn inspect_schema_catalog(
    settings: &StoredSettings,
    password: String,
) -> Result<SchemaCatalog, String> {
    let pool = database_pool(settings, password);
    let mut connection = timeout(Duration::from_secs(12), pool.get_conn())
        .await
        .map_err(|_| "Az adatbázis-kapcsolat időtúllépés miatt megszakadt.".to_string())?
        .map_err(|error| format!("A MySQL-szerver nem érhető el: {error}"))?;
    connection
        .query_drop("SET SESSION TRANSACTION READ ONLY")
        .await
        .map_err(|error| format!("A csak olvasható munkamenet nem indítható: {error}"))?;

    let table_rows: Vec<Row> = connection
        .exec(
            "SELECT TABLE_NAME, TABLE_TYPE, TABLE_ROWS \
             FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
            (settings.mysql_database.clone(),),
        )
        .await
        .map_err(|error| format!("A tábla-metaadatok nem olvashatók: {error}"))?;
    let column_rows: Vec<Row> = connection
        .exec(
            "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_KEY \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME, ORDINAL_POSITION",
            (settings.mysql_database.clone(),),
        )
        .await
        .map_err(|error| format!("Az oszlop-metaadatok nem olvashatók: {error}"))?;
    let relationship_rows: Vec<Row> = connection
        .exec(
            "SELECT TABLE_NAME, COLUMN_NAME, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
             FROM information_schema.KEY_COLUMN_USAGE \
             WHERE TABLE_SCHEMA = ? AND REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION",
            (settings.mysql_database.clone(),),
        )
        .await
        .map_err(|error| format!("A kapcsolati metaadatok nem olvashatók: {error}"))?;
    drop(connection);
    pool.disconnect().await.ok();

    let mut tables = table_rows
        .into_iter()
        .map(|row| {
            let name = row.get::<String, _>("TABLE_NAME").unwrap_or_default();
            (
                name.clone(),
                SchemaTable {
                    name,
                    table_type: row.get::<String, _>("TABLE_TYPE").unwrap_or_default(),
                    estimated_rows: row.get::<Option<u64>, _>("TABLE_ROWS").flatten(),
                    columns: Vec::new(),
                    relationships: Vec::new(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    for row in column_rows {
        let table_name = row.get::<String, _>("TABLE_NAME").unwrap_or_default();
        if let Some(table) = tables.get_mut(&table_name) {
            table.columns.push(CatalogColumn {
                name: row.get::<String, _>("COLUMN_NAME").unwrap_or_default(),
                column_type: row.get::<String, _>("COLUMN_TYPE").unwrap_or_default(),
                nullable: row.get::<String, _>("IS_NULLABLE").unwrap_or_default() == "YES",
                key: row.get::<String, _>("COLUMN_KEY").unwrap_or_default(),
            });
        }
    }

    for row in relationship_rows {
        let table_name = row.get::<String, _>("TABLE_NAME").unwrap_or_default();
        if let Some(table) = tables.get_mut(&table_name) {
            table.relationships.push(CatalogRelationship {
                column: row.get::<String, _>("COLUMN_NAME").unwrap_or_default(),
                referenced_table: row
                    .get::<String, _>("REFERENCED_TABLE_NAME")
                    .unwrap_or_default(),
                referenced_column: row
                    .get::<String, _>("REFERENCED_COLUMN_NAME")
                    .unwrap_or_default(),
            });
        }
    }

    Ok(SchemaCatalog {
        database: settings.mysql_database.clone(),
        source: format!(
            "{}:{}/{}",
            settings.mysql_host, settings.mysql_port, settings.mysql_database
        ),
        generated_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        tables: tables.into_values().collect(),
    })
}

async fn load_schema_catalog(
    app: &AppHandle,
    settings: &StoredSettings,
    password: String,
) -> Result<SchemaCatalog, String> {
    let path = schema_cache_path(app)?;
    let expected_source = format!(
        "{}:{}/{}",
        settings.mysql_host, settings.mysql_port, settings.mysql_database
    );
    if let Ok(contents) = fs::read_to_string(&path) {
        if let Ok(catalog) = serde_json::from_str::<SchemaCatalog>(&contents) {
            let age = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_sub(catalog.generated_at);
            if catalog.source == expected_source && age <= SCHEMA_CACHE_MAX_AGE.as_secs() {
                return Ok(catalog);
            }
        }
    }

    let catalog = inspect_schema_catalog(settings, password).await?;
    let directory = path
        .parent()
        .ok_or_else(|| "Érvénytelen sémagyorsítótár-útvonal.".to_string())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("A sémagyorsítótár mappája nem hozható létre: {error}"))?;
    let contents = serde_json::to_vec(&catalog)
        .map_err(|error| format!("A sémagyorsítótár nem alakítható át: {error}"))?;
    fs::write(path, contents)
        .map_err(|error| format!("A sémagyorsítótár nem menthető: {error}"))?;
    Ok(catalog)
}

fn normalize_for_search(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|character| match character {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' | 'ö' | 'ő' => 'o',
            'ú' | 'ü' | 'ű' => 'u',
            other if other.is_ascii_alphanumeric() => other,
            _ => ' ',
        })
        .collect()
}

fn capability_answer(question: &str) -> Option<&'static str> {
    let normalized = normalize_for_search(question);
    let capability_phrases = [
        "milyen adatokbol tudsz",
        "milyen adatokkal tudsz",
        "milyen adatokat ersz el",
        "milyen adatokat latsz",
        "milyen adatokat tudsz",
        "milyen elemzeseket tudsz",
        "miben tudsz segiteni",
        "mire vagy kepes",
        "mit tudsz",
        "mi van az adatbazisban",
    ];
    if !capability_phrases
        .iter()
        .any(|phrase| normalized.contains(phrase))
    {
        return None;
    }

    Some(
        r#"A **Trans-Europe Zrt. köztes, csak olvasható ERP-adatbázisában** található átfogó üzleti adatokból tudok dolgozni.

Többek között képes vagyok:

- napi, havi vagy egyedi időszakra vonatkozó kimenő és bejövő számlákat, valamint számlatételeket kilistázni;
- árbevételt, költségeket, kintlévőségeket, fizetéseket és főkönyvi adatokat elemezni;
- vevői, szállítói, rendelési, fuvarozási és egyéb ERP-adatokat összesíteni;
- trendeket, eltéréseket és kiemelkedő értékeket azonosítani;
- az eredményeket táblázatban, illetve oszlop- vagy vonaldiagramon megjeleníteni.

Az adatokat nem módosítom. A lekérdezéseket a kérdésben megadott időszakra, illetve a fejlécben kiválasztott 1, 3 vagy 5 üzletiéves időablakra szűkítem."#,
    )
}

fn schema_search_terms(question: &str, history: &[HistoryMessage]) -> HashSet<String> {
    let stop_words = [
        "adat", "adatok", "alapjan", "az", "egy", "es", "hogy", "kerlek", "legyen", "meg", "mely",
        "melyik", "mi", "mind", "mutasd", "osszes", "szerint", "van", "volt",
    ];
    let mut search_text = history
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .take(2)
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    search_text.push(question);
    let normalized = normalize_for_search(&search_text.join(" "));
    let mut terms = normalized
        .split_whitespace()
        .filter(|term| term.len() >= 3 && !stop_words.contains(term))
        .map(str::to_owned)
        .collect::<HashSet<_>>();

    let domain_terms: &[(&[&str], &[&str])] = &[
        (
            &["arbevetel", "bevetel", "forgalom", "sales"],
            &["invoice", "invoiceline", "revenue", "amount", "net", "vat"],
        ),
        (
            &["ugyfel", "vevo", "customer"],
            &[
                "customer",
                "companycustomer",
                "endcustomer",
                "customernumber",
                "name",
            ],
        ),
        (
            &["kintlevoseg", "tartozas", "lejart", "koveteles"],
            &[
                "customerentry",
                "customerbalance",
                "remainder",
                "duedate",
                "openclosed",
            ],
        ),
        (
            &["szamla", "invoice"],
            &["invoice", "invoiceline", "invoicenumber", "invoicedate"],
        ),
        (
            &["fuvar", "szallitas", "megbizas", "transport"],
            &[
                "jobheader",
                "jobtrafficinformation",
                "orderheader",
                "deliverynote",
                "freight",
            ],
        ),
        (
            &["rendeles", "order"],
            &["orderheader", "orderline", "ordernumber"],
        ),
        (
            &["beszallito", "vendor"],
            &[
                "vendor",
                "companyvendor",
                "vendorentry",
                "vendorinvoicejournal",
            ],
        ),
        (
            &["fizetes", "payment"],
            &[
                "customerpayment",
                "vendorpayment",
                "payment",
                "amount",
                "entrydate",
            ],
        ),
        (
            &["koltseg", "eredmeny", "profit", "fokonyv"],
            &[
                "financeentry",
                "account",
                "debit",
                "credit",
                "amount",
                "jobbalance",
            ],
        ),
        (
            &["dolgozo", "munkatars", "employee"],
            &["employee", "employeerevision", "employeenumber", "name"],
        ),
        (
            &["raktar", "keszlet", "termek", "cikk"],
            &["warehouse", "inventory", "item", "stock", "quantity"],
        ),
    ];
    for (triggers, expansions) in domain_terms {
        if triggers.iter().any(|trigger| normalized.contains(trigger)) {
            terms.extend(expansions.iter().map(|term| (*term).to_string()));
        }
    }
    terms
}

fn table_business_metadata(table_name: &str) -> (&'static str, Option<&'static str>) {
    let name = table_name.to_ascii_lowercase();
    let description = match name.as_str() {
        "invoice" => Some("kimenő számlák fejléce; egy sor egy számla"),
        "invoiceline" => Some("kimenő számlák tételsorai"),
        "invoicevatspecification" => Some("kimenő számlák áfabontása"),
        "invoiceallocationline" => {
            Some("bejövő szállítói számlák könyvelési felosztása; nem kimenő számla")
        }
        "vendorinvoicejournal" => Some("bejövő szállítói számlanapló; nem kimenő számla"),
        "purchasevoucher" => Some("könyvelt szállítói bizonylatok és bejövő számlák"),
        "customerentry" => Some("vevői folyószámla- és kintlévőségi tételek"),
        "vendorentry" => Some("szállítói folyószámla-tételek"),
        "customerpayment" => Some("vevői befizetések és kiegyenlítések"),
        "vendorpayment" => Some("szállítói kifizetések és kiegyenlítések"),
        "orderheader" => Some("értékesítési rendelések fejléce"),
        "orderline" => Some("értékesítési rendelések tételsorai"),
        "deliverynote" => Some("szállítólevelek fejléce"),
        "deliverynoteline" => Some("szállítólevelek tételsorai"),
        "financeentry" => Some("főkönyvi könyvelési tételek"),
        "customer" | "companycustomer" => Some("vevői törzsadatok"),
        "vendor" | "companyvendor" => Some("szállítói törzsadatok"),
        _ => None,
    };
    let category = if name.starts_with("invoice")
        || name.starts_with("customer")
        || name.starts_with("order")
        || name.starts_with("deliverynote")
    {
        "Értékesítés és kimenő számlázás"
    } else if name.starts_with("vendor")
        || name.starts_with("purchase")
        || name.starts_with("itempurchase")
    {
        "Beszerzés és bejövő számlázás"
    } else if name.contains("finance")
        || name.starts_with("account")
        || name.contains("journal")
        || name.contains("vat")
        || name.contains("fiscal")
    {
        "Pénzügy és főkönyv"
    } else if name.starts_with("job")
        || name.contains("traffic")
        || name.contains("freight")
        || name.starts_with("transport")
    {
        "Fuvarozás és munkák"
    } else if name.starts_with("item")
        || name.contains("inventory")
        || name.contains("warehouse")
        || name.contains("stock")
    {
        "Készlet és termékek"
    } else if name.starts_with("employee")
        || name.starts_with("timesheet")
        || name.contains("absence")
        || name.contains("salary")
    {
        "Munkaügy és időráfordítás"
    } else if name.starts_with("company")
        || name.starts_with("system")
        || name.contains("user")
        || name.contains("role")
        || name.contains("parameter")
    {
        "Törzsadat és rendszerbeállítás"
    } else {
        "Egyéb ERP-adat"
    };
    (category, description)
}

fn select_schema_context(
    catalog: &SchemaCatalog,
    question: &str,
    history: &[HistoryMessage],
) -> (String, SchemaSelectionStats) {
    let terms = schema_search_terms(question, history);
    let mut scored_tables = catalog
        .tables
        .iter()
        .map(|table| {
            let name = table.name.to_lowercase();
            let table_score = terms
                .iter()
                .filter(|term| name.contains(term.as_str()))
                .count()
                * 100;
            let column_score = table
                .columns
                .iter()
                .map(|column| {
                    terms
                        .iter()
                        .filter(|term| column.name.contains(term.as_str()))
                        .count()
                })
                .sum::<usize>()
                * 8;
            (table_score + column_score, table)
        })
        .collect::<Vec<_>>();
    scored_tables.sort_by_key(|(score, table)| (Reverse(*score), table.name.clone()));

    let mut selected = scored_tables
        .iter()
        .filter(|(score, _)| *score > 0)
        .take(MAX_SCHEMA_TABLES_PER_QUESTION)
        .map(|(_, table)| *table)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        let fallbacks = [
            "invoice",
            "invoiceline",
            "customer",
            "customerentry",
            "jobheader",
            "orderheader",
            "financeentry",
        ];
        selected = fallbacks
            .iter()
            .filter_map(|name| catalog.tables.iter().find(|table| table.name == *name))
            .collect();
    }

    let analytical_fragments = [
        "amount",
        "balance",
        "company",
        "credit",
        "currency",
        "customer",
        "date",
        "debit",
        "due",
        "invoice",
        "item",
        "job",
        "name",
        "net",
        "number",
        "open",
        "order",
        "paid",
        "payment",
        "price",
        "quantity",
        "remainder",
        "status",
        "total",
        "vat",
        "vendor",
    ];
    let mut context = String::from(
        "Csak az alábbi, kérdéshez helyben kiválasztott MySQL-séma használható. A dátum mezők formátumát ne feltételezd; szükség esetén szövegként kezeld.\n",
    );
    let mut selected_columns = 0;
    for table in &selected {
        let relationship_columns = table
            .relationships
            .iter()
            .map(|relationship| relationship.column.as_str())
            .collect::<HashSet<_>>();
        let mut columns = table
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let term_score = terms
                    .iter()
                    .filter(|term| column.name.contains(term.as_str()))
                    .count()
                    * 40;
                let key_score = match column.key.as_str() {
                    "PRI" => 80,
                    "UNI" => 50,
                    "MUL" => 12,
                    _ => 0,
                };
                let relationship_score =
                    usize::from(relationship_columns.contains(column.name.as_str())) * 60;
                let analytical_score = analytical_fragments
                    .iter()
                    .filter(|fragment| column.name.contains(**fragment))
                    .count()
                    * 10;
                let leading_score = usize::from(index < 4) * 4;
                (
                    term_score + key_score + relationship_score + analytical_score + leading_score,
                    index,
                    column,
                )
            })
            .collect::<Vec<_>>();
        columns.sort_by_key(|(score, index, _)| (Reverse(*score), *index));
        columns.truncate(MAX_SCHEMA_COLUMNS_PER_TABLE);
        columns.sort_by_key(|(_, index, _)| *index);
        selected_columns += columns.len();

        let (category, description) = table_business_metadata(&table.name);
        context.push_str(&format!("\n[{}] {}", category, table.name));
        if let Some(description) = description {
            context.push_str(&format!(" — {description}"));
        }
        if let Some(rows) = table.estimated_rows {
            context.push_str(&format!(" (~{rows} sor)"));
        }
        context.push_str(": ");
        let formatted_columns = columns
            .into_iter()
            .map(|(_, _, column)| {
                let mut flags = Vec::new();
                if column.key == "PRI" {
                    flags.push("PK".to_string());
                } else if column.key == "UNI" {
                    flags.push("UNIQUE".to_string());
                }
                for relationship in table
                    .relationships
                    .iter()
                    .filter(|relationship| relationship.column == column.name)
                    .take(2)
                {
                    flags.push(format!(
                        "FK->{}.{}",
                        relationship.referenced_table, relationship.referenced_column
                    ));
                }
                let suffix = if flags.is_empty() {
                    String::new()
                } else {
                    format!("[{}]", flags.join(","))
                };
                format!("{}:{}{}", column.name, column.column_type, suffix)
            })
            .collect::<Vec<_>>()
            .join(", ");
        context.push_str(&formatted_columns);
    }

    let stats = SchemaSelectionStats {
        total_tables: catalog.tables.len(),
        total_columns: catalog.tables.iter().map(|table| table.columns.len()).sum(),
        selected_tables: selected.len(),
        selected_columns,
        context_characters: context.len(),
    };
    (context, stats)
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

fn responses_input(user: &str, json_mode: bool) -> String {
    if json_mode {
        format!("A válasz kizárólag JSON objektum legyen.\n\n{user}")
    } else {
        user.to_string()
    }
}

async fn call_ai(
    settings: &StoredSettings,
    api_key: &str,
    system: &str,
    user: &str,
    json_mode: bool,
) -> Result<AiCallResult, String> {
    let uses_responses_api =
        settings.ai_model.starts_with("gpt-5") || settings.ai_model.starts_with("gpt-6");
    let endpoint = if uses_responses_api {
        format!("{}/responses", settings.ai_base_url.trim_end_matches('/'))
    } else {
        format!(
            "{}/chat/completions",
            settings.ai_base_url.trim_end_matches('/')
        )
    };
    let max_completion_tokens = if json_mode { 1_600 } else { 1_200 };
    let responses_input = responses_input(user, json_mode);
    let mut request = if uses_responses_api {
        json!({
            "model": settings.ai_model,
            "instructions": system,
            "input": responses_input,
            "max_output_tokens": max_completion_tokens,
            "store": false
        })
    } else {
        json!({
            "model": settings.ai_model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ],
            "max_completion_tokens": max_completion_tokens
        })
    };
    if json_mode && uses_responses_api {
        request["text"] = json!({ "format": { "type": "json_object" } });
    } else if json_mode {
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
        if let Ok(error_json) = serde_json::from_str::<JsonValue>(&body) {
            if error_json["error"]["code"].as_str() == Some("credit_balance_exhausted") {
                return Err(
                    "Az OpenAI API-egyenlege elfogyott. Tölts fel kreditet az OpenAI Platform Billing oldalon, majd próbáld újra."
                        .into(),
                );
            }
        }
        let concise = body.chars().take(500).collect::<String>();
        return Err(format!(
            "Az AI-szolgáltatás hibát jelzett ({status}): {concise}"
        ));
    }

    let response_json: JsonValue = serde_json::from_str(&body)
        .map_err(|error| format!("Az AI-válasz formátuma érvénytelen: {error}"))?;
    let (content, usage) = if uses_responses_api {
        let content = response_json["output"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .find_map(|content| {
                (content["type"].as_str() == Some("output_text"))
                    .then(|| content["text"].as_str().map(str::to_owned))
                    .flatten()
            })
            .ok_or_else(|| "Az AI-válasz nem tartalmazott szöveget.".to_string())?;
        let usage = TokenUsage {
            input_tokens: response_json["usage"]["input_tokens"]
                .as_u64()
                .unwrap_or_default(),
            output_tokens: response_json["usage"]["output_tokens"]
                .as_u64()
                .unwrap_or_default(),
            cached_input_tokens: response_json["usage"]["input_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or_default(),
            total_tokens: response_json["usage"]["total_tokens"]
                .as_u64()
                .unwrap_or_default(),
        };
        (content, usage)
    } else {
        let content = response_json["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "Az AI-válasz nem tartalmazott szöveget.".to_string())?;
        let usage = TokenUsage {
            input_tokens: response_json["usage"]["prompt_tokens"]
                .as_u64()
                .unwrap_or_default(),
            output_tokens: response_json["usage"]["completion_tokens"]
                .as_u64()
                .unwrap_or_default(),
            cached_input_tokens: response_json["usage"]["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or_default(),
            total_tokens: response_json["usage"]["total_tokens"]
                .as_u64()
                .unwrap_or_default(),
        };
        (content, usage)
    };
    Ok(AiCallResult { content, usage })
}

async fn summarize_query_result(
    settings: &StoredSettings,
    api_key: &str,
    question: &str,
    rows: &[JsonValue],
    truncated: bool,
    analysis_fy_window: u8,
) -> Result<AiCallResult, String> {
    let preview_rows = rows.iter().take(SUMMARY_PREVIEW_ROWS).collect::<Vec<_>>();
    let full_preview = serde_json::to_string(&preview_rows)
        .map_err(|error| format!("Az eredmény nem alakítható át: {error}"))?;
    let result_preview = full_preview
        .chars()
        .take(SUMMARY_PREVIEW_CHARACTERS)
        .collect::<String>();
    let summary_system = r#"Te az ERGO, a Trans-Europe üzleti adatelemzője vagy.
Magyarul válaszolj. Kezdd a legfontosabb üzleti következtetéssel, majd támaszd alá a kapott számokkal.
Ne találj ki adatot, pénznemet, mértékegységet vagy üzleti definíciót. Ha az eredmény üres vagy kétértelmű, ezt mondd ki.
Minden pénzösszeget magyar formátumban, hármas számjegycsoportokkal és legfeljebb két tizedessel írj, például: 24 752 384,12.
Legyél tömör és gyakorlatias, legfeljebb 180 szóban. Jelezd, ha a látható eredmény korlátozott. Markdown használható."#;
    let summary_user = format!(
        "Eredeti kérdés: {question}\nBeállított elemzési időablak: {analysis_fy_window} FY.\n\nLekérdezési eredmény első {} sora (JSON, legfeljebb {} karakter):\n{}\n\nTeljes visszaadott sorszám: {}\nAdatbázis-lekérdezés csonkolt: {}",
        SUMMARY_PREVIEW_ROWS,
        SUMMARY_PREVIEW_CHARACTERS,
        result_preview,
        rows.len(),
        truncated
    );
    call_ai(settings, api_key, summary_system, &summary_user, false).await
}

#[tauri::command]
async fn analyze_quick_erp(
    app: AppHandle,
    quick_analysis: String,
    question: String,
) -> Result<AnalyzeResponse, String> {
    if question.trim().is_empty() {
        return Err("A kérdés nem lehet üres.".into());
    }
    let settings = read_settings(&app)?;
    let analysis_fy_window = validate_analysis_fy_window(settings.analysis_fy_window)?;
    let plan = fixed_quick_plan(&quick_analysis, analysis_fy_window)?;
    let sql = assert_read_only_sql(&plan.sql)?;
    let mysql_password = read_secret(
        &app,
        "MYSQL_PASSWORD",
        MYSQL_PASSWORD_ACCOUNT,
        "MySQL-jelszó",
    )?;
    let api_key = read_secret(&app, "AI_API_KEY", AI_API_KEY_ACCOUNT, "AI API-kulcs")?;
    let (columns, rows, truncated) =
        run_query(&settings, mysql_password, &sql, plan.max_rows).await?;
    let summary_call = summarize_query_result(
        &settings,
        &api_key,
        question.trim(),
        &rows,
        truncated,
        analysis_fy_window,
    )
    .await?;
    let row_count = rows.len();

    Ok(AnalyzeResponse {
        summary: summary_call.content,
        result: Some(QueryResult {
            kind: "query-result",
            title: plan.title,
            visualization: plan.visualization,
            columns,
            rows,
            row_count,
            truncated,
            sql,
            query_source: "fixed",
            token_usage: summary_call.usage,
            schema_selection: SchemaSelectionStats {
                total_tables: 0,
                total_columns: 0,
                selected_tables: 0,
                selected_columns: 0,
                context_characters: 0,
            },
        }),
    })
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
    if let Some(summary) = capability_answer(question.trim()) {
        return Ok(AnalyzeResponse {
            summary: summary.into(),
            result: None,
        });
    }
    let settings = read_settings(&app)?;
    let mysql_password = read_secret(
        &app,
        "MYSQL_PASSWORD",
        MYSQL_PASSWORD_ACCOUNT,
        "MySQL-jelszó",
    )?;
    let api_key = read_secret(&app, "AI_API_KEY", AI_API_KEY_ACCOUNT, "AI API-kulcs")?;
    let catalog = load_schema_catalog(&app, &settings, mysql_password.clone()).await?;
    let (schema_context, schema_selection) =
        select_schema_context(&catalog, question.trim(), &history);
    let mut relevant_history = history
        .iter()
        .filter(|message| message.role == "user" || message.role == "assistant")
        .rev()
        .take(4)
        .collect::<Vec<_>>();
    relevant_history.reverse();
    let history_text = relevant_history
        .into_iter()
        .map(|message| {
            let compact_content = message.content.chars().take(3_500).collect::<String>();
            format!("{}: {compact_content}", message.role)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let analysis_fy_window = validate_analysis_fy_window(settings.analysis_fy_window)?;
    let fiscal_year_start = format!(
        "MAKEDATE(YEAR(CURRENT_DATE) - {}, 1)",
        analysis_fy_window - 1
    );
    let planner_system = format!(
        r#"Te az ERGO, a Trans-Europe óvatos ERP-adatelemzője vagy.
Készíts pontos MySQL lekérdezési tervet a megadott adatbázis-séma alapján.
Kizárólag egy SELECT vagy WITH lekérdezést adhatsz. Tilos minden adatmódosítás, DDL, zárolás, fájlművelet, komment, rendszer-séma és több utasítás.
Az sql mező pontosan egyetlen SELECT vagy WITH utasítást tartalmazzon, záró pontosvessző nélkül. Ne használj SET, DECLARE, ideiglenes táblát vagy tárolt eljárást.
Ne találj ki táblát vagy oszlopot. Használj explicit oszlopokat és aggregálj SQL-ben. A sorok száma legfeljebb 200 legyen.
Az üzleti kategóriát és a táblaleírást tekintsd mérvadónak; az azonos szavakat tartalmazó, de más üzleti célú táblákat ne keverd össze.
Időfüggő üzleti adatoknál kötelező közvetlenül az SQL WHERE feltételében időszakot szűrni:
- ha a kérdés napot vagy időszakot ad meg (például ma, tegnap, adott hónap), pontosan arra, és ne olvass be azon kívüli rekordot;
- a beállított elemzési időablak {analysis_fy_window} FY; ennek kezdete {fiscal_year_start};
- külön időszak hiányában ettől a kezdettől szűrj;
- a beállított {analysis_fy_window} FY időablaknál régebbi tranzakciót akkor se használj, ha a felhasználó tágabb időszakot kér;
- dátumként tárolt varchar mezőnél vedd figyelembe a sémában jelzett típust és a ponttal tagolt YYYY.MM.DD formátumot;
- fejlécszámhoz ne kapcsolj tételtáblát, ha a kérdés nem kér tételszintű adatot.
Kizárólag JSON objektummal válaszolj ebben az alakban:
{{"sql":"...","title":"rövid magyar cím","visualization":"table|bar|line","maxRows":50}}

{schema_context}"#
    );
    let planner_user = format!(
        "Korábbi beszélgetés:\n{history_text}\n\nFelhasználói kérdés:\n{}",
        question.trim()
    );
    let planner_call = call_ai(&settings, &api_key, &planner_system, &planner_user, true).await?;
    let mut planner_usage = planner_call.usage;
    let mut plan: QueryPlan = extract_json(&planner_call.content)?;
    plan.sql = match assert_read_only_sql(&plan.sql) {
        Ok(sql) => sql,
        Err(first_error) => {
            let repair_user = format!(
                r#"Az előző lekérdezési terv biztonsági ellenőrzése sikertelen volt.
Hiba: {first_error}

Hibás SQL:
{}

Eredeti felhasználói kérdés:
{}

Javítsd ki a tervet. A JSON sql mezője pontosan egyetlen, záró pontosvessző nélküli SELECT vagy WITH utasítás legyen. Ne használj SET, DECLARE, ideiglenes táblát, tárolt eljárást, kommentet vagy több utasítást. Kizárólag a kért JSON objektummal válaszolj."#,
                plan.sql,
                question.trim()
            );
            let repaired_call =
                call_ai(&settings, &api_key, &planner_system, &repair_user, true).await?;
            planner_usage.add(&repaired_call.usage);
            let repaired_plan: QueryPlan = extract_json(&repaired_call.content)?;
            plan = repaired_plan;
            assert_read_only_sql(&plan.sql).map_err(|second_error| {
                format!(
                    "Az AI két próbálkozás után sem adott biztonságos, egyetlen SELECT lekérdezést: {second_error}"
                )
            })?
        }
    };
    plan.max_rows = plan.max_rows.clamp(1, MAX_RESULT_ROWS);
    if !matches!(plan.visualization.as_str(), "table" | "bar" | "line") {
        plan.visualization = "table".into();
    }

    let (columns, rows, truncated) =
        run_query(&settings, mysql_password, &plan.sql, plan.max_rows).await?;
    let summary_call = summarize_query_result(
        &settings,
        &api_key,
        question.trim(),
        &rows,
        truncated,
        analysis_fy_window,
    )
    .await?;
    let mut token_usage = planner_usage;
    token_usage.add(&summary_call.usage);
    let row_count = rows.len();

    Ok(AnalyzeResponse {
        summary: summary_call.content,
        result: Some(QueryResult {
            kind: "query-result",
            title: plan.title,
            visualization: plan.visualization,
            columns,
            rows,
            row_count,
            truncated,
            sql: plan.sql,
            query_source: "generated",
            token_usage,
            schema_selection,
        }),
    })
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            analyze_erp,
            analyze_quick_erp,
            check_database,
            list_ai_models,
            load_settings,
            open_feedback_email,
            save_settings,
            select_ai_model,
            select_analysis_fy_window
        ])
        .run(tauri::generate_context!())
        .expect("az ERGO alkalmazás nem indítható");
}

#[cfg(test)]
mod tests {
    use super::{
        assert_read_only_sql, capability_answer, extract_json, fixed_quick_plan, is_chat_model,
        percent_encode_mailto, responses_input, run_query, schema_search_terms,
        select_schema_context, table_business_metadata, validate_analysis_fy_window,
        validate_model, CatalogColumn, QueryPlan, SchemaCatalog, SchemaTable, StoredSettings,
    };

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
    fn mentions_json_in_structured_responses_input() {
        let input = responses_input("Készíts lekérdezési tervet.", true);
        assert!(input.to_lowercase().contains("json"));
        assert!(input.contains("Készíts lekérdezési tervet."));
        assert_eq!(responses_input("Egyszerű kérdés", false), "Egyszerű kérdés");
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

    #[test]
    fn validates_supported_fiscal_year_windows() {
        assert_eq!(validate_analysis_fy_window(1).unwrap(), 1);
        assert_eq!(validate_analysis_fy_window(3).unwrap(), 3);
        assert_eq!(validate_analysis_fy_window(5).unwrap(), 5);
        assert!(validate_analysis_fy_window(2).is_err());
    }

    #[test]
    fn fixed_quick_queries_are_single_read_only_statements() {
        for quick_analysis in ["revenue-trend", "top-customers", "overdue-receivables"] {
            let plan = fixed_quick_plan(quick_analysis, 3).unwrap();
            assert_read_only_sql(&plan.sql).unwrap();
        }
        assert!(fixed_quick_plan("unknown", 3).is_err());
    }

    #[test]
    #[ignore = "VPN-t és helyi .env fájlt igényel"]
    fn live_fixed_quick_queries_return_rows() {
        let env_path = std::env::var("ERGO_ENV_PATH").expect("ERGO_ENV_PATH is required");
        let contents = std::fs::read_to_string(env_path).expect("the .env file must be readable");
        let password = contents
            .lines()
            .find_map(|line| line.trim().strip_prefix("MYSQL_PASSWORD="))
            .map(|value| value.trim().trim_matches(['\'', '"']).to_string())
            .filter(|value| !value.is_empty())
            .expect("MYSQL_PASSWORD must be present");

        tauri::async_runtime::block_on(async {
            let settings = StoredSettings::default();
            for quick_analysis in ["revenue-trend", "top-customers", "overdue-receivables"] {
                let plan = fixed_quick_plan(quick_analysis, 1).unwrap();
                let (_, rows, _) = run_query(&settings, password.clone(), &plan.sql, plan.max_rows)
                    .await
                    .unwrap_or_else(|error| panic!("{quick_analysis} failed: {error}"));
                assert!(!rows.is_empty(), "{quick_analysis} returned no rows");
                if quick_analysis == "overdue-receivables" {
                    let first = rows[0].as_object().expect("row must be an object");
                    assert!(first.contains_key("szamlaszam"));
                    assert!(first.contains_key("cikksorok_szama"));
                    assert!(first.contains_key("cikkek"));
                }
            }
        });
    }

    #[test]
    fn encodes_feedback_for_a_mailto_url() {
        assert_eq!(
            percent_encode_mailto("Hiba és kérdés"),
            "Hiba%20%C3%A9s%20k%C3%A9rd%C3%A9s"
        );
    }

    #[test]
    fn expands_hungarian_business_terms() {
        let terms = schema_search_terms("Mutasd a lejárt kintlévőségeket", &[]);
        assert!(terms.contains("customerentry"));
        assert!(terms.contains("duedate"));
        assert!(terms.contains("remainder"));
    }

    #[test]
    fn answers_capability_questions_without_a_query() {
        let answer = capability_answer("Milyen adatokból tudsz dolgozni?").unwrap();
        assert!(answer.contains("Trans-Europe Zrt."));
        assert!(answer.contains("1, 3 vagy 5 üzletiéves"));
        assert!(capability_answer("Mennyi a mai árbevétel?").is_none());
    }

    #[test]
    fn distinguishes_outgoing_and_incoming_invoice_tables() {
        assert_eq!(
            table_business_metadata("invoice").1,
            Some("kimenő számlák fejléce; egy sor egy számla")
        );
        assert!(table_business_metadata("vendorinvoicejournal")
            .1
            .unwrap()
            .contains("bejövő"));
    }

    #[test]
    fn selects_only_relevant_schema_context() {
        let catalog = SchemaCatalog {
            database: "test".into(),
            source: "localhost/test".into(),
            generated_at: 0,
            tables: vec![
                SchemaTable {
                    name: "customerentry".into(),
                    table_type: "BASE TABLE".into(),
                    estimated_rows: Some(100),
                    columns: vec![
                        CatalogColumn {
                            name: "customernumber".into(),
                            column_type: "varchar(255)".into(),
                            nullable: false,
                            key: "MUL".into(),
                        },
                        CatalogColumn {
                            name: "duedate".into(),
                            column_type: "varchar(12)".into(),
                            nullable: false,
                            key: String::new(),
                        },
                        CatalogColumn {
                            name: "remainderbase".into(),
                            column_type: "decimal(20,2)".into(),
                            nullable: false,
                            key: String::new(),
                        },
                    ],
                    relationships: vec![],
                },
                SchemaTable {
                    name: "employee".into(),
                    table_type: "BASE TABLE".into(),
                    estimated_rows: Some(10),
                    columns: vec![CatalogColumn {
                        name: "employeenumber".into(),
                        column_type: "varchar(255)".into(),
                        nullable: false,
                        key: "PRI".into(),
                    }],
                    relationships: vec![],
                },
            ],
        };
        let (context, stats) = select_schema_context(&catalog, "Mennyi a lejárt kintlévőség?", &[]);
        assert!(context.contains("customerentry"));
        assert!(!context.contains("employee"));
        assert_eq!(stats.selected_tables, 1);
        assert_eq!(stats.total_columns, 4);
    }
}
