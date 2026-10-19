import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import ts from 'typescript'

const source = fs.readFileSync(path.join(path.dirname(fileURLToPath(import.meta.url)), '../lib/product-taxonomy.ts'), 'utf8')
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText
const moduleScope = { exports: {} }
new Function('module', 'exports', compiled)(moduleScope, moduleScope.exports)
const { classifyProduct } = moduleScope.exports

test('MDF-cikkszám és Movink kampány nem termékértékesítés', () => {
  for (const [code, name] of [
    ['MDF', 'AnyDesk MDF'],
    ['ANYDESK_MDF_2026', 'Partner support'],
    ['SEE42', 'SEE FY42 May - Trans-Europe - Movink Product Launch - Marketing'],
  ]) {
    assert.equal(classifyProduct(code, name).category, 'Marketing fund')
    assert.equal(classifyProduct(code, name).kind, 'marketing-fund')
  }
})

test('Autodesk jutalék rebate a CAD-kategóriában', () => {
  const result = classifyProduct('AUTODESK_COSMM_COST', 'S/Autodesk Commission Cost Reseller TDS')
  assert.equal(result.category, 'Mérnöki és CAD')
  assert.equal(result.manufacturer, 'Autodesk')
  assert.equal(result.kind, 'rebate')
})

test('Pantone és Wacom hardver, DeepL AI', () => {
  assert.equal(classifyProduct('GG1504', 'Pantone Plus Pastels and Neons Guide').category, 'Hardver és periféria')
  assert.equal(classifyProduct('W1', 'Wacom Movink').category, 'Hardver és periféria')
  assert.equal(classifyProduct('D1', 'DeepL Pro').category, 'AI és analitika')
})
