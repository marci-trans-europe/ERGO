export type JsonPrimitive = string | number | boolean | null

export type QueryValue =
  JsonPrimitive | QueryValue[] | { [key: string]: QueryValue }

export type QueryRow = Record<string, QueryValue>

export type QueryVisualization = 'table' | 'bar' | 'line'
export type AnalysisFyWindow = 1 | 3 | 5

export type QueryResult = {
  kind: 'query-result'
  title: string
  visualization: QueryVisualization
  columns: string[]
  rows: QueryRow[]
  rowCount: number
  truncated: boolean
  sql: string
  tokenUsage: {
    inputTokens: number
    outputTokens: number
    cachedInputTokens: number
    totalTokens: number
  }
  schemaSelection: {
    totalTables: number
    totalColumns: number
    selectedTables: number
    selectedColumns: number
    contextCharacters: number
  }
}

export type SchemaColumn = {
  tableName: string
  tableType: string
  columnName: string
  dataType: string
  nullable: boolean
  ordinalPosition: number
}

export type DesktopSettings = {
  mysqlHost: string
  mysqlPort: number
  mysqlDatabase: string
  mysqlUser: string
  mysqlSsl: boolean
  aiBaseUrl: string
  aiModel: string
  analysisFyWindow: AnalysisFyWindow
  hasMysqlPassword: boolean
  hasAiApiKey: boolean
}

export type SettingsInput = Omit<
  DesktopSettings,
  'hasMysqlPassword' | 'hasAiApiKey'
> & {
  mysqlPassword: string
  aiApiKey: string
}

export type ChatMessage = {
  id: string
  role: 'user' | 'assistant'
  content: string
  result?: QueryResult
}

export type AnalyzeResponse = {
  summary: string
  result?: QueryResult
}
