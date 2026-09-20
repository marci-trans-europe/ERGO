'use client'

import { Braces, Download, Table2 } from 'lucide-react'

import type { QueryResult, QueryRow, QueryValue } from '@/lib/types'

type QueryResultCardProps = {
  result: QueryResult
}

const displayValue = (value: QueryValue | undefined): string => {
  if (value === null || value === undefined) return '—'
  if (typeof value === 'object') return JSON.stringify(value)
  if (typeof value === 'number') {
    return new Intl.NumberFormat('hu-HU', { maximumFractionDigits: 2 }).format(
      value,
    )
  }
  return String(value)
}

const escapeCsv = (value: QueryValue | undefined): string => {
  const text = displayValue(value)
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
  const labelColumn = result.columns[0]
  const valueColumn = result.columns
    .slice(1)
    .find((column) =>
      result.rows.some((row) => Number.isFinite(Number(row[column]))),
    )

  return labelColumn && valueColumn ? { labelColumn, valueColumn } : null
}

const ResultChart = ({ result }: QueryResultCardProps) => {
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

  const width = 760
  const height = 280
  const padding = { top: 24, right: 24, bottom: 54, left: 58 }
  const chartWidth = width - padding.left - padding.right
  const chartHeight = height - padding.top - padding.bottom
  const minValue = Math.min(0, ...points.map((point) => point.value))
  const maxValue = Math.max(0, ...points.map((point) => point.value))
  const range = maxValue - minValue || 1
  const y = (value: number) =>
    padding.top + ((maxValue - value) / range) * chartHeight
  const x = (index: number) =>
    padding.left + (index / Math.max(points.length - 1, 1)) * chartWidth
  const baseline = y(0)
  const labelEvery = Math.max(1, Math.ceil(points.length / 7))

  return (
    <div className="result-chart">
      <svg
        aria-label={result.title}
        role="img"
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

        {result.visualization === 'bar' ? (
          points.map((point, index) => {
            const gap = Math.max(3, (chartWidth / points.length) * 0.18)
            const barWidth = Math.max(4, chartWidth / points.length - gap)
            const pointY = y(point.value)
            return (
              <rect
                className="chart-bar"
                height={Math.abs(baseline - pointY)}
                key={`${point.label}-${index}`}
                rx={Math.min(5, barWidth / 3)}
                width={barWidth}
                x={
                  padding.left + (index / points.length) * chartWidth + gap / 2
                }
                y={Math.min(pointY, baseline)}
              >
                <title>{`${point.label}: ${displayValue(point.value)}`}</title>
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
                className="chart-point"
                cx={x(index)}
                cy={y(point.value)}
                key={`${point.label}-${index}`}
                r="4"
              >
                <title>{`${point.label}: ${displayValue(point.value)}`}</title>
              </circle>
            ))}
          </>
        )}

        {points.map((point, index) =>
          index % labelEvery === 0 || index === points.length - 1 ? (
            <text
              className="chart-x-label"
              key={`${point.label}-label`}
              textAnchor="middle"
              x={
                result.visualization === 'bar'
                  ? padding.left + ((index + 0.5) / points.length) * chartWidth
                  : x(index)
              }
              y={height - 20}
            >
              {point.label.length > 14
                ? `${point.label.slice(0, 12)}…`
                : point.label}
            </text>
          ) : null,
        )}
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
              <td key={column}>{displayValue(row[column])}</td>
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
        {result.truncated ? (
          <span>A lista a beállított sorkorlátig látható.</span>
        ) : (
          <span>Teljes lekérdezési eredmény</span>
        )}
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
