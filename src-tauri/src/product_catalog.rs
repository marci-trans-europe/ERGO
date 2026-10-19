use mysql_async::{prelude::Queryable, Row};
use serde::Serialize;
use tauri::AppHandle;
use tokio::time::{timeout, Duration};

use crate::{database_pool, read_secret, read_settings, MYSQL_PASSWORD_ACCOUNT};

const MAX_CATALOG_ROWS: usize = 25_000;
const MAX_SALE_ROWS: usize = 300;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogPeriod {
    item_number: String,
    item_name: String,
    year: u16,
    quarter: u8,
    quantity: f64,
    net_base: f64,
    invoice_lines: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductCatalog {
    current_year: u16,
    periods: Vec<CatalogPeriod>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductSale {
    invoice_number: String,
    invoice_date: String,
    reseller_code: String,
    reseller: String,
    end_customer_code: String,
    end_customer: String,
    quantity: f64,
    net_base: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductSales {
    rows: Vec<ProductSale>,
    truncated: bool,
}

const CATALOG_SQL: &str = r#"
SELECT
    COALESCE(il.itemnumber, '') AS item_number,
    COALESCE(NULLIF(TRIM(il.itemtext1), ''), NULLIF(TRIM(il.externalitemtext), ''), '(nincs megnevezés)') AS item_name,
    CAST(LEFT(i.invoicedate, 4) AS UNSIGNED) AS fiscal_year,
    CAST(CEIL(CAST(SUBSTRING(i.invoicedate, 6, 2) AS UNSIGNED) / 3) AS UNSIGNED) AS fiscal_quarter,
    CAST(ROUND(SUM(CASE WHEN i.debitcredit = 0 THEN COALESCE(il.numberinvoiced, 0)
                        ELSE -COALESCE(NULLIF(il.numbercredited, 0), il.numberinvoiced, 0) END), 2) AS DOUBLE) AS quantity,
    CAST(ROUND(SUM(CASE WHEN i.debitcredit = 0 THEN COALESCE(il.linepricebase, 0)
                        ELSE -COALESCE(il.linepricebase, 0) END), 2) AS DOUBLE) AS net_base,
    COUNT(*) AS invoice_lines
FROM invoice i
STRAIGHT_JOIN invoiceline il
  ON il.companynumber = i.companynumber AND il.invoicenumber = i.invoicenumber
WHERE i.companynumber = '1' AND i.posted = 1
  AND i.invoicedate >= CONCAT(YEAR(CURRENT_DATE()) - 2, '.01.01')
  AND i.invoicedate <= DATE_FORMAT(CURRENT_DATE(), '%Y.%m.%d')
GROUP BY item_number, item_name, fiscal_year, fiscal_quarter
LIMIT 25001
"#;

#[tauri::command]
pub(crate) async fn load_product_catalog(app: AppHandle) -> Result<ProductCatalog, String> {
    let settings = read_settings(&app)?;
    let password = read_secret(&app, "MYSQL_PASSWORD", MYSQL_PASSWORD_ACCOUNT, "MySQL-jelszó")?;
    let pool = database_pool(&settings, password);
    let mut connection = timeout(Duration::from_secs(12), pool.get_conn())
        .await
        .map_err(|_| "Az adatbázis-kapcsolat időtúllépés miatt megszakadt.".to_string())?
        .map_err(|error| format!("Az ERP-adatbázis nem érhető el: {error}"))?;
    connection
        .query_drop("START TRANSACTION READ ONLY")
        .await
        .map_err(|error| format!("A csak olvasható tranzakció nem indítható: {error}"))?;
    let current_year: u16 = connection
        .query_first("SELECT YEAR(CURRENT_DATE())")
        .await
        .map_err(|error| format!("Az aktuális év nem olvasható: {error}"))?
        .ok_or_else(|| "Az aktuális év nem olvasható.".to_string())?;
    let rows = timeout(Duration::from_secs(90), connection.query::<Row, _>(CATALOG_SQL))
        .await
        .map_err(|_| "A termékkatalógus lekérdezése időtúllépésbe futott.".to_string())?
        .map_err(|error| format!("A termékkatalógus nem olvasható: {error}"))?;
    connection.query_drop("COMMIT").await.ok();
    drop(connection);
    pool.disconnect().await.ok();
    if rows.len() > MAX_CATALOG_ROWS {
        return Err("A katalógus túllépte a biztonságos sorkorlátot; nem mutatok hiányos kategóriákat.".into());
    }
    let periods = rows
        .into_iter()
        .map(|row| CatalogPeriod {
            item_number: row.get("item_number").unwrap_or_default(),
            item_name: row.get("item_name").unwrap_or_default(),
            year: row.get("fiscal_year").unwrap_or_default(),
            quarter: row.get("fiscal_quarter").unwrap_or_default(),
            quantity: row.get("quantity").unwrap_or_default(),
            net_base: row.get("net_base").unwrap_or_default(),
            invoice_lines: row.get("invoice_lines").unwrap_or_default(),
        })
        .collect();
    Ok(ProductCatalog {
        current_year,
        periods,
    })
}

const SALES_SQL: &str = r#"
SELECT
    i.invoicenumber AS invoice_number,
    i.invoicedate AS invoice_date,
    i.customernumber AS reseller_code,
    COALESCE(NULLIF(TRIM(CONCAT_WS(' ', c.name1, c.name2)), ''), i.customernumber) AS reseller,
    COALESCE(NULLIF(i.endcustomernumber, ''), NULLIF(il.endcustomernumber, ''), '') AS end_customer_code,
    COALESCE(NULLIF(TRIM(CONCAT_WS(' ', ec.name1, ec.name2)), ''),
             COALESCE(NULLIF(i.endcustomernumber, ''), NULLIF(il.endcustomernumber, ''), '–')) AS end_customer,
    CAST(CASE WHEN i.debitcredit = 0 THEN COALESCE(il.numberinvoiced, 0)
              ELSE -COALESCE(NULLIF(il.numbercredited, 0), il.numberinvoiced, 0) END AS DOUBLE) AS quantity,
    CAST(CASE WHEN i.debitcredit = 0 THEN COALESCE(il.linepricebase, 0)
              ELSE -COALESCE(il.linepricebase, 0) END AS DOUBLE) AS net_base
FROM invoiceline il
JOIN invoice i ON i.companynumber = il.companynumber AND i.invoicenumber = il.invoicenumber
LEFT JOIN customer c ON c.customernumber = i.customernumber
LEFT JOIN endcustomer ec ON ec.endcustomernumber = COALESCE(NULLIF(i.endcustomernumber, ''), NULLIF(il.endcustomernumber, ''))
WHERE il.companynumber = '1' AND il.itemnumber = ?
  AND COALESCE(NULLIF(TRIM(il.itemtext1), ''), NULLIF(TRIM(il.externalitemtext), ''), '(nincs megnevezés)') = ?
  AND i.posted = 1
  AND i.invoicedate >= CONCAT(YEAR(CURRENT_DATE()) - 2, '.01.01')
  AND i.invoicedate <= DATE_FORMAT(CURRENT_DATE(), '%Y.%m.%d')
ORDER BY i.invoicedate DESC, i.invoicenumber DESC
LIMIT 301
"#;

#[tauri::command]
pub(crate) async fn load_product_sales(
    app: AppHandle,
    item_number: String,
    item_name: String,
) -> Result<ProductSales, String> {
    if item_number.trim().is_empty()
        || item_number.len() > 255
        || item_name.trim().is_empty()
        || item_name.len() > 255
    {
        return Err("A cikk azonosítója vagy megnevezése érvénytelen.".into());
    }
    let settings = read_settings(&app)?;
    let password = read_secret(&app, "MYSQL_PASSWORD", MYSQL_PASSWORD_ACCOUNT, "MySQL-jelszó")?;
    let pool = database_pool(&settings, password);
    let mut connection = timeout(Duration::from_secs(12), pool.get_conn())
        .await
        .map_err(|_| "Az adatbázis-kapcsolat időtúllépés miatt megszakadt.".to_string())?
        .map_err(|error| format!("Az ERP-adatbázis nem érhető el: {error}"))?;
    connection
        .query_drop("START TRANSACTION READ ONLY")
        .await
        .map_err(|error| format!("A csak olvasható tranzakció nem indítható: {error}"))?;
    let rows = timeout(
        Duration::from_secs(45),
        connection.exec::<Row, _, _>(SALES_SQL, (item_number, item_name)),
    )
    .await
    .map_err(|_| "A cikk értékesítési tételeinek lekérdezése időtúllépésbe futott.".to_string())?
    .map_err(|error| format!("A cikk értékesítési tételei nem olvashatók: {error}"))?;
    connection.query_drop("COMMIT").await.ok();
    drop(connection);
    pool.disconnect().await.ok();
    let truncated = rows.len() > MAX_SALE_ROWS;
    let rows = rows
        .into_iter()
        .take(MAX_SALE_ROWS)
        .map(|row| ProductSale {
            invoice_number: row.get("invoice_number").unwrap_or_default(),
            invoice_date: row.get("invoice_date").unwrap_or_default(),
            reseller_code: row.get("reseller_code").unwrap_or_default(),
            reseller: row.get("reseller").unwrap_or_default(),
            end_customer_code: row.get("end_customer_code").unwrap_or_default(),
            end_customer: row.get("end_customer").unwrap_or_default(),
            quantity: row.get("quantity").unwrap_or_default(),
            net_base: row.get("net_base").unwrap_or_default(),
        })
        .collect();
    Ok(ProductSales { rows, truncated })
}
