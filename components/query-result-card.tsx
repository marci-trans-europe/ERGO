'use client'

import { Braces, Download, Table2 } from 'lucide-react'
import { useState } from 'react'

import type { QueryResult, QueryRow, QueryValue } from '@/lib/types'

type QueryResultCardProps = {
  result: QueryResult
}

const amountColumnTerms = [
  'amount',
  'arbevetel',
  'árbevétel',
  'credit',
  'debit',
  'ertek',
  'érték',
  'itemsum',
  'kintlevoseg',
  'kintlévőség',
  'koltseg',
  'költség',
  'lineprice',
  'netto',
  'nettó',
  'osszeg',
  'összeg',
  'price',
  'remainder',
  'total',
  'vatbase',
  'vatcurrency',
]

const amountFormatter = new Intl.NumberFormat('hu-HU', {
  maximumFractionDigits: 2,
})

const isAmountColumn = (column?: string) => {
  const normalized = column?.toLocaleLowerCase('hu-HU') ?? ''
  return amountColumnTerms.some((term) => normalized.includes(term))
}

const formatEmbeddedAmounts = (value: string) =>
  value.replace(
    /(nettó:\s*)(-?\d+(?:\.\d+)?)/giu,
    (_, label, amount) => `${label}${amountFormatter.format(Number(amount))}`,
  )

const displayValue = (
  value: QueryValue | undefined,
  column?: string,
): string => {
  if (value === null || value === undefined) return '—'
  if (typeof value === 'object') return JSON.stringify(value)
  if (typeof value === 'number') {
    return amountFormatter.format(value)
  }
  if (
    isAmountColumn(column) &&
    typeof value === 'string' &&
    /^-?\d+(?:\.\d+)?$/.test(value)
  ) {
    return amountFormatter.format(Number(value))
  }
  return String(value)
}

const escapeCsv = (value: QueryValue | undefined): string => {
  const text =
    value === null || value === undefined
      ? ''
      : typeof value === 'object'
        ? JSON.stringify(value)
        : String(value)
  return `"${text.replaceAll('"', '""')}"`
}

const downloadCsv = (result: QueryResult) => {
  const rows = [
    result.columns.map((column) => escapeCsv(column)).join(','),
    ...result.rows.map((row) =>
      result.columns.map((column) => escapeCsv(row[column])).join(','),
    ),
  ]
  const blob = new Blob([`\uFEFF${rows.join('\n')}`], {
    type: 'text/csv;charset=utf-8',
  })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = `${result.title.toLowerCase().replace(/[^a-z0-9áéíóöőúüű]+/gi, '-')}.csv`
  link.click()
  URL.revokeObjectURL(url)
}

const findChartColumns = (result: QueryResult) => {
  const numericColumns = result.columns.filter((column) =>
    result.rows.some((row) => Number.isFinite(Number(row[column]))),
  )
  const valueColumn =
    numericColumns.find((column) => isAmountColumn(column)) ?? numericColumns[0]
  const preferredLabels = [
    'ugyfel',
    'ügyfél',
    'customer_name',
    'customername',
    'name',
    'honap',
    'hónap',
    'month',
    'datum',
    'dátum',
    'date',
    'label',
  ]
  const labelColumn =
    result.columns.find((column) =>
      preferredLabels.includes(column.toLocaleLowerCase('hu-HU')),
    ) ??
    result.columns
      .filter((column) => column !== valueColumn)
      .find((column) =>
        result.rows.some((row) => !Number.isFinite(Number(row[column]))),
      ) ??
    result.columns[0]

  return labelColumn && valueColumn ? { labelColumn, valueColumn } : null
}

const ResultChart = ({ result }: QueryResultCardProps) => {
  const [hoveredIndex, setHoveredIndex] = useState<number | null>(null)
  const columns = findChartColumns(result)
  if (!columns) return null

  const points = result.rows
    .map((row) => ({
      label: displayValue(row[columns.labelColumn]),
      value: Number(row[columns.valueColumn]),
    }))
    .filter((point) => Number.isFinite(point.value))
    .slice(0, 50)

  if (points.length < 2) return null

  const isBarChart = result.visualization === 'bar'
  const width = Math.max(760, points.length * (isBarChart ? 88 : 72))
  const height = isBarChart ? 330 : 290
  const padding = {
    top: 24,
    right: 24,
    bottom: isBarChart ? 100 : 64,
    left: 58,
  }
  const chartWidth = width - padding.left - padding.right
  const chartHeight = height - padding.top - padding.bottom
  const minValue = Math.min(0, ...points.map((point) => point.value))
  const maxValue = Math.max(0, ...points.map((point) => point.value))
  const range = maxValue - minValue || 1
  const y = (value: number) =>
    padding.top + ((maxValue - value) / range) * chartHeight
  const x = (index: number) =>
    padding.left + (index / Math.max(points.length - 1, 1)) * chartWidth
  const pointX = (index: number) =>
    isBarChart
      ? padding.left + ((index + 0.5) / points.length) * chartWidth
      : x(index)
  const baseline = y(0)
  const hoveredPoint = hoveredIndex === null ? null : points[hoveredIndex]

  return (
    <div className="result-chart">
      {hoveredPoint ? (
        <div aria-live="polite" className="chart-hover-value">
          <>
            <strong>{hoveredPoint.label}</strong>
            <span>{displayValue(hoveredPoint.value, columns.valueColumn)}</span>
          </>
        </div>
      ) : null}
      <svg
        aria-label={result.title}
        role="img"
        style={{ minWidth: width }}
        viewBox={`0 0 ${width} ${height}`}
      >
        <line
          className="chart-grid"
          x1={padding.left}
          x2={width - padding.right}
          y1={y(maxValue)}
          y2={y(maxValue)}
        />
        <line
          className="chart-grid"
          x1={padding.left}
          x2={width - padding.right}
          y1={y((minValue + maxValue) / 2)}
          y2={y((minValue + maxValue) / 2)}
        />
        <line
          className="chart-axis"
          x1={padding.left}
          x2={width - padding.right}
          y1={baseline}
          y2={baseline}
        />

        <text
          className="chart-y-label"
          x={padding.left - 10}
          y={y(maxValue) + 4}
        >
          {new Intl.NumberFormat('hu-HU', { notation: 'compact' }).format(
            maxValue,
          )}
        </text>
        <text
          className="chart-y-label"
          x={padding.left - 10}
          y={y(minValue) + 4}
        >
          {new Intl.NumberFormat('hu-HU', { notation: 'compact' }).format(
            minValue,
          )}
        </text>

        {isBarChart ? (
          points.map((point, index) => {
            const gap = Math.max(3, (chartWidth / points.length) * 0.18)
            const barWidth = Math.max(4, chartWidth / points.length - gap)
            const pointY = y(point.value)
            return (
              <rect
                aria-label={`${point.label}: ${displayValue(point.value, columns.valueColumn)}`}
                className="chart-bar"
                height={Math.abs(baseline - pointY)}
                key={`${point.label}-${index}`}
                onBlur={() => setHoveredIndex(null)}
                onFocus={() => setHoveredIndex(index)}
                onMouseEnter={() => setHoveredIndex(index)}
                onMouseLeave={() => setHoveredIndex(null)}
                rx={Math.min(5, barWidth / 3)}
                tabIndex={0}
                width={barWidth}
                x={
                  padding.left + (index / points.length) * chartWidth + gap / 2
                }
                y={Math.min(pointY, baseline)}
              >
                <title>{`${point.label}: ${displayValue(point.value, columns.valueColumn)}`}</title>
              </rect>
            )
          })
        ) : (
          <>
            <polyline
              className="chart-line"
              points={points
                .map((point, index) => `${x(index)},${y(point.value)}`)
                .join(' ')}
            />
            {points.map((point, index) => (
              <circle
                aria-label={`${point.label}: ${displayValue(point.value, columns.valueColumn)}`}
                className="chart-point"
                cx={x(index)}
                cy={y(point.value)}
                key={`${point.label}-${index}`}
                onBlur={() => setHoveredIndex(null)}
                onFocus={() => setHoveredIndex(index)}
                onMouseEnter={() => setHoveredIndex(index)}
                onMouseLeave={() => setHoveredIndex(null)}
                r="6"
                tabIndex={0}
              >
                <title>{`${point.label}: ${displayValue(point.value, columns.valueColumn)}`}</title>
              </circle>
            ))}
          </>
        )}

        {points.map((point, index) => (
          <text
            className="chart-x-label"
            key={`${point.label}-label`}
            textAnchor={isBarChart ? 'end' : 'middle'}
            transform={
              isBarChart
                ? `rotate(-32 ${pointX(index)} ${height - 22})`
                : undefined
            }
            x={pointX(index)}
            y={height - 22}
          >
            <title>{point.label}</title>
            {point.label.length > 20
              ? `${point.label.slice(0, 18)}…`
              : point.label}
          </text>
        ))}
      </svg>
      <div className="chart-legend">
        <span />
        {columns.valueColumn.replaceAll('_', ' ')}
      </div>
    </div>
  )
}

const ResultTable = ({
  columns,
  rows,
}: {
  columns: string[]
  rows: QueryRow[]
}) => (
  <div className="table-scroll">
    <table>
      <thead>
        <tr>
          {columns.map((column) => (
            <th key={column}>{column.replaceAll('_', ' ')}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((row, rowIndex) => (
          <tr key={rowIndex}>
            {columns.map((column) => (
              <td
                className={
                  column === 'cikkek' ? 'item-breakdown-cell' : undefined
                }
                key={column}
              >
                {column === 'cikkek' && typeof row[column] === 'string' ? (
                  <ul className="item-breakdown">
                    {row[column].split(' | ').map((item, itemIndex) => (
                      <li key={`${item}-${itemIndex}`}>
                        {formatEmbeddedAmounts(item)}
                      </li>
                    ))}
                  </ul>
                ) : (
                  displayValue(row[column], column)
                )}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  </div>
)

export const QueryResultCard = ({ result }: QueryResultCardProps) => {
  const hasChart =
    result.visualization !== 'table' && Boolean(findChartColumns(result))

  return (
    <section className="result-card">
      <header>
        <div>
          <p className="result-kicker">
            {hasChart ? 'VIZUALIZÁCIÓ' : 'LEKÉRDEZÉSI EREDMÉNY'}
          </p>
          <h3>{result.title}</h3>
        </div>
        <div className="result-actions">
          <span>{result.rowCount} sor</span>
          <button
            aria-label="CSV letöltése"
            onClick={() => downloadCsv(result)}
            title="CSV letöltése"
            type="button"
          >
            <Download aria-hidden="true" size={16} />
          </button>
        </div>
      </header>

      {result.rows.length === 0 ? (
        <div className="empty-result">
          <Table2 aria-hidden="true" size={20} />A feltételeknek megfelelő
          rekord nem található.
        </div>
      ) : hasChart ? (
        <ResultChart result={result} />
      ) : (
        <ResultTable columns={result.columns} rows={result.rows} />
      )}

      <footer>
        <div className="result-meta">
          {result.truncated ? (
            <span>A lista a beállított sorkorlátig látható.</span>
          ) : (
            <span>Teljes lekérdezési eredmény</span>
          )}
          {result.querySource === 'fixed' ? (
            <span>Fix, ellenőrzött SQL · AI-tervezés nélkül</span>
          ) : (
            <span>
              Séma: {result.schemaSelection.selectedTables}/
              {result.schemaSelection.totalTables} tábla ·{' '}
              {result.schemaSelection.selectedColumns}/
              {result.schemaSelection.totalColumns} oszlop
            </span>
          )}
          <span>
            Token: {result.tokenUsage.inputTokens} be ·{' '}
            {result.tokenUsage.outputTokens} ki
            {result.tokenUsage.cachedInputTokens > 0
              ? ` · ${result.tokenUsage.cachedInputTokens} cache`
              : ''}
          </span>
        </div>
        <details>
          <summary>
            <Braces aria-hidden="true" size={14} /> SQL megtekintése
          </summary>
          <pre>{result.sql}</pre>
        </details>
      </footer>
    </section>
  )
}
