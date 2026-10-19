use std::{
    cmp::Reverse,
    collections::{hash_map::DefaultHasher, BTreeMap, HashMap, HashSet},
    fs,
    hash::{Hash, Hasher},
    path::PathBuf,
    process::Command,
    sync::{Mutex, OnceLock},
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

mod product_catalog;

const KEYCHAIN_SERVICE: &str = "hu.transeurope.ergo";
const MYSQL_PASSWORD_ACCOUNT: &str = "mysql-password";
const AI_API_KEY_ACCOUNT: &str = "ai-api-key";
const MAX_RESULT_ROWS: usize = 200;
const MAX_SCHEMA_TABLES_PER_QUESTION: usize = 8;
const MAX_SCHEMA_COLUMNS_PER_TABLE: usize = 36;
/// Ezek a táblák minden dinamikus kérdésnél bekerülnek a sémarészletbe.
const SCHEMA_CORE_TABLES: &[&str] = &[
    "invoice",
    "invoiceline",
    "customer",
    "endcustomer",
    "customerentry",
];
const SCHEMA_CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const HUNGARIAN_STOP_WORDS: [&str; 19] = [
    "adat", "adatok", "alapjan", "az", "egy", "es", "hogy", "kerlek", "legyen", "meg", "mely",
    "melyik", "mi", "mind", "mutasd", "osszes", "szerint", "van", "volt",
];
/// A tanult memória retrieval korlátai.
const MAX_MEMORY_EXAMPLES: usize = 4;
const MAX_MEMORY_PITFALLS: usize = 4;
const MEMORY_MIN_SIMILARITY: f64 = 0.18;
const MEMORY_EXAMPLE_SQL_CHARACTERS: usize = 1_400;
const MEMORY_CORRECTION_CHARACTERS: usize = 600;
/// A magyar ragozás miatt a tokeneket előtagra vágjuk: az "arbevetel" és az
/// "arbevetelt" így ugyanarra a kulcsra esik.
const MEMORY_TERM_PREFIX: usize = 6;
const MAX_SAMPLED_COLUMNS: usize = 80;
const MAX_SAMPLES_PER_COLUMN: usize = 10;
const MAX_SAMPLE_CHARACTERS: usize = 40;
const SAMPLE_SCAN_ROWS: usize = 5_000;
const SAMPLE_QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const TOTAL_SAMPLING_BUDGET: Duration = Duration::from_secs(30);
const SUMMARY_PREVIEW_ROWS: usize = 40;
const SUMMARY_PREVIEW_CHARACTERS: usize = 18_000;
const PLANNER_MAX_OUTPUT_TOKENS: u32 = 8_000;
const SUMMARY_MAX_OUTPUT_TOKENS: u32 = 4_000;
const MAX_OUTPUT_TOKEN_RETRY_FACTOR: u32 = 2;
/// A reasoning modellek lassabbak, mint a korábbi 55 másodperces korlát.
const AI_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
/// Az első terv plusz két javító kör a validációs és a MySQL-hibákra.
const MAX_PLAN_ATTEMPTS: usize = 3;
/// Az agentikus tervezés körkorlátja és eszközköltségei.
const MAX_PLANNER_TOOL_ROUNDS: usize = 8;
const MAX_PROBE_QUERIES: usize = 5;
const PROBE_QUERY_ROWS: usize = 20;
const PROBE_RESULT_CHARACTERS: usize = 4_000;
const SEARCH_SCHEMA_MAX_TABLES: usize = 10;
const SEARCH_SCHEMA_MAX_COLUMNS: usize = 20;
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

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnowledgePack {
    version: String,
    metrics: Vec<KnowledgeMetric>,
    relationships: Vec<KnowledgeRelationship>,
    entities: Vec<KnowledgeEntity>,
    normalization_aliases: Vec<NormalizationAlias>,
    skills: Vec<KnowledgeSkill>,
}

#[derive(Clone, Debug, Deserialize)]
struct KnowledgeMetric {
    id: String,
    label: String,
    definition: String,
    rules: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct KnowledgeRelationship {
    id: String,
    label: String,
    path: String,
    join: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KnowledgeEntity {
    id: String,
    label: String,
    query_aliases: Vec<String>,
    exact_codes: Vec<String>,
    excluded_codes: Vec<String>,
    #[serde(default)]
    include_token_family: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct NormalizationAlias {
    from: String,
    to: String,
}

#[derive(Clone, Debug, Deserialize)]
struct KnowledgeSkill {
    id: String,
    label: String,
    execution: String,
    examples: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct KnowledgeOverview {
    version: String,
    metric_count: usize,
    relationship_count: usize,
    entity_rule_count: usize,
    skill_count: usize,
    feedback_total: usize,
    feedback_pending: usize,
    recent_feedback: Vec<FeedbackSummary>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackSummary {
    id: String,
    created_at: u64,
    category: Option<String>,
    question: String,
    correction: String,
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalysisFeedbackInput {
    rating: String,
    category: Option<String>,
    correction: String,
    question: String,
    answer: String,
    result_title: Option<String>,
    sql: Option<String>,
    query_source: Option<String>,
    row_count: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalysisFeedbackRecord {
    id: String,
    created_at: u64,
    app_version: String,
    model: String,
    analysis_fy_window: u8,
    rating: String,
    category: Option<String>,
    correction: String,
    question: String,
    answer: String,
    result_title: Option<String>,
    sql: Option<String>,
    query_source: Option<String>,
    row_count: Option<usize>,
    status: String,
}

static KNOWLEDGE_PACK: OnceLock<KnowledgePack> = OnceLock::new();
static FEEDBACK_FILE_LOCK: Mutex<()> = Mutex::new(());
static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();

/// Egyetlen, újrahasznosított kliens: a kérdésenkénti új TLS-kézfogás
/// és a hiányzó kapcsolatpool több száz milliszekundumot vitt el.
fn http_client() -> &'static Client {
    HTTP_CLIENT.get_or_init(|| {
        Client::builder()
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .unwrap_or_default()
    })
}

fn knowledge_pack() -> &'static KnowledgePack {
    KNOWLEDGE_PACK.get_or_init(|| {
        serde_json::from_str(include_str!("../knowledge/knowledge-pack.json"))
            .expect("the embedded ERGO knowledge pack must be valid JSON")
    })
}

fn knowledge_prompt_context() -> String {
    let pack = knowledge_pack();
    let mut context = format!("ERGO Knowledge Pack {}\n", pack.version);
    for metric in &pack.metrics {
        context.push_str(&format!(
            "- Mérőszám [{}] {}: {} Szabályok: {}.\n",
            metric.id,
            metric.label,
            metric.definition,
            metric.rules.join("; ")
        ));
    }
    for relationship in &pack.relationships {
        context.push_str(&format!(
            "- Reláció [{}] {}: {}; kapcsolás: {}.\n",
            relationship.id, relationship.label, relationship.path, relationship.join
        ));
    }
    for entity in &pack.entities {
        context.push_str(&format!(
            "- Entitás [{}] {}: kérdésaliasok {}; pontos kódok {}; kizárt kódok {}.\n",
            entity.id,
            entity.label,
            entity.query_aliases.join(" | "),
            entity.exact_codes.join(" | "),
            entity.excluded_codes.join(" | ")
        ));
    }
    for skill in &pack.skills {
        context.push_str(&format!(
            "- Skill [{}] {}: végrehajtás {}; példák: {}.\n",
            skill.id,
            skill.label,
            skill.execution,
            skill.examples.join(" | ")
        ));
    }
    context
}

fn fixed_quick_plan(quick_analysis: &str, analysis_fy_window: u8) -> Result<QueryPlan, String> {
    let fiscal_year_start = format!(
        "MAKEDATE(YEAR(CURRENT_DATE) - {}, 1)",
        analysis_fy_window - 1
    );
    match quick_analysis {
        "revenue-trend" => Ok(QueryPlan {
            sql: format!(
                "SELECT DATE_FORMAT(STR_TO_DATE(i.invoicedate, '%Y.%m.%d'), '%Y-%m') AS honap, ROUND(SUM(CASE WHEN i.debitcredit = 0 THEN i.itemsumbase ELSE -i.itemsumbase END), 2) AS netto_szamlazott_arbevetel FROM invoice AS i WHERE i.companynumber = '1' AND i.posted = 1 AND STR_TO_DATE(i.invoicedate, '%Y.%m.%d') >= GREATEST(DATE_SUB(CURRENT_DATE, INTERVAL 12 MONTH), {fiscal_year_start}) AND STR_TO_DATE(i.invoicedate, '%Y.%m.%d') <= CURRENT_DATE GROUP BY DATE_FORMAT(STR_TO_DATE(i.invoicedate, '%Y.%m.%d'), '%Y-%m') ORDER BY honap ASC"
            ),
            title: "Havi nettó számlázott árbevétel".into(),
            visualization: "line".into(),
            max_rows: 50,
        }),
        "top-customers" => Ok(QueryPlan {
            sql: "SELECT i.customernumber AS ugyfelszam, COALESCE(NULLIF(TRIM(CONCAT_WS(' ', c.name1, c.name2, c.name3, c.name4, c.name5)), ''), i.customernumber) AS ugyfel, ROUND(SUM(CASE WHEN i.debitcredit = 0 THEN i.itemsumbase ELSE -i.itemsumbase END), 2) AS netto_szamlazott_arbevetel FROM invoice AS i LEFT JOIN customer AS c ON c.customernumber = i.customernumber WHERE i.companynumber = '1' AND i.posted = 1 AND STR_TO_DATE(i.invoicedate, '%Y.%m.%d') >= MAKEDATE(YEAR(CURRENT_DATE), 1) AND STR_TO_DATE(i.invoicedate, '%Y.%m.%d') <= CURRENT_DATE GROUP BY i.customernumber, c.name1, c.name2, c.name3, c.name4, c.name5 ORDER BY netto_szamlazott_arbevetel DESC LIMIT 10".into(),
            title: "Top 10 ügyfél idei nettó számlázott árbevétele".into(),
            visualization: "bar".into(),
            max_rows: 10,
        }),
        "overdue-receivables" => Ok(QueryPlan {
            sql: format!(
                "SELECT ce.customernumber AS ugyfelszam, COALESCE(NULLIF(TRIM(CONCAT_WS(' ', c.name1, c.name2, c.name3, c.name4, c.name5)), ''), ce.customernumber) AS ugyfel, ce.transactionnumber AS szamlaszam, DATE_FORMAT(STR_TO_DATE(ce.invoicedate, '%Y.%m.%d'), '%Y-%m-%d') AS szamla_datum, DATE_FORMAT(STR_TO_DATE(ce.duedate, '%Y.%m.%d'), '%Y-%m-%d') AS esedekesseg, DATEDIFF(CURRENT_DATE, STR_TO_DATE(ce.duedate, '%Y.%m.%d')) AS kesedelmes_napok, ROUND(COALESCE(i.totalstandard, ce.debitstandard - ce.creditstandard), 2) AS szamla_osszeg, ROUND(ce.remainderstandard, 2) AS kintlevoseg, COALESCE(i.standardcurrency, ce.originalcurrency) AS penznemkod, COUNT(il.linenumber) AS cikksorok_szama, GROUP_CONCAT(CASE WHEN il.linenumber IS NULL THEN 'Nincs kapcsolt számlasor' ELSE CONCAT_WS(' · ', COALESCE(NULLIF(il.itemnumber, ''), 'cikkszám nélkül'), COALESCE(NULLIF(il.itemtext1, ''), NULLIF(il.externalitemtext, ''), 'megnevezés nélkül'), CONCAT('menny.: ', COALESCE(il.numberinvoiced, 0)), CONCAT('nettó: ', COALESCE(il.linepricebase, 0))) END ORDER BY il.linenumber SEPARATOR ' | ') AS cikkek FROM customerentry AS ce LEFT JOIN invoice AS i ON i.invoicenumber = ce.transactionnumber AND i.customernumber = ce.customernumber AND i.companynumber = ce.companynumber LEFT JOIN invoiceline AS il ON il.invoicenumber = i.invoicenumber AND il.companynumber = i.companynumber LEFT JOIN customer AS c ON c.customernumber = ce.customernumber WHERE ce.companynumber = '1' AND STR_TO_DATE(ce.duedate, '%Y.%m.%d') < CURRENT_DATE AND STR_TO_DATE(ce.postingdate, '%Y.%m.%d') >= {fiscal_year_start} AND STR_TO_DATE(ce.postingdate, '%Y.%m.%d') <= CURRENT_DATE AND ce.remainderstandard > 0 GROUP BY ce.customernumber, c.name1, c.name2, c.name3, c.name4, c.name5, ce.transactionnumber, ce.invoicedate, ce.duedate, i.totalstandard, ce.debitstandard, ce.creditstandard, ce.remainderstandard, i.standardcurrency, ce.originalcurrency ORDER BY kintlevoseg DESC LIMIT 50"
            ),
            title: "Lejárt kintlévőségek számlánként és cikksoronként".into(),
            visualization: "table".into(),
            max_rows: 50,
        }),
        _ => Err("Ismeretlen gyors elemzés.".into()),
    }
}

/// A bejövő számlák könyvelt felosztása csak költségjelölt: a teljes
/// működési költséghez bér, értékcsökkenés és további főkönyvi tételek is
/// szükségesek. Ezt a gyakori kérdést nem bízzuk nyolckörös AI-sémafeltárásra.
fn incoming_operating_cost_plan(question: &str, analysis_fy_window: u8) -> Option<QueryPlan> {
    let normalized = normalize_for_search(question);
    let incoming_invoice = (normalized.contains("beerkezo")
        || normalized.contains("bejovo")
        || normalized.contains("szallitoi")
        || normalized.contains("incoming")
        || normalized.contains("supplier"))
        && (normalized.contains("szamla") || normalized.contains("invoice"));
    let operating_cost = (normalized.contains("mukodesi") || normalized.contains("operating"))
        && (normalized.contains("koltseg")
            || normalized.contains("cost")
            || normalized.contains("expense"));
    // Kifejezetten megadott évhez más dátumszűrés kell; azt nem helyettesítjük
    // csendben a beállított FY-ablakkal.
    let explicit_year = Regex::new(r"\b20\d{2}\b")
        .expect("valid explicit year regex")
        .is_match(&normalized);
    if !incoming_invoice || !operating_cost || explicit_year {
        return None;
    }
    let fiscal_year_start = format!(
        "MAKEDATE(YEAR(CURRENT_DATE) - {}, 1)",
        analysis_fy_window.saturating_sub(1)
    );
    Some(QueryPlan {
        sql: format!(
            "SELECT LEFT(l.invoicedate, 7) AS honap, COUNT(*) AS konyvelt_felosztasi_sorok, ROUND(SUM(COALESCE(l.amountbase, 0)), 2) AS netto_bejovo_szamla_felosztas_alapdevizaban, ROUND(SUM(CASE WHEN l.accountnumber LIKE '5%' THEN COALESCE(l.amountbase, 0) ELSE 0 END), 2) AS otos_koltsegnemre_konyvelt_resz, ROUND(SUM(CASE WHEN l.accountnumber LIKE '8%' THEN COALESCE(l.amountbase, 0) ELSE 0 END), 2) AS nyolcas_raforditasra_konyvelt_resz, ROUND(SUM(CASE WHEN l.accountnumber IS NULL OR (l.accountnumber NOT LIKE '5%' AND l.accountnumber NOT LIKE '8%') THEN COALESCE(l.amountbase, 0) ELSE 0 END), 2) AS egyeb_szamlakra_konyvelt_resz FROM invoiceallocationline l WHERE l.companynumber = '1' AND l.posted = 1 AND l.invoicedate >= DATE_FORMAT({fiscal_year_start}, '%Y.%m.%d') AND l.invoicedate <= DATE_FORMAT(CURRENT_DATE, '%Y.%m.%d') GROUP BY LEFT(l.invoicedate, 7) ORDER BY honap ASC LIMIT 60"
        ),
        title: format!("Bejövő számlák könyvelt nettó felosztása — {analysis_fy_window} FY"),
        visualization: "table".into(),
        max_rows: 60,
    })
}

fn requested_top_end_customer_limit(question: &str) -> Option<usize> {
    let normalized = normalize_for_search(question);
    if !normalized.contains("vegfelhasznalo") {
        return None;
    }
    let captures = Regex::new(r"\btop\s+(\d{1,2})\b")
        .expect("valid top end-customer regex")
        .captures(&normalized)?;
    captures
        .get(1)?
        .as_str()
        .parse::<usize>()
        .ok()
        .filter(|limit| (1..=20).contains(limit))
}

fn top_end_customers_plan(question: &str, analysis_fy_window: u8) -> Option<QueryPlan> {
    let limit = requested_top_end_customer_limit(question)?;
    let fiscal_year_start = format!(
        "MAKEDATE(YEAR(CURRENT_DATE) - {}, 1)",
        analysis_fy_window - 1
    );
    Some(QueryPlan {
        sql: format!(
            "SELECT sales.vegfelhasznalo_kod, COALESCE(NULLIF(TRIM(CONCAT_WS(' ', ec.name1, ec.name2, ec.name3, ec.name4, ec.name5)), ''), sales.vegfelhasznalo_kod) AS vegfelhasznalo, ROUND(SUM(CASE WHEN sales.debitcredit = 0 THEN sales.linepricebase ELSE -sales.linepricebase END), 2) AS netto_szamlazott_arbevetel_alapdevizaban, COUNT(DISTINCT sales.invoicenumber) AS szamlak_szama FROM (SELECT i.invoicenumber, i.debitcredit, COALESCE(il.linepricebase, 0) AS linepricebase, COALESCE(NULLIF(i.endcustomernumber, ''), NULLIF(il.endcustomernumber, '')) AS vegfelhasznalo_kod FROM invoice i JOIN invoiceline il ON il.companynumber = i.companynumber AND il.invoicenumber = i.invoicenumber WHERE i.companynumber = '1' AND i.posted = 1 AND STR_TO_DATE(i.invoicedate, '%Y.%m.%d') >= {fiscal_year_start} AND STR_TO_DATE(i.invoicedate, '%Y.%m.%d') <= CURRENT_DATE AND COALESCE(NULLIF(i.endcustomernumber, ''), NULLIF(il.endcustomernumber, '')) IS NOT NULL) sales LEFT JOIN endcustomer ec ON ec.endcustomernumber = sales.vegfelhasznalo_kod GROUP BY sales.vegfelhasznalo_kod, ec.name1, ec.name2, ec.name3, ec.name4, ec.name5 HAVING SUM(CASE WHEN sales.debitcredit = 0 THEN sales.linepricebase ELSE -sales.linepricebase END) > 0.004 ORDER BY netto_szamlazott_arbevetel_alapdevizaban DESC LIMIT {limit}"
        ),
        title: format!(
            "Top {limit} végfelhasználó az elmúlt {analysis_fy_window} FY nettó számlázott árbevétele alapján"
        ),
        visualization: "bar".into(),
        max_rows: limit,
    })
}

fn end_customer_purchase_plan(
    label: &str,
    end_customer_codes: &[String],
    analysis_fy_window: u8,
) -> QueryPlan {
    let quoted_codes = end_customer_codes
        .iter()
        .map(|code| format!("'{}'", code.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    let fiscal_year_start = format!(
        "MAKEDATE(YEAR(CURRENT_DATE) - {}, 1)",
        analysis_fy_window - 1
    );
    QueryPlan {
        sql: format!(
            "SELECT i.customernumber AS szamlazott_partner_kod, COALESCE(NULLIF(TRIM(CONCAT_WS(' ', c.name1, c.name2, c.name3, c.name4, c.name5)), ''), i.customernumber) AS szamlazott_partner, COALESCE(NULLIF(i.endcustomernumber, ''), il.endcustomernumber) AS vegfelhasznalo_kod, COALESCE(NULLIF(TRIM(CONCAT_WS(' ', ec.name1, ec.name2, ec.name3, ec.name4, ec.name5)), ''), COALESCE(NULLIF(i.endcustomernumber, ''), il.endcustomernumber)) AS vegfelhasznalo, il.itemnumber AS cikkszam, COALESCE(NULLIF(il.itemtext1, ''), NULLIF(il.externalitemtext, ''), '(nincs megnevezés)') AS cikk, ROUND(SUM(CASE WHEN i.debitcredit = 0 THEN COALESCE(il.numberinvoiced, 0) ELSE -COALESCE(NULLIF(il.numbercredited, 0), il.numberinvoiced, 0) END), 2) AS mennyiseg, ROUND(SUM(CASE WHEN i.debitcredit = 0 THEN COALESCE(il.linepricebase, 0) ELSE -COALESCE(il.linepricebase, 0) END), 2) AS netto_ertek_alapdevizaban, COUNT(DISTINCT i.invoicenumber) AS szamlak_szama, DATE_FORMAT(MIN(STR_TO_DATE(i.invoicedate, '%Y.%m.%d')), '%Y-%m-%d') AS elso_szamla, DATE_FORMAT(MAX(STR_TO_DATE(i.invoicedate, '%Y.%m.%d')), '%Y-%m-%d') AS utolso_szamla FROM invoice i JOIN invoiceline il ON il.companynumber = i.companynumber AND il.invoicenumber = i.invoicenumber LEFT JOIN customer c ON c.customernumber = i.customernumber LEFT JOIN endcustomer ec ON ec.endcustomernumber = COALESCE(NULLIF(i.endcustomernumber, ''), il.endcustomernumber) WHERE i.companynumber = '1' AND i.posted = 1 AND STR_TO_DATE(i.invoicedate, '%Y.%m.%d') >= {fiscal_year_start} AND STR_TO_DATE(i.invoicedate, '%Y.%m.%d') <= CURRENT_DATE AND (i.endcustomernumber IN ({quoted_codes}) OR il.endcustomernumber IN ({quoted_codes})) GROUP BY i.customernumber, c.name1, c.name2, c.name3, c.name4, c.name5, i.endcustomernumber, il.endcustomernumber, ec.name1, ec.name2, ec.name3, ec.name4, ec.name5, il.itemnumber, il.itemtext1, il.externalitemtext HAVING ABS(SUM(CASE WHEN i.debitcredit = 0 THEN COALESCE(il.linepricebase, 0) ELSE -COALESCE(il.linepricebase, 0) END)) > 0.004 OR ABS(SUM(CASE WHEN i.debitcredit = 0 THEN COALESCE(il.numberinvoiced, 0) ELSE -COALESCE(NULLIF(il.numbercredited, 0), il.numberinvoiced, 0) END)) > 0.004 ORDER BY netto_ertek_alapdevizaban DESC LIMIT 200"
        ),
        title: format!("{label} vásárolt cikkei az elmúlt {analysis_fy_window} FY-ban"),
        visualization: "table".into(),
        max_rows: 200,
    }
}

fn billed_customer_purchase_plan(
    label: &str,
    customer_codes: &[String],
    analysis_fy_window: u8,
    english: bool,
) -> QueryPlan {
    let quoted_codes = customer_codes
        .iter()
        .map(|code| format!("'{}'", code.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    let mut plan = end_customer_purchase_plan(label, customer_codes, analysis_fy_window);
    let old_filter = format!(
        "(i.endcustomernumber IN ({quoted_codes}) OR il.endcustomernumber IN ({quoted_codes}))"
    );
    plan.sql = plan.sql.replace(
        &old_filter,
        &format!("i.customernumber IN ({quoted_codes})"),
    );
    let fiscal_year_start = format!(
        "MAKEDATE(YEAR(CURRENT_DATE) - {}, 1)",
        analysis_fy_window - 1
    );
    plan.sql = plan
        .sql
        .replace(
            "FROM invoice i JOIN invoiceline il",
            "FROM invoice i STRAIGHT_JOIN invoiceline il",
        )
        .replace(
            &format!("STR_TO_DATE(i.invoicedate, '%Y.%m.%d') >= {fiscal_year_start}"),
            &format!("i.invoicedate >= DATE_FORMAT({fiscal_year_start}, '%Y.%m.%d')"),
        )
        .replace(
            "STR_TO_DATE(i.invoicedate, '%Y.%m.%d') <= CURRENT_DATE",
            "i.invoicedate <= DATE_FORMAT(CURRENT_DATE, '%Y.%m.%d')",
        );
    plan.title = if english {
        format!("Products purchased by {label} as invoiced customer — last {analysis_fy_window} FY")
    } else {
        format!("{label} által vásárolt cikkek — számlázott vevőként")
    };
    plan
}

fn fixed_relationship_plan(question: &str, analysis_fy_window: u8) -> Option<QueryPlan> {
    let normalized = normalize_for_search(question);
    if !is_customer_purchase_question(question) {
        return None;
    }
    let entity = knowledge_pack().entities.iter().find(|entity| {
        entity
            .query_aliases
            .iter()
            .any(|alias| contains_token_phrase(&normalized, alias))
    })?;
    if entity.id == "one_hungary"
        && ["one year", "one fiscal year", "one fy"]
            .iter()
            .any(|period| normalized.contains(period))
        && ![
            "one end customer",
            "one end user",
            "one magyarorszag",
            "one zrt",
        ]
        .iter()
        .any(|name| normalized.contains(name))
    {
        return None;
    }
    let excluded = entity
        .excluded_codes
        .iter()
        .map(|code| normalize_for_search(code))
        .collect::<HashSet<_>>();
    let codes = entity
        .exact_codes
        .iter()
        .filter(|code| !excluded.contains(&normalize_for_search(code)))
        .cloned()
        .collect::<Vec<_>>();
    if codes.is_empty() {
        return None;
    }
    let mut plan = end_customer_purchase_plan(&entity.label, &codes, analysis_fy_window);
    if is_english_question(question) {
        plan.title = format!(
            "Products purchased by {} as end customer — last {analysis_fy_window} FY",
            entity.label
        );
    }
    Some(plan)
}

fn purchase_role_clarification(question: &str) -> Option<String> {
    if !is_customer_purchase_question(question)
        || fixed_relationship_plan(question, 1).is_some()
        || explicit_billed_customer_name(question).is_some()
        || explicit_end_customer_name(question).is_some()
    {
        return None;
    }
    let company_named = ambiguous_purchase_company_name(question).is_some();
    if is_english_question(question) {
        Some(if company_named {
            "For this company, do you mean the invoiced reseller/customer or the end customer?"
        } else {
            "For this purchase search, do you mean the invoiced reseller/customer or the end customer? Please also name the company."
        }.into())
    } else {
        Some(if company_named {
            "A megnevezett cégnél a számlázott viszonteladót/vevőt vagy a végfelhasználót keresed?"
        } else {
            "Ehhez a vásárlási kereséshez a számlázott viszonteladót/vevőt vagy a végfelhasználót keresed? Kérlek, add meg a cég nevét is."
        }.into())
    }
}

fn ambiguous_purchase_company_name(question: &str) -> Option<String> {
    let normalized = normalize_for_search(question);
    let patterns = [
        r"\b(?:a|az|the)\s+([a-z0-9][a-z0-9 ]{1,80}?)\s+(?:partner|viszontelado|vegfelhasznalo|reseller|end customer|customer)\b",
        r"\b(?:a|az|the)\s+([a-z0-9][a-z0-9 ]{1,80}?)\s+(?:idei|iden|this year|last year)\s+vasarlas",
        r"\b(?:vasarolt|vasrolt|rendelt|vett)\s+(?:a|az)\s+([a-z0-9][a-z0-9 ]{1,80}?)(?:\s+(?:iden|az|a|elmult|[0-9])\b|$)",
        r"\bdid\s+(?:the\s+)?([a-z0-9][a-z0-9 ]{1,80}?)\s+(?:buy|purchase|order)\b",
        r"\b(?:products?|items?)\s+(?:the\s+)?([a-z0-9][a-z0-9 ]{1,80}?)\s+(?:bought|purchased|ordered)\b",
    ];
    patterns.iter().find_map(|pattern| {
        let captures = Regex::new(pattern)
            .expect("valid ambiguous company regex")
            .captures(&normalized)?;
        let name = captures.get(1)?.as_str().trim();
        (name.len() >= 2 && name.split_whitespace().count() <= 5).then(|| name.to_string())
    })
}

fn clarified_purchase_question(question: &str, history: &[HistoryMessage]) -> Option<String> {
    let answer = normalize_for_search(question);
    if answer.split_whitespace().count() > 6 {
        return None;
    }
    let billed = [
        "viszontelado",
        "szamlazott vevo",
        "szamlazott partner",
        "reseller",
        "billed customer",
        "invoiced customer",
    ]
    .iter()
    .any(|role| answer.contains(role));
    let end_customer = ["vegfelhasznalo", "end customer", "end user"]
        .iter()
        .any(|role| answer.contains(role));
    if billed == end_customer || history.len() < 2 {
        return None;
    }
    let clarification = history.last()?;
    let original = &history[history.len() - 2];
    let clarification_text = normalize_for_search(&clarification.content);
    if clarification.role != "assistant"
        || original.role != "user"
        || (!clarification_text.contains("viszontelado")
            && !clarification_text.contains("reseller"))
        || (!clarification_text.contains("vegfelhasznalo")
            && !clarification_text.contains("end customer"))
        || !is_customer_purchase_question(&original.content)
    {
        return None;
    }
    let company = ambiguous_purchase_company_name(&original.content)?;
    let canonical = if is_english_question(&original.content) {
        if billed {
            format!("What products did the {company} reseller purchase?")
        } else {
            format!("What products did the {company} end customer purchase?")
        }
    } else if billed {
        format!("Milyen cikkeket vásárolt a {company} viszonteladó?")
    } else {
        format!("Milyen cikkeket vásárolt a {company} végfelhasználó?")
    };
    Some(format!("{canonical} {}", original.content))
}

fn purchase_fy_window(question: &str, selected_fy_window: u8) -> u8 {
    let normalized = normalize_for_search(question);
    if ["iden", "idei", "this year", "current year"]
        .iter()
        .any(|period| normalized.contains(period))
    {
        1
    } else {
        selected_fy_window
    }
}

fn requests_chart(question: &str) -> bool {
    let normalized = normalize_for_search(question);
    ["diagram", "abrazol", "grafikon", "chart", "graph"]
        .iter()
        .any(|term| normalized.contains(term))
}

fn is_english_question(question: &str) -> bool {
    let normalized = normalize_for_search(question);
    [
        "what",
        "which",
        "list",
        "show",
        "find",
        "products",
        "items",
        "bought",
        "buy",
        "purchased",
        "reseller",
        "customer",
        "end user",
        "this year",
        "last year",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
        && ![
            "mely",
            "milyen",
            "listazd",
            "mutasd",
            "vasarolt",
            "vegfelhasznalo",
            "idén",
            "iden",
        ]
        .iter()
        .any(|marker| normalized.contains(marker))
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
    /// Tényleges adatbázis-értékek kis kardinalitású kód-, státusz- és
    /// dátummezőkre. Enélkül a modell nem tudhatta meg, mit jelent a
    /// `debitcredit`, vagy hogy az `invoicedate` valóban YYYY.MM.DD-e.
    /// `default`: a korábban gyorsítótárazott katalógusok is betölthetők.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    samples: Vec<String>,
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

fn feedback_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("analysis-feedback.json"))
        .map_err(|error| format!("A helyi visszajelzési tár útvonala nem érhető el: {error}"))
}

fn read_feedback_records(app: &AppHandle) -> Result<Vec<AnalysisFeedbackRecord>, String> {
    let path = feedback_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("A helyi visszajelzések nem olvashatók: {error}"))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("A helyi visszajelzési tár sérült: {error}"))
}

fn write_feedback_records(
    app: &AppHandle,
    records: &[AnalysisFeedbackRecord],
) -> Result<(), String> {
    let path = feedback_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "Érvénytelen visszajelzési útvonal.".to_string())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("A visszajelzési mappa nem hozható létre: {error}"))?;
    let contents = serde_json::to_vec_pretty(records)
        .map_err(|error| format!("A visszajelzések nem alakíthatók át: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, contents)
        .map_err(|error| format!("A visszajelzés nem menthető: {error}"))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("A visszajelzési tár nem véglegesíthető: {error}"))
}

/// Megtanult entitásalias (3. réteg): cégnév -> ellenőrzött ERP-kódok.
///
/// Ez a réteg KÖZVETLENÜL befolyásolja a pénzügyi számokat, mert azt dönti
/// el, melyik cég adatait összegezzük. Ezért emberi jóváhagyás nélkül soha
/// nem lép életbe: a bejegyzés `pending` állapotban keletkezik, és csak
/// `approved` állapotban használja a feloldás. A modell nem írhatja.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LearnedAlias {
    id: String,
    created_at: u64,
    /// A kérdésben szereplő név, ahogy a felhasználó írta.
    search_name: String,
    /// "end_customer" vagy "billed_customer" — a két szerep két külön törzs.
    role: String,
    /// A jóváhagyott kódok. `pending` állapotban üres.
    codes: Vec<String>,
    label: String,
    /// Az összes jelölt, amiből a felhasználó választhat. A jóváhagyás csak
    /// ezekből engedi a kódokat, hogy a UI ne tudjon idegen kódot bejuttatni.
    candidates: Vec<LearnedAliasCandidate>,
    status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LearnedAliasCandidate {
    code: String,
    label: String,
}

static ALIAS_FILE_LOCK: Mutex<()> = Mutex::new(());

fn learned_alias_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join("learned-aliases.json"))
        .map_err(|error| format!("A helyi aliastár útvonala nem érhető el: {error}"))
}

fn read_learned_aliases(app: &AppHandle) -> Result<Vec<LearnedAlias>, String> {
    let path = learned_alias_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("A helyi aliastár nem olvasható: {error}"))?;
    serde_json::from_str(&contents).map_err(|error| format!("A helyi aliastár sérült: {error}"))
}

fn write_learned_aliases(app: &AppHandle, aliases: &[LearnedAlias]) -> Result<(), String> {
    let path = learned_alias_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "Érvénytelen aliastár-útvonal.".to_string())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Az aliastár mappája nem hozható létre: {error}"))?;
    let contents = serde_json::to_vec_pretty(aliases)
        .map_err(|error| format!("Az aliastár nem alakítható át: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, contents)
        .map_err(|error| format!("Az aliastár nem menthető: {error}"))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("Az aliastár nem véglegesíthető: {error}"))
}

fn alias_matches(alias: &LearnedAlias, search_name: &str, role: &str) -> bool {
    alias.role == role
        && normalize_for_search(&alias.search_name) == normalize_for_search(search_name)
}

/// A jóváhagyott kódok, ha vannak. Ez az egyetlen pont, ahol a tanult alias
/// életbe lép, és kizárólag `approved` állapotot fogad el.
fn approved_alias_codes(
    app: &AppHandle,
    search_name: &str,
    role: &str,
) -> Option<(String, Vec<String>)> {
    let aliases = read_learned_aliases(app).unwrap_or_default();
    aliases
        .iter()
        .rev()
        .find(|alias| {
            alias.status == "approved"
                && !alias.codes.is_empty()
                && alias_matches(alias, search_name, role)
        })
        .map(|alias| (alias.label.clone(), alias.codes.clone()))
}

/// Jóváhagyásra váró bejegyzés felvétele, amikor a feloldás több céget talált.
/// Ugyanarra a névre és szerepre nem gyűjt duplikátumot.
fn record_pending_alias(
    app: &AppHandle,
    search_name: &str,
    role: &str,
    candidates: &[(String, String)],
) {
    if candidates.len() < 2 {
        return;
    }
    let Ok(_guard) = ALIAS_FILE_LOCK.lock() else {
        return;
    };
    let mut aliases = read_learned_aliases(app).unwrap_or_default();
    if aliases
        .iter()
        .any(|alias| alias.status != "rejected" && alias_matches(alias, search_name, role))
    {
        return;
    }
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    aliases.push(LearnedAlias {
        id: format!("alias-{created_at}-{}", std::process::id()),
        created_at,
        search_name: search_name.trim().into(),
        role: role.into(),
        codes: Vec::new(),
        label: search_name.trim().into(),
        candidates: candidates
            .iter()
            .take(20)
            .map(|(code, label)| LearnedAliasCandidate {
                code: code.clone(),
                label: label.clone(),
            })
            .collect(),
        status: "pending".into(),
    });
    if aliases.len() > 200 {
        let remove_count = aliases.len() - 200;
        aliases.drain(0..remove_count);
    }
    let _ = write_learned_aliases(app, &aliases);
}

#[tauri::command]
fn load_learned_aliases(app: AppHandle) -> Result<Vec<LearnedAlias>, String> {
    let _guard = ALIAS_FILE_LOCK
        .lock()
        .map_err(|_| "A helyi aliastár zárolása sikertelen.".to_string())?;
    let mut aliases = read_learned_aliases(&app)?;
    aliases.sort_by_key(|alias| Reverse(alias.created_at));
    Ok(aliases)
}

/// A jóváhagyási kapu. A kódoknak a rögzített jelöltek közül kell jönniük,
/// így a felület nem tud olyan kódot bejuttatni, amit az adatbázis-feloldás
/// nem talált meg.
#[tauri::command]
fn approve_learned_alias(
    app: AppHandle,
    alias_id: String,
    codes: Vec<String>,
) -> Result<Vec<LearnedAlias>, String> {
    if codes.is_empty() {
        return Err("Legalább egy kódot ki kell választani a jóváhagyáshoz.".into());
    }
    let _guard = ALIAS_FILE_LOCK
        .lock()
        .map_err(|_| "A helyi aliastár zárolása sikertelen.".to_string())?;
    let mut aliases = read_learned_aliases(&app)?;
    let alias = aliases
        .iter_mut()
        .find(|alias| alias.id == alias_id)
        .ok_or_else(|| "Nincs ilyen aliasbejegyzés.".to_string())?;
    let allowed = alias
        .candidates
        .iter()
        .map(|candidate| candidate.code.as_str())
        .collect::<HashSet<_>>();
    if let Some(unknown) = codes.iter().find(|code| !allowed.contains(code.as_str())) {
        return Err(format!(
            "A {unknown} kód nem szerepelt a feloldás jelöltjei között, ezért nem hagyható jóvá."
        ));
    }
    alias.label = alias
        .candidates
        .iter()
        .find(|candidate| candidate.code == codes[0])
        .map(|candidate| candidate.label.clone())
        .unwrap_or_else(|| alias.search_name.clone());
    alias.codes = codes;
    alias.status = "approved".into();
    let updated = aliases.clone();
    write_learned_aliases(&app, &aliases)?;
    Ok(updated)
}

#[tauri::command]
fn reject_learned_alias(app: AppHandle, alias_id: String) -> Result<Vec<LearnedAlias>, String> {
    let _guard = ALIAS_FILE_LOCK
        .lock()
        .map_err(|_| "A helyi aliastár zárolása sikertelen.".to_string())?;
    let mut aliases = read_learned_aliases(&app)?;
    let alias = aliases
        .iter_mut()
        .find(|alias| alias.id == alias_id)
        .ok_or_else(|| "Nincs ilyen aliasbejegyzés.".to_string())?;
    alias.codes = Vec::new();
    alias.status = "rejected".into();
    let updated = aliases.clone();
    write_learned_aliases(&app, &aliases)?;
    Ok(updated)
}

/// A kérdés tartalmi kulcsai a hasonlóságkereséshez. A magyar ragozást
/// előtagvágással kezeljük: nincs szótövező függőség, és a visszatérő
/// megfogalmazások így is egymásra illeszkednek.
fn memory_terms(text: &str) -> HashSet<String> {
    normalize_for_search(text)
        .split_whitespace()
        .filter(|term| term.len() >= 3 && !HUNGARIAN_STOP_WORDS.contains(term))
        .map(|term| term.chars().take(MEMORY_TERM_PREFIX).collect::<String>())
        .collect()
}

/// Jaccard-hasonlóság két kérdés kulcskészletén.
fn question_similarity(left: &HashSet<String>, right: &HashSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(right).count() as f64;
    let union = left.union(right).count() as f64;
    intersection / union
}

/// A visszajelzésekből felépített, kérdéshez szabott memória (1-2. réteg).
///
/// Ez az, ami eddig hiányzott: a visszajelzés eltárolta a kérdést, az SQL-t
/// és a javítást, de soha semmi nem olvasta vissza a promptba. A modell
/// súlyai nem tanulnak; amit itt építünk, az visszakeresett tapasztalat.
///
/// Szándékosan csak TÁMPONT kerül a promptba: a végleges SQL ugyanúgy
/// átmegy a `validate_planned_sql` üzleti ellenőrzésen, ezért egy téves
/// visszajelzés nem tud szabályt felülírni.
fn memory_prompt_context(records: &[AnalysisFeedbackRecord], question: &str) -> String {
    let question_terms = memory_terms(question);
    if question_terms.is_empty() {
        return String::new();
    }

    let mut examples = Vec::new();
    let mut pitfalls = Vec::new();
    for record in records {
        let similarity = question_similarity(&question_terms, &memory_terms(&record.question));
        if similarity < MEMORY_MIN_SIMILARITY {
            continue;
        }
        match record.rating.as_str() {
            "positive" if record.status != "rejected" => {
                if let Some(sql) = record
                    .sql
                    .as_ref()
                    .map(|sql| sql.trim())
                    .filter(|sql| !sql.is_empty())
                {
                    examples.push((similarity, record, sql.to_string()));
                }
            }
            "negative" if !record.correction.trim().is_empty() => {
                pitfalls.push((similarity, record));
            }
            _ => {}
        }
    }

    // A legfrissebb tapasztalat döntsön egyenlő hasonlóságnál.
    examples.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then(right.1.created_at.cmp(&left.1.created_at))
    });
    pitfalls.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then(right.1.created_at.cmp(&left.1.created_at))
    });
    examples.truncate(MAX_MEMORY_EXAMPLES);
    pitfalls.truncate(MAX_MEMORY_PITFALLS);

    if examples.is_empty() && pitfalls.is_empty() {
        return String::new();
    }

    let mut context = String::from(
        "Tanult tapasztalat a korábbi, felhasználói visszajelzéssel ellenőrzött kérdésekből.\n\
         Ezek TÁMPONTOK, nem szabályok: ha ütköznek a sémával vagy az üzleti definíciókkal, a séma és az üzleti definíció az erősebb.\n",
    );
    if !examples.is_empty() {
        context.push_str("\nHelyesnek jelölt korábbi lekérdezések hasonló kérdésekre:\n");
        for (index, (_, record, sql)) in examples.iter().enumerate() {
            context.push_str(&format!(
                "{}. Kérdés: {}\n   Elfogadott SQL: {}\n",
                index + 1,
                record.question,
                sql.chars()
                    .take(MEMORY_EXAMPLE_SQL_CHARACTERS)
                    .collect::<String>()
            ));
        }
    }
    if !pitfalls.is_empty() {
        context.push_str(
            "\nKorábban jelzett hibák hasonló kérdéseknél — ezeket kifejezetten kerüld el:\n",
        );
        for (_, record) in &pitfalls {
            context.push_str(&format!(
                "- Kérdés: {}\n  Kategória: {}\n  A felhasználó javítása: {}\n",
                record.question,
                record.category.as_deref().unwrap_or("nincs megadva"),
                record
                    .correction
                    .chars()
                    .take(MEMORY_CORRECTION_CHARACTERS)
                    .collect::<String>()
            ));
        }
    }
    context
}

/// Csak az összefoglalót érintő korábbi javítások. Az az eset, amikor az SQL
/// jó volt, de a szöveges válasz félreértelmezte az eredményt.
fn summary_pitfall_context(records: &[AnalysisFeedbackRecord], question: &str) -> String {
    let question_terms = memory_terms(question);
    if question_terms.is_empty() {
        return String::new();
    }
    let mut pitfalls = records
        .iter()
        .filter(|record| {
            record.rating == "negative"
                && record.category.as_deref() == Some("summary")
                && !record.correction.trim().is_empty()
        })
        .map(|record| {
            (
                question_similarity(&question_terms, &memory_terms(&record.question)),
                record,
            )
        })
        .filter(|(similarity, _)| *similarity >= MEMORY_MIN_SIMILARITY)
        .collect::<Vec<_>>();
    pitfalls.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then(right.1.created_at.cmp(&left.1.created_at))
    });
    pitfalls.truncate(MAX_MEMORY_PITFALLS);
    if pitfalls.is_empty() {
        return String::new();
    }

    let mut context = String::from(
        "Korábban az összefoglalóra adott javítások hasonló kérdéseknél — ezeket vedd figyelembe:\n",
    );
    for (_, record) in pitfalls {
        context.push_str(&format!(
            "- {}\n",
            record
                .correction
                .chars()
                .take(MEMORY_CORRECTION_CHARACTERS)
                .collect::<String>()
        ));
    }
    context
}

fn validate_feedback(input: &AnalysisFeedbackInput) -> Result<(), String> {
    if !matches!(input.rating.as_str(), "positive" | "negative") {
        return Err("Ismeretlen visszajelzési értékelés.".into());
    }
    if input.question.trim().is_empty() || input.question.chars().count() > 5_000 {
        return Err("A visszajelzés kérdése üres vagy túl hosszú.".into());
    }
    if input.answer.trim().is_empty() || input.answer.chars().count() > 16_000 {
        return Err("A visszajelzés válasza üres vagy túl hosszú.".into());
    }
    if input.correction.chars().count() > 4_000 {
        return Err("A javítás legfeljebb 4000 karakter lehet.".into());
    }
    if input.rating == "negative" && input.correction.trim().is_empty() {
        return Err("Írd le röviden, mit kellene javítani.".into());
    }
    if input
        .sql
        .as_ref()
        .is_some_and(|sql| sql.chars().count() > 12_000)
    {
        return Err("A visszajelzéshez tartozó SQL túl hosszú.".into());
    }
    let allowed_categories = [
        "entity",
        "metric",
        "period",
        "relationship",
        "missing-results",
        "summary",
        "visualization",
        "other",
    ];
    if input
        .category
        .as_ref()
        .is_some_and(|category| !allowed_categories.contains(&category.as_str()))
    {
        return Err("Ismeretlen visszajelzési kategória.".into());
    }
    Ok(())
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
fn save_analysis_feedback(
    app: AppHandle,
    feedback: AnalysisFeedbackInput,
) -> Result<String, String> {
    validate_feedback(&feedback)?;
    let _guard = FEEDBACK_FILE_LOCK
        .lock()
        .map_err(|_| "A helyi visszajelzési tár zárolása sikertelen.".to_string())?;
    let settings = read_settings(&app)?;
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let id = format!("feedback-{created_at}-{}", std::process::id());
    let status = if feedback.rating == "negative" {
        "pending"
    } else {
        "recorded"
    };
    let record = AnalysisFeedbackRecord {
        id: id.clone(),
        created_at,
        app_version: env!("CARGO_PKG_VERSION").into(),
        model: settings.ai_model,
        analysis_fy_window: settings.analysis_fy_window,
        rating: feedback.rating,
        category: feedback.category,
        correction: feedback.correction.trim().into(),
        question: feedback.question.trim().into(),
        answer: feedback.answer.trim().into(),
        result_title: feedback.result_title,
        sql: feedback.sql,
        query_source: feedback.query_source,
        row_count: feedback.row_count,
        status: status.into(),
    };
    let mut records = read_feedback_records(&app)?;
    records.push(record);
    if records.len() > 500 {
        let remove_count = records.len() - 500;
        records.drain(0..remove_count);
    }
    write_feedback_records(&app, &records)?;
    Ok(id)
}

#[tauri::command]
fn load_knowledge_overview(app: AppHandle) -> Result<KnowledgeOverview, String> {
    let _guard = FEEDBACK_FILE_LOCK
        .lock()
        .map_err(|_| "A helyi visszajelzési tár zárolása sikertelen.".to_string())?;
    let feedback = read_feedback_records(&app)?;
    let pack = knowledge_pack();
    let recent_feedback = feedback
        .iter()
        .rev()
        .filter(|record| record.rating == "negative")
        .take(10)
        .map(|record| FeedbackSummary {
            id: record.id.clone(),
            created_at: record.created_at,
            category: record.category.clone(),
            question: record.question.clone(),
            correction: record.correction.clone(),
            status: record.status.clone(),
        })
        .collect();
    Ok(KnowledgeOverview {
        version: pack.version.clone(),
        metric_count: pack.metrics.len(),
        relationship_count: pack.relationships.len(),
        entity_rule_count: pack.entities.len(),
        skill_count: pack.skills.len(),
        feedback_total: feedback.len(),
        feedback_pending: feedback
            .iter()
            .filter(|record| record.status == "pending")
            .count(),
        recent_feedback,
    })
}

#[tauri::command]
fn open_analysis_feedback_email(app: AppHandle, feedback_id: String) -> Result<(), String> {
    let record = {
        let _guard = FEEDBACK_FILE_LOCK
            .lock()
            .map_err(|_| "A helyi visszajelzési tár zárolása sikertelen.".to_string())?;
        read_feedback_records(&app)?
            .into_iter()
            .find(|record| record.id == feedback_id)
            .ok_or_else(|| "A visszajelzés nem található.".to_string())?
    };
    let category = record.category.as_deref().unwrap_or("nincs megadva");
    let result_title = record
        .result_title
        .as_deref()
        .unwrap_or("nincs eredménykártya");
    let query_source = record.query_source.as_deref().unwrap_or("nincs adat");
    let sql = record.sql.as_deref().unwrap_or("nincs SQL");
    let concise_answer = record.answer.chars().take(3_000).collect::<String>();
    let concise_sql = sql.chars().take(3_000).collect::<String>();
    let subject = format!("ERGO {} elemzési javítás", record.app_version);
    let body = format!(
        "ERGO elemzési visszajelzés\n\nKategória: {category}\nJavítás:\n{}\n\nEredeti kérdés:\n{}\n\nERGO válasza:\n{concise_answer}\n\nEredmény: {result_title}\nForrás: {query_source}\nSorok: {}\n\nSQL:\n{concise_sql}\n\n---\nVisszajelzés: {}\nVerzió: {}\nModell: {}\nIdőablak: {} FY\n\nA levél nem tartalmaz API-kulcsot, adatbázis-jelszót vagy nyers lekérdezési sorokat.",
        record.correction,
        record.question,
        record.row_count.map(|count| count.to_string()).unwrap_or_else(|| "nincs adat".into()),
        record.id,
        record.app_version,
        record.model,
        record.analysis_fy_window,
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
        http_client().get(endpoint).bearer_auth(api_key).send(),
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

/// Újrahasznosított kapcsolatpool a lekérdezési utakhoz. Korábban minden
/// kérdés új poolt épített és azonnal szét is kapcsolta, így a kapcsolatpool
/// sosem fejtett ki hatást.
fn cached_database_pool(settings: &StoredSettings, password: &str) -> Pool {
    static POOLS: OnceLock<Mutex<HashMap<u64, Pool>>> = OnceLock::new();
    let mut hasher = DefaultHasher::new();
    settings.mysql_host.hash(&mut hasher);
    settings.mysql_port.hash(&mut hasher);
    settings.mysql_database.hash(&mut hasher);
    settings.mysql_user.hash(&mut hasher);
    settings.mysql_ssl.hash(&mut hasher);
    // A jelszó is a kulcs része, hogy jelszócsere után ne maradjon
    // használatban a régi beállításokkal felépített pool.
    password.hash(&mut hasher);
    let key = hasher.finish();

    let pools = POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = match pools.lock() {
        Ok(guard) => guard,
        // Megmérgezett zár esetén inkább egyszer használatos poolt adunk,
        // mint hogy a lekérdezés elhasaljon.
        Err(_) => return database_pool(settings, password.to_string()),
    };
    guard
        .entry(key)
        .or_insert_with(|| database_pool(settings, password.to_string()))
        .clone()
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
    let pool = cached_database_pool(settings, &password);
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
                samples: Vec::new(),
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

    collect_column_samples(&mut connection, &mut tables).await;
    drop(connection);

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

/// Az `information_schema` nem mondja meg, hogy a `debitcredit` értéke 0/1
/// vagy D/C, és hogy az `invoicedate` varchar mezőben pont vagy kötőjel
/// választja-e el a dátumot. Ezt csak tényleges adatból lehet megtudni.
///
/// A mintavétel szándékosan szűk: korlátozott ablakból olvas (nem fut
/// teljes táblaszkennt a replikán), legfeljebb `MAX_SAMPLED_COLUMNS`
/// oszlopot vizsgál, és minden hiba csendben kimarad — a séma akkor is
/// használható, ha a mintavétel nem sikerül.
async fn collect_column_samples(
    connection: &mut mysql_async::Conn,
    tables: &mut BTreeMap<String, SchemaTable>,
) {
    let identifier = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("valid identifier regex");
    let mut probes = Vec::new();
    for (table_name, table) in tables.iter() {
        if table.table_type != "BASE TABLE" || !identifier.is_match(table_name) {
            continue;
        }
        for column in &table.columns {
            if !identifier.is_match(&column.name) || !is_sampleable_column(column) {
                continue;
            }
            probes.push((table_name.clone(), column.name.clone()));
        }
    }
    probes.truncate(MAX_SAMPLED_COLUMNS);

    let sampling_started = SystemTime::now();
    for (table_name, column_name) in probes {
        // A sémafrissítés egy felhasználói kérdés közben fut, ezért a
        // mintavétel összesített ideje is korlátos.
        if sampling_started
            .elapsed()
            .is_ok_and(|elapsed| elapsed >= TOTAL_SAMPLING_BUDGET)
        {
            break;
        }
        // A belső LIMIT tartja korlátok között az olvasást: kódmezőknél egy
        // néhány ezer soros ablak is megmutatja az előforduló értékeket.
        let sql = format!(
            "SELECT DISTINCT `{column_name}` AS sample_value \
             FROM (SELECT `{column_name}` FROM `{table_name}` LIMIT {SAMPLE_SCAN_ROWS}) AS sample_window \
             WHERE `{column_name}` IS NOT NULL LIMIT {}",
            MAX_SAMPLES_PER_COLUMN + 1
        );
        let sampled = timeout(SAMPLE_QUERY_TIMEOUT, connection.query::<Row, _>(sql)).await;
        let Ok(Ok(rows)) = sampled else { continue };
        if rows.len() > MAX_SAMPLES_PER_COLUMN {
            // Nagy kardinalitású mező: a felsorolás nem mond semmit.
            continue;
        }
        let mut values = rows
            .iter()
            .map(|row| match mysql_value_to_json(row.as_ref(0)) {
                JsonValue::String(text) => text,
                other => other.to_string(),
            })
            .filter(|value| !value.is_empty() && value.chars().count() <= MAX_SAMPLE_CHARACTERS)
            .collect::<Vec<_>>();
        values.sort();
        values.dedup();
        if values.is_empty() {
            continue;
        }
        if let Some(column) = tables
            .get_mut(&table_name)
            .and_then(|table| table.columns.iter_mut().find(|c| c.name == column_name))
        {
            column.samples = values;
        }
    }
}

/// Kód-, státusz-, jelző- és dátummezők, ahol a tényleges érték számít.
/// A szabad szöveget és a nagy mezőket kihagyjuk.
fn is_sampleable_column(column: &CatalogColumn) -> bool {
    let column_type = column.column_type.to_ascii_lowercase();
    if ["text", "blob", "json", "binary"]
        .iter()
        .any(|excluded| column_type.contains(excluded))
    {
        return false;
    }
    let name = column.name.to_ascii_lowercase();
    [
        "companynumber",
        "credit",
        "currency",
        "date",
        "debit",
        "flag",
        "kind",
        "openclosed",
        "posted",
        "status",
        "type",
        "unit",
    ]
    .iter()
    .any(|fragment| name.contains(fragment))
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

fn is_customer_purchase_question(question: &str) -> bool {
    let normalized = normalize_for_search(question);
    let known_hungarian_forms = [
        "mit vasarolt",
        "miket vasarolt",
        "mit vasrolt",
        "miket vasrolt",
        "megvasarolt cikk",
        "megvasarolt termek",
        "vasarolt cikk",
        "vasarolt termek",
        "milyen cikkeket",
        "milyen termekeket",
        "mit rendelt",
        "miket rendelt",
        "mit vett",
        "miket vett",
        "beszerzett cikk",
        "beszerzett termek",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase));
    let purchase_verbs = [
        "vasarol",
        "vasrol",
        "rendel",
        "vett",
        "megvasarol",
        "buy",
        "bought",
        "purchase",
        "purchased",
        "order",
        "ordered",
    ];
    let product_words = ["termek", "cikk", "product", "item", "goods"];
    known_hungarian_forms
        || (normalized.contains("vasarlas")
            && [
                "listaz", "mutas", "abrazol", "diagram", "milyen", "what", "show",
            ]
            .iter()
            .any(|word| normalized.contains(word)))
        || (purchase_verbs.iter().any(|verb| normalized.contains(verb))
            && product_words.iter().any(|word| normalized.contains(word)))
        || ((normalized.contains("what did")
            || normalized.contains("which did")
            || normalized.contains("what has")
            || normalized.contains("list "))
            && purchase_verbs.iter().any(|verb| normalized.contains(verb)))
}

fn explicit_billed_customer_name(question: &str) -> Option<String> {
    let normalized = normalize_for_search(question);
    if !is_customer_purchase_question(question) {
        return None;
    }
    if (normalized.contains("end customer")
        || normalized.contains("end user")
        || normalized.contains("final customer")
        || normalized.contains("vegfelhasznalo"))
        && !normalized.contains("reseller")
        && !normalized.contains("viszontelado")
        && !normalized.contains("billed customer")
        && !normalized.contains("invoiced customer")
    {
        return None;
    }

    let patterns = [
        // Hungarian: "termékeket, melyeket a DesignShop viszonteladó vásárolt"
        r"\b(?:melyeket|amelyeket|amiket)\s+(?:az?\s+)?(.+?)\s+(?:viszontelado|szamlazott\s+(?:partner|vevo))\b",
        r"\b(?:mit|miket|milyen\s+(?:cikkeket|termekeket))\s+(?:vasarolt|vasrolt|rendelt|vett)\s+(?:az?\s+)?(.+?)\s+(?:viszontelado|szamlazott\s+(?:partner|vevo))\b",
        r"\b(?:az?\s+)?(.+?)\s+(?:viszontelado|szamlazott\s+(?:partner|vevo))\s+(?:vasarol|vasrol|rendel|vett)\b",
        // English: "products purchased by the DesignShop reseller" / "what did DesignShop buy"
        r"\b(?:by|from|did|has)\s+(?:the\s+)?(.+?)\s+(?:reseller|customer|billed\s+customer|invoiced\s+customer)\b",
        r"\b(?:products?|items?)\s+(?:that\s+)?(?:the\s+)?(.+?)\s+(?:reseller|billed\s+customer)\s+(?:bought|purchased|ordered)\b",
    ];
    for pattern in patterns {
        let regex = Regex::new(pattern).expect("valid buyer extraction regex");
        if let Some(captures) = regex.captures(&normalized) {
            let candidate = captures.get(1)?.as_str().trim();
            let candidate = candidate
                .trim_start_matches("the ")
                .trim_start_matches("a ")
                .trim_start_matches("az ")
                .trim();
            let candidate = candidate
                .split_whitespace()
                .take_while(|word| {
                    !["this", "last", "year", "today", "idén", "iden"].contains(word)
                })
                .collect::<Vec<_>>()
                .join(" ");
            if candidate.len() >= 2 {
                return Some(candidate);
            }
        }
    }
    None
}

fn explicit_end_customer_name(question: &str) -> Option<String> {
    let normalized = normalize_for_search(question);
    if !is_customer_purchase_question(question) {
        return None;
    }
    if normalized.contains("end customer")
        || normalized.contains("end user")
        || normalized.contains("final customer")
    {
        let patterns = [
            r"\bby\s+(?:the\s+)?(.+?)\s+(?:as\s+(?:an?\s+)?)?(?:end\s+customer|end\s+user|final\s+customer)\b",
            r"\bwhat\s+did\s+(?:the\s+)?(.+?)\s+(?:as\s+(?:an?\s+)?)?(?:end\s+customer|end\s+user|final\s+customer)\b",
            r"\bwhat\s+(?:products?|items?).*?\bdid\s+(?:the\s+)?(.+?)\s+(?:as\s+(?:an?\s+)?)?(?:end\s+customer|end\s+user|final\s+customer)\b",
            r"\b(?:end\s+customer|end\s+user|final\s+customer)\s+(.+?)\s+(?:buy|bought|purchase|purchased)\b",
        ];
        for pattern in patterns {
            let regex = Regex::new(pattern).expect("valid end-customer extraction regex");
            if let Some(captures) = regex.captures(&normalized) {
                let candidate = captures.get(1)?.as_str().trim();
                let candidate = candidate
                    .trim_start_matches("the ")
                    .trim_start_matches("a ")
                    .trim();
                if candidate.len() >= 2 {
                    return Some(candidate.to_string());
                }
            }
        }
    }
    if let Some((prefix, suffix)) = normalized.split_once(" altal ") {
        if ["megvasarolt", "vasarolt", "rendelt", "vett"]
            .iter()
            .any(|marker| suffix.contains(marker))
        {
            let mut candidate = prefix.trim();
            for command_prefix in [
                "listazd ki ",
                "sorold fel ",
                "mutasd meg ",
                "mutasd ",
                "az ",
                "a ",
            ] {
                candidate = candidate.strip_prefix(command_prefix).unwrap_or(candidate);
            }
            let safe = candidate
                .chars()
                .filter(|character| {
                    character.is_ascii_alphanumeric() || character.is_ascii_whitespace()
                })
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if safe.len() >= 3 {
                return Some(safe);
            }
        }
    }
    if !normalized.contains("vegfelhasznalo") {
        return None;
    }
    let purchase_marker = ["vasarolt", "vasrolt", "rendelt", "vett"]
        .iter()
        .filter_map(|marker| normalized.find(marker).map(|index| (index, *marker)))
        .min_by_key(|(index, _)| *index)?;
    let mut candidate = normalized[purchase_marker.0 + purchase_marker.1.len()..].trim();
    candidate = candidate
        .strip_prefix("az ")
        .or_else(|| candidate.strip_prefix("a "))
        .unwrap_or(candidate);
    let candidate = candidate
        .split("vegfelhasznalo")
        .next()
        .unwrap_or_default()
        .trim();
    let safe = candidate
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || character.is_ascii_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (safe.len() >= 3).then_some(safe)
}

async fn resolve_end_customer(
    settings: &StoredSettings,
    password: String,
    search_name: &str,
) -> Result<Vec<(String, String)>, String> {
    let compact_search = search_name.replace(' ', "");
    if compact_search.len() < 3
        || !compact_search
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Ok(Vec::new());
    }
    let mut search_variants = vec![search_name.to_string()];
    for alias in &knowledge_pack().normalization_aliases {
        if search_name.contains(&alias.from) {
            search_variants.push(search_name.replace(&alias.from, &alias.to));
        }
    }
    search_variants.sort();
    search_variants.dedup();
    let include_token_family = knowledge_pack().entities.iter().any(|entity| {
        entity.include_token_family
            && entity
                .query_aliases
                .iter()
                .any(|alias| normalize_for_search(alias) == normalize_for_search(search_name))
    });
    let name_conditions = search_variants
        .iter()
        .map(|variant| {
            let like_search = variant.split_whitespace().collect::<Vec<_>>().join("%");
            format!(
                "LOWER(endcustomernumber) = '{variant}' OR LOWER(CONCAT_WS(' ', name1, name2, name3, name4, name5)) LIKE '%{like_search}%'"
            )
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    let exact_code_conditions = search_variants
        .iter()
        .map(|variant| format!("LOWER(endcustomernumber) = '{variant}'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let sql = format!(
        "SELECT endcustomernumber AS vegfelhasznalo_kod, COALESCE(NULLIF(TRIM(name1), ''), TRIM(CONCAT_WS(' ', name1, name2, name3, name4, name5))) AS vegfelhasznalo, TRIM(CONCAT_WS(' ', name1, name2, name3, name4, name5)) AS vegfelhasznalo_keresheto FROM endcustomer WHERE {name_conditions} ORDER BY CASE WHEN {exact_code_conditions} THEN 0 ELSE 1 END, endcustomernumber LIMIT 100"
    );
    let (_, rows, _) = run_query(settings, password, &sql, 100).await?;
    let mut seen_codes = HashSet::new();
    let matches = rows
        .into_iter()
        .filter_map(|row| {
            let code = row.get("vegfelhasznalo_kod")?.as_str()?.trim().to_string();
            let name = row
                .get("vegfelhasznalo")
                .and_then(JsonValue::as_str)
                .unwrap_or(&code)
                .trim()
                .to_string();
            let searchable_name = row
                .get("vegfelhasznalo_keresheto")
                .and_then(JsonValue::as_str)
                .unwrap_or(&name);
            let matches_organization = search_variants.iter().any(|variant| {
                normalize_for_search(&code)
                    .split_whitespace()
                    .collect::<String>()
                    == variant.split_whitespace().collect::<String>()
                    || contains_token_phrase(searchable_name, variant)
            });
            (!code.is_empty() && matches_organization && seen_codes.insert(code.clone()))
                .then_some((code, name))
        })
        .collect::<Vec<_>>();
    if !include_token_family {
        let exact_matches = matches
            .iter()
            .filter(|(code, _)| {
                let compact_code = normalize_for_search(code)
                    .split_whitespace()
                    .collect::<String>();
                search_variants.iter().any(|variant| {
                    compact_code
                        == normalize_for_search(variant)
                            .split_whitespace()
                            .collect::<String>()
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !exact_matches.is_empty() {
            return Ok(exact_matches);
        }
    }
    Ok(matches)
}

async fn resolve_billed_customer(
    settings: &StoredSettings,
    password: String,
    search_name: &str,
) -> Result<Vec<(String, String)>, String> {
    let compact_search = search_name.replace(' ', "");
    if compact_search.len() < 2
        || !compact_search
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Ok(Vec::new());
    }
    let like_search = search_name.split_whitespace().collect::<Vec<_>>().join("%");
    let sql = format!(
        "SELECT customernumber AS customer_code, COALESCE(NULLIF(TRIM(CONCAT_WS(' ', name1, name2)), ''), customernumber) AS customer_name, TRIM(CONCAT_WS(' ', name1, name2, name3, name4, name5)) AS customer_searchable FROM customer WHERE LOWER(customernumber) = '{search_name}' OR LOWER(CONCAT_WS(' ', name1, name2, name3, name4, name5)) LIKE '%{like_search}%' LIMIT 100"
    );
    let (_, rows, _) = run_query(settings, password, &sql, 100).await?;
    let needle = normalize_for_search(search_name);
    let mut seen_codes = HashSet::new();
    let matches = rows
        .into_iter()
        .filter_map(|row| {
            let code = row.get("customer_code")?.as_str()?.trim().to_string();
            let name = row
                .get("customer_name")
                .and_then(JsonValue::as_str)
                .unwrap_or(&code)
                .trim()
                .to_string();
            let searchable_name = row
                .get("customer_searchable")
                .and_then(JsonValue::as_str)
                .unwrap_or(&name);
            let exact_code = normalize_for_search(&code)
                .split_whitespace()
                .collect::<String>()
                == needle.split_whitespace().collect::<String>();
            (!code.is_empty()
                && (exact_code || contains_token_phrase(searchable_name, &needle))
                && seen_codes.insert(code.clone()))
            .then_some((code, name))
        })
        .collect::<Vec<_>>();
    let exact_codes = matches
        .iter()
        .filter(|(code, _)| {
            normalize_for_search(code)
                .split_whitespace()
                .collect::<String>()
                == needle.split_whitespace().collect::<String>()
        })
        .cloned()
        .collect::<Vec<_>>();
    Ok(if exact_codes.is_empty() {
        matches
    } else {
        exact_codes
    })
}

fn contains_token_phrase(value: &str, phrase: &str) -> bool {
    let value = normalize_for_search(value);
    let phrase = normalize_for_search(phrase);
    let value_tokens = value.split_whitespace().collect::<Vec<_>>();
    let phrase_tokens = phrase.split_whitespace().collect::<Vec<_>>();
    !phrase_tokens.is_empty()
        && value_tokens
            .windows(phrase_tokens.len())
            .any(|window| window == phrase_tokens)
}

fn business_relationship_skill(question: &str) -> &'static str {
    if !is_customer_purchase_question(question) {
        return "";
    }

    let normalized = normalize_for_search(question);
    if normalized.contains("obh") || normalized.contains("orszagos birosagi hivatal") {
        return r#"Aktív üzleti relációs skill — vevői vásárlások és OBH:
- A számlázott partner és a tényleges végfelhasználó eltérhet. Mindkét relációt vizsgáld meg.
- Kimenő értékesítési lánc: invoice -> invoiceline a companynumber + invoicenumber mezőkön; invoice.customernumber -> customer.customernumber; invoice.endcustomernumber vagy invoiceline.endcustomernumber -> endcustomer.endcustomernumber.
- Szervezetet a customer és endcustomer name1..name5 mezőiben, továbbá a számlafej név-pillanatképében keress. Ne feltételezd, hogy a rövidítés az adatbázisbeli kód.
- Az „OBH” az Országos Bírósági Hivatalt jelenti. Ellenőrzött végfelhasználói kódjai: 'birosag' és 'orszagosbirosagi'. Az 'obh' kód NEM az Országos Bírósági Hivatal, hanem az Országgyűlési Biztosok Hivatalának régi rekordja; ezt ne használd OBH-találatként.
- A találatokban mutasd meg a számlázott partnert, a végfelhasználót, a számlaszámot és dátumot, a cikkszámot, megnevezést, előjeles mennyiséget és előjeles nettó linepricebase értéket.
- Csak companynumber = '1' és posted = 1 számlákat használj; debitcredit = 1 esetén a jóváírást vond le."#;
    }

    r#"Aktív üzleti relációs skill — vevői vásárlások / customer purchases:
- A számlázott partner és a tényleges végfelhasználó eltérhet. Mindkét relációt vizsgáld meg.
- English wording: "reseller", "billed customer" or "invoiced customer" means customer.customernumber through invoice.customernumber. "End customer" or "end user" means endcustomer.endcustomernumber through invoice/invoiceline.endcustomernumber.
- If the role is not stated and cannot be established from a curated exact entity rule, ask which role is intended before querying. Do not silently substitute an entity or role.
- Kimenő értékesítési lánc: invoice -> invoiceline a companynumber + invoicenumber mezőkön; invoice.customernumber -> customer.customernumber; invoice.endcustomernumber vagy invoiceline.endcustomernumber -> endcustomer.endcustomernumber.
- A kérdésben szereplő szervezetet a customer és endcustomer name1..name5 mezőiben, továbbá a számlafej név-pillanatképében keresd. Ne feltételezd, hogy a rövidítés az adatbázisbeli kód.
- A találatokban mutasd meg a számlázott partnert, a végfelhasználót, a számlaszámot és dátumot, a cikkszámot, megnevezést, előjeles mennyiséget és előjeles nettó linepricebase értéket.
- Csak companynumber = '1' és posted = 1 számlákat használj; debitcredit = 1 esetén a jóváírást vond le."#
}

fn schema_search_terms(question: &str, history: &[HistoryMessage]) -> HashSet<String> {
    let stop_words = HUNGARIAN_STOP_WORDS;
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
    if is_customer_purchase_question(&search_text.join(" ")) {
        terms.extend(
            [
                "invoice",
                "invoiceline",
                "customer",
                "endcustomer",
                "name",
                "customernumber",
                "endcustomernumber",
                "invoicenumber",
                "invoicedate",
                "itemnumber",
                "itemtext",
                "numberinvoiced",
                "numbercredited",
                "linepricebase",
                "debitcredit",
                "posted",
                "companynumber",
            ]
            .into_iter()
            .map(str::to_owned),
        );
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
        "customer" | "companycustomer" => Some("közvetlen, számlázott vevői törzsadatok"),
        "endcustomer" => Some(
            "végfelhasználói törzsadatok; a tényleges felhasználó eltérhet a számlázott vevőtől",
        ),
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
    // A kulcsszavas pontozás tévedhet, és a modell nem kérhet pótlást: egy
    // félig találó, irreleváns tábla rosszabb, mint a biztos mag. Ezért a
    // magot MINDIG hozzáfűzzük, nem csak nulla találat esetén.
    for name in SCHEMA_CORE_TABLES {
        if selected.len() >= MAX_SCHEMA_TABLES_PER_QUESTION + SCHEMA_CORE_TABLES.len() {
            break;
        }
        if selected.iter().any(|table| table.name == *name) {
            continue;
        }
        if let Some(table) = catalog.tables.iter().find(|table| table.name == *name) {
            selected.push(table);
        }
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
        "endcustomer",
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
        "posted",
        "quantity",
        "remainder",
        "status",
        "total",
        "vat",
        "vendor",
    ];
    let mut context = String::from(
        "Az alábbi, kérdéshez helyben kiválasztott MySQL-sémarészlet oszlopai használhatók. A mintaértékek tényleges adatbázis-tartalomból származnak.\n",
    );
    // A modell eddig nem tudta, mit NEM kapott meg. A puszta táblanévlista
    // néhány száz token, és megakadályozza a nem létező tábla kitalálását.
    context.push_str(&format!(
        "\nAz adatbázis összes táblája (csak tájékozódásra, oszlopok nélkül):\n{}\n",
        catalog
            .tables
            .iter()
            .map(|table| table.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    context.push_str("\nRészletes sémarészlet:\n");
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
                let samples = if column.samples.is_empty() {
                    String::new()
                } else {
                    format!(" példaértékek: {}", column.samples.join(" | "))
                };
                format!(
                    "{}:{}{}{}",
                    column.name, column.column_type, suffix, samples
                )
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

fn validate_planned_sql(question: &str, sql: &str) -> Result<String, String> {
    let normalized = assert_read_only_sql(sql)?;
    let normalized_question = normalize_for_search(question);
    let asks_for_revenue = ["arbevetel", "bevetel", "forgalom", "sales", "revenue"]
        .iter()
        .any(|term| normalized_question.contains(term));
    let lower = normalized.to_lowercase();
    let uses_invoice_data = Regex::new(r"(?i)\b(invoice|invoiceline)\b")
        .expect("valid invoice table regex")
        .is_match(&normalized);

    if asks_for_revenue && uses_invoice_data {
        if !lower.contains("itemsumbase") && !lower.contains("linepricebase") {
            return Err(
                "Árbevételhez nettó számlaértéket kell használni: invoice.itemsumbase vagy tételszinten invoiceline.linepricebase. A vatbase áfaösszeg, nem árbevétel."
                    .into(),
            );
        }
        if !Regex::new(r"(?i)\b(?:[a-z_]\w*\.)?companynumber\s*=\s*'1'")
            .expect("valid company filter regex")
            .is_match(&normalized)
        {
            return Err(
                "A Trans Europe Zrt. árbevételéhez kötelező a companynumber = '1' szűrés.".into(),
            );
        }
        if !Regex::new(r"(?i)\b(?:[a-z_]\w*\.)?posted\s*=\s*1\b")
            .expect("valid posted filter regex")
            .is_match(&normalized)
        {
            return Err("Árbevételhez csak a posted = 1 könyvelt számlák használhatók.".into());
        }
        if !lower.contains("debitcredit") {
            return Err(
                "Az árbevételben a debitcredit mező alapján le kell vonni a jóváírásokat.".into(),
            );
        }
    }

    if is_customer_purchase_question(question) {
        if !Regex::new(r"(?i)\binvoice\b")
            .expect("valid invoice regex")
            .is_match(&normalized)
            || !Regex::new(r"(?i)\binvoiceline\b")
                .expect("valid invoice line regex")
                .is_match(&normalized)
        {
            return Err(
                "Vevői vásárlásokhoz az invoice számlafejet és az invoiceline cikksorokat együtt kell használni."
                    .into(),
            );
        }
        let filters_end_customer = lower.contains("endcustomernumber in (");
        let filters_billed_customer = Regex::new(r"(?i)\bi\.customernumber\s+in\s*\(")
            .expect("valid billed-customer filter regex")
            .is_match(&normalized);
        let question_role = normalize_for_search(question);
        let explicitly_end_customer = [
            "vegfelhasznalo",
            "end customer",
            "end user",
            "final customer",
        ]
        .iter()
        .any(|role| question_role.contains(role));
        let explicitly_billed_customer = [
            "viszontelado",
            "szamlazott partner",
            "szamlazott vevo",
            "reseller",
            "billed customer",
            "invoiced customer",
        ]
        .iter()
        .any(|role| question_role.contains(role));
        if (explicitly_end_customer && !filters_end_customer)
            || (explicitly_billed_customer && !filters_billed_customer)
            || (!filters_end_customer && !filters_billed_customer)
        {
            return Err(
                "A vásárlási lekérdezésben a kérdezett szerepet kell szűrni: számlázott vevőhöz invoice.customernumber, végfelhasználóhoz endcustomernumber szükséges."
                    .into(),
            );
        }
        if !Regex::new(r"(?i)\b(?:[a-z_]\w*\.)?companynumber\s*=\s*'1'")
            .expect("valid company filter regex")
            .is_match(&normalized)
        {
            return Err(
                "Vevői vásárlásokhoz kötelező a Trans Europe Zrt. companynumber = '1' szűrése."
                    .into(),
            );
        }
        if !Regex::new(r"(?i)\b(?:[a-z_]\w*\.)?posted\s*=\s*1\b")
            .expect("valid posted filter regex")
            .is_match(&normalized)
        {
            return Err(
                "Vevői vásárlásokhoz csak posted = 1 könyvelt számlák használhatók.".into(),
            );
        }
        if !lower.contains("debitcredit") {
            return Err(
                "A vásárlási mennyiségekben és értékekben a debitcredit mező alapján kezelni kell a jóváírásokat."
                    .into(),
            );
        }
        if !lower.contains("linepricebase") {
            return Err(
                "A vásárolt cikksorok összehasonlítható nettó értékéhez az invoiceline.linepricebase mezőt kell használni."
                    .into(),
            );
        }
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

/// A planner promptjának kérdéstől független része. Egyszer épül fel, és
/// szó szerint azonos minden hívásban, hogy a prompt-cache illeszkedni
/// tudjon rá — korábban a dinamikus mezők a prompt közepén ültek, és
/// gyakorlatilag minden kérdés cache-tévesztéssel indult.
fn planner_static_instructions() -> &'static str {
    static INSTRUCTIONS: OnceLock<String> = OnceLock::new();
    INSTRUCTIONS.get_or_init(|| {
        let mut instructions = String::from(
            r#"Te az ERGO, a Trans-Europe óvatos ERP-adatelemzője vagy.
Magyarul és angolul is értelmezd a kérdéseket. A title mezőt a kérdés nyelvén add meg.
Készíts pontos MySQL lekérdezési tervet a megadott adatbázis-séma alapján.
Kizárólag egy SELECT vagy WITH lekérdezést adhatsz. Tilos minden adatmódosítás, DDL, zárolás, fájlművelet, komment, rendszer-séma és több utasítás.
Az sql mező pontosan egyetlen SELECT vagy WITH utasítást tartalmazzon, záró pontosvessző nélkül. Ne használj SET, DECLARE, ideiglenes táblát vagy tárolt eljárást.
Ne találj ki táblát vagy oszlopot. Használj explicit oszlopokat és aggregálj SQL-ben. A sorok száma legfeljebb 200 legyen.
Az üzleti kategóriát és a táblaleírást tekintsd mérvadónak; az azonos szavakat tartalmazó, de más üzleti célú táblákat ne keverd össze.
Kötelező árbevétel-definíció számlaadatok használatakor:
- az árbevétel nettó számlázott árbevételt jelent a Trans Europe Zrt. vállalati alapdevizájában;
- a Trans Europe Zrt. rekordjait companynumber = '1' azonosítja;
- csak a posted = 1 könyvelt számlákat használd;
- számlafejes összesítéshez invoice.itemsumbase mezőt használj, és a debitcredit = 1 jóváírásokat vond le;
- cikkszintű elemzéshez invoiceline.linepricebase mezőt használj, az invoice táblához companynumber és invoicenumber alapján kapcsolva, hogy a könyveltséget és a jóváírás előjelét alkalmazhasd;
- a vatbase áfaösszeg, soha ne nevezd vagy összegezd árbevételként;
- eltérő invoice.currency értékeket ne add össze dokumentumdevizában; összehasonlításhoz a *base mezőket használd.
Időfüggő üzleti adatoknál kötelező közvetlenül az SQL WHERE feltételében időszakot szűrni:
- ha a kérdés napot vagy időszakot ad meg (például ma, tegnap, adott hónap), pontosan arra, és ne olvass be azon kívüli rekordot;
- dátumként tárolt varchar mezőnél vedd figyelembe a sémában jelzett típust és a ponttal tagolt YYYY.MM.DD formátumot;
- fejlécszámhoz ne kapcsolj tételtáblát, ha a kérdés nem kér tételszintű adatot.
Vevői termékvásárlásnál ne keverd össze a számlázott vevőt/viszonteladót (invoice.customernumber -> customer.customernumber) a tényleges végfelhasználóval (endcustomernumber -> endcustomer.endcustomernumber). Ha a kérdés szerepe nem egyértelmű, ne válassz helyette: kérdezz vissza, hogy a számlázott viszonteladót/vevőt vagy a végfelhasználót keressük.
A sémarészlet a kérdéshez helyben kiválasztott táblákat tartalmazza. Ha a feladathoz olyan tábla kell, amelyik csak a teljes táblalistában szerepel, oszlopokkal nem, akkor NE találd ki az oszlopneveit: adj olyan tervet, amely a megadott oszlopokkal megoldható, vagy kérdezz vissza.
A mintaértékek tényleges adatbázis-tartalomból származnak: a kódok, státuszjelzők és dátumformátumok ellenőrzéséhez ezeket használd, ne a feltételezést.

Ha eszközök állnak rendelkezésre, a tervezés menete:
1. Ha a kérdéshez a sémarészletben nem szereplő tábla vagy oszlop kell, keresd meg a search_schema és describe_table eszközzel. Ne találd ki az oszlopnevet.
2. Cégnevet mindig a resolve_entity eszközzel oldj fel, és add meg a szerepet; a számlázott vevőt és a végfelhasználót soha ne mosd össze.
3. Bizonytalan join, dátumformátum vagy kódérték esetén futtass probe_query próbalekérdezést, és a visszakapott adatból dolgozz tovább.
4. Végül a submit_plan eszközzel add be a végleges tervet. Csak ezzel fejezd be, szöveges válasszal ne.
Ne hívj eszközt olyan információért, amit a sémarészlet vagy az üzleti tudás már megad.

Helyi, verziózott ERP-tudás:
"#,
        );
        instructions.push_str(&knowledge_prompt_context());
        instructions.push_str(
            r#"Kizárólag JSON objektummal válaszolj ebben az alakban:
{"sql":"...","title":"rövid magyar cím","visualization":"table|bar|line","maxRows":50}"#,
        );
        instructions
    })
}

/// A kérdésben kifejezetten megnevezett naptári évek.
fn requested_years(question: &str) -> Vec<i64> {
    let mut years = Regex::new(r"\b(19[89]\d|20[0-9]\d)\b")
        .expect("valid year regex")
        .find_iter(question)
        .filter_map(|found| found.as_str().parse::<i64>().ok())
        .collect::<Vec<_>>();
    years.sort_unstable();
    years.dedup();
    years
}

/// Eldönti, hogy a kérdés kilép-e a beállított elemzési időablakból.
/// Korábban a planner azt az utasítást kapta, hogy az ablakon kívüli
/// tranzakciót akkor se használja, ha a felhasználó kifejezetten kéri —
/// ettől az app csendben rossz vagy üres választ adott.
struct PeriodScope {
    window_start_year: i64,
    window_end_year: i64,
    outside_years: Vec<i64>,
}

impl PeriodScope {
    fn resolve(question: &str, analysis_fy_window: u8) -> Self {
        let (window_start_year, window_end_year) = fy_window_years(analysis_fy_window);
        let outside_years = requested_years(question)
            .into_iter()
            .filter(|year| *year < window_start_year || *year > window_end_year)
            .collect();
        Self {
            window_start_year,
            window_end_year,
            outside_years,
        }
    }

    fn years_label(&self) -> String {
        self.outside_years
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A planner időszak-szabálya. Ha a felhasználó kifejezetten az ablakon
    /// kívüli évet kért, azt az ablak felülírása helyett ki kell szolgálni.
    fn planner_rule(&self, analysis_fy_window: u8, fiscal_year_start: &str) -> String {
        if self.outside_years.is_empty() {
            format!(
                "- a beállított elemzési időablak {analysis_fy_window} FY ({}–{}); ennek kezdete {fiscal_year_start};\n\
                 - külön megnevezett időszak hiányában ettől a kezdettől szűrj;\n\
                 - ha a kérdés nem nevez meg időszakot, az időablaknál régebbi tranzakciót ne használj;",
                self.window_start_year, self.window_end_year
            )
        } else {
            format!(
                "- a beállított elemzési időablak {analysis_fy_window} FY ({}–{}); a kérdésben ez a négyjegyű szám szerepel: {};\n\
                 - ha ez a szám időszakra utal, akkor FELÜLÍRJA az időablakot: pontosan arra az időszakra szűrj, ne szűkítsd az ablakra, és azon kívüli rekordot ne olvass be;\n\
                 - ha a szám nem időszak (például mennyiség, cikkszám vagy azonosító), hagyd figyelmen kívül, és az időablakot használd;",
                self.window_start_year,
                self.window_end_year,
                self.years_label()
            )
        }
    }

    /// Az összefoglaló ebből tudja meg, mekkora időszakot látott valójában.
    fn summary_note(&self, analysis_fy_window: u8) -> String {
        if self.outside_years.is_empty() {
            format!(
                "Időszak: a kérdés nem nevezett meg az időablakon kívüli évet, ezért a {analysis_fy_window} FY ablak ({}–{}) érvényes. A záró {} év még nem zárt teljes év, ezért a részleges évet trendösszehasonlításnál mondd ki.",
                self.window_start_year, self.window_end_year, self.window_end_year
            )
        } else {
            format!(
                "Időszak: a kérdésben szerepel a(z) {} szám, ami az elemzési {analysis_fy_window} FY ablakon ({}–{}) kívüli évre utalhat. Ha a lekérdezés tényleg arra az időszakra szűrt, akkor az időablak nem szűkítette az eredményt — a lefuttatott SQL WHERE feltételéből ellenőrizd, és a válaszban a tényleges időszakot mondd ki.",
                self.years_label(),
                self.window_start_year,
                self.window_end_year
            )
        }
    }
}

/// Mindaz, amiből az összefoglaló meg tudja ítélni, hogy a lefuttatott
/// lekérdezés tényleg a feltett kérdést válaszolja-e meg.
struct SummaryContext<'a> {
    question: &'a str,
    title: &'a str,
    sql: &'a str,
    columns: &'a [String],
    rows: &'a [JsonValue],
    truncated: bool,
    analysis_fy_window: u8,
    fy_window_start: String,
    /// Kimondja, ha a kérdés az időablakon kívülre mutatott.
    period_note: String,
    /// A feloldott vevő/végfelhasználó kódjai, ha volt feloldás.
    entity_note: Option<String>,
    /// Korábbi, kifejezetten az összefoglalóra adott javítások (2. réteg).
    summary_pitfalls: String,
    history: &'a [HistoryMessage],
}

fn recent_history_text(
    history: &[HistoryMessage],
    turns: usize,
    characters_per_turn: usize,
) -> String {
    let mut recent = history
        .iter()
        .filter(|message| message.role == "user" || message.role == "assistant")
        .rev()
        .take(turns)
        .collect::<Vec<_>>();
    recent.reverse();
    recent
        .into_iter()
        .map(|message| {
            let compact = message
                .content
                .chars()
                .take(characters_per_turn)
                .collect::<String>();
            format!("{}: {compact}", message.role)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Naptári dátum az epoch-másodpercből, Howard Hinnant `civil_from_days`
/// algoritmusával. Csak ezért nem érdemes dátumkönyvtárat behúzni.
fn civil_date_from_unix(seconds: i64) -> (i64, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

fn today_date() -> (i64, u32, u32) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    civil_date_from_unix(seconds)
}

/// Az elemzési időablak első és utolsó naptári éve. Az ERP az FY-t naptári
/// évként kezeli, ezért a záró év a folyó, még nem zárt év.
fn fy_window_years(analysis_fy_window: u8) -> (i64, i64) {
    let (current_year, _, _) = today_date();
    (
        current_year - i64::from(analysis_fy_window.saturating_sub(1)),
        current_year,
    )
}

/// A MySQL a DECIMAL és DATE oszlopokat szövegként adja vissza, ezért a
/// számértéket a JSON-stringekből is ki kell nyerni.
fn numeric_value(value: &JsonValue) -> Option<f64> {
    match value {
        JsonValue::Number(number) => number.as_f64(),
        JsonValue::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn format_amount(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Determinisztikus, a **teljes** visszaadott eredményhalmazon számolt
/// összesítés. A modell így nem a 40 soros előnézetből próbál összegezni.
fn result_statistics(columns: &[String], rows: &[JsonValue]) -> String {
    if rows.is_empty() || columns.is_empty() {
        return "Nincs összesíthető sor.".into();
    }

    let mut lines = Vec::new();
    for column in columns {
        let values = rows
            .iter()
            .filter_map(|row| row.get(column))
            .filter(|value| !value.is_null())
            .collect::<Vec<_>>();
        if values.is_empty() {
            lines.push(format!(
                "- {column}: minden értéke NULL ({} sor)",
                rows.len()
            ));
            continue;
        }

        let numbers = values
            .iter()
            .filter_map(|value| numeric_value(value))
            .collect::<Vec<_>>();
        if numbers.len() == values.len() {
            let sum = numbers.iter().sum::<f64>();
            let min = numbers.iter().copied().fold(f64::INFINITY, f64::min);
            let max = numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let average = sum / numbers.len() as f64;
            lines.push(format!(
                "- {column} (numerikus, {} érték): összeg {}, átlag {}, min {}, max {}",
                numbers.len(),
                format_amount(sum),
                format_amount(average),
                format_amount(min),
                format_amount(max)
            ));
            continue;
        }

        let texts = values
            .iter()
            .map(|value| match value {
                JsonValue::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>();
        let distinct = texts.iter().collect::<HashSet<_>>();
        let mut sorted = texts.clone();
        sorted.sort();
        let mut line = format!(
            "- {column} (szöveges, {} érték, {} különböző)",
            texts.len(),
            distinct.len()
        );
        if distinct.len() <= 8 {
            let mut labels = distinct.into_iter().cloned().collect::<Vec<_>>();
            labels.sort();
            line.push_str(&format!(": {}", labels.join(" | ")));
        } else if let (Some(first), Some(last)) = (sorted.first(), sorted.last()) {
            line.push_str(&format!(": rendezve {first} … {last}"));
        }
        lines.push(line);
    }

    format!(
        "Sorok száma: {}\nOszloponkénti összesítés a TELJES visszaadott eredményen:\n{}",
        rows.len(),
        lines.join("\n")
    )
}

/// A lekérdezés kudarcának oka. Csak a `Sql` változatot érdemes a modellel
/// javíttatni; a kapcsolati és időkorlát-hibák újratervezéssel nem múlnak el.
enum QueryFailure {
    Sql(String),
    Infrastructure(String),
}

impl QueryFailure {
    fn message(self) -> String {
        match self {
            Self::Sql(message) | Self::Infrastructure(message) => message,
        }
    }
}

async fn run_query(
    settings: &StoredSettings,
    password: String,
    sql: &str,
    max_rows: usize,
) -> Result<(Vec<String>, Vec<JsonValue>, bool), String> {
    run_query_classified(settings, password, sql, max_rows)
        .await
        .map_err(QueryFailure::message)
}

async fn run_query_classified(
    settings: &StoredSettings,
    password: String,
    sql: &str,
    max_rows: usize,
) -> Result<(Vec<String>, Vec<JsonValue>, bool), QueryFailure> {
    let normalized = assert_read_only_sql(sql).map_err(QueryFailure::Sql)?;
    let applied_limit = max_rows.clamp(1, MAX_RESULT_ROWS);
    let wrapped = format!(
        "SELECT * FROM ({normalized}) AS ergo_readonly_query LIMIT {}",
        applied_limit + 1
    );
    let pool = cached_database_pool(settings, &password);
    let mut connection = timeout(Duration::from_secs(12), pool.get_conn())
        .await
        .map_err(|_| {
            QueryFailure::Infrastructure(
                "Az adatbázis-kapcsolat időtúllépés miatt megszakadt.".into(),
            )
        })?
        .map_err(|error| {
            QueryFailure::Infrastructure(format!("A MySQL-szerver nem érhető el: {error}"))
        })?;
    connection
        .query_drop("START TRANSACTION READ ONLY")
        .await
        .map_err(|error| {
            QueryFailure::Infrastructure(format!(
                "A csak olvasható tranzakció nem indítható: {error}"
            ))
        })?;

    let started = timeout(Duration::from_secs(30), async {
        let mut query_result = connection.query_iter(wrapped).await?;
        // Az oszlopneveket a sorok begyűjtése előtt kell kiolvasni: üres
        // eredménynél csak így marad meg, mit is kérdeztünk le.
        let columns = query_result
            .columns_ref()
            .iter()
            .map(|column| column.name_str().into_owned())
            .collect::<Vec<_>>();
        let rows = query_result.collect::<Row>().await?;
        Ok::<_, mysql_async::Error>((columns, rows))
    })
    .await;

    let (columns, rows) = match started {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => {
            connection.query_drop("ROLLBACK").await.ok();
            return Err(QueryFailure::Sql(format!(
                "Az ERP-lekérdezés sikertelen: {error}"
            )));
        }
        Err(_) => {
            connection.query_drop("ROLLBACK").await.ok();
            return Err(QueryFailure::Infrastructure(
                "Az ERP-lekérdezés 30 másodperces időkorlátja lejárt.".into(),
            ));
        }
    };
    connection.query_drop("COMMIT").await.ok();
    drop(connection);

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

/// A Responses API-t használó modellcsalád. Az eszközös tervezés ennek a
/// function callingjára épül, ezért a chat/completions modellek a korábbi
/// egylövésű úton maradnak.
fn uses_responses_api(settings: &StoredSettings) -> bool {
    settings.ai_model.starts_with("gpt-5") || settings.ai_model.starts_with("gpt-6")
}

/// A reasoning modellek a `max_output_tokens` plafont a belső gondolkodással
/// együtt költik el, és csak explicit `effort` mellett kiszámítható a budget.
fn supports_reasoning_effort(model: &str) -> bool {
    let reasoning_family = model.starts_with("gpt-5")
        || model.starts_with("gpt-6")
        || model
            .strip_prefix('o')
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(|character| character.is_ascii_digit());
    reasoning_family && !model.contains("chat")
}

#[derive(Clone, Copy, Debug)]
struct AiCallOptions {
    json_mode: bool,
    reasoning_effort: &'static str,
    max_output_tokens: u32,
}

impl AiCallOptions {
    const fn planner() -> Self {
        Self {
            json_mode: true,
            reasoning_effort: "medium",
            max_output_tokens: PLANNER_MAX_OUTPUT_TOKENS,
        }
    }

    /// A javító kör már egyszer elhasalt az ellenőrzésen, ezért több
    /// gondolkodást kap, mint az első terv.
    const fn repair() -> Self {
        Self {
            reasoning_effort: "high",
            ..Self::planner()
        }
    }

    const fn summary() -> Self {
        Self {
            json_mode: false,
            reasoning_effort: "low",
            max_output_tokens: SUMMARY_MAX_OUTPUT_TOKENS,
        }
    }
}

/// `None`, ha a válasz azért lett csonka, mert elfogyott a kimeneti budget.
/// Ilyenkor a hívó megemelt plafonnal újrapróbálja.
enum AiCallOutcome {
    Complete(AiCallResult),
    OutOfOutputBudget,
}

/// Agentikus tervezés: a modell eszközökkel nézhet utána a sémának, a
/// kódértékeknek és a tényleges adatnak, mielőtt végleges tervet ad.
///
/// Ez zárja be a szakadékot az egylövésű pipeline és a plugin-változat
/// között: utóbbi pontosan ezért adott jobb válaszokat ugyanazzal a
/// modellel. A biztonsági határ nem változik — a végleges SQL ugyanazon a
/// `validate_planned_sql` ellenőrzésen megy át, a próbalekérdezések pedig
/// a `assert_read_only_sql` őr mögött futnak.
async fn plan_with_tools(
    settings: &StoredSettings,
    api_key: &str,
    system: &str,
    planner_user: &str,
    catalog: &SchemaCatalog,
    password: &str,
) -> Result<(QueryPlan, TokenUsage), String> {
    let endpoint = format!("{}/responses", settings.ai_base_url.trim_end_matches('/'));
    let options = AiCallOptions::planner();
    let mut input = vec![json!({ "role": "user", "content": planner_user })];
    let mut usage = TokenUsage::default();
    let mut probes_left = MAX_PROBE_QUERIES;

    for round in 1..=MAX_PLANNER_TOOL_ROUNDS {
        // Az utolsó körben a kötelező eszközhívás már csak a terv leadása
        // lehet; különben a modell korlátlanul keresgélhet a sémában.
        let available_tools = planner_tools_for_round(round);
        let mut request = json!({
            "model": settings.ai_model,
            "instructions": system,
            "input": input,
            "tools": available_tools,
            "tool_choice": "required",
            "max_output_tokens": options.max_output_tokens,
            "store": false
        });
        if supports_reasoning_effort(&settings.ai_model) {
            request["reasoning"] = json!({ "effort": options.reasoning_effort });
        }

        let response = post_ai_request(api_key, &endpoint, &request).await?;
        usage.add(&responses_usage(&response));

        match response["status"].as_str() {
            Some("incomplete") => {
                return Err(format!(
                    "Az AI a tervezés {round}. körében nem fejezte be a választ ({}).",
                    response["incomplete_details"]["reason"]
                        .as_str()
                        .unwrap_or("ismeretlen ok")
                ));
            }
            Some("failed") => {
                return Err(format!(
                    "Az AI-hívás sikertelen a tervezés {round}. körében: {}",
                    response["error"]["message"]
                        .as_str()
                        .unwrap_or("ismeretlen hiba")
                ));
            }
            _ => {}
        }

        let output = response["output"].as_array().cloned().unwrap_or_default();
        let calls = output
            .iter()
            .filter(|item| item["type"].as_str() == Some("function_call"))
            .collect::<Vec<_>>();
        if calls.is_empty() {
            return Err(
                "Az AI nem adott végleges lekérdezési tervet és eszközt sem hívott.".into(),
            );
        }

        // A végleges terv zárja a ciklust, a többi hívás a feltárás.
        if let Some(submitted) = calls
            .iter()
            .find(|call| call["name"].as_str() == Some("submit_plan"))
        {
            let arguments = submitted["arguments"].as_str().unwrap_or("{}");
            let plan: QueryPlan = extract_json(arguments)?;
            return Ok((plan, usage));
        }

        // A következő kör bemenetének tartalmaznia kell a modell saját
        // eszközhívásait, különben nem tudja hova kötni az eredményt.
        // Csak azokat: `store: false` mellett a reasoning elem visszaadása
        // titkosított tartalom nélkül érvénytelen bemenet.
        input.extend(calls.iter().map(|call| (*call).clone()));
        for call in calls {
            let name = call["name"].as_str().unwrap_or_default();
            let arguments = call["arguments"].as_str().unwrap_or("{}");
            let result = execute_planner_tool(
                name,
                arguments,
                catalog,
                settings,
                password,
                &mut probes_left,
            )
            .await;
            input.push(json!({
                "type": "function_call_output",
                "call_id": call["call_id"].as_str().unwrap_or_default(),
                "output": result
            }));
        }
    }

    Err(format!(
        "Az AI {MAX_PLANNER_TOOL_ROUNDS} körben sem jutott el a végleges lekérdezési tervig."
    ))
}

/// A planner eszközei. Eddig a modell egyetlen lövésből, a kulcsszavas
/// heurisztika által kiválasztott sémarészletből dolgozott: ha a válogatás
/// tévedett, esélye sem volt jó tervet adni. Ezekkel az eszközökkel maga
/// nézhet utána a sémának, a kódértékeknek és a tényleges adatnak.
fn planner_tools() -> JsonValue {
    json!([
        {
            "type": "function",
            "name": "search_schema",
            "description": "Táblák és oszlopok keresése a teljes adatbázis-katalógusban. Rövid, célzott kifejezéseket adj meg (például \"invoice invoiceline itemnumber\"), ne a teljes kérdést.",
            "strict": true,
            "parameters": {
                "type": "object",
                "properties": {
                    "terms": {
                        "type": "string",
                        "description": "Szóközzel elválasztott keresőkifejezések tábla- és oszlopnevekre."
                    }
                },
                "required": ["terms"],
                "additionalProperties": false
            }
        },
        {
            "type": "function",
            "name": "describe_table",
            "description": "Egy tábla összes oszlopa, típusa, kulcsai, idegen kulcsai és a kis kardinalitású mezők tényleges mintaértékei.",
            "strict": true,
            "parameters": {
                "type": "object",
                "properties": {
                    "table": { "type": "string", "description": "A tábla pontos neve." }
                },
                "required": ["table"],
                "additionalProperties": false
            }
        },
        {
            "type": "function",
            "name": "resolve_entity",
            "description": "Cégnév feloldása ellenőrzött ERP-kódokra. A számlázott vevő és a végfelhasználó két külön törzs, ezért a szerepet meg kell adni.",
            "strict": true,
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "A keresett cég neve a kérdésből." },
                    "role": {
                        "type": "string",
                        "enum": ["end_customer", "billed_customer"],
                        "description": "end_customer = tényleges végfelhasználó, billed_customer = számlázott vevő/viszonteladó."
                    }
                },
                "required": ["name", "role"],
                "additionalProperties": false
            }
        },
        {
            "type": "function",
            "name": "submit_plan",
            "description": "A VÉGLEGES lekérdezési terv beadása. Csak akkor hívd, ha az oszlopneveket és a szükséges kódértékeket már ellenőrizted. Ezzel a tervezés befejeződik.",
            "strict": true,
            "parameters": {
                "type": "object",
                "properties": {
                    "sql": {
                        "type": "string",
                        "description": "Egyetlen SELECT vagy WITH utasítás, záró pontosvessző nélkül."
                    },
                    "title": {
                        "type": "string",
                        "description": "Rövid cím a kérdés nyelvén."
                    },
                    "visualization": {
                        "type": "string",
                        "enum": ["table", "bar", "line"]
                    },
                    "maxRows": { "type": "integer", "minimum": 1, "maximum": 200 }
                },
                "required": ["sql", "title", "visualization", "maxRows"],
                "additionalProperties": false
            }
        },
        {
            "type": "function",
            "name": "probe_query",
            "description": "Próbalekérdezés futtatása a tervezés közben, legfeljebb néhány soros eredménnyel. Arra való, hogy ellenőrizd a join-t, a dátumformátumot vagy a kódértékeket, MIELŐTT végleges tervet adsz. Csak SELECT vagy WITH.",
            "strict": true,
            "parameters": {
                "type": "object",
                "properties": {
                    "sql": {
                        "type": "string",
                        "description": "Egyetlen SELECT vagy WITH utasítás, záró pontosvessző nélkül."
                    }
                },
                "required": ["sql"],
                "additionalProperties": false
            }
        }
    ])
}

fn planner_tools_for_round(round: usize) -> Vec<JsonValue> {
    let tools = planner_tools().as_array().cloned().unwrap_or_default();
    if round == MAX_PLANNER_TOOL_ROUNDS {
        tools
            .into_iter()
            .filter(|tool| tool["name"].as_str() == Some("submit_plan"))
            .collect()
    } else {
        tools
    }
}

/// Egy eszközhívás végrehajtása. A visszaadott szöveg a modellhez kerül,
/// ezért a hibát is olvasható mondatként adjuk vissza, nem hibaként felfelé:
/// a modellnek abból kell tanulnia, nem a kérdésnek elhasalnia.
async fn execute_planner_tool(
    name: &str,
    arguments: &str,
    catalog: &SchemaCatalog,
    settings: &StoredSettings,
    password: &str,
    probes_left: &mut usize,
) -> String {
    let parsed: JsonValue = match serde_json::from_str(arguments) {
        Ok(parsed) => parsed,
        Err(error) => return format!("Az eszköz paraméterei nem értelmezhetők: {error}"),
    };

    match name {
        "search_schema" => {
            let terms = parsed["terms"].as_str().unwrap_or_default();
            search_schema_tool(catalog, terms)
        }
        "describe_table" => {
            let table = parsed["table"].as_str().unwrap_or_default();
            describe_table_tool(catalog, table)
        }
        "resolve_entity" => {
            let entity_name = parsed["name"].as_str().unwrap_or_default();
            let role = parsed["role"].as_str().unwrap_or_default();
            let resolved = if role == "billed_customer" {
                resolve_billed_customer(settings, password.to_string(), entity_name).await
            } else {
                resolve_end_customer(settings, password.to_string(), entity_name).await
            };
            match resolved {
                Err(error) => format!("A feloldás nem sikerült: {error}"),
                Ok(matches) if matches.is_empty() => {
                    format!("Nincs találat erre a névre: {entity_name}")
                }
                Ok(matches) => matches
                    .iter()
                    .take(10)
                    .map(|(code, label)| format!("{label} -> kód: {code}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            }
        }
        "probe_query" => {
            if *probes_left == 0 {
                return "Elérted a próbalekérdezések számának korlátját. Add meg a végleges tervet a már összegyűjtött információból.".into();
            }
            *probes_left -= 1;
            let sql = parsed["sql"].as_str().unwrap_or_default();
            // Szándékosan csak a read-only őr fut, nem az üzleti validátor:
            // egy próbalekérdezés (például `SELECT DISTINCT posted FROM
            // invoice`) jogosan nem felel meg az árbevétel-szabályoknak.
            // A VÉGLEGES terv továbbra is átmegy a teljes ellenőrzésen.
            match run_query_classified(settings, password.to_string(), sql, PROBE_QUERY_ROWS).await
            {
                Err(failure) => format!("A próbalekérdezés sikertelen: {}", failure.message()),
                Ok((columns, rows, truncated)) => {
                    let preview = serde_json::to_string(&rows).unwrap_or_default();
                    format!(
                        "Oszlopok: {}\nSorok: {}{}\n{}",
                        columns.join(", "),
                        rows.len(),
                        if truncated { " (csonkolt)" } else { "" },
                        preview
                            .chars()
                            .take(PROBE_RESULT_CHARACTERS)
                            .collect::<String>()
                    )
                }
            }
        }
        unknown => format!("Nincs ilyen eszköz: {unknown}"),
    }
}

fn search_schema_tool(catalog: &SchemaCatalog, terms: &str) -> String {
    let needles = normalize_for_search(terms)
        .split_whitespace()
        .filter(|term| term.len() >= 3)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if needles.is_empty() {
        return "Adj meg legalább egy, három karakternél hosszabb keresőkifejezést.".into();
    }

    let mut hits = catalog
        .tables
        .iter()
        .filter_map(|table| {
            let table_name = table.name.to_lowercase();
            let name_score = needles
                .iter()
                .filter(|needle| table_name.contains(needle.as_str()))
                .count()
                * 100;
            let matching_columns = table
                .columns
                .iter()
                .filter(|column| {
                    let column_name = column.name.to_lowercase();
                    needles
                        .iter()
                        .any(|needle| column_name.contains(needle.as_str()))
                })
                .collect::<Vec<_>>();
            let score = name_score + matching_columns.len() * 10;
            (score > 0).then_some((score, table, matching_columns))
        })
        .collect::<Vec<_>>();
    hits.sort_by_key(|(score, table, _)| (Reverse(*score), table.name.clone()));
    if hits.is_empty() {
        return "Nincs találat. Próbálj más kifejezést, vagy kérd a describe_table eszközzel egy konkrét tábla leírását.".into();
    }

    hits.truncate(SEARCH_SCHEMA_MAX_TABLES);
    hits.into_iter()
        .map(|(_, table, matching_columns)| {
            let columns = if matching_columns.is_empty() {
                "a találat a táblanévre illeszkedik; a describe_table adja az oszlopokat"
                    .to_string()
            } else {
                matching_columns
                    .iter()
                    .take(SEARCH_SCHEMA_MAX_COLUMNS)
                    .map(|column| format!("{}:{}", column.name, column.column_type))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let (category, description) = table_business_metadata(&table.name);
            format!(
                "[{category}] {}{} — illeszkedő oszlopok: {columns}",
                table.name,
                description
                    .map(|text| format!(" ({text})"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn describe_table_tool(catalog: &SchemaCatalog, table_name: &str) -> String {
    let Some(table) = catalog
        .tables
        .iter()
        .find(|table| table.name.eq_ignore_ascii_case(table_name))
    else {
        return format!("Nincs ilyen tábla: {table_name}. Keress rá a search_schema eszközzel.");
    };

    let (category, description) = table_business_metadata(&table.name);
    let mut text = format!("[{category}] {}", table.name);
    if let Some(description) = description {
        text.push_str(&format!(" — {description}"));
    }
    if let Some(rows) = table.estimated_rows {
        text.push_str(&format!(" (~{rows} sor)"));
    }
    text.push_str("\nOszlopok:\n");
    for column in &table.columns {
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
        {
            flags.push(format!(
                "FK->{}.{}",
                relationship.referenced_table, relationship.referenced_column
            ));
        }
        if column.nullable {
            flags.push("NULL".to_string());
        }
        text.push_str(&format!("- {}:{}", column.name, column.column_type));
        if !flags.is_empty() {
            text.push_str(&format!(" [{}]", flags.join(",")));
        }
        if !column.samples.is_empty() {
            text.push_str(&format!(" példaértékek: {}", column.samples.join(" | ")));
        }
        text.push('\n');
    }
    text
}

/// A nyers HTTP-réteg: a tool-ciklus és az egylövésű hívás is ezt használja.
async fn post_ai_request(
    api_key: &str,
    endpoint: &str,
    request: &JsonValue,
) -> Result<JsonValue, String> {
    let response = timeout(
        AI_REQUEST_TIMEOUT,
        http_client()
            .post(endpoint)
            .bearer_auth(api_key)
            .json(request)
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

    serde_json::from_str(&body)
        .map_err(|error| format!("Az AI-válasz formátuma érvénytelen: {error}"))
}

fn responses_usage(response_json: &JsonValue) -> TokenUsage {
    TokenUsage {
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
    }
}

async fn call_ai(
    settings: &StoredSettings,
    api_key: &str,
    system: &str,
    user: &str,
    options: AiCallOptions,
) -> Result<AiCallResult, String> {
    match call_ai_once(settings, api_key, system, user, options).await? {
        AiCallOutcome::Complete(result) => Ok(result),
        AiCallOutcome::OutOfOutputBudget => {
            let retried = AiCallOptions {
                max_output_tokens: options.max_output_tokens * MAX_OUTPUT_TOKEN_RETRY_FACTOR,
                ..options
            };
            match call_ai_once(settings, api_key, system, user, retried).await? {
                AiCallOutcome::Complete(result) => Ok(result),
                AiCallOutcome::OutOfOutputBudget => Err(format!(
                    "Az AI {} tokenes kimeneti korlátot is elérte a gondolkodással együtt, ezért nem adott teljes választ. Válassz kisebb reasoning igényű modellt, vagy szűkítsd a kérdést.",
                    retried.max_output_tokens
                )),
            }
        }
    }
}

async fn call_ai_once(
    settings: &StoredSettings,
    api_key: &str,
    system: &str,
    user: &str,
    options: AiCallOptions,
) -> Result<AiCallOutcome, String> {
    let AiCallOptions {
        json_mode,
        reasoning_effort,
        max_output_tokens,
    } = options;
    let uses_responses_api = uses_responses_api(settings);
    let endpoint = if uses_responses_api {
        format!("{}/responses", settings.ai_base_url.trim_end_matches('/'))
    } else {
        format!(
            "{}/chat/completions",
            settings.ai_base_url.trim_end_matches('/')
        )
    };
    let reasoning_effort =
        supports_reasoning_effort(&settings.ai_model).then_some(reasoning_effort);
    let responses_input = responses_input(user, json_mode);
    let mut request = if uses_responses_api {
        json!({
            "model": settings.ai_model,
            "instructions": system,
            "input": responses_input,
            "max_output_tokens": max_output_tokens,
            "store": false
        })
    } else {
        json!({
            "model": settings.ai_model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ],
            "max_completion_tokens": max_output_tokens
        })
    };
    if let Some(effort) = reasoning_effort {
        if uses_responses_api {
            request["reasoning"] = json!({ "effort": effort });
        } else {
            request["reasoning_effort"] = json!(effort);
        }
    }
    if json_mode && uses_responses_api {
        request["text"] = json!({
            "format": {
                "type": "json_schema",
                "name": "erp_query_plan",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": {
                        "sql": { "type": "string" },
                        "title": { "type": "string" },
                        "visualization": {
                            "type": "string",
                            "enum": ["table", "bar", "line"]
                        },
                        "maxRows": { "type": "integer", "minimum": 1, "maximum": 200 }
                    },
                    "required": ["sql", "title", "visualization", "maxRows"],
                    "additionalProperties": false
                }
            }
        });
    } else if json_mode {
        request["response_format"] = json!({ "type": "json_object" });
    }

    let response_json = post_ai_request(api_key, &endpoint, &request).await?;
    let (content, usage) = if uses_responses_api {
        // A reasoning modell a kimeneti budgetet a belső gondolkodásra is
        // költi, ezért a csonka választ a szöveg kiolvasása előtt kell
        // észrevenni: ilyenkor az `output` csak reasoning elemet tartalmaz.
        match response_json["status"].as_str() {
            Some("incomplete") => {
                let reason = response_json["incomplete_details"]["reason"].as_str();
                if reason == Some("max_output_tokens") {
                    return Ok(AiCallOutcome::OutOfOutputBudget);
                }
                return Err(format!(
                    "Az AI nem fejezte be a választ ({}).",
                    reason.unwrap_or("ismeretlen ok")
                ));
            }
            Some("failed") => {
                return Err(format!(
                    "Az AI-hívás sikertelen: {}",
                    response_json["error"]["message"]
                        .as_str()
                        .unwrap_or("ismeretlen hiba")
                ));
            }
            _ => {}
        }
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
        (content, responses_usage(&response_json))
    } else {
        if response_json["choices"][0]["finish_reason"].as_str() == Some("length") {
            return Ok(AiCallOutcome::OutOfOutputBudget);
        }
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
    Ok(AiCallOutcome::Complete(AiCallResult { content, usage }))
}

async fn summarize_query_result(
    settings: &StoredSettings,
    api_key: &str,
    context: SummaryContext<'_>,
) -> Result<AiCallResult, String> {
    let SummaryContext {
        question,
        title,
        sql,
        columns,
        rows,
        truncated,
        analysis_fy_window,
        fy_window_start,
        period_note,
        entity_note,
        summary_pitfalls,
        history,
    } = context;
    let preview_rows = rows.iter().take(SUMMARY_PREVIEW_ROWS).collect::<Vec<_>>();
    let full_preview = serde_json::to_string(&preview_rows)
        .map_err(|error| format!("Az eredmény nem alakítható át: {error}"))?;
    let result_preview = full_preview
        .chars()
        .take(SUMMARY_PREVIEW_CHARACTERS)
        .collect::<String>();
    let summary_system = r#"Te az ERGO, a Trans-Europe üzleti adatelemzője vagy.
Válaszolj a felhasználó kérdésének nyelvén: magyar kérdésre magyarul, angol kérdésre angolul. Kezdd a legfontosabb üzleti következtetéssel, majd támaszd alá a kapott számokkal.
Ne találj ki adatot, pénznemet, mértékegységet vagy üzleti definíciót. Ha az eredmény üres vagy kétértelmű, ezt mondd ki.
Minden pénzösszeget magyar formátumban, hármas számjegycsoportokkal és legfeljebb két tizedessel írj, például: 24 752 384,12.

Összegeket, átlagokat és szélsőértékeket KIZÁRÓLAG a megadott "Összesítés" blokkból vegyél: azt a teljes eredményhalmazon számoltuk ki. A soronkénti előnézet csak illusztráció, abból soha ne összegezz.
Nézd meg a lefuttatott SQL-t, és ha az nem a kérdést válaszolja meg — más időszakot, más szerepet vagy más mutatót szűr —, ezt mondd ki a válasz elején, ne értelmezd bele a kért jelentést.
Ha az eredmény üres, ne elégedj meg annyival, hogy "nincs adat": írd le az SQL alapján, pontosan mire szűrtünk, és mi a legvalószínűbb oka (időszak, cégszám, szerep vagy entitáskód).

A terjedelmet a kérdés szabja meg: ténykérdésre 2-4 mondat, elemző vagy összehasonlító kérdésre legfeljebb 400 szó. Ne told fel a választ üres általánossággal. Jelezd, ha a látható eredmény korlátozott. Markdown használható."#;
    let history_text = recent_history_text(history, 3, 1_200);
    let mut summary_user = format!(
        r#"Eredeti kérdés: {question}

Lefuttatott lekérdezés címe: {title}
Lefuttatott SQL:
{sql}

Eredmény oszlopai: {}
Beállított elemzési időablak: {analysis_fy_window} FY, kezdete {fy_window_start}.
{period_note}
Adatbázis-lekérdezés csonkolt (a sorlimit miatt): {truncated}

Összesítés (determinisztikusan, a teljes visszaadott eredményen számolva — ezeket a számokat használd):
{}

Soronkénti előnézet, az első {} sor (JSON, legfeljebb {} karakter), csak illusztrációként:
{}"#,
        if columns.is_empty() {
            "(nincs)".to_string()
        } else {
            columns.join(", ")
        },
        result_statistics(columns, rows),
        SUMMARY_PREVIEW_ROWS,
        SUMMARY_PREVIEW_CHARACTERS,
        result_preview,
    );
    if let Some(entity_note) = entity_note {
        summary_user.push_str(&format!("\n\nFeloldott entitás: {entity_note}"));
    }
    if !summary_pitfalls.is_empty() {
        summary_user.push_str(&format!("\n\n{summary_pitfalls}"));
    }
    if sql.to_ascii_lowercase().contains("invoiceallocationline") {
        summary_user.push_str("\n\nKötelező értelmezési korlát: ez kizárólag a könyvelt bejövő számlák nettó alapdevizás felosztása, nem a cég teljes működési költsége. Az 5-ös és 8-as főkönyvi számlaosztályra kerülő részek költség-/ráfordításjelöltek, nem auditált OPEX. A bérek, értékcsökkenés és más, számla nélkül könyvelt tételek hiányozhatnak. A számlaosztályok közötti átsorolásokat és előjeleket külön főkönyvi egyeztetés nélkül ne értelmezd végleges költségként. Ezt a korlátot a válasz első bekezdésében mondd ki.");
    }
    if !history_text.is_empty() {
        summary_user.push_str(&format!(
            "\n\nKorábbi beszélgetés (a követő kérdések értelmezéséhez):\n{history_text}"
        ));
    }
    call_ai(
        settings,
        api_key,
        summary_system,
        &summary_user,
        AiCallOptions::summary(),
    )
    .await
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
    let scope = PeriodScope::resolve(question.trim(), analysis_fy_window);
    let summary_call = summarize_query_result(
        &settings,
        &api_key,
        SummaryContext {
            question: question.trim(),
            title: &plan.title,
            sql: &sql,
            columns: &columns,
            rows: &rows,
            truncated,
            analysis_fy_window,
            fy_window_start: format!("{}-01-01", scope.window_start_year),
            period_note: scope.summary_note(analysis_fy_window),
            entity_note: None,
            summary_pitfalls: summary_pitfall_context(
                &read_feedback_records(&app).unwrap_or_default(),
                question.trim(),
            ),
            history: &[],
        },
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

/// A fix relációs tervek közös végrehajtási útja. Azért külön függvény,
/// mert a jóváhagyott aliasból érkező terv is pontosan ezt futtatja.
struct RelationshipAnalysis<'a> {
    settings: &'a StoredSettings,
    api_key: &'a str,
    mysql_password: String,
    question: &'a str,
    history: &'a [HistoryMessage],
    feedback_records: &'a [AnalysisFeedbackRecord],
    entity_note: Option<String>,
    purchase_fy_window: u8,
    plan: QueryPlan,
}

async fn finish_relationship_analysis(
    analysis: RelationshipAnalysis<'_>,
) -> Result<AnalyzeResponse, String> {
    let RelationshipAnalysis {
        settings,
        api_key,
        mysql_password,
        question,
        history,
        feedback_records,
        entity_note,
        purchase_fy_window,
        mut plan,
    } = analysis;

    if requests_chart(question) && is_customer_purchase_question(question) {
        plan.visualization = "bar".into();
    }
    let sql = validate_planned_sql(question, &plan.sql)?;
    let (columns, rows, truncated) =
        run_query(settings, mysql_password, &sql, plan.max_rows).await?;
    let scope = PeriodScope::resolve(question, purchase_fy_window);
    let summary_call = summarize_query_result(
        settings,
        api_key,
        SummaryContext {
            question,
            title: &plan.title,
            sql: &sql,
            columns: &columns,
            rows: &rows,
            truncated,
            analysis_fy_window: purchase_fy_window,
            fy_window_start: format!("{}-01-01", scope.window_start_year),
            period_note: scope.summary_note(purchase_fy_window),
            entity_note,
            summary_pitfalls: summary_pitfall_context(feedback_records, question),
            history,
        },
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
    let question = clarified_purchase_question(question.trim(), &history).unwrap_or(question);
    if let Some(summary) = capability_answer(question.trim()) {
        return Ok(AnalyzeResponse {
            summary: summary.into(),
            result: None,
        });
    }
    if let Some(clarification) = purchase_role_clarification(question.trim()) {
        return Ok(AnalyzeResponse {
            summary: clarification,
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
    let analysis_fy_window = validate_analysis_fy_window(settings.analysis_fy_window)?;
    let purchase_fy_window = purchase_fy_window(question.trim(), analysis_fy_window);
    // Legjobb szándékú betöltés: sérült vagy hiányzó memóriafájl miatt ne
    // hasaljon el a kérdés, csak maradjon el a tanult tapasztalat.
    let feedback_records = read_feedback_records(&app).unwrap_or_default();
    let billed_customer_name = explicit_billed_customer_name(question.trim());
    let mut entity_note: Option<String> = None;
    let mut relationship_plan = if let Some(search_name) = billed_customer_name {
        // 3. réteg: a jóváhagyott alias felülírja a név szerinti keresést, így
        // ugyanarra a cégre nem kérdez vissza minden alkalommal.
        if let Some((label, codes)) = approved_alias_codes(&app, &search_name, "billed_customer") {
            entity_note = Some(format!(
                "számlázott vevő „{label}” jóváhagyott aliasból, ügyfélkódok: {}",
                codes.join(", ")
            ));
            return finish_relationship_analysis(RelationshipAnalysis {
                settings: &settings,
                api_key: &api_key,
                mysql_password,
                question: question.trim(),
                history: &history,
                feedback_records: &feedback_records,
                entity_note,
                purchase_fy_window,
                plan: billed_customer_purchase_plan(
                    &label,
                    &codes,
                    purchase_fy_window,
                    is_english_question(question.trim()),
                ),
            })
            .await;
        }
        let matches =
            resolve_billed_customer(&settings, mysql_password.clone(), &search_name).await?;
        if matches.is_empty() {
            return Ok(AnalyzeResponse {
                summary: if is_english_question(question.trim()) {
                    format!("I could not find an invoiced customer named {search_name}. Please provide the exact company name, or clarify if you mean the end customer.")
                } else {
                    format!("Nem találtam számlázott vevőt ehhez a névhez: {search_name}. Kérlek, add meg a pontos céget vagy jelezd, ha végfelhasználót keresel.")
                },
                result: None,
            });
        }
        if matches.len() > 1 {
            let candidates = matches
                .iter()
                .take(5)
                .map(|(code, name)| format!("{name} ({code})"))
                .collect::<Vec<_>>()
                .join("; ");
            // Jóváhagyásra váró bejegyzés, hogy a következő alkalommal már
            // ne kelljen visszakérdezni ugyanerre a névre.
            record_pending_alias(&app, &search_name, "billed_customer", &matches);
            return Ok(AnalyzeResponse {
                summary: if is_english_question(question.trim()) {
                    format!("I found several invoiced customers matching {search_name}: {candidates}. Which one do you mean? Please use the customer code if possible. You can also approve the right one for good under Settings / Knowledge and corrections.")
                } else {
                    format!("Több számlázott vevő is illik erre: {search_name}. Találatok: {candidates}. Melyikre gondolsz? Ha lehet, add meg az ügyfélkódot. A helyes céget a Beállítások / Tudás és javítások alatt véglegesen is jóváhagyhatod.")
                },
                result: None,
            });
        }
        let label = matches[0].1.clone();
        let codes = matches
            .into_iter()
            .map(|(code, _)| code)
            .collect::<Vec<_>>();
        entity_note = Some(format!(
            "számlázott vevő „{label}”, ügyfélkódok: {}",
            codes.join(", ")
        ));
        Some(billed_customer_purchase_plan(
            &label,
            &codes,
            purchase_fy_window,
            is_english_question(question.trim()),
        ))
    } else {
        incoming_operating_cost_plan(question.trim(), analysis_fy_window)
            .or_else(|| top_end_customers_plan(question.trim(), analysis_fy_window))
            .or_else(|| fixed_relationship_plan(question.trim(), purchase_fy_window))
    };
    if relationship_plan.is_none() {
        if let Some(search_name) = explicit_end_customer_name(question.trim()) {
            if let Some((label, codes)) = approved_alias_codes(&app, &search_name, "end_customer") {
                entity_note = Some(format!(
                    "végfelhasználó „{label}” jóváhagyott aliasból, entitáskódok: {}",
                    codes.join(", ")
                ));
                let mut plan = end_customer_purchase_plan(&label, &codes, purchase_fy_window);
                if is_english_question(question.trim()) {
                    plan.title = format!(
                        "Products purchased by {label} as end customer — last {purchase_fy_window} FY"
                    );
                }
                return finish_relationship_analysis(RelationshipAnalysis {
                    settings: &settings,
                    api_key: &api_key,
                    mysql_password,
                    question: question.trim(),
                    history: &history,
                    feedback_records: &feedback_records,
                    entity_note,
                    purchase_fy_window,
                    plan,
                })
                .await;
            }
            let matches =
                resolve_end_customer(&settings, mysql_password.clone(), &search_name).await?;
            if matches.is_empty() {
                return Ok(AnalyzeResponse {
                    summary: if is_english_question(question.trim()) {
                        format!("I could not find an end customer named {search_name}. Please provide the exact company name or code.")
                    } else {
                        format!("Nem találtam végfelhasználót ehhez a névhez: {search_name}. Kérlek, add meg a pontos cégnevet vagy kódot.")
                    },
                    result: None,
                });
            }
            let known_family = knowledge_pack().entities.iter().any(|entity| {
                entity.include_token_family
                    && entity.query_aliases.iter().any(|alias| {
                        normalize_for_search(alias) == normalize_for_search(&search_name)
                    })
            });
            if matches.len() > 1 && !known_family {
                let candidates = matches
                    .iter()
                    .take(5)
                    .map(|(code, name)| format!("{name} ({code})"))
                    .collect::<Vec<_>>()
                    .join("; ");
                record_pending_alias(&app, &search_name, "end_customer", &matches);
                return Ok(AnalyzeResponse {
                    summary: if is_english_question(question.trim()) {
                        format!("I found several end customers matching {search_name}: {candidates}. Which one do you mean? You can also approve the right one for good under Settings / Knowledge and corrections.")
                    } else {
                        format!("Több végfelhasználó is illik erre: {search_name}. Találatok: {candidates}. Melyikre gondolsz? A helyes céget a Beállítások / Tudás és javítások alatt véglegesen is jóváhagyhatod.")
                    },
                    result: None,
                });
            }
            let label = matches[0].1.clone();
            let codes = matches
                .into_iter()
                .map(|(code, _)| code)
                .collect::<Vec<_>>();
            entity_note = Some(format!(
                "végfelhasználó „{label}”, entitáskódok: {}",
                codes.join(", ")
            ));
            let mut plan = end_customer_purchase_plan(&label, &codes, purchase_fy_window);
            if is_english_question(question.trim()) {
                plan.title = format!(
                    "Products purchased by {label} as end customer — last {purchase_fy_window} FY"
                );
            }
            relationship_plan = Some(plan);
        }
    }
    if let Some(plan) = relationship_plan {
        return finish_relationship_analysis(RelationshipAnalysis {
            settings: &settings,
            api_key: &api_key,
            mysql_password,
            question: question.trim(),
            history: &history,
            feedback_records: &feedback_records,
            entity_note,
            purchase_fy_window,
            plan,
        })
        .await;
    }
    let catalog = load_schema_catalog(&app, &settings, mysql_password.clone()).await?;
    let (schema_context, schema_selection) =
        select_schema_context(&catalog, question.trim(), &history);
    let history_text = recent_history_text(&history, 4, 3_500);

    let fiscal_year_start = format!(
        "MAKEDATE(YEAR(CURRENT_DATE) - {}, 1)",
        analysis_fy_window - 1
    );
    let period_scope = PeriodScope::resolve(question.trim(), analysis_fy_window);
    let relationship_skill = business_relationship_skill(question.trim());
    // A prompt-cache csak a szó szerint egyező előtagra illeszkedik, ezért a
    // kérdésfüggő rész (időszak, reláció-skill, séma) a változatlan blokk
    // UTÁN következik, nem a közepébe tűzve.
    // A tanult memória a kérdéstől függ, ezért a statikus előtag UTÁN
    // következik — különben minden kérdés cache-tévesztéssel indulna.
    let memory_context = memory_prompt_context(&feedback_records, question.trim());
    let planner_system = format!(
        "{}\n\n--- A kérdéshez tartozó rész ---\nIdőszakszabály:\n{}\n\n{relationship_skill}\n\n{memory_context}\n{schema_context}",
        planner_static_instructions(),
        period_scope.planner_rule(analysis_fy_window, &fiscal_year_start),
    );
    let planner_user = format!(
        "Korábbi beszélgetés:\n{history_text}\n\nFelhasználói kérdés:\n{}",
        question.trim()
    );
    // A Responses API-t használó modellek eszközökkel tervezhetnek: maguk
    // nézhetnek utána a sémának és a tényleges adatnak. A chat/completions
    // modellek a korábbi egylövésű úton maradnak.
    let (mut plan, mut planner_usage) = if uses_responses_api(&settings) {
        plan_with_tools(
            &settings,
            &api_key,
            &planner_system,
            &planner_user,
            &catalog,
            &mysql_password,
        )
        .await?
    } else {
        let planner_call = call_ai(
            &settings,
            &api_key,
            &planner_system,
            &planner_user,
            AiCallOptions::planner(),
        )
        .await?;
        let plan: QueryPlan = extract_json(&planner_call.content)?;
        (plan, planner_call.usage)
    };

    // Egyetlen javító ciklus a biztonsági/üzleti ellenőrzésre ÉS a MySQL
    // futási hibájára. Korábban csak a validáció kapott második esélyt, egy
    // elírt oszlopnév pedig azonnal a felhasználónál landolt.
    let mut attempt = 1;
    let (columns, rows, truncated) = loop {
        plan.max_rows = plan.max_rows.clamp(1, MAX_RESULT_ROWS);
        let attempt_outcome = match validate_planned_sql(question.trim(), &plan.sql) {
            Err(validation_error) => Err(validation_error),
            Ok(validated_sql) => {
                plan.sql = validated_sql;
                match run_query_classified(
                    &settings,
                    mysql_password.clone(),
                    &plan.sql,
                    plan.max_rows,
                )
                .await
                {
                    Ok(result) => Ok(result),
                    // A kapcsolati és időkorlát-hibát nem a terv okozta,
                    // ezért nincs értelme újratervezni.
                    Err(QueryFailure::Infrastructure(message)) => return Err(message),
                    Err(QueryFailure::Sql(message)) => Err(message),
                }
            }
        };

        match attempt_outcome {
            Ok(result) => break result,
            Err(error) => {
                if attempt >= MAX_PLAN_ATTEMPTS {
                    return Err(format!(
                        "Az AI {attempt} próbálkozás után sem adott futtatható, biztonságos és üzletileg érvényes SELECT lekérdezést: {error}"
                    ));
                }
                let repair_user = format!(
                    r#"Az előző lekérdezési terv nem futott le, vagy nem ment át a biztonsági és üzleti ellenőrzésen.
Hiba: {error}

Hibás SQL:
{}

Eredeti felhasználói kérdés:
{}

Javítsd ki a tervet. Ha a hiba ismeretlen táblára vagy oszlopra utal, kizárólag a fenti sémarészletben szereplő neveket használd. A JSON sql mezője pontosan egyetlen, záró pontosvessző nélküli SELECT vagy WITH utasítás legyen. Ne használj SET, DECLARE, ideiglenes táblát, tárolt eljárást, kommentet vagy több utasítást. Kizárólag a kért JSON objektummal válaszolj."#,
                    plan.sql,
                    question.trim()
                );
                let repaired_call = call_ai(
                    &settings,
                    &api_key,
                    &planner_system,
                    &repair_user,
                    AiCallOptions::repair(),
                )
                .await?;
                planner_usage.add(&repaired_call.usage);
                plan = extract_json(&repaired_call.content)?;
                attempt += 1;
            }
        }
    };
    if !matches!(plan.visualization.as_str(), "table" | "bar" | "line") {
        plan.visualization = "table".into();
    }
    let summary_call = summarize_query_result(
        &settings,
        &api_key,
        SummaryContext {
            question: question.trim(),
            title: &plan.title,
            sql: &plan.sql,
            columns: &columns,
            rows: &rows,
            truncated,
            analysis_fy_window,
            fy_window_start: format!("{}-01-01", period_scope.window_start_year),
            period_note: period_scope.summary_note(analysis_fy_window),
            entity_note: entity_note.clone(),
            summary_pitfalls: summary_pitfall_context(&feedback_records, question.trim()),
            history: &history,
        },
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
            product_catalog::load_product_catalog,
            product_catalog::load_product_sales,
            load_knowledge_overview,
            load_settings,
            open_analysis_feedback_email,
            open_feedback_email,
            save_analysis_feedback,
            save_settings,
            select_ai_model,
            select_analysis_fy_window,
            load_learned_aliases,
            approve_learned_alias,
            reject_learned_alias
        ])
        .run(tauri::generate_context!())
        .expect("az ERGO alkalmazás nem indítható");
}

#[cfg(test)]
mod tests {
    use super::{
        alias_matches, ambiguous_purchase_company_name, assert_read_only_sql,
        billed_customer_purchase_plan, business_relationship_skill, capability_answer,
        civil_date_from_unix, clarified_purchase_question, contains_token_phrase,
        describe_table_tool, end_customer_purchase_plan, explicit_billed_customer_name,
        explicit_end_customer_name, extract_json, fixed_quick_plan, fixed_relationship_plan,
        fy_window_years, incoming_operating_cost_plan, is_chat_model,
        is_customer_purchase_question, is_english_question, is_sampleable_column, knowledge_pack,
        knowledge_prompt_context, memory_prompt_context, memory_terms, normalize_for_search,
        percent_encode_mailto, planner_tools, planner_tools_for_round, purchase_fy_window,
        purchase_role_clarification, question_similarity, requested_top_end_customer_limit,
        requests_chart, resolve_end_customer, responses_input, result_statistics, run_query,
        schema_search_terms, search_schema_tool, select_schema_context, summary_pitfall_context,
        table_business_metadata, top_end_customers_plan, validate_analysis_fy_window,
        validate_feedback, validate_model, validate_planned_sql, AnalysisFeedbackInput,
        AnalysisFeedbackRecord, CatalogColumn, HistoryMessage, JsonValue, LearnedAlias,
        LearnedAliasCandidate, PeriodScope, QueryPlan, SchemaCatalog, SchemaTable, StoredSettings,
        MEMORY_MIN_SIMILARITY,
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
    fn incoming_invoice_operating_cost_uses_a_bounded_fixed_plan() {
        let question =
            "Elemezd ki a beérkező számlákat és próbáld meg kiszámolni a cég működési költségeit";
        let plan = incoming_operating_cost_plan(question, 3).expect("fixed cost candidate plan");
        assert!(plan.sql.contains("FROM invoiceallocationline l"));
        assert!(plan.sql.contains("l.companynumber = '1' AND l.posted = 1"));
        assert!(plan.sql.contains("l.amountbase"));
        assert!(plan.sql.contains("l.accountnumber LIKE '5%'"));
        assert!(plan.sql.contains("MAKEDATE(YEAR(CURRENT_DATE) - 2, 1)"));
        assert_eq!(plan.max_rows, 60);
        assert!(validate_planned_sql(question, &plan.sql).is_ok());
        assert!(incoming_operating_cost_plan("Mennyi volt a működési költség?", 3).is_none());
        assert!(incoming_operating_cost_plan(
            "Elemezd a bejövő számlákat és a működési költségeket 2024-ben",
            3
        )
        .is_none());
    }

    #[test]
    fn final_planning_round_only_allows_submitting_the_plan() {
        assert!(planner_tools_for_round(1).len() > 1);
        let final_tools = planner_tools_for_round(super::MAX_PLANNER_TOOL_ROUNDS);
        assert_eq!(final_tools.len(), 1);
        assert_eq!(final_tools[0]["name"], "submit_plan");
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
    fn fixed_revenue_queries_use_the_validated_business_definition() {
        for quick_analysis in ["revenue-trend", "top-customers"] {
            let sql = fixed_quick_plan(quick_analysis, 3).unwrap().sql;
            let lower = sql.to_lowercase();
            assert!(lower.contains("itemsumbase"));
            assert!(lower.contains("companynumber = '1'"));
            assert!(lower.contains("posted = 1"));
            assert!(lower.contains("debitcredit"));
            assert!(!lower.contains("vatbase"));
            assert!(!lower.contains("invoiceline"));
        }
    }

    #[test]
    fn rejects_invalid_revenue_semantics_in_ai_plans() {
        assert!(validate_planned_sql(
            "Mennyi az árbevétel?",
            "SELECT SUM(vatbase) FROM invoiceline"
        )
        .is_err());
        assert!(validate_planned_sql(
            "Mennyi az árbevétel?",
            "SELECT SUM(CASE WHEN debitcredit = 0 THEN itemsumbase ELSE -itemsumbase END) FROM invoice WHERE companynumber = '1' AND posted = 1"
        )
        .is_ok());
        assert!(validate_planned_sql("Hány ügyfél van?", "SELECT COUNT(*) FROM customer").is_ok());
    }

    #[test]
    fn customer_purchase_questions_require_end_customer_relations() {
        let question = "Miket vásárolt az Országos Bírósági Hivatal az elmúlt 3 évben?";
        assert!(validate_planned_sql(
            question,
            "SELECT il.itemnumber FROM invoice i JOIN invoiceline il ON il.invoicenumber = i.invoicenumber WHERE i.customernumber = 'obh' AND i.companynumber = '1' AND i.posted = 1"
        )
        .is_err());
        assert!(validate_planned_sql(
            question,
            "SELECT il.itemnumber, CASE WHEN i.debitcredit = 0 THEN il.linepricebase ELSE -il.linepricebase END AS netto FROM invoice i JOIN invoiceline il ON il.companynumber = i.companynumber AND il.invoicenumber = i.invoicenumber LEFT JOIN endcustomer ec ON ec.endcustomernumber = COALESCE(i.endcustomernumber, il.endcustomernumber) WHERE i.companynumber = '1' AND i.posted = 1 AND i.endcustomernumber IN ('birosag', 'orszagosbirosagi')"
        )
        .is_ok());
    }

    #[test]
    fn obh_purchase_question_has_a_fixed_relationship_plan() {
        let plan = fixed_relationship_plan(
            "Miket vásárolt az Országos Bírósági Hivatal az elmúlt 3 évben?",
            3,
        )
        .expect("OBH purchase questions must have a fixed plan");
        assert!(plan.sql.contains("'birosag', 'orszagosbirosagi'"));
        assert!(plan.sql.contains("invoice i JOIN invoiceline il"));
        assert!(plan.sql.contains("i.companynumber = '1'"));
        assert!(plan.sql.contains("i.posted = 1"));
        assert!(plan.sql.contains("i.debitcredit"));
        assert!(plan.sql.contains("il.linepricebase"));
        assert!(fixed_relationship_plan("Miket vásárolt a Designshop?", 3).is_none());
    }

    #[test]
    fn extracts_an_explicit_end_customer_from_purchase_questions() {
        assert_eq!(
            explicit_end_customer_name(
                "Miket vásárolt az Idomszoft végfelhasználó az elmúlt 3 évben?"
            ),
            Some("idomszoft".into())
        );
        assert_eq!(
            explicit_end_customer_name("Mit rendelt a Magyar Posta végfelhasználó?"),
            Some("magyar posta".into())
        );
        assert_eq!(
            explicit_end_customer_name("Mit vásárolt a MÁV végfelhasználó?"),
            Some("mav".into())
        );
        assert_eq!(
            explicit_end_customer_name(
                "Milyen cikkeket vásrolt a One végfelhasználó az elmúlt 1 évben?"
            ),
            Some("one".into())
        );
        assert_eq!(
            explicit_end_customer_name(
                "Listázd ki a 4iG által megvásárolt termékeket az elmúlt 1 évből"
            ),
            Some("4ig".into())
        );
        assert!(explicit_end_customer_name("Miket vásárolt a Designshop?").is_none());
    }

    #[test]
    fn recognizes_hungarian_and_english_purchase_wording_and_reseller_roles() {
        let hu = "Listázd ki azokat a termékeket, melyeket a DesignShop viszonteladó vásárolt idén";
        let en = "List the products that the DesignShop reseller purchased this year";
        assert!(is_customer_purchase_question(hu));
        assert!(is_customer_purchase_question(en));
        assert_eq!(explicit_billed_customer_name(hu), Some("designshop".into()));
        assert_eq!(explicit_billed_customer_name(en), Some("designshop".into()));
        assert!(purchase_role_clarification(hu).is_none());
        assert!(purchase_role_clarification(en).is_none());
        assert!(is_english_question(en));
    }

    #[test]
    fn extracts_other_company_names_in_both_languages() {
        let cases = [
            ("Miket vásárolt a Northwind viszonteladó idén?", "northwind"),
            (
                "Milyen termékeket vásárolt a Kék Duna Kft. viszonteladó?",
                "kek duna kft",
            ),
            (
                "List products purchased by the Contoso reseller this year",
                "contoso",
            ),
            (
                "What items did the Fabrikam reseller buy this year?",
                "fabrikam",
            ),
        ];
        for (question, expected) in cases {
            assert_eq!(
                explicit_billed_customer_name(question).as_deref(),
                Some(expected),
                "failed for {question}"
            );
        }
    }

    #[test]
    fn asks_before_query_when_purchase_role_is_ambiguous() {
        let question = "What products did DesignShop buy this year?";
        let clarification = purchase_role_clarification(question)
            .expect("ambiguous purchase role must trigger clarification");
        assert!(clarification.contains("invoiced reseller/customer"));
        assert!(clarification.contains("end customer"));
        assert_eq!(explicit_billed_customer_name(question), None);
        assert_eq!(explicit_end_customer_name(question), None);
        assert!(fixed_relationship_plan("Which products were bought in one year?", 1).is_none());
        assert!(purchase_role_clarification("Which products were bought in one year?").is_some());
    }

    #[test]
    fn role_answer_reuses_designshop_purchase_request_without_ai_schema_planning() {
        let original = "Listázd ki és ábrázold diagramon a DesignShop partner idei vásárlásait";
        let clarification = purchase_role_clarification(original)
            .expect("purchase noun must be recognized before AI planning");
        assert_eq!(
            ambiguous_purchase_company_name(original).as_deref(),
            Some("designshop")
        );
        let history = vec![
            HistoryMessage {
                role: "user".into(),
                content: original.into(),
            },
            HistoryMessage {
                role: "assistant".into(),
                content: clarification,
            },
        ];
        let resumed = clarified_purchase_question("Viszonteladóra gondolok", &history)
            .expect("a role answer must resume the original request");
        assert_eq!(
            explicit_billed_customer_name(&resumed).as_deref(),
            Some("designshop")
        );
        assert_eq!(purchase_fy_window(&resumed, 5), 1);
        assert!(requests_chart(&resumed));
        let plan = billed_customer_purchase_plan("DesignShop", &["designshop".into()], 1, false);
        assert!(plan.sql.contains("i.customernumber IN ('designshop')"));
        assert!(validate_planned_sql(&resumed, &plan.sql).is_ok());

        let mut old_history = history;
        old_history[1].content = "A vásárlások listázásához előbb pontosítani kell: számlázott vevőre/viszonteladóra vagy tényleges végfelhasználóra gondolsz?".into();
        assert!(clarified_purchase_question("Viszonteladóra gondolok", &old_history).is_some());
    }

    #[test]
    fn role_answer_can_resume_end_customer_or_english_purchase_request() {
        let hu = "Listázd ki a DesignShop partner idei vásárlásait";
        let hu_history = vec![
            HistoryMessage {
                role: "user".into(),
                content: hu.into(),
            },
            HistoryMessage {
                role: "assistant".into(),
                content: purchase_role_clarification(hu).unwrap(),
            },
        ];
        let resumed =
            clarified_purchase_question("Végfelhasználóra gondolok", &hu_history).unwrap();
        assert_eq!(
            explicit_end_customer_name(&resumed).as_deref(),
            Some("designshop")
        );
        assert_eq!(explicit_billed_customer_name(&resumed), None);

        let en = "Show the products DesignShop bought this year as a chart";
        let en_history = vec![
            HistoryMessage {
                role: "user".into(),
                content: en.into(),
            },
            HistoryMessage {
                role: "assistant".into(),
                content: purchase_role_clarification(en).unwrap(),
            },
        ];
        let resumed = clarified_purchase_question("The reseller", &en_history).unwrap();
        assert_eq!(
            explicit_billed_customer_name(&resumed).as_deref(),
            Some("designshop")
        );
        assert!(requests_chart(&resumed));
        assert_eq!(purchase_fy_window(&resumed, 3), 1);
        assert!(clarified_purchase_question("The reseller", &hu_history).is_some());
        assert!(clarified_purchase_question("A DesignShop másik cég", &hu_history).is_none());
    }

    #[test]
    fn recognizes_english_end_customer_and_does_not_misclassify_it_as_billed_customer() {
        let question = "What products did the One end customer buy this year?";
        assert_eq!(explicit_end_customer_name(question), Some("one".into()));
        assert_eq!(explicit_billed_customer_name(question), None);
        assert!(purchase_role_clarification(question).is_none());
    }

    #[test]
    fn billed_customer_plan_filters_invoice_customer_not_end_customer() {
        let plan = billed_customer_purchase_plan("DesignShop Kft.", &["webworx".into()], 1, true);
        assert!(plan.title.contains("invoiced customer"));
        assert!(plan.sql.contains("i.customernumber IN ('webworx')"));
        assert!(!plan.sql.contains("i.endcustomernumber IN ('webworx')"));
        assert!(plan.sql.contains("invoice i STRAIGHT_JOIN invoiceline il"));
        assert!(plan.sql.contains("i.invoicedate >= DATE_FORMAT(MAKEDATE"));
        assert!(!plan
            .sql
            .contains("STR_TO_DATE(i.invoicedate, '%Y.%m.%d') >= "));
        assert!(plan.sql.contains("i.companynumber = '1'"));
        assert!(plan.sql.contains("i.posted = 1"));
        assert!(validate_planned_sql(
            "List the products that the DesignShop reseller purchased this year",
            &plan.sql
        )
        .is_ok());
    }

    #[test]
    fn embedded_knowledge_pack_is_available_to_planning() {
        let pack = knowledge_pack();
        assert_eq!(pack.version, "1.0.2");
        assert!(pack
            .metrics
            .iter()
            .any(|metric| metric.id == "net_invoiced_revenue_base"));
        assert!(pack
            .relationships
            .iter()
            .any(|relationship| relationship.id == "actual_end_customer"));
        assert!(pack
            .skills
            .iter()
            .any(|skill| skill.id == "top_end_customers"));
        let one = pack
            .entities
            .iter()
            .find(|entity| entity.id == "one_hungary")
            .expect("One Magyarország must have an entity rule");
        assert_eq!(one.exact_codes, ["one"]);
        assert!(!one.include_token_family);
        let mav = pack
            .entities
            .iter()
            .find(|entity| entity.id == "mav_family")
            .expect("MÁV must have a company-family rule");
        assert!(mav.include_token_family);
        let four_ig = pack
            .entities
            .iter()
            .find(|entity| entity.id == "4ig_group")
            .expect("4iG must have a verified company-group rule");
        assert_eq!(four_ig.exact_codes, ["4ig", "4igzrt", "4igurvedelmi"]);
        assert!(four_ig.excluded_codes.contains(&"rheinmetall4ig".into()));
        let context = knowledge_prompt_context();
        assert!(context.contains("vatbase nem árbevétel"));
        assert!(context.contains("invoice/invoiceline -> endcustomer"));
    }

    #[test]
    fn validates_structured_analysis_feedback() {
        let valid = AnalysisFeedbackInput {
            rating: "negative".into(),
            category: Some("entity".into()),
            correction: "Az AERONAUTICA nem AERON.".into(),
            question: "Mit vásárolt az AERON végfelhasználó?".into(),
            answer: "Három találat érkezett.".into(),
            result_title: Some("AERON vásárlásai".into()),
            sql: Some("SELECT 1".into()),
            query_source: Some("fixed".into()),
            row_count: Some(3),
        };
        assert!(validate_feedback(&valid).is_ok());
        let invalid = AnalysisFeedbackInput {
            correction: String::new(),
            ..valid
        };
        assert!(validate_feedback(&invalid).is_err());
    }

    #[test]
    fn end_customer_names_match_complete_tokens_instead_of_substrings() {
        assert!(contains_token_phrase("AERON Zrt. Dorottya u. 1.", "aeron"));
        assert!(!contains_token_phrase(
            "AUTORITATEA AERONAUTICA CIVILA ROMANA",
            "aeron"
        ));
        assert!(contains_token_phrase("MÁV-START Zrt.", "mav"));
        assert!(contains_token_phrase("MÁV Szolgáltató Központ Zrt.", "mav"));
        assert!(!contains_token_phrase("MAVIR Zrt.", "mav"));
    }

    #[test]
    fn top_end_customer_questions_have_a_fixed_plan() {
        let question = "Sorold fel a top 5 végfelhasználót!";
        assert_eq!(requested_top_end_customer_limit(question), Some(5));
        let plan = top_end_customers_plan(question, 1).expect("top query must have a fixed plan");
        assert_eq!(plan.max_rows, 5);
        assert_eq!(plan.visualization, "bar");
        assert!(plan.sql.contains("invoice i JOIN invoiceline il"));
        assert!(plan.sql.contains("endcustomernumber"));
        assert!(plan.sql.contains("i.companynumber = '1'"));
        assert!(plan.sql.contains("i.posted = 1"));
        assert!(plan.sql.contains("i.debitcredit"));
        assert!(plan.sql.contains("il.linepricebase"));
        assert!(plan.sql.ends_with("LIMIT 5"));
        assert!(top_end_customers_plan("Sorold fel a top 5 ügyfelet!", 1).is_none());
    }

    #[test]
    fn obh_relationship_skill_disambiguates_the_legacy_code() {
        let skill = business_relationship_skill(
            "Miket vásárolt az Országos Bírósági Hivatal az elmúlt 3 évben?",
        );
        assert!(skill.contains("invoice.endcustomernumber"));
        assert!(skill.contains("'birosag'"));
        assert!(skill.contains("'orszagosbirosagi'"));
        assert!(skill.contains("NEM az Országos Bírósági Hivatal"));
    }

    #[test]
    fn one_hungary_relationship_plan_uses_only_the_exact_entity_code() {
        let plan = fixed_relationship_plan(
            "Milyen cikkeket vásárolt a One végfelhasználó az elmúlt 1 évben?",
            1,
        )
        .expect("One Magyarország must use a fixed relationship plan");
        assert!(plan.title.contains("One Magyarország Zrt."));
        assert!(plan.sql.contains("IN ('one')"));
        assert!(!plan.sql.contains("allforone"));
        assert!(!plan.sql.contains("euroo"));
    }

    #[test]
    fn four_ig_purchase_wording_uses_verified_end_customer_codes() {
        let plan = fixed_relationship_plan(
            "Listázd ki a 4iG által megvásárolt termékeket az elmúlt 1 évből",
            1,
        )
        .expect("4iG purchase wording must use a fixed relationship plan");
        assert!(plan.title.contains("4iG vállalatcsoport"));
        assert!(plan.sql.contains("'4ig', '4igzrt', '4igurvedelmi'"));
        assert!(!plan.sql.contains("rheinmetall4ig"));
        assert!(plan.sql.contains("invoice i JOIN invoiceline il"));
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
                if quick_analysis == "revenue-trend" {
                    let first = rows[0].as_object().expect("row must be an object");
                    assert!(first.contains_key("netto_szamlazott_arbevetel"));
                }
                if quick_analysis == "top-customers" {
                    let first = rows[0].as_object().expect("row must be an object");
                    assert_eq!(
                        first
                            .get("ugyfelszam")
                            .and_then(|value| value.as_str())
                            .map(str::to_lowercase),
                        Some("basis".into())
                    );
                    assert!(first.contains_key("netto_szamlazott_arbevetel"));
                }
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
    #[ignore = "VPN-t és helyi .env fájlt igényel"]
    fn live_obh_end_customer_relationship_returns_invoice_items() {
        let env_path = std::env::var("ERGO_ENV_PATH").expect("ERGO_ENV_PATH is required");
        let contents = std::fs::read_to_string(env_path).expect("the .env file must be readable");
        let password = contents
            .lines()
            .find_map(|line| line.trim().strip_prefix("MYSQL_PASSWORD="))
            .map(|value| value.trim().trim_matches(['\'', '"']).to_string())
            .filter(|value| !value.is_empty())
            .expect("MYSQL_PASSWORD must be present");
        let plan = fixed_relationship_plan(
            "Miket vásárolt az Országos Bírósági Hivatal az elmúlt 3 évben?",
            3,
        )
        .expect("OBH purchase questions must have a fixed plan");

        tauri::async_runtime::block_on(async {
            let settings = StoredSettings::default();
            let (_, rows, truncated) = run_query(&settings, password, &plan.sql, plan.max_rows)
                .await
                .expect("the OBH relationship query must succeed");
            assert!(!rows.is_empty(), "the OBH relationship returned no rows");
            assert!(!truncated);
            assert!(rows.iter().any(|row| {
                row.get("cikkszam").and_then(JsonValue::as_str) == Some("360-TEAM")
            }));
        });
    }

    #[test]
    #[ignore = "VPN-t és helyi .env fájlt igényel"]
    fn live_idomsoft_end_customer_resolution_returns_invoice_items() {
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
            let matches = resolve_end_customer(&settings, password.clone(), "idomszoft")
                .await
                .expect("IdomSoft resolution must succeed");
            assert!(!matches.is_empty(), "IdomSoft was not resolved");
            let label = matches[0].1.clone();
            let codes = matches
                .into_iter()
                .map(|(code, _)| code)
                .collect::<Vec<_>>();
            let plan = end_customer_purchase_plan(&label, &codes, 3);
            let (_, rows, _) = run_query(&settings, password, &plan.sql, plan.max_rows)
                .await
                .expect("IdomSoft item query must succeed");
            assert!(!rows.is_empty(), "IdomSoft item query returned no rows");
        });
    }

    #[test]
    #[ignore = "VPN-t és helyi .env fájlt igényel"]
    fn live_end_customer_resolution_avoids_substrings_and_keeps_company_families() {
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
            let aeron = resolve_end_customer(&settings, password.clone(), "aeron")
                .await
                .expect("AERON resolution must succeed");
            assert!(!aeron.is_empty(), "AERON was not resolved");
            assert!(aeron.iter().all(|(code, name)| {
                code.eq_ignore_ascii_case("aeron") || contains_token_phrase(name, "aeron")
            }));
            assert!(aeron.iter().all(|(_, name)| {
                !normalize_for_search(name).contains("autoritatea aeronautica")
            }));

            let mav = resolve_end_customer(&settings, password.clone(), "mav")
                .await
                .expect("MÁV resolution must succeed");
            assert!(
                mav.len() > 1,
                "MÁV company-family variants were not resolved"
            );
            assert!(mav.iter().all(|(code, name)| {
                code.eq_ignore_ascii_case("mav") || contains_token_phrase(name, "mav")
            }));

            let one = resolve_end_customer(&settings, password.clone(), "one")
                .await
                .expect("One resolution must succeed");
            assert_eq!(one.len(), 1, "Only the exact One entity may be resolved");
            assert_eq!(one[0].0.to_ascii_lowercase(), "one");
            let one_codes = one.iter().map(|(code, _)| code.clone()).collect::<Vec<_>>();
            let one_plan = end_customer_purchase_plan("A One", &one_codes, 1);
            let (_, rows, _) = run_query(&settings, password, &one_plan.sql, one_plan.max_rows)
                .await
                .expect("One purchase query must succeed");
            assert!(!rows.is_empty(), "One purchase query returned no rows");
        });
    }

    #[test]
    #[ignore = "VPN-t és helyi .env fájlt igényel"]
    fn live_top_end_customers_returns_five_ranked_rows() {
        let env_path = std::env::var("ERGO_ENV_PATH").expect("ERGO_ENV_PATH is required");
        let contents = std::fs::read_to_string(env_path).expect("the .env file must be readable");
        let password = contents
            .lines()
            .find_map(|line| line.trim().strip_prefix("MYSQL_PASSWORD="))
            .map(|value| value.trim().trim_matches(['\'', '"']).to_string())
            .filter(|value| !value.is_empty())
            .expect("MYSQL_PASSWORD must be present");
        let plan = top_end_customers_plan("Sorold fel a top 5 végfelhasználót!", 1)
            .expect("top query must have a fixed plan");

        tauri::async_runtime::block_on(async {
            let settings = StoredSettings::default();
            let (_, rows, truncated) = run_query(&settings, password, &plan.sql, plan.max_rows)
                .await
                .expect("top end-customer query must succeed");
            assert_eq!(rows.len(), 5);
            assert!(!truncated);
            assert!(rows[0].get("vegfelhasznalo_kod").is_some());
            assert!(rows[0]
                .get("netto_szamlazott_arbevetel_alapdevizaban")
                .is_some());
        });
    }

    #[test]
    #[ignore = "VPN-t és helyi .env fájlt igényel"]
    fn live_4ig_purchase_relationship_returns_items() {
        let env_path = std::env::var("ERGO_ENV_PATH").expect("ERGO_ENV_PATH is required");
        let contents = std::fs::read_to_string(env_path).expect("the .env file must be readable");
        let password = contents
            .lines()
            .find_map(|line| line.trim().strip_prefix("MYSQL_PASSWORD="))
            .map(|value| value.trim().trim_matches(['\'', '"']).to_string())
            .filter(|value| !value.is_empty())
            .expect("MYSQL_PASSWORD must be present");

        let plan = fixed_relationship_plan(
            "Listázd ki a 4iG által megvásárolt termékeket az elmúlt 1 évből",
            1,
        )
        .expect("4iG purchase question must have a fixed plan");

        tauri::async_runtime::block_on(async {
            let settings = StoredSettings::default();
            let (_, rows, truncated) = run_query(&settings, password, &plan.sql, plan.max_rows)
                .await
                .expect("4iG purchase query must succeed");
            assert!(!rows.is_empty(), "4iG purchase query returned no rows");
            assert!(!truncated);
            assert!(rows.iter().all(|row| {
                row.get("vegfelhasznalo_kod")
                    .and_then(JsonValue::as_str)
                    .is_some_and(|code| {
                        ["4ig", "4igzrt", "4igurvedelmi"]
                            .contains(&code.to_ascii_lowercase().as_str())
                    })
            }));
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

        let purchase_terms = schema_search_terms(
            "Miket vásárolt az Országos Bírósági Hivatal az elmúlt 3 évben?",
            &[],
        );
        for expected in [
            "invoice",
            "invoiceline",
            "customer",
            "endcustomer",
            "endcustomernumber",
            "linepricebase",
        ] {
            assert!(purchase_terms.contains(expected), "missing {expected}");
        }
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
                            samples: Vec::new(),
                        },
                        CatalogColumn {
                            name: "duedate".into(),
                            column_type: "varchar(12)".into(),
                            nullable: false,
                            key: String::new(),
                            samples: vec!["2024.01.31".into(), "2024.02.29".into()],
                        },
                        CatalogColumn {
                            name: "remainderbase".into(),
                            column_type: "decimal(20,2)".into(),
                            nullable: false,
                            key: String::new(),
                            samples: Vec::new(),
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
                        samples: Vec::new(),
                    }],
                    relationships: vec![],
                },
            ],
        };
        let (context, stats) = select_schema_context(&catalog, "Mennyi a lejárt kintlévőség?", &[]);
        let (inventory, detail) = context
            .split_once("Részletes sémarészlet:")
            .expect("a sémakontextus tartalmazza a részletes szakaszt");
        assert!(detail.contains("customerentry"));
        // A részletes szakasz továbbra sem éget tokent irreleváns táblára...
        assert!(!detail.contains("employeenumber"));
        // ...de a puszta táblanévlistában szerepel, hogy a modell tudja,
        // mi létezik, és ne találjon ki táblanevet.
        assert!(inventory.contains("employee"));
        assert_eq!(stats.selected_tables, 1);
        assert_eq!(stats.total_columns, 4);
    }

    #[test]
    fn schema_context_shows_sampled_column_values() {
        let catalog = SchemaCatalog {
            database: "test".into(),
            source: "localhost/test".into(),
            generated_at: 0,
            tables: vec![SchemaTable {
                name: "invoice".into(),
                table_type: "BASE TABLE".into(),
                estimated_rows: Some(1_000),
                columns: vec![CatalogColumn {
                    name: "debitcredit".into(),
                    column_type: "tinyint(1)".into(),
                    nullable: false,
                    key: String::new(),
                    samples: vec!["0".into(), "1".into()],
                }],
                relationships: vec![],
            }],
        };
        let (context, _) = select_schema_context(&catalog, "Mennyi az árbevétel?", &[]);
        assert!(context.contains("debitcredit:tinyint(1) példaértékek: 0 | 1"));
    }

    #[test]
    fn statistics_are_computed_over_every_returned_row() {
        let columns = vec!["partner".to_string(), "netto".to_string()];
        let rows = (1..=50)
            .map(|index| super::json!({ "partner": format!("P{index}"), "netto": "100.50" }))
            .collect::<Vec<_>>();
        let statistics = result_statistics(&columns, &rows);
        assert!(statistics.contains("Sorok száma: 50"));
        // A DECIMAL szövegként jön vissza, mégis összegezhető kell legyen,
        // és az összeg a teljes 50 sorra szóljon, nem a 40 soros előnézetre.
        assert!(statistics.contains("összeg 5025"), "{statistics}");
        assert!(statistics.contains("50 különböző"), "{statistics}");
    }

    #[test]
    fn explicitly_requested_years_override_the_analysis_window() {
        let (window_start, _) = fy_window_years(3);
        let older_year = window_start - 2;
        let scope = PeriodScope::resolve(&format!("Mennyi volt a {older_year} évi árbevétel?"), 3);
        assert_eq!(scope.outside_years, vec![older_year]);
        let rule = scope.planner_rule(3, "MAKEDATE(YEAR(CURRENT_DATE) - 2, 1)");
        assert!(rule.contains("FELÜLÍRJA"), "{rule}");
        // A négyjegyű szám nem biztosan évszám, ezért a döntést a modellre
        // hagyjuk, nem adunk rá feltétlen utasítást.
        assert!(rule.contains("ha a szám nem időszak"), "{rule}");
        assert!(scope.summary_note(3).contains("az időablak nem szűkítette"));

        let inside = PeriodScope::resolve("Mennyi volt az árbevétel?", 3);
        assert!(inside.outside_years.is_empty());
        assert!(inside.planner_rule(3, "x").contains("ne használj"));
    }

    #[test]
    fn planner_tools_let_the_model_explore_and_finish() {
        let tools = planner_tools();
        let names = tools
            .as_array()
            .expect("az eszközlista tömb")
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        for expected in [
            "search_schema",
            "describe_table",
            "resolve_entity",
            "probe_query",
            "submit_plan",
        ] {
            assert!(names.contains(&expected.to_string()), "{names:?}");
        }
        // A végleges terv ugyanazokat a mezőket adja, mint az egylövésű út,
        // így a validáló és futtató réteg változatlanul használható.
        let submit = tools
            .as_array()
            .expect("az eszközlista tömb")
            .iter()
            .find(|tool| tool["name"].as_str() == Some("submit_plan"))
            .expect("submit_plan eszköz");
        let required = submit["parameters"]["required"]
            .as_array()
            .expect("kötelező mezők");
        assert_eq!(required.len(), 4);
    }

    #[test]
    fn schema_search_tool_finds_tables_and_reports_misses() {
        let catalog = SchemaCatalog {
            database: "test".into(),
            source: "localhost/test".into(),
            generated_at: 0,
            tables: vec![SchemaTable {
                name: "invoiceline".into(),
                table_type: "BASE TABLE".into(),
                estimated_rows: Some(10),
                columns: vec![CatalogColumn {
                    name: "linepricebase".into(),
                    column_type: "decimal(20,2)".into(),
                    nullable: false,
                    key: String::new(),
                    samples: Vec::new(),
                }],
                relationships: vec![],
            }],
        };
        let found = search_schema_tool(&catalog, "invoiceline linepricebase");
        assert!(found.contains("invoiceline"), "{found}");
        assert!(found.contains("linepricebase"), "{found}");

        // A téves találat helyett érthető üzenet megy vissza a modellhez,
        // hogy tudjon másik kifejezéssel próbálkozni.
        let missing = search_schema_tool(&catalog, "raktarkeszlet");
        assert!(missing.contains("Nincs találat"), "{missing}");

        let described = describe_table_tool(&catalog, "INVOICELINE");
        assert!(
            described.contains("linepricebase:decimal(20,2)"),
            "{described}"
        );
        assert!(describe_table_tool(&catalog, "nincsilyen").contains("Nincs ilyen tábla"));
    }

    fn feedback_fixture(
        rating: &str,
        question: &str,
        sql: Option<&str>,
        correction: &str,
        category: Option<&str>,
        created_at: u64,
    ) -> AnalysisFeedbackRecord {
        AnalysisFeedbackRecord {
            id: format!("feedback-{created_at}"),
            created_at,
            app_version: "test".into(),
            model: "gpt-6-sol".into(),
            analysis_fy_window: 3,
            rating: rating.into(),
            category: category.map(str::to_owned),
            correction: correction.into(),
            question: question.into(),
            answer: "válasz".into(),
            result_title: None,
            sql: sql.map(str::to_owned),
            query_source: Some("generated".into()),
            row_count: Some(3),
            status: "recorded".into(),
        }
    }

    #[test]
    fn similar_questions_match_through_hungarian_inflection() {
        let asked = memory_terms("Mennyi volt az árbevétel 2024-ben?");
        let stored = memory_terms("Mutasd az árbevételt 2024 évre");
        // Az "arbevetel" és az "arbevetelt" előtagvágás után egybeesik.
        assert!(
            question_similarity(&asked, &stored) >= MEMORY_MIN_SIMILARITY,
            "{asked:?} vs {stored:?}"
        );
        let unrelated = memory_terms("Kik a legnagyobb beszállítóink?");
        assert!(question_similarity(&asked, &unrelated) < MEMORY_MIN_SIMILARITY);
    }

    #[test]
    fn memory_offers_verified_examples_and_known_pitfalls() {
        let records = vec![
            feedback_fixture(
                "positive",
                "Mennyi volt az árbevétel 2024-ben?",
                Some("SELECT SUM(itemsumbase) FROM invoice WHERE companynumber = '1'"),
                "",
                None,
                1,
            ),
            feedback_fixture(
                "negative",
                "Mutasd az árbevételt 2024 évre",
                None,
                "A jóváírásokat le kell vonni.",
                Some("metric"),
                2,
            ),
            feedback_fixture(
                "positive",
                "Kik a legnagyobb beszállítóink?",
                Some("SELECT vendornumber FROM vendor"),
                "",
                None,
                3,
            ),
        ];
        let context = memory_prompt_context(&records, "Mennyi az árbevétel 2024-ben?");
        assert!(context.contains("Elfogadott SQL"), "{context}");
        assert!(context.contains("itemsumbase"), "{context}");
        assert!(
            context.contains("A jóváírásokat le kell vonni."),
            "{context}"
        );
        // A nem kapcsolódó kérdés példája nem kerül be.
        assert!(!context.contains("vendornumber"), "{context}");
        // A tapasztalat csak támpont; az üzleti szabály az erősebb.
        assert!(context.contains("TÁMPONTOK, nem szabályok"), "{context}");
    }

    #[test]
    fn rejected_and_sqlless_feedback_never_becomes_an_example() {
        let mut rejected = feedback_fixture(
            "positive",
            "Mennyi volt az árbevétel 2024-ben?",
            Some("SELECT 1"),
            "",
            None,
            1,
        );
        rejected.status = "rejected".into();
        let without_sql = feedback_fixture(
            "positive",
            "Mennyi volt az árbevétel 2024-ben?",
            None,
            "",
            None,
            2,
        );
        let context =
            memory_prompt_context(&[rejected, without_sql], "Mennyi az árbevétel 2024-ben?");
        assert!(context.is_empty(), "{context}");
    }

    #[test]
    fn only_summary_corrections_reach_the_summarizer() {
        let records = vec![
            feedback_fixture(
                "negative",
                "Mennyi volt az árbevétel 2024-ben?",
                None,
                "Az összeget ezer forintban írd ki.",
                Some("summary"),
                1,
            ),
            feedback_fixture(
                "negative",
                "Mennyi volt az árbevétel 2024-ben?",
                None,
                "Rossz táblát használt.",
                Some("relationship"),
                2,
            ),
        ];
        let context = summary_pitfall_context(&records, "Mennyi az árbevétel 2024-ben?");
        assert!(context.contains("ezer forintban"), "{context}");
        assert!(!context.contains("Rossz táblát"), "{context}");
    }

    #[test]
    fn learned_alias_matching_respects_role_and_approval() {
        let pending = LearnedAlias {
            id: "alias-1".into(),
            created_at: 1,
            search_name: "OBH".into(),
            role: "end_customer".into(),
            codes: Vec::new(),
            label: "OBH".into(),
            candidates: vec![
                LearnedAliasCandidate {
                    code: "birosag".into(),
                    label: "Országos Bírósági Hivatal".into(),
                },
                LearnedAliasCandidate {
                    code: "obh".into(),
                    label: "OBH Kft.".into(),
                },
            ],
            status: "pending".into(),
        };
        // A jóváhagyásra váró bejegyzés nem azonosít céget.
        assert!(pending.codes.is_empty());
        assert_ne!(pending.status, "approved");
        // A név illesztése kis-nagybetűtől és ragozástól független...
        assert!(alias_matches(&pending, "obh", "end_customer"));
        // ...a szerep viszont kötelezően egyezik: a számlázott vevő és a
        // végfelhasználó két külön törzs, és soha nem keverhető össze.
        assert!(!alias_matches(&pending, "obh", "billed_customer"));
        assert!(!alias_matches(&pending, "one", "end_customer"));
    }

    #[test]
    fn computes_calendar_dates_without_a_date_library() {
        assert_eq!(civil_date_from_unix(0), (1970, 1, 1));
        assert_eq!(civil_date_from_unix(951_782_400), (2000, 2, 29));
        assert_eq!(civil_date_from_unix(1_760_832_000), (2025, 10, 19));
    }

    #[test]
    fn samples_only_low_cardinality_code_and_date_columns() {
        let sampleable = CatalogColumn {
            name: "debitcredit".into(),
            column_type: "tinyint(1)".into(),
            nullable: false,
            key: String::new(),
            samples: Vec::new(),
        };
        let free_text = CatalogColumn {
            name: "itemtext1".into(),
            column_type: "varchar(255)".into(),
            nullable: false,
            key: String::new(),
            samples: Vec::new(),
        };
        let large = CatalogColumn {
            name: "statustext".into(),
            column_type: "text".into(),
            nullable: true,
            key: String::new(),
            samples: Vec::new(),
        };
        assert!(is_sampleable_column(&sampleable));
        assert!(!is_sampleable_column(&free_text));
        assert!(!is_sampleable_column(&large));
    }
}
